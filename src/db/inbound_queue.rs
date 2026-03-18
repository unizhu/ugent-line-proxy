//! Inbound message queue entry

use serde::{Deserialize, Serialize};

/// Inbound queue entry for messages pending delivery to UGENT
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboundQueueEntry {
    /// Unique queue entry ID
    pub id: String,
    /// Reference to the messages table
    pub message_id: String,
    /// Queue status
    pub status: String,
    /// Expiry timestamp (Unix ms)
    pub expires_at: i64,
    /// Worker ID that claimed this entry
    pub locked_by: Option<String>,
    /// Lock timestamp (Unix ms)
    pub locked_at: Option<i64>,
    /// Enqueued timestamp (Unix ms)
    pub created_at: i64,
}

/// Inbound entry status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InboundStatus {
    /// Waiting for delivery
    Pending,
    /// Currently being processed
    Processing,
    /// Successfully delivered to UGENT
    Completed,
    /// Expired (TTL exceeded)
    Expired,
}

impl InboundStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            InboundStatus::Pending => "pending",
            InboundStatus::Processing => "processing",
            InboundStatus::Completed => "completed",
            InboundStatus::Expired => "expired",
        }
    }
}

impl std::fmt::Display for InboundStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
