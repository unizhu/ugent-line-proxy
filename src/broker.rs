//! Message broker for routing messages between LINE webhooks and UGENT clients
//!
//! The broker is responsible for:
//! - Receiving messages from LINE webhooks
//! - Routing messages to connected UGENT clients via WebSocket
//! - Handling responses from UGENT and sending them back to LINE
//! - Managing outbound artifacts (files, images, etc.)

use std::sync::Arc;

use reqwest::Client;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::line_api::{self, artifact_to_message, build_text_message, LineApiClient};
use crate::types::{ProxyMessage, WsProtocol};
use crate::ws_manager::WebSocketManager;

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
    /// Pending messages waiting for response (message_id -> original message)
    pending_messages: RwLock<Vec<String>>,
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
            pending_messages: RwLock::new(Vec::new()),
        }
    }

    /// Send a message to all connected UGENT clients
    pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError> {
        let msg_id = message.id.to_string();

        // Track pending message
        {
            let mut pending = self.pending_messages.write().await;
            pending.push(msg_id.clone());
            // Keep only last 1000 pending messages
            if pending.len() > 1000 {
                *pending = pending.split_off(900);
            }
        }

        // Create WebSocket protocol message
        let ws_msg = WsProtocol::Message {
            data: Box::new(message),
        };

        // Broadcast to all clients
        self.ws_manager.broadcast(ws_msg).await?;

        Ok(())
    }

    /// Handle response from UGENT client
    pub async fn handle_response(
        &self,
        original_id: &str,
        content: &str,
        artifacts: Vec<crate::types::OutboundArtifact>,
    ) -> Result<(), BrokerError> {
        info!(
            "Handling response: original_id={}, content_len={}, artifacts={}",
            original_id,
            content.len(),
            artifacts.len()
        );

        // Remove from pending
        {
            let mut pending = self.pending_messages.write().await;
            pending.retain(|id| id != original_id);
        }

        // Build LINE messages
        let mut messages = Vec::new();

        // Add text content
        if !content.is_empty() {
            // Split long content into multiple messages (LINE has 5000 char limit)
            for chunk in split_text(content, 4900) {
                messages.push(build_text_message(&chunk));
            }
        }

        // Add artifacts
        for artifact in &artifacts {
            if let Some(msg) = artifact_to_message(artifact) {
                messages.push(msg);
            } else {
                // For artifacts that can't be sent as LINE messages,
                // send a text description
                warn!(
                    "Artifact {} cannot be sent directly to LINE",
                    artifact.file_name
                );
            }
        }

        // Send messages (this would need the original conversation_id)
        // For now, we'll log this
        info!("Prepared {} messages for LINE", messages.len());

        // Note: To actually send, we need to store the original message's
        // conversation_id and reply_token. This is a TODO.

        Ok(())
    }

    /// Send artifact to LINE user
    pub async fn send_artifact(
        &self,
        conversation_id: &str,
        reply_token: Option<&str>,
        artifact: &crate::types::OutboundArtifact,
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
}
