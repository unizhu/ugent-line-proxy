//! RMS Data Types
//!
//! Core types for the Relationship Management System.

use serde::{Deserialize, Serialize};
use std::fmt;

/// LINE Entity Types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum LineEntityType {
    /// Individual user (1:1 chat)
    User,
    /// LINE group
    Group,
    /// LINE room (multi-person without group)
    Room,
}

impl LineEntityType {
    /// Get string representation for database storage
    pub fn as_str(&self) -> &'static str {
        match self {
            LineEntityType::User => "user",
            LineEntityType::Group => "group",
            LineEntityType::Room => "room",
        }
    }

    /// Parse from string
    pub fn parse_entity_type(s: &str) -> Option<Self> {
        match s {
            "user" => Some(LineEntityType::User),
            "group" => Some(LineEntityType::Group),
            "room" => Some(LineEntityType::Room),
            _ => None,
        }
    }

    /// Determine entity type from LINE ID prefix
    pub fn from_line_id(id: &str) -> Self {
        if id.starts_with('U') {
            LineEntityType::User
        } else if id.starts_with('C') {
            LineEntityType::Group
        } else if id.starts_with('R') {
            LineEntityType::Room
        } else {
            // Default to user for unknown prefixes
            LineEntityType::User
        }
    }
}

impl fmt::Display for LineEntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// LINE Entity (Contact/Group/Room)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineEntity {
    /// LINE ID (Uxxx, Rxxx, Cxxx)
    pub id: String,
    /// Entity type
    pub entity_type: LineEntityType,
    /// Display name from LINE API or cached
    pub display_name: Option<String>,
    /// Profile picture URL
    pub picture_url: Option<String>,
    /// Last message timestamp (Unix)
    pub last_message_at: Option<i64>,
    /// Creation timestamp (Unix)
    pub created_at: i64,
    /// Last update timestamp (Unix)
    pub updated_at: i64,
}

/// UGENT Client Information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    /// WebSocket client ID
    pub client_id: String,
    /// Whether client is currently connected
    pub connected: bool,
    /// Connection timestamp (Unix)
    pub connected_at: Option<i64>,
    /// Last activity timestamp (Unix)
    pub last_activity: i64,
    /// Number of owned conversations
    pub owned_conversations: usize,
    /// Client-provided metadata
    pub metadata: Option<serde_json::Value>,
}

/// Relationship (Routing Rule)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    /// Auto-increment ID
    pub id: i64,
    /// LINE user/group/room ID
    pub line_entity_id: String,
    /// Entity type
    pub entity_type: LineEntityType,
    /// Assigned UGENT client
    pub client_id: String,
    /// Priority for multi-client support (future)
    pub priority: i32,
    /// true = manually set, false = auto-detected
    pub is_manual: bool,
    /// Creation timestamp (Unix)
    pub created_at: i64,
    /// Last update timestamp (Unix)
    pub updated_at: i64,
    /// Admin notes
    pub notes: Option<String>,
}

/// Dispatch Rule (Computed from relationships + runtime state)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRule {
    /// Conversation ID (LINE entity ID)
    pub conversation_id: String,
    /// Entity type
    pub entity_type: LineEntityType,
    /// Assigned client (if any)
    pub assigned_client: Option<String>,
    /// Whether assigned client is connected
    pub assigned_client_connected: bool,
    /// Whether relationship is manual
    pub is_manual: bool,
    /// Last routed timestamp (Unix)
    pub last_routed_at: Option<i64>,
    /// Message count
    pub message_count: i64,
}

/// Entity filter for queries
#[derive(Debug, Clone, Default)]
pub struct EntityFilter {
    /// Filter by entity type
    pub entity_type: Option<LineEntityType>,
    /// Filter by relationship status
    pub has_relationship: Option<bool>,
    /// Search string for display name
    pub search: Option<String>,
    /// Limit results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
}

/// Relationship import data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationshipImport {
    /// LINE entity ID
    pub entity_id: String,
    /// Client ID to assign
    pub client_id: String,
    /// Admin notes
    pub notes: Option<String>,
}

/// Import result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResult {
    /// Number of relationships imported
    pub imported: usize,
    /// Number of relationships updated
    pub updated: usize,
    /// Number of errors
    pub errors: Vec<String>,
}

/// Sync result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    /// Number of relationships added
    pub added: usize,
    /// Number of relationships updated
    pub updated: usize,
    /// Number of relationships removed
    pub removed: usize,
}

/// System status summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    /// Number of connected clients
    pub connected_clients: usize,
    /// Total LINE entities tracked
    pub total_entities: usize,
    /// Total relationships
    pub total_relationships: usize,
    /// Manual relationships count
    pub manual_relationships: usize,
    /// Auto-detected relationships count
    pub auto_relationships: usize,
    /// Pending messages count
    pub pending_messages: usize,
    /// Server uptime in seconds
    pub uptime_secs: u64,
}

/// RMS Error types
#[derive(Debug, thiserror::Error)]
pub enum RmsError {
    #[error("Storage error: {0}")]
    Storage(#[from] crate::storage::StorageError),

    #[error("Entity not found: {0}")]
    EntityNotFound(String),

    #[error("Client not found: {0}")]
    ClientNotFound(String),

    #[error("Relationship not found: {0}")]
    RelationshipNotFound(String),

    #[error("Invalid entity ID: {0}")]
    InvalidEntityId(String),

    #[error("LINE API error: {0}")]
    LineApi(#[from] crate::error::ProxyError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Import error: {0}")]
    Import(String),
}

impl From<rusqlite::Error> for RmsError {
    fn from(err: rusqlite::Error) -> Self {
        RmsError::Storage(crate::storage::StorageError::from(err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_type_from_line_id() {
        assert_eq!(
            LineEntityType::from_line_id("U123456"),
            LineEntityType::User
        );
        assert_eq!(
            LineEntityType::from_line_id("C123456"),
            LineEntityType::Group
        );
        assert_eq!(
            LineEntityType::from_line_id("R123456"),
            LineEntityType::Room
        );
    }

    #[test]
    fn test_entity_type_str_roundtrip() {
        for et in &[
            LineEntityType::User,
            LineEntityType::Group,
            LineEntityType::Room,
        ] {
            assert_eq!(LineEntityType::parse_entity_type(et.as_str()), Some(*et));
        }
    }

    #[test]
    fn test_entity_type_serialize() {
        let et = LineEntityType::User;
        let json = serde_json::to_string(&et).unwrap();
        assert_eq!(json, "\"user\"");

        let et2: LineEntityType = serde_json::from_str(&json).unwrap();
        assert_eq!(et2, LineEntityType::User);
    }
}
