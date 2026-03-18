//! Group and room record types

use serde::{Deserialize, Serialize};

/// Group record (stored in groups table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupRecord {
    /// LINE group ID (Cxxx)
    pub line_group_id: String,
    /// Group name
    pub group_name: Option<String>,
    /// Group picture URL
    pub picture_url: Option<String>,
    /// Number of members
    pub member_count: Option<i64>,
    /// First seen timestamp (Unix ms)
    pub first_seen_at: i64,
    /// Last message timestamp (Unix ms)
    pub last_message_at: Option<i64>,
    /// Record creation timestamp (Unix ms)
    pub created_at: i64,
    /// Record update timestamp (Unix ms)
    pub updated_at: i64,
}

/// Group member record (stored in group_members table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMemberRecord {
    /// LINE group ID
    pub line_group_id: String,
    /// LINE user ID
    pub line_user_id: String,
    /// Join timestamp (Unix ms)
    pub joined_at: i64,
    /// Whether this member is a bot
    pub is_bot: bool,
}
