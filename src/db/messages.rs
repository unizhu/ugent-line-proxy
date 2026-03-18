//! Message record type and delivery status

use serde::{Deserialize, Serialize};

/// Message delivery status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeliveryStatus {
    /// Message waiting to be delivered
    Pending,
    /// Successfully delivered
    Delivered,
    /// Delivery failed (max retries exceeded)
    Failed,
    /// Message expired (TTL exceeded)
    Expired,
}

impl DeliveryStatus {
    /// Get string representation for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            DeliveryStatus::Pending => "pending",
            DeliveryStatus::Delivered => "delivered",
            DeliveryStatus::Failed => "failed",
            DeliveryStatus::Expired => "expired",
        }
    }

    /// Parse from database string
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(DeliveryStatus::Pending),
            "delivered" => Some(DeliveryStatus::Delivered),
            "failed" => Some(DeliveryStatus::Failed),
            "expired" => Some(DeliveryStatus::Expired),
            _ => None,
        }
    }
}

impl std::fmt::Display for DeliveryStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Message record (stored in messages table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageRecord {
    /// Unique message ID
    pub id: String,
    /// Message direction: 'inbound' or 'outbound'
    pub direction: String,
    /// Conversation ID (user_id, group_id, or room_id)
    pub conversation_id: String,
    /// Source type: 'user', 'group', 'room'
    pub source_type: String,
    /// Sender LINE user ID
    pub sender_id: Option<String>,
    /// Message type: 'text', 'image', 'audio', 'video', 'file', 'sticker', 'location'
    pub message_type: String,
    /// Text content (nullable)
    pub text_content: Option<String>,
    /// Full LINE message as JSON
    pub message_json: Option<String>,
    /// Media content as JSON (nullable)
    pub media_content_json: Option<String>,
    /// LINE reply token
    pub reply_token: Option<String>,
    /// LINE quote token
    pub quote_token: Option<String>,
    /// Webhook event ID (for deduplication)
    pub webhook_event_id: Option<String>,
    /// LINE's original timestamp
    pub line_timestamp: Option<i64>,
    /// When the proxy received this message (Unix ms)
    pub received_at: i64,
    /// When the message was delivered (Unix ms)
    pub delivered_at: Option<i64>,
    /// Delivery status
    pub delivery_status: DeliveryStatus,
    /// Number of delivery retry attempts
    pub retry_count: i64,
    /// Last retry timestamp (Unix ms)
    pub last_retry_at: Option<i64>,
    /// Error message if delivery failed
    pub error_message: Option<String>,
    /// UGENT's request ID (for outbound messages)
    pub ugent_request_id: Option<String>,
    /// UGENT's correlation ID (for outbound messages)
    pub ugent_correlation_id: Option<String>,
    /// Record creation timestamp (Unix ms)
    pub created_at: i64,
}
