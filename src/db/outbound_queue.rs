//! Outbound message queue entry

use serde::{Deserialize, Serialize};

/// Outbound queue entry for retry logic
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundQueueEntry {
    /// Unique queue entry ID
    pub id: String,
    /// Reference to the messages table
    pub message_id: String,
    /// Queue status
    pub status: String,
    /// Current retry attempt (0-based)
    pub attempt: i64,
    /// Max retry attempts
    pub max_attempts: i64,
    /// Next retry after timestamp (Unix ms)
    pub next_retry_at: i64,
    /// Worker ID that claimed this entry
    pub locked_by: Option<String>,
    /// Lock timestamp (Unix ms)
    pub locked_at: Option<i64>,
    /// Last error message
    pub last_error: Option<String>,
    /// Enqueued timestamp (Unix ms)
    pub created_at: i64,
    /// Updated timestamp (Unix ms)
    pub updated_at: i64,
}

/// Outbound entry status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutboundStatus {
    /// Waiting for retry
    Pending,
    /// Currently being processed by a worker
    Processing,
    /// Successfully delivered
    Completed,
    /// All retries exhausted
    Failed,
}

impl OutboundStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutboundStatus::Pending => "pending",
            OutboundStatus::Processing => "processing",
            OutboundStatus::Completed => "completed",
            OutboundStatus::Failed => "failed",
        }
    }
}

impl std::fmt::Display for OutboundStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
