//! Message broker for routing messages between LINE webhooks and UGENT clients
//!
//! The broker is responsible for:
//! - Receiving messages from LINE webhooks
//! - Routing messages to connected UGENT clients via WebSocket
//! - Handling responses from UGENT and sending them back to LINE
//! - Managing outbound artifacts (files, images, etc.)
//! - Tracking pending messages with reply token expiry
//! - Sending ResponseResult acknowledgments

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use reqwest::Client;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::line_api::{self, artifact_to_message, build_text_message, LineApiClient};
use crate::storage::{
    Storage, METRIC_MESSAGES_RECEIVED, METRIC_MESSAGES_SENT, METRIC_PENDING_MESSAGES,
    METRIC_RESPONSES_RECEIVED,
};
use crate::types::{Capabilities, OutboundArtifact, PendingMessage, ProxyMessage, WsProtocol};
use crate::ws_manager::{SendError, WebSocketManager};

/// Current protocol version
const PROTOCOL_VERSION: u32 = 2;

/// Maximum pending messages to keep in memory
const MAX_PENDING_MESSAGES: usize = 1000;

/// Message broker
#[derive(Debug)]
pub struct MessageBroker {
    /// Configuration
    pub config: Arc<Config>,
    /// WebSocket manager
    ws_manager: Arc<WebSocketManager>,
    /// LINE API client
    line_client: LineApiClient,
    /// HTTP client for artifact downloads
    http_client: Client,
    /// Pending messages awaiting response (original_id -> PendingMessage)
    pending_messages: RwLock<HashMap<String, PendingMessage>>,
    /// Persistent storage (optional)
    storage: Option<Arc<Storage>>,
}

impl MessageBroker {
    /// Create a new message broker
    pub fn new(config: Arc<Config>, ws_manager: Arc<WebSocketManager>) -> Self {
        let line_client = LineApiClient::new(config.line.channel_access_token.clone());

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            ws_manager,
            line_client,
            http_client,
            pending_messages: RwLock::new(HashMap::new()),
            storage: None,
        }
    }

    /// Create a new message broker with persistent storage
    pub fn with_storage(
        config: Arc<Config>,
        ws_manager: Arc<WebSocketManager>,
        storage: Storage,
    ) -> Self {
        let line_client = LineApiClient::new(config.line.channel_access_token.clone());

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            ws_manager,
            line_client,
            http_client,
            pending_messages: RwLock::new(HashMap::new()),
            storage: Some(Arc::new(storage)),
        }
    }

    /// Get server capabilities
    pub fn capabilities() -> Capabilities {
        Capabilities::default()
    }

    /// Get protocol version
    pub fn protocol_version() -> u32 {
        PROTOCOL_VERSION
    }

    /// Get storage reference (if enabled)
    pub fn storage(&self) -> Option<&Storage> {
        self.storage.as_deref()
    }

    /// Route a message to the appropriate UGENT client.
    /// If a conversation has an owner, route only to that client.
    /// Otherwise, broadcast to all clients (first-response-wins).
    pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError> {
        let original_id = message.id.clone();
        let conversation_id = message.conversation_id.clone();

        // Create pending message for tracking
        let mut pending = PendingMessage::from_proxy_message(&message);

        // Check if conversation has an owner
        if let Some(owner_client_id) = self.ws_manager.get_conversation_owner(&conversation_id) {
            // Route to owning client only
            info!(
                "Routing message for conversation {} to owner: {}",
                conversation_id, owner_client_id
            );

            pending.client_id = Some(owner_client_id.clone());

            let ws_msg = WsProtocol::Message {
                data: Box::new(message.clone()),
            };

            match self.ws_manager.send_to(&owner_client_id, ws_msg).await {
                Ok(()) => {
                    // Track pending message
                    self.track_pending_message(original_id.clone(), pending);
                    return Ok(());
                }
                Err(SendError::ClientDisconnected) | Err(SendError::ClientNotFound) => {
                    warn!(
                        "Owner client {} disconnected, releasing ownership and falling back to broadcast",
                        owner_client_id
                    );
                    self.ws_manager
                        .release_client_conversations(&owner_client_id);
                    // Fall through to broadcast
                }
            }
        }

        // No owner or owner disconnected - broadcast to all clients
        info!(
            "No owner for conversation {}, broadcasting to all clients",
            conversation_id
        );

        // Track pending message
        self.track_pending_message(original_id.clone(), pending);

        let ws_msg = WsProtocol::Message {
            data: Box::new(message),
        };

        // Broadcast for first-response-wins
        self.ws_manager.broadcast(ws_msg).await?;

        Ok(())
    }

    /// Track a pending message
    fn track_pending_message(&self, original_id: String, pending: PendingMessage) {
        let mut pending_map = self.pending_messages.write();
        pending_map.insert(original_id.clone(), pending.clone());

        // Cleanup expired entries if over limit
        if pending_map.len() > MAX_PENDING_MESSAGES {
            pending_map.retain(|_, v| !v.is_expired());
        }

        // If still over limit, remove oldest entries
        if pending_map.len() > MAX_PENDING_MESSAGES {
            let mut entries: Vec<_> = pending_map
                .iter()
                .map(|(k, v)| (k.clone(), v.received_at))
                .collect();
            entries.sort_by_key(|(_, t)| *t);
            let to_remove = entries.len().saturating_sub(MAX_PENDING_MESSAGES - 100);
            for (key, _) in entries.into_iter().take(to_remove) {
                pending_map.remove(&key);
            }
        }

        // Persist to storage if enabled
        if let Some(ref storage) = self.storage {
            if let Err(e) = storage.pending().store(&pending) {
                warn!("Failed to persist pending message: {}", e);
            }
            // Record metric
            if let Err(e) = storage.metrics().increment(METRIC_PENDING_MESSAGES) {
                warn!("Failed to record pending message metric: {}", e);
            }
            // Record message received metric
            if let Err(e) = storage.metrics().increment(METRIC_MESSAGES_RECEIVED) {
                warn!("Failed to record message received metric: {}", e);
            }
        }
        drop(pending_map);
    }

    /// Handle response from UGENT client and send ResponseResult
    pub async fn handle_response(
        &self,
        request_id: Option<String>,
        original_id: String,
        content: String,
        artifacts: Vec<OutboundArtifact>,
    ) -> Result<(), BrokerError> {
        info!(
            "Handling response: original_id={}, request_id={:?}, content_len={}, artifacts={}",
            original_id,
            request_id,
            content.len(),
            artifacts.len()
        );

        // Get and remove pending message
        let pending = {
            let mut pending_map = self.pending_messages.write();
            pending_map.remove(&original_id)
        };

        let pending = match pending {
            Some(p) => p,
            None => {
                warn!("No pending message found for original_id={}", original_id);
                // Send failure ResponseResult if request_id present
                if let Some(ref req_id) = request_id {
                    self.send_response_result(
                        Some(req_id.clone()),
                        original_id.clone(),
                        false,
                        Some("No pending message found".to_string()),
                    )
                    .await?;
                }
                return Err(BrokerError::NoPendingMessage(original_id));
            }
        };

        // Build LINE messages
        let mut messages = Vec::new();

        // Add text content
        if !content.is_empty() {
            // Split long content into multiple messages (LINE has 5000 char limit)
            for chunk in split_text(&content, 4900) {
                messages.push(build_text_message(&chunk));
            }
        }

        // Add artifacts
        for artifact in &artifacts {
            if let Some(msg) = artifact_to_message(artifact) {
                messages.push(msg);
            } else {
                warn!(
                    "Artifact {} cannot be sent directly to LINE",
                    artifact.file_name
                );
            }
        }

        // Try to send messages
        let send_result = self.send_line_messages(&pending, messages).await;

        // Record response received metric if storage enabled
        if let Some(ref storage) = self.storage {
            if let Err(e) = storage.metrics().increment(METRIC_RESPONSES_RECEIVED) {
                warn!("Failed to record response received metric: {}", e);
            }
        }

        // Send ResponseResult to client
        if let Some(ref req_id) = request_id {
            match &send_result {
                Ok(()) => {
                    self.send_response_result(Some(req_id.clone()), original_id, true, None)
                        .await?;
                }
                Err(e) => {
                    self.send_response_result(
                        Some(req_id.clone()),
                        original_id,
                        false,
                        Some(e.to_string()),
                    )
                    .await?;
                }
            }
        }

        send_result
    }

    /// Send LINE messages using reply token or push fallback
    async fn send_line_messages(
        &self,
        pending: &PendingMessage,
        messages: Vec<serde_json::Value>,
    ) -> Result<(), BrokerError> {
        if messages.is_empty() {
            info!("No messages to send");
            return Ok(());
        }

        // Try reply token first if valid
        if pending.is_reply_token_valid() {
            if let Some(ref token) = pending.reply_token {
                // LINE limits to 5 messages per reply
                for chunk in messages.chunks(5) {
                    match self.line_client.reply_message(token, chunk.to_vec()).await {
                        Ok(()) => {
                            info!("Sent {} message(s) via reply token", chunk.len());
                        }
                        Err(line_api::LineApiError::InvalidReplyToken) => {
                            warn!("Reply token expired during batch, falling back to push");
                            return self.send_via_push(&pending.conversation_id, messages).await;
                        }
                        Err(e) => {
                            error!("LINE API error: {}", e);
                            return Err(BrokerError::LineApi(e.to_string()));
                        }
                    }
                }
                return Ok(());
            }
        }

        // Fall back to push message
        info!(
            "Using push message (reply token {:?})",
            if pending.reply_token.is_some() {
                "expired"
            } else {
                "not available"
            }
        );
        self.send_via_push(&pending.conversation_id, messages).await
    }

    /// Send messages via LINE push API
    async fn send_via_push(
        &self,
        conversation_id: &str,
        messages: Vec<serde_json::Value>,
    ) -> Result<(), BrokerError> {
        // LINE limits to 5 messages per push
        for chunk in messages.chunks(5) {
            self.line_client
                .push_message(conversation_id, chunk.to_vec())
                .await?;
            info!(
                "Sent {} message(s) via push to {}",
                chunk.len(),
                conversation_id
            );
        }

        // Record metrics if storage enabled
        if let Some(ref storage) = self.storage {
            if let Err(e) = storage.metrics().increment(METRIC_MESSAGES_SENT) {
                warn!("Failed to record message sent metric: {}", e);
            }
        }

        Ok(())
    }

    /// Send ResponseResult to client
    async fn send_response_result(
        &self,
        request_id: Option<String>,
        original_id: String,
        success: bool,
        error: Option<String>,
    ) -> Result<(), BrokerError> {
        let result = WsProtocol::ResponseResult {
            request_id,
            original_id,
            success,
            error,
        };

        // Broadcast to all clients (or could target specific client with targeted routing)
        self.ws_manager.broadcast(result).await?;
        Ok(())
    }

    /// Send artifact to LINE user
    pub async fn send_artifact(
        &self,
        conversation_id: &str,
        reply_token: Option<&str>,
        artifact: &OutboundArtifact,
    ) -> Result<(), BrokerError> {
        debug!(
            "Sending artifact: conversation={}, file={}, kind={:?}",
            conversation_id, artifact.file_name, artifact.kind
        );

        // Try to convert artifact to LINE message
        if let Some(message) = artifact_to_message(artifact) {
            // Use reply token if available and not expired
            if let Some(token) = reply_token {
                match self
                    .line_client
                    .reply_message(token, vec![message.clone()])
                    .await
                {
                    Ok(()) => {
                        info!("Artifact sent via reply");
                        return Ok(());
                    }
                    Err(line_api::LineApiError::InvalidReplyToken) => {
                        warn!("Reply token expired, falling back to push message");
                    }
                    Err(e) => {
                        return Err(BrokerError::LineApi(e.to_string()));
                    }
                }
            }

            // Fall back to push message
            self.line_client
                .push_message(conversation_id, vec![message])
                .await?;
            info!("Artifact sent via push message");
        } else {
            // Can't send this artifact type directly
            error!(
                "Failed to send artifact {}: cannot convert to LINE message",
                artifact.file_name
            );
            return Err(BrokerError::UnsupportedArtifactType);
        }

        Ok(())
    }

    /// Download media content from LINE
    pub async fn download_media(&self, message_id: &str) -> Result<(Vec<u8>, String), BrokerError> {
        let (data, content_type) = self.line_client.download_content(message_id).await?;
        Ok((data, content_type))
    }

    /// Get LINE API client
    pub fn line_client(&self) -> &LineApiClient {
        &self.line_client
    }

    /// Get connected client count
    pub fn client_count(&self) -> usize {
        self.ws_manager.client_count()
    }

    /// Get list of connected clients
    pub fn connected_clients(&self) -> Vec<String> {
        self.ws_manager.get_connected_client_ids()
    }

    /// Get WebSocket manager
    pub fn ws_manager(&self) -> Arc<WebSocketManager> {
        self.ws_manager.clone()
    }

    /// Get a pending message by original_id (for ownership claiming)
    pub fn get_pending_message(&self, original_id: &str) -> Option<PendingMessage> {
        self.pending_messages.read().get(original_id).cloned()
    }

    /// Remove a pending message by original_id
    pub fn remove_pending_message(&self, original_id: &str) -> Option<PendingMessage> {
        self.pending_messages.write().remove(original_id)
    }

    /// Get pending message count
    pub fn pending_count(&self) -> usize {
        self.pending_messages.read().len()
    }
}

/// Broker errors
#[derive(Debug, thiserror::Error)]
pub enum BrokerError {
    #[error("Broadcast error: {0}")]
    Broadcast(#[from] crate::ws_manager::BroadcastError),

    #[error("Send error: {0}")]
    Send(#[from] crate::ws_manager::SendError),

    #[error("LINE API error: {0}")]
    LineApi(String),

    #[error("HTTP error: {0}")]
    Http(String),

    #[error("Unsupported artifact type")]
    UnsupportedArtifactType,

    #[error("Failed to resolve path: {0}")]
    PathError(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("No pending message found: {0}")]
    NoPendingMessage(String),
}

impl From<line_api::LineApiError> for BrokerError {
    fn from(err: line_api::LineApiError) -> Self {
        BrokerError::LineApi(err.to_string())
    }
}

/// Split text into chunks at safe boundaries
fn split_text(text: &str, max_len: usize) -> Vec<String> {
    if text.len() <= max_len {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_len {
            chunks.push(remaining.to_string());
            break;
        }

        // Find a safe split point (newline or space)
        let split_point = remaining[..max_len]
            .rfind('\n')
            .or_else(|| remaining[..max_len].rfind(' '))
            .unwrap_or(max_len);

        chunks.push(remaining[..split_point].to_string());
        remaining = &remaining[split_point..];

        // Skip leading whitespace
        remaining = remaining.trim_start();
    }

    chunks
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_text_short() {
        let text = "Hello, World!";
        let chunks = split_text(text, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], text);
    }

    #[test]
    fn test_split_text_long() {
        let text = "Line 1\nLine 2\nLine 3\nLine 4\nLine 5";
        let chunks = split_text(text, 15);
        assert!(chunks.len() > 1);
    }

    #[test]
    fn test_split_text_preserves_newlines() {
        let text = "Line 1\nLine 2\nLine 3";
        let chunks = split_text(text, 20);
        assert!(chunks.iter().any(|c| c.contains('\n')));
    }

    #[test]
    fn test_capabilities_default() {
        let caps = Capabilities::default();
        assert!(caps.response_result);
        assert!(caps.push_fallback);
        assert!(!caps.artifact_staging);
        assert!(caps.targeted_routing); // Now implemented with first-response-wins ownership
    }
}
