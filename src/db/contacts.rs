//! Contact record type

use serde::{Deserialize, Serialize};

/// Contact record (stored in contacts table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactRecord {
    /// LINE user ID (Uxxx)
    pub line_user_id: String,
    /// Display name from LINE API
    pub display_name: Option<String>,
    /// Profile picture URL
    pub picture_url: Option<String>,
    /// Status message
    pub status_message: Option<String>,
    /// Language preference
    pub language: Option<String>,
    /// First interaction timestamp (Unix ms)
    pub first_seen_at: i64,
    /// Last seen timestamp (Unix ms)
    pub last_seen_at: i64,
    /// Last message interaction timestamp (Unix ms)
    pub last_interacted_at: Option<i64>,
    /// Whether the user has blocked the bot
    pub is_blocked: bool,
    /// Whether the user is a friend (added the bot)
    pub is_friend: bool,
    /// Record creation timestamp (Unix ms)
    pub created_at: i64,
    /// Record update timestamp (Unix ms)
    pub updated_at: i64,
}
