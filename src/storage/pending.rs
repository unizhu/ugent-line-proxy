//! Pending messages persistence

use crate::storage::StorageError;
use crate::types::PendingMessage;
use parking_lot::Mutex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Pending message record for database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMessageRecord {
    pub original_id: String,
    pub conversation_id: String,
    pub reply_token: Option<String>,
    pub received_at: i64,
    pub reply_token_expires_at: Option<i64>,
    pub webhook_event_id: String,
    pub client_id: Option<String>,
}

impl From<&PendingMessage> for PendingMessageRecord {
    fn from(msg: &PendingMessage) -> Self {
        // Convert Instant to Unix timestamp (approximate, since we can't get exact timestamp from Instant)
        let now = chrono::Utc::now().timestamp();
        Self {
            original_id: msg.original_id.clone(),
            conversation_id: msg.conversation_id.clone(),
            reply_token: msg.reply_token.clone(),
            received_at: now,
            reply_token_expires_at: if msg.reply_token_expires_at.is_some() {
                Some(now + 55)
            } else {
                None
            },
            webhook_event_id: msg.webhook_event_id.clone(),
            client_id: msg.client_id.clone(),
        }
    }
}

/// Store for pending messages
#[derive(Debug)]
pub struct PendingMessageStore {
    conn: Arc<Mutex<Connection>>,
}

impl PendingMessageStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Store a pending message
    pub fn store(&self, msg: &PendingMessage) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let record = PendingMessageRecord::from(msg);

        conn.execute(
            "INSERT OR REPLACE INTO pending_messages 
             (original_id, conversation_id, reply_token, received_at, 
              reply_token_expires_at, webhook_event_id, client_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                record.original_id,
                record.conversation_id,
                record.reply_token,
                record.received_at,
                record.reply_token_expires_at,
                record.webhook_event_id,
                record.client_id,
            ],
        )?;

        tracing::trace!("Stored pending message: {}", msg.original_id);
        Ok(())
    }

    /// Get and remove a pending message
    pub fn take(&self, original_id: &str) -> Result<Option<PendingMessageRecord>, StorageError> {
        let conn = self.conn.lock();

        // First, get the record
        let result = conn.query_row(
            "SELECT original_id, conversation_id, reply_token, received_at,
                    reply_token_expires_at, webhook_event_id, client_id
             FROM pending_messages WHERE original_id = ?1",
            [original_id],
            |row| {
                Ok(PendingMessageRecord {
                    original_id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    reply_token: row.get(2)?,
                    received_at: row.get(3)?,
                    reply_token_expires_at: row.get(4)?,
                    webhook_event_id: row.get(5)?,
                    client_id: row.get(6)?,
                })
            },
        );

        match result {
            Ok(record) => {
                // Delete it
                conn.execute(
                    "DELETE FROM pending_messages WHERE original_id = ?1",
                    [original_id],
                )?;
                Ok(Some(record))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get a pending message without removing it
    pub fn get(&self, original_id: &str) -> Result<Option<PendingMessageRecord>, StorageError> {
        let conn = self.conn.lock();

        let result = conn.query_row(
            "SELECT original_id, conversation_id, reply_token, received_at,
                    reply_token_expires_at, webhook_event_id, client_id
             FROM pending_messages WHERE original_id = ?1",
            [original_id],
            |row| {
                Ok(PendingMessageRecord {
                    original_id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    reply_token: row.get(2)?,
                    received_at: row.get(3)?,
                    reply_token_expires_at: row.get(4)?,
                    webhook_event_id: row.get(5)?,
                    client_id: row.get(6)?,
                })
            },
        );

        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update the client_id for a pending message
    pub fn set_client(&self, original_id: &str, client_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();

        conn.execute(
            "UPDATE pending_messages SET client_id = ?1 WHERE original_id = ?2",
            [client_id, original_id],
        )?;

        Ok(())
    }

    /// Get all pending messages for a conversation
    pub fn get_by_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<PendingMessageRecord>, StorageError> {
        let conn = self.conn.lock();

        let mut stmt = conn.prepare(
            "SELECT original_id, conversation_id, reply_token, received_at,
                    reply_token_expires_at, webhook_event_id, client_id
             FROM pending_messages WHERE conversation_id = ?1",
        )?;

        let records = stmt
            .query_map([conversation_id], |row| {
                Ok(PendingMessageRecord {
                    original_id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    reply_token: row.get(2)?,
                    received_at: row.get(3)?,
                    reply_token_expires_at: row.get(4)?,
                    webhook_event_id: row.get(5)?,
                    client_id: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Delete expired messages (messages older than 5 minutes)
    pub fn delete_expired(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();

        let rows = conn.execute(
            "DELETE FROM pending_messages WHERE received_at < strftime('%s', 'now') - 300",
            [],
        )?;

        if rows > 0 {
            tracing::debug!("Deleted {} expired pending messages", rows);
        }

        Ok(rows)
    }

    /// Count pending messages
    pub fn count(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();

        let count: i64 = conn.query_row("SELECT COUNT(*) FROM pending_messages", [], |row| {
            row.get(0)
        })?;

        Ok(count as usize)
    }

    /// Get oldest pending messages (for debugging)
    pub fn get_oldest(&self, limit: usize) -> Result<Vec<PendingMessageRecord>, StorageError> {
        let conn = self.conn.lock();

        let mut stmt = conn.prepare(
            "SELECT original_id, conversation_id, reply_token, received_at,
                    reply_token_expires_at, webhook_event_id, client_id
             FROM pending_messages ORDER BY received_at ASC LIMIT ?1",
        )?;

        let records = stmt
            .query_map([limit as i64], |row| {
                Ok(PendingMessageRecord {
                    original_id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    reply_token: row.get(2)?,
                    received_at: row.get(3)?,
                    reply_token_expires_at: row.get(4)?,
                    webhook_event_id: row.get(5)?,
                    client_id: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use tempfile::tempdir;

    fn create_test_message() -> PendingMessage {
        PendingMessage {
            original_id: "test-msg-1".to_string(),
            conversation_id: "conv-123".to_string(),
            reply_token: Some("reply-token-abc".to_string()),
            received_at: std::time::Instant::now(),
            reply_token_expires_at: Some(
                std::time::Instant::now() + std::time::Duration::from_secs(55),
            ),
            webhook_event_id: "event-789".to_string(),
            client_id: None,
            mark_as_read_token: None,
        }
    }

    #[test]
    fn test_store_and_take() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::with_path(&db_path).unwrap();
        let store = storage.pending();

        let msg = create_test_message();

        // Store
        store.store(&msg).unwrap();

        // Count
        let count = store.count().unwrap();
        assert_eq!(count, 1);

        // Take
        let retrieved = store.take(&msg.original_id).unwrap();
        assert!(retrieved.is_some());

        let record = retrieved.unwrap();
        assert_eq!(record.conversation_id, "conv-123");

        // Should be gone
        let count = store.count().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_set_client() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::with_path(&db_path).unwrap();
        let store = storage.pending();

        let msg = create_test_message();
        store.store(&msg).unwrap();

        // Set client
        store.set_client(&msg.original_id, "client-789").unwrap();

        // Verify
        let record = store.get(&msg.original_id).unwrap().unwrap();
        assert_eq!(record.client_id, Some("client-789".to_string()));
    }
}
