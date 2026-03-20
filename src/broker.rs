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
use std::path::Path;
use std::sync::Arc;

use base64::Engine;
use parking_lot::RwLock;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::file_hosting::FileHostingService;
use crate::line_api::{self, LineApiClient, artifact_to_message, build_text_message};
use crate::storage::{
    METRIC_MESSAGES_RECEIVED, METRIC_MESSAGES_SENT, METRIC_PENDING_MESSAGES,
    METRIC_RESPONSES_RECEIVED, Storage,
};
use crate::types::ArtifactKind;
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
    /// Pending messages awaiting response (original_id -> PendingMessage)
    pending_messages: RwLock<HashMap<String, PendingMessage>>,
    /// Persistent storage (optional)
    storage: Option<Arc<Storage>>,
    /// File hosting service (optional)
    file_hosting: RwLock<Option<Arc<FileHostingService>>>,
}

impl MessageBroker {
    /// Create a new message broker
    pub fn new(config: Arc<Config>, ws_manager: Arc<WebSocketManager>) -> Self {
        let line_client = LineApiClient::new(config.line.channel_access_token.clone());

        Self {
            config,
            ws_manager,
            line_client,
            pending_messages: RwLock::new(HashMap::new()),
            storage: None,
            file_hosting: RwLock::new(None),
        }
    }

    /// Create a new message broker with persistent storage
    pub fn with_storage(
        config: Arc<Config>,
        ws_manager: Arc<WebSocketManager>,
        storage: Storage,
    ) -> Self {
        let line_client = LineApiClient::new(config.line.channel_access_token.clone());

        Self {
            config,
            ws_manager,
            line_client,
            pending_messages: RwLock::new(HashMap::new()),
            storage: Some(Arc::new(storage)),
            file_hosting: RwLock::new(None),
        }
    }

    /// Get server capabilities
    pub fn capabilities() -> Capabilities {
        Capabilities::default()
    }

    /// Get protocol version
    pub const fn protocol_version() -> u32 {
        PROTOCOL_VERSION
    }

    /// Get storage reference (if enabled)
    pub fn storage(&self) -> Option<&Storage> {
        self.storage.as_deref()
    }

    /// Set the file hosting service (called from main.rs after initialization)
    pub fn set_file_hosting(&self, service: Arc<FileHostingService>) {
        let mut fh = self.file_hosting.write();
        *fh = Some(service);
    }

    /// Get a clone of the file hosting service Arc (if enabled)
    pub fn get_file_hosting(&self) -> Option<Arc<FileHostingService>> {
        self.file_hosting.read().clone()
    }

    /// Route a message to the appropriate UGENT client.
    /// If a conversation has an owner, route only to that client.
    /// Otherwise, broadcast to all clients (first-response-wins).
    pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError> {
        let original_id = message.id.clone();
        let conversation_id = message.conversation_id.clone();

        // Start typing indicator if enabled (non-fatal if it fails)
        if self.config.line.auto_loading_indicator
            && let Err(e) = self.line_client.start_loading(&conversation_id).await
        {
            warn!("Failed to start loading indicator: {}", e);
        }

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
                    self.track_pending_message(&original_id, &pending);
                    return Ok(());
                }
                Err(SendError::ClientDisconnected | SendError::ClientNotFound) => {
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
        self.track_pending_message(&original_id, &pending);

        let ws_msg = WsProtocol::Message {
            data: Box::new(message),
        };

        // Broadcast for first-response-wins
        self.ws_manager.broadcast(ws_msg).await?;

        Ok(())
    }

    /// Track a pending message
    fn track_pending_message(&self, original_id: &str, pending: &PendingMessage) {
        let mut pending_map = self.pending_messages.write();
        pending_map.insert(original_id.to_string(), pending.clone());

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
            if let Err(e) = storage.pending().store(pending) {
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
        responding_client_id: Option<String>,
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

        let Some(pending) = pending else {
            warn!("No pending message found for original_id={}", original_id);
            // Send failure ResponseResult if request_id present
            if let Some(ref req_id) = request_id {
                self.send_response_result(
                    Some(req_id.clone()),
                    original_id.clone(),
                    false,
                    Some("No pending message found".to_string()),
                    responding_client_id.as_deref(),
                )
                .await?;
            }
            return Err(BrokerError::NoPendingMessage(original_id));
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
                warn!("Artifact {} cannot be sent directly to LINE", artifact.name);
            }
        }

        // Try to send messages
        let send_result = self.send_line_messages(&pending, messages).await;

        // Auto mark-as-read if enabled and send succeeded (non-fatal)
        if send_result.is_ok()
            && self.config.line.auto_mark_as_read
            && let Some(ref token) = pending.mark_as_read_token
            && let Err(e) = self.line_client.mark_as_read(token).await
        {
            warn!("Failed to mark as read: {}", e);
        }

        // Record response received metric if storage enabled
        if let Some(ref storage) = self.storage
            && let Err(e) = storage.metrics().increment(METRIC_RESPONSES_RECEIVED)
        {
            warn!("Failed to record response received metric: {}", e);
        }

        // Send ResponseResult to client
        if let Some(ref req_id) = request_id {
            match &send_result {
                Ok(()) => {
                    self.send_response_result(
                        Some(req_id.clone()),
                        original_id,
                        true,
                        None,
                        responding_client_id.as_deref(),
                    )
                    .await?;
                }
                Err(e) => {
                    self.send_response_result(
                        Some(req_id.clone()),
                        original_id,
                        false,
                        Some(e.to_string()),
                        responding_client_id.as_deref(),
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
        if pending.is_reply_token_valid()
            && let Some(ref token) = pending.reply_token
        {
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
        if let Some(ref storage) = self.storage
            && let Err(e) = storage.metrics().increment(METRIC_MESSAGES_SENT)
        {
            warn!("Failed to record message sent metric: {}", e);
        }

        Ok(())
    }

    /// Send ResponseResult to a specific client
    async fn send_response_result(
        &self,
        request_id: Option<String>,
        original_id: String,
        success: bool,
        error: Option<String>,
        target_client_id: Option<&str>,
    ) -> Result<(), BrokerError> {
        let result = WsProtocol::ResponseResult {
            request_id,
            original_id,
            success,
            error,
        };

        if let Some(client_id) = target_client_id {
            match self.ws_manager.send_to(client_id, result).await {
                Ok(()) => {
                    debug!("ResponseResult sent to client {}", client_id);
                    Ok(())
                }
                Err(e) => {
                    warn!(
                        "Failed to send ResponseResult to client {}: {}",
                        client_id, e
                    );
                    Ok(())
                }
            }
        } else {
            self.ws_manager
                .broadcast(result)
                .await
                .map_err(BrokerError::Broadcast)?;
            Ok(())
        }
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
            conversation_id, artifact.name, artifact.kind
        );

        // For Document/Other artifacts with local_path, use file hosting if available
        if matches!(artifact.kind, ArtifactKind::Document | ArtifactKind::Other)
            && artifact.local_path.is_some()
            && let Some(fh) = self.get_file_hosting()
        {
            return self
                .send_file_via_hosting(&fh, conversation_id, reply_token, artifact)
                .await;
        }

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
                artifact.name
            );
            return Err(BrokerError::UnsupportedArtifactType);
        }

        Ok(())
    }

    /// Send a file via the file hosting service.
    /// Saves the file (from base64 data or local path), generates a signed download URL, and sends it as text.
    async fn send_file_via_hosting(
        &self,
        fh: &crate::file_hosting::FileHostingService,
        conversation_id: &str,
        reply_token: Option<&str>,
        artifact: &OutboundArtifact,
    ) -> Result<(), BrokerError> {
        let saved = if let Some(ref b64_data) = artifact.data {
            // Cross-machine delivery: decode base64 data
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64_data)
                .map_err(|e| BrokerError::LineApi(format!("Base64 decode error: {e}")))?;
            fh.save_bytes(&bytes, &artifact.name, artifact.content_type.as_deref().unwrap_or("application/octet-stream"))
                .await
                .map_err(|e| BrokerError::LineApi(format!("File hosting save error: {e}")))?
        } else if let Some(ref local_path) = artifact.local_path {
            // Same-machine fallback: read from local filesystem
            let path = Path::new(local_path);
            if !path.exists() {
                error!("Local file not found for hosting: {}", local_path);
                return Err(BrokerError::LineApi(format!(
                    "Local file not found: {local_path}"
                )));
            }
            fh.save_file(path, Some(&artifact.name), artifact.content_type.as_deref())
                .await
                .map_err(|e| BrokerError::LineApi(format!("File hosting save error: {e}")))?
        } else {
            return Err(BrokerError::LineApi(
                "No file data or local_path in artifact".to_string(),
            ));
        };

        let download_url = fh.generate_download_url(&saved.uuid_name, None);

        info!(
            "File hosted: {} -> {}",
            artifact.name,
            &download_url[..download_url.len().min(80)]
        );

        let size_info = artifact
            .size_bytes
            .map(|s| format!(" ({:.1} KB)", s as f64 / 1024.0))
            .unwrap_or_default();

        let text = format!("\u{1f4ce} {}{size_info}\n{download_url}", artifact.name);

        let message = build_text_message(&text);

        // Send via reply or push
        if let Some(token) = reply_token {
            match self
                .line_client
                .reply_message(token, vec![message.clone()])
                .await
            {
                Ok(()) => {
                    info!("Hosted file sent via reply");
                    return Ok(());
                }
                Err(line_api::LineApiError::InvalidReplyToken) => {
                    warn!("Reply token expired, falling back to push");
                }
                Err(e) => {
                    return Err(BrokerError::LineApi(e.to_string()));
                }
            }
        }

        self.line_client
            .push_message(conversation_id, vec![message])
            .await?;
        info!("Hosted file sent via push");

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

/// Axum handler for file download — extracts file hosting service from broker state.
pub async fn handle_file_download(
    axum::extract::State(broker): axum::extract::State<Arc<MessageBroker>>,
    axum::extract::Query(params): axum::extract::Query<crate::file_hosting::DownloadParams>,
) -> axum::response::Response {
    use axum::response::IntoResponse;

    let fh = match broker.get_file_hosting() {
        Some(s) => s,
        None => {
            return (
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "File hosting not configured",
            )
                .into_response();
        }
    };

    let expires = match params.expires {
        Some(e) => e,
        None => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "Missing expires parameter",
            )
                .into_response();
        }
    };

    crate::file_hosting::serve_download(&fh, &params.file, &params.code, expires).await
}

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
