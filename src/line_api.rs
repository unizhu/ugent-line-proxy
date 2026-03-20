//! LINE Messaging API client
//!
//! Provides methods for:
//! - Sending reply messages (using reply token)
//! - Sending push messages (proactive)
//! - Downloading media content (image/audio/video)
//! - Getting user profiles

use reqwest::Client;
use serde_json::{Value, json};
use thiserror::Error;
use tracing::{debug, error, info, warn};

use crate::types::{ArtifactKind, BotInfo, OutboundArtifact, UserProfile};

/// LINE API base URL
const API_BASE: &str = "https://api.line.me/v2/bot";
/// LINE Data API base URL (for content download)
const DATA_API_BASE: &str = "https://api-data.line.me/v2/bot";

/// LINE API errors
#[derive(Debug, Error)]
pub enum LineApiError {
    #[error("HTTP request failed: {0}")]
    RequestFailed(#[from] reqwest::Error),

    #[error("API error: {0} - {1}")]
    ApiError(u16, String),

    #[error("Rate limited. Retry after {0} seconds")]
    RateLimited(u64),

    #[error("Content download failed: {0}")]
    DownloadFailed(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Reply token expired or invalid")]
    InvalidReplyToken,
}

/// LINE API client
#[derive(Debug, Clone)]
pub struct LineApiClient {
    /// HTTP client
    client: Client,
    /// Channel access token
    access_token: String,
}

impl LineApiClient {
    /// Create a new LINE API client
    pub fn new(access_token: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            access_token,
        }
    }

    /// Create with custom HTTP client
    pub fn with_client(access_token: String, client: Client) -> Self {
        Self {
            client,
            access_token,
        }
    }

    /// Reply to a webhook event using reply token
    ///
    /// Note: Reply tokens expire after about 1 minute
    pub async fn reply_message(
        &self,
        reply_token: &str,
        messages: Vec<Value>,
    ) -> Result<(), LineApiError> {
        self.reply_message_with_retry_key(reply_token, messages, None)
            .await
    }

    /// Reply to a webhook event with optional retry key for idempotency
    ///
    /// Note: Reply tokens expire after about 1 minute
    pub async fn reply_message_with_retry_key(
        &self,
        reply_token: &str,
        messages: Vec<Value>,
        retry_key: Option<&str>,
    ) -> Result<(), LineApiError> {
        if messages.is_empty() {
            warn!("No messages to send in reply");
            return Ok(());
        }

        // LINE allows max 5 messages in a reply
        let messages: Vec<Value> = messages.into_iter().take(5).collect();

        let url = format!("{API_BASE}/message/reply");
        let body = json!({
            "replyToken": reply_token,
            "messages": messages
        });

        debug!("Sending reply to LINE: {} messages", messages.len());

        let mut request = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body);

        // Add X-Line-Retry-Key header for idempotency if provided
        if let Some(key) = retry_key {
            request = request.header("X-Line-Retry-Key", key);
            debug!("Using retry key: {}", key);
        }

        let response = request.send().await?;

        let status = response.status();
        if status.is_success() {
            info!("Reply sent successfully");
            Ok(())
        } else {
            // Handle rate limiting (429) - read header before consuming response
            if status.as_u16() == 429 {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);
                warn!("Rate limited. Retry after {} seconds", retry_after);
                return Err(LineApiError::RateLimited(retry_after));
            }

            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("Reply failed: {} - {}", status, error_text);

            if status.as_u16() == 400 && error_text.contains("Invalid reply token") {
                Err(LineApiError::InvalidReplyToken)
            } else {
                Err(LineApiError::ApiError(status.as_u16(), error_text))
            }
        }
    }

    /// Send a push message to a user/group/room
    ///
    /// Use this for proactive messaging or when reply token is expired
    pub async fn push_message(&self, to: &str, messages: Vec<Value>) -> Result<(), LineApiError> {
        self.push_message_with_retry_key(to, messages, None).await
    }

    /// Send a push message with optional retry key for idempotency
    ///
    /// The retry key prevents duplicate messages when retrying failed requests.
    /// Valid for 24 hours after the first request.
    pub async fn push_message_with_retry_key(
        &self,
        to: &str,
        messages: Vec<Value>,
        retry_key: Option<&str>,
    ) -> Result<(), LineApiError> {
        if messages.is_empty() {
            warn!("No messages to send in push");
            return Ok(());
        }

        // LINE allows max 5 messages in a push
        let messages: Vec<Value> = messages.into_iter().take(5).collect();

        let url = format!("{API_BASE}/message/push");
        let body = json!({
            "to": to,
            "messages": messages
        });

        debug!(
            "Sending push message to {}: {} messages",
            to,
            messages.len()
        );

        let mut request = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body);

        // Add X-Line-Retry-Key header for idempotency if provided
        if let Some(key) = retry_key {
            request = request.header("X-Line-Retry-Key", key);
            debug!("Using retry key: {}", key);
        }

        let response = request.send().await?;

        let status = response.status();
        if status.is_success() {
            info!("Push message sent successfully to {}", to);
            Ok(())
        } else {
            // Handle rate limiting (429) - read header before consuming response
            if status.as_u16() == 429 {
                let retry_after = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(60);
                warn!("Rate limited. Retry after {} seconds", retry_after);
                return Err(LineApiError::RateLimited(retry_after));
            }

            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("Push failed: {} - {}", status, error_text);
            Err(LineApiError::ApiError(status.as_u16(), error_text))
        }
    }

    /// Download media content (image/audio/video/file)
    pub async fn download_content(
        &self,
        message_id: &str,
    ) -> Result<(Vec<u8>, String), LineApiError> {
        let url = format!("{DATA_API_BASE}/message/{message_id}/content");

        debug!("Downloading content: {}", message_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("Content download failed: {} - {}", status, error_text);
            return Err(LineApiError::DownloadFailed(format!(
                "{status}: {error_text}"
            )));
        }

        // Get content type
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let bytes = response.bytes().await?;
        info!(
            "Downloaded {} bytes, content-type: {}",
            bytes.len(),
            content_type
        );

        Ok((bytes.to_vec(), content_type))
    }

    /// Download preview image for video/image
    pub async fn download_preview(&self, message_id: &str) -> Result<Vec<u8>, LineApiError> {
        let url = format!("{DATA_API_BASE}/message/{message_id}/content/preview");

        debug!("Downloading preview: {}", message_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            return Err(LineApiError::DownloadFailed(format!(
                "Preview download failed: {status}"
            )));
        }

        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }

    /// Get user profile
    pub async fn get_profile(&self, user_id: &str) -> Result<UserProfile, LineApiError> {
        let url = format!("{API_BASE}/profile/{user_id}");

        debug!("Getting profile for user: {}", user_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LineApiError::ApiError(status.as_u16(), error_text));
        }

        let profile = response.json().await?;
        Ok(profile)
    }

    /// Get bot info
    pub async fn get_bot_info(&self) -> Result<BotInfo, LineApiError> {
        let url = format!("{API_BASE}/info");

        debug!("Getting bot info");

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LineApiError::ApiError(status.as_u16(), error_text));
        }

        let info = response.json().await?;
        Ok(info)
    }

    /// Start a loading animation ("typing indicator") in a chat
    ///
    /// This shows a "typing..." indicator to the user while UGENT is processing.
    /// The loading animation lasts for about 20 seconds maximum.
    ///
    /// API endpoint: POST /v2/bot/chat/loading/start
    pub async fn start_loading(&self, chat_id: &str) -> Result<(), LineApiError> {
        let url = format!("{API_BASE}/chat/loading/start");
        let body = json!({
            "chatId": chat_id
        });

        debug!("Starting loading animation for chat: {}", chat_id);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if status.is_success() {
            info!("Loading animation started for chat: {}", chat_id);
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("Failed to start loading: {} - {}", status, error_text);
            Err(LineApiError::ApiError(status.as_u16(), error_text))
        }
    }

    /// Mark messages as read using the mark-as-read token.
    ///
    /// This marks all messages prior to the one with the given token as read.
    /// Read tokens have no expiration date.
    ///
    /// API endpoint: POST /v2/bot/chat/markAsRead
    pub async fn mark_as_read(&self, mark_as_read_token: &str) -> Result<(), LineApiError> {
        let url = format!("{API_BASE}/chat/markAsRead");
        let body = json!({
            "markAsReadToken": mark_as_read_token
        });

        debug!(
            "Marking messages as read with token: {}...",
            &mark_as_read_token[..8.min(mark_as_read_token.len())]
        );

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if status.is_success() {
            info!("Messages marked as read");
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            error!("Failed to mark as read: {} - {}", status, error_text);
            Err(LineApiError::ApiError(status.as_u16(), error_text))
        }
    }

    /// Get group summary
    pub async fn get_group_summary(&self, group_id: &str) -> Result<GroupSummary, LineApiError> {
        let url = format!("{API_BASE}/group/{group_id}/summary");

        debug!("Getting group summary: {}", group_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LineApiError::ApiError(status.as_u16(), error_text));
        }

        let summary = response.json().await?;
        Ok(summary)
    }

    /// Get group member IDs
    pub async fn get_group_member_ids(
        &self,
        group_id: &str,
    ) -> Result<MemberIdsResponse, LineApiError> {
        let url = format!("{API_BASE}/group/{group_id}/members/ids");

        debug!("Getting group member IDs: {}", group_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LineApiError::ApiError(status.as_u16(), error_text));
        }

        let ids = response.json().await?;
        Ok(ids)
    }

    /// Get group member profile
    pub async fn get_group_member_profile(
        &self,
        group_id: &str,
        user_id: &str,
    ) -> Result<UserProfile, LineApiError> {
        let url = format!("{API_BASE}/group/{group_id}/member/{user_id}");

        debug!("Getting group member profile: {}/{}", group_id, user_id);

        let response = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(LineApiError::ApiError(status.as_u16(), error_text));
        }

        let profile = response.json().await?;
        Ok(profile)
    }

    /// Leave a group
    pub async fn leave_group(&self, group_id: &str) -> Result<(), LineApiError> {
        let url = format!("{API_BASE}/group/{group_id}/leave");

        debug!("Leaving group: {}", group_id);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if status.is_success() {
            info!("Left group: {}", group_id);
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(LineApiError::ApiError(status.as_u16(), error_text))
        }
    }

    /// Leave a room
    pub async fn leave_room(&self, room_id: &str) -> Result<(), LineApiError> {
        let url = format!("{API_BASE}/room/{room_id}/leave");

        debug!("Leaving room: {}", room_id);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.access_token))
            .send()
            .await?;

        let status = response.status();
        if status.is_success() {
            info!("Left room: {}", room_id);
            Ok(())
        } else {
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            Err(LineApiError::ApiError(status.as_u16(), error_text))
        }
    }
}

// =============================================================================
// Message Builders
// =============================================================================

/// Build a text message
pub fn build_text_message(text: &str) -> Value {
    json!({
        "type": "text",
        "text": text
    })
}

/// Build an image message
pub fn build_image_message(original_url: &str, preview_url: &str) -> Value {
    json!({
        "type": "image",
        "originalContentUrl": original_url,
        "previewImageUrl": preview_url
    })
}

/// Build a video message
pub fn build_video_message(original_url: &str, preview_url: &str) -> Value {
    json!({
        "type": "video",
        "originalContentUrl": original_url,
        "previewImageUrl": preview_url
    })
}

/// Build an audio message
pub fn build_audio_message(original_url: &str, duration_ms: i64) -> Value {
    json!({
        "type": "audio",
        "originalContentUrl": original_url,
        "duration": duration_ms
    })
}

/// Build a sticker message
pub fn build_sticker_message(package_id: &str, sticker_id: &str) -> Value {
    json!({
        "type": "sticker",
        "packageId": package_id,
        "stickerId": sticker_id
    })
}

/// Build a location message
pub fn build_location_message(title: &str, address: &str, latitude: f64, longitude: f64) -> Value {
    json!({
        "type": "location",
        "title": title,
        "address": address,
        "latitude": latitude,
        "longitude": longitude
    })
}

/// Convert outbound artifact to LINE message
pub fn artifact_to_message(artifact: &OutboundArtifact) -> Option<Value> {
    match artifact.kind {
        ArtifactKind::Image => {
            // For images, check URL first, then local_path
            let url = artifact
                .url
                .as_ref()
                .and_then(|u| {
                    if u.starts_with("http://") || u.starts_with("https://") {
                        Some(u.as_str())
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    artifact.local_path.as_ref().and_then(|p| {
                        if p.starts_with("http://") || p.starts_with("https://") {
                            Some(p.as_str())
                        } else {
                            None
                        }
                    })
                });
            url.map(|u| build_image_message(u, u))
        }
        ArtifactKind::Audio => {
            let url = artifact
                .url
                .as_ref()
                .and_then(|u| {
                    if u.starts_with("http://") || u.starts_with("https://") {
                        Some(u.as_str())
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    artifact.local_path.as_ref().and_then(|p| {
                        if p.starts_with("http://") || p.starts_with("https://") {
                            Some(p.as_str())
                        } else {
                            None
                        }
                    })
                });
            // Extract duration from metadata if available, else estimate
            let duration_ms = artifact
                .metadata
                .as_ref()
                .and_then(|m| m.get("duration_ms"))
                .and_then(|v| v.as_u64())
                .unwrap_or(60_000);
            url.map(|u| build_audio_message(u, duration_ms as i64))
        }
        ArtifactKind::Video => {
            let url = artifact
                .url
                .as_ref()
                .and_then(|u| {
                    if u.starts_with("http://") || u.starts_with("https://") {
                        Some(u.as_str())
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    artifact.local_path.as_ref().and_then(|p| {
                        if p.starts_with("http://") || p.starts_with("https://") {
                            Some(p.as_str())
                        } else {
                            None
                        }
                    })
                });
            url.map(|u| build_video_message(u, u))
        }
        ArtifactKind::Document | ArtifactKind::Other => {
            // LINE doesn't support sending files directly
            // Send a text message with the file info instead
            let size_info = artifact
                .size_bytes
                .map(|s| format!(" ({:.1} KB)", s as f64 / 1024.0))
                .unwrap_or_default();
            Some(build_text_message(&format!(
                "\u{1f4c4} File: {}{size_info}",
                artifact.name
            )))
        }
    }
}

// =============================================================================
// Additional Types
// =============================================================================

/// Group summary
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GroupSummary {
    /// Group ID
    #[serde(rename = "groupId")]
    pub group_id: String,
    /// Group name
    #[serde(rename = "groupName")]
    pub group_name: String,
    /// Group picture URL
    #[serde(rename = "pictureUrl")]
    pub picture_url: Option<String>,
}

/// Member IDs response
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MemberIdsResponse {
    /// List of user IDs
    pub member_ids: Vec<String>,
    /// Next page token (for pagination)
    pub next: Option<String>,
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_text_message() {
        let msg = build_text_message("Hello, World!");
        assert_eq!(msg["type"], "text");
        assert_eq!(msg["text"], "Hello, World!");
    }

    #[test]
    fn test_build_image_message() {
        let msg = build_image_message(
            "https://example.com/original.jpg",
            "https://example.com/preview.jpg",
        );
        assert_eq!(msg["type"], "image");
        assert_eq!(
            msg["originalContentUrl"],
            "https://example.com/original.jpg"
        );
        assert_eq!(msg["previewImageUrl"], "https://example.com/preview.jpg");
    }

    #[test]
    fn test_build_sticker_message() {
        let msg = build_sticker_message("446", "1988");
        assert_eq!(msg["type"], "sticker");
        assert_eq!(msg["packageId"], "446");
        assert_eq!(msg["stickerId"], "1988");
    }
}
