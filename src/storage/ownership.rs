//! Conversation ownership persistence

use crate::storage::StorageError;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::Arc;

/// Ownership record from database
#[derive(Debug, Clone)]
pub struct OwnershipRecord {
    pub conversation_id: String,
    pub client_id: String,
    pub claimed_at: i64,
    pub last_activity: i64,
}

/// Store for conversation ownership
#[derive(Debug)]
pub struct OwnershipStore {
    conn: Arc<Mutex<Connection>>,
}

impl OwnershipStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Claim ownership of a conversation
    pub fn claim(&self, conversation_id: &str, client_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT OR REPLACE INTO conversation_ownership 
             (conversation_id, client_id, claimed_at, last_activity)
             VALUES (?1, ?2, ?3, ?4)",
            [
                conversation_id,
                client_id,
                &now.to_string(),
                &now.to_string(),
            ],
        )?;

        tracing::debug!(
            "Claimed ownership: conversation={}, client={}",
            conversation_id,
            client_id
        );
        Ok(())
    }

    /// Release ownership of a conversation
    pub fn release(&self, conversation_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();

        conn.execute(
            "DELETE FROM conversation_ownership WHERE conversation_id = ?1",
            [conversation_id],
        )?;

        tracing::debug!("Released ownership: conversation={}", conversation_id);
        Ok(())
    }

    /// Release all conversations owned by a client (on disconnect)
    pub fn release_by_client(&self, client_id: &str) -> Result<usize, StorageError> {
        let conn = self.conn.lock();

        let rows = conn.execute(
            "DELETE FROM conversation_ownership WHERE client_id = ?1",
            [client_id],
        )?;

        tracing::debug!("Released {} conversations for client={}", rows, client_id);
        Ok(rows)
    }

    /// Get owner of a conversation
    pub fn get_owner(&self, conversation_id: &str) -> Result<Option<String>, StorageError> {
        let conn = self.conn.lock();

        let result = conn.query_row(
            "SELECT client_id FROM conversation_ownership WHERE conversation_id = ?1",
            [conversation_id],
            |row| row.get(0),
        );

        match result {
            Ok(client_id) => Ok(Some(client_id)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Update last activity for a conversation
    pub fn touch(&self, conversation_id: &str) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let now = chrono::Utc::now().timestamp();

        conn.execute(
            "UPDATE conversation_ownership SET last_activity = ?1 WHERE conversation_id = ?2",
            [&now.to_string(), conversation_id],
        )?;

        Ok(())
    }

    /// Get all conversations owned by a client
    pub fn get_client_conversations(&self, client_id: &str) -> Result<Vec<String>, StorageError> {
        let conn = self.conn.lock();

        let mut stmt = conn
            .prepare("SELECT conversation_id FROM conversation_ownership WHERE client_id = ?1")?;

        let conversations = stmt
            .query_map([client_id], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(conversations)
    }

    /// Get all ownership records (for debugging)
    pub fn get_all(&self) -> Result<Vec<OwnershipRecord>, StorageError> {
        let conn = self.conn.lock();

        let mut stmt = conn.prepare(
            "SELECT conversation_id, client_id, claimed_at, last_activity 
             FROM conversation_ownership",
        )?;

        let records = stmt
            .query_map([], |row| {
                Ok(OwnershipRecord {
                    conversation_id: row.get(0)?,
                    client_id: row.get(1)?,
                    claimed_at: row.get(2)?,
                    last_activity: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Count total owned conversations
    pub fn count(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();

        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM conversation_ownership", [], |row| {
                row.get(0)
            })?;

        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use crate::storage::Storage;
    use tempfile::tempdir;

    #[test]
    fn test_ownership_claim_release() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::with_path(&db_path).unwrap();
        let store = storage.ownership();

        // Claim ownership
        store.claim("conv1", "client1").unwrap();

        // Get owner
        let owner = store.get_owner("conv1").unwrap();
        assert_eq!(owner, Some("client1".to_string()));

        // Release
        store.release("conv1").unwrap();

        let owner = store.get_owner("conv1").unwrap();
        assert_eq!(owner, None);
    }

    #[test]
    fn test_release_by_client() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::with_path(&db_path).unwrap();
        let store = storage.ownership();

        // Claim multiple conversations
        store.claim("conv1", "client1").unwrap();
        store.claim("conv2", "client1").unwrap();
        store.claim("conv3", "client2").unwrap();

        // Release all for client1
        let released = store.release_by_client("client1").unwrap();
        assert_eq!(released, 2);

        // Verify client2 still owns conv3
        let owner = store.get_owner("conv3").unwrap();
        assert_eq!(owner, Some("client2".to_string()));
    }
}
