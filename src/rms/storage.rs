//! RMS Storage Operations
//!
//! Database operations specific to the Relationship Management System.

use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::Arc;

use super::types::*;
use crate::storage::StorageError;

/// RMS-specific storage operations
pub struct RmsStorage {
    conn: Arc<Mutex<Connection>>,
}

impl RmsStorage {
    /// Create a new RMS storage from shared connection
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Run schema migrations for RMS tables
    pub fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
        conn.execute_batch(
            r#"
            -- LINE entities (contacts, groups, rooms)
            CREATE TABLE IF NOT EXISTS line_entities (
                id TEXT PRIMARY KEY,
                entity_type TEXT NOT NULL CHECK (entity_type IN ('user', 'group', 'room')),
                display_name TEXT,
                picture_url TEXT,
                last_message_at INTEGER,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            -- Manual/override relationships
            CREATE TABLE IF NOT EXISTS relationships (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                line_entity_id TEXT NOT NULL,
                entity_type TEXT NOT NULL CHECK (entity_type IN ('user', 'group', 'room')),
                client_id TEXT NOT NULL,
                priority INTEGER DEFAULT 0,
                is_manual INTEGER DEFAULT 1,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                notes TEXT,
                FOREIGN KEY (line_entity_id) REFERENCES line_entities(id) ON DELETE CASCADE,
                UNIQUE(line_entity_id)
            );

            -- Dispatch history (for analytics)
            CREATE TABLE IF NOT EXISTS dispatch_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL,
                client_id TEXT NOT NULL,
                message_id TEXT,
                dispatched_at INTEGER NOT NULL,
                success INTEGER DEFAULT 1
            );

            -- Indexes
            CREATE INDEX IF NOT EXISTS idx_relationships_client ON relationships(client_id);
            CREATE INDEX IF NOT EXISTS idx_relationships_entity ON relationships(line_entity_id);
            CREATE INDEX IF NOT EXISTS idx_dispatch_conversation ON dispatch_history(conversation_id);
            CREATE INDEX IF NOT EXISTS idx_dispatch_time ON dispatch_history(dispatched_at);
            CREATE INDEX IF NOT EXISTS idx_entities_type ON line_entities(entity_type);
            CREATE INDEX IF NOT EXISTS idx_entities_last_message ON line_entities(last_message_at);
            "#,
        )?;
        Ok(())
    }

    // ========== Entity Operations ==========

    /// Get all LINE entities with optional filter
    pub fn get_entities(&self, filter: &EntityFilter) -> Result<Vec<LineEntity>, StorageError> {
        let conn = self.conn.lock();

        let mut sql = String::from(
            "SELECT id, entity_type, display_name, picture_url, last_message_at, created_at, updated_at 
             FROM line_entities WHERE 1=1"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(et) = filter.entity_type {
            sql.push_str(" AND entity_type = ?");
            params.push(Box::new(et.as_str().to_string()));
        }

        if let Some(search) = &filter.search {
            sql.push_str(" AND display_name LIKE ?");
            params.push(Box::new(format!("%{}%", search)));
        }

        // Handle has_relationship filter with subquery
        if let Some(has_rel) = filter.has_relationship {
            if has_rel {
                sql.push_str(" AND id IN (SELECT line_entity_id FROM relationships)");
            } else {
                sql.push_str(" AND id NOT IN (SELECT line_entity_id FROM relationships)");
            }
        }

        sql.push_str(" ORDER BY last_message_at DESC NULLS LAST, created_at DESC");

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {}", limit));
        }

        if let Some(offset) = filter.offset {
            sql.push_str(&format!(" OFFSET {}", offset));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();

        let mut stmt = conn.prepare(&sql)?;
        let entities = stmt
            .query_map(&params_refs[..], |row| {
                Ok(LineEntity {
                    id: row.get(0)?,
                    entity_type: LineEntityType::parse_entity_type(&row.get::<_, String>(1)?)
                        .unwrap_or(LineEntityType::User),
                    display_name: row.get(2)?,
                    picture_url: row.get(3)?,
                    last_message_at: row.get(4)?,
                    created_at: row.get(5)?,
                    updated_at: row.get(6)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(entities)
    }

    /// Get entity by ID
    pub fn get_entity(&self, id: &str) -> Result<Option<LineEntity>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, entity_type, display_name, picture_url, last_message_at, created_at, updated_at 
             FROM line_entities WHERE id = ?"
        )?;

        let result = stmt.query_row([id], |row| {
            Ok(LineEntity {
                id: row.get(0)?,
                entity_type: LineEntityType::parse_entity_type(&row.get::<_, String>(1)?)
                    .unwrap_or(LineEntityType::User),
                display_name: row.get(2)?,
                picture_url: row.get(3)?,
                last_message_at: row.get(4)?,
                created_at: row.get(5)?,
                updated_at: row.get(6)?,
            })
        });

        match result {
            Ok(entity) => Ok(Some(entity)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Upsert entity (create or update)
    pub fn upsert_entity(&self, entity: &LineEntity) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO line_entities (id, entity_type, display_name, picture_url, last_message_at, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT(id) DO UPDATE SET
                   entity_type = excluded.entity_type,
                   display_name = COALESCE(excluded.display_name, display_name),
                   picture_url = COALESCE(excluded.picture_url, picture_url),
                   last_message_at = COALESCE(excluded.last_message_at, last_message_at),
                   updated_at = excluded.updated_at"#,
            rusqlite::params![
                entity.id,
                entity.entity_type.as_str(),
                entity.display_name,
                entity.picture_url,
                entity.last_message_at,
                entity.created_at,
                entity.updated_at,
            ],
        )?;
        Ok(())
    }

    /// Update entity's last message time
    pub fn touch_entity(&self, id: &str, timestamp: i64) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE line_entities SET last_message_at = ?, updated_at = ? WHERE id = ?",
            rusqlite::params![timestamp, timestamp, id],
        )?;
        Ok(())
    }

    /// Count entities
    pub fn count_entities(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM line_entities", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    // ========== Relationship Operations ==========

    /// Get all relationships
    pub fn get_relationships(&self) -> Result<Vec<Relationship>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, line_entity_id, entity_type, client_id, priority, is_manual, created_at, updated_at, notes 
             FROM relationships ORDER BY updated_at DESC"
        )?;

        let relationships = stmt
            .query_map([], |row| {
                Ok(Relationship {
                    id: row.get(0)?,
                    line_entity_id: row.get(1)?,
                    entity_type: LineEntityType::parse_entity_type(&row.get::<_, String>(2)?)
                        .unwrap_or(LineEntityType::User),
                    client_id: row.get(3)?,
                    priority: row.get(4)?,
                    is_manual: row.get::<_, i64>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    notes: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(relationships)
    }

    /// Get relationship for an entity
    pub fn get_relationship(&self, entity_id: &str) -> Result<Option<Relationship>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, line_entity_id, entity_type, client_id, priority, is_manual, created_at, updated_at, notes 
             FROM relationships WHERE line_entity_id = ?"
        )?;

        let result = stmt.query_row([entity_id], |row| {
            Ok(Relationship {
                id: row.get(0)?,
                line_entity_id: row.get(1)?,
                entity_type: LineEntityType::parse_entity_type(&row.get::<_, String>(2)?)
                    .unwrap_or(LineEntityType::User),
                client_id: row.get(3)?,
                priority: row.get(4)?,
                is_manual: row.get::<_, i64>(5)? != 0,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
                notes: row.get(8)?,
            })
        });

        match result {
            Ok(rel) => Ok(Some(rel)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Set (create or update) a relationship
    pub fn set_relationship(
        &self,
        entity_id: &str,
        client_id: &str,
        is_manual: bool,
        notes: Option<&str>,
    ) -> Result<Relationship, StorageError> {
        let now = chrono::Utc::now().timestamp();
        let entity_type = LineEntityType::from_line_id(entity_id);

        let conn = self.conn.lock();
        conn.execute(
            r#"INSERT INTO relationships (line_entity_id, entity_type, client_id, priority, is_manual, created_at, updated_at, notes)
               VALUES (?, ?, ?, 0, ?, ?, ?, ?)
               ON CONFLICT(line_entity_id) DO UPDATE SET
                   client_id = excluded.client_id,
                   is_manual = excluded.is_manual,
                   updated_at = excluded.updated_at,
                   notes = COALESCE(excluded.notes, notes)"#,
            rusqlite::params![
                entity_id,
                entity_type.as_str(),
                client_id,
                is_manual as i64,
                now,
                now,
                notes,
            ],
        )?;

        // Fetch the inserted/updated relationship
        drop(conn);
        self.get_relationship(entity_id)?
            .ok_or_else(|| StorageError::Connection(rusqlite::Error::QueryReturnedNoRows))
    }

    /// Remove a relationship
    pub fn remove_relationship(&self, entity_id: &str) -> Result<bool, StorageError> {
        let conn = self.conn.lock();
        let affected = conn.execute(
            "DELETE FROM relationships WHERE line_entity_id = ?",
            [entity_id],
        )?;
        Ok(affected > 0)
    }

    /// Count relationships
    pub fn count_relationships(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();
        let count: i64 =
            conn.query_row("SELECT COUNT(*) FROM relationships", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Count manual relationships
    pub fn count_manual_relationships(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM relationships WHERE is_manual = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Get relationships by client
    pub fn get_relationships_by_client(
        &self,
        client_id: &str,
    ) -> Result<Vec<Relationship>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, line_entity_id, entity_type, client_id, priority, is_manual, created_at, updated_at, notes 
             FROM relationships WHERE client_id = ? ORDER BY updated_at DESC"
        )?;

        let relationships = stmt
            .query_map([client_id], |row| {
                Ok(Relationship {
                    id: row.get(0)?,
                    line_entity_id: row.get(1)?,
                    entity_type: LineEntityType::parse_entity_type(&row.get::<_, String>(2)?)
                        .unwrap_or(LineEntityType::User),
                    client_id: row.get(3)?,
                    priority: row.get(4)?,
                    is_manual: row.get::<_, i64>(5)? != 0,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    notes: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(relationships)
    }

    // ========== Dispatch History Operations ==========

    /// Record a dispatch event
    pub fn record_dispatch(
        &self,
        conversation_id: &str,
        client_id: &str,
        message_id: Option<&str>,
        success: bool,
    ) -> Result<(), StorageError> {
        let now = chrono::Utc::now().timestamp();
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO dispatch_history (conversation_id, client_id, message_id, dispatched_at, success)
             VALUES (?, ?, ?, ?, ?)",
            rusqlite::params![conversation_id, client_id, message_id, now, success as i64],
        )?;
        Ok(())
    }

    /// Get message count for a conversation
    pub fn get_message_count(&self, conversation_id: &str) -> Result<i64, StorageError> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM dispatch_history WHERE conversation_id = ? AND success = 1",
            [conversation_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get last dispatch time for a conversation
    pub fn get_last_dispatch_time(
        &self,
        conversation_id: &str,
    ) -> Result<Option<i64>, StorageError> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare("SELECT MAX(dispatched_at) FROM dispatch_history WHERE conversation_id = ?")?;
        let mut rows = stmt.query([conversation_id])?;
        if let Some(row) = rows.next()? {
            let val: Option<i64> = row.get(0)?;
            Ok(val)
        } else {
            Ok(None)
        }
    }

    /// Count pending messages (from main storage)
    pub fn count_pending_messages(&self) -> Result<i64, StorageError> {
        let conn = self.conn.lock();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM pending_messages", [], |row| {
            row.get(0)
        })?;
        Ok(count)
    }

    /// Clear all manual relationships
    pub fn clear_manual_relationships(&self) -> Result<usize, StorageError> {
        let conn = self.conn.lock();
        let affected = conn.execute("DELETE FROM relationships WHERE is_manual = 1", [])?;
        Ok(affected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn create_test_storage() -> RmsStorage {
        let conn = Connection::open_in_memory().unwrap();
        RmsStorage::run_migrations(&conn).unwrap();
        RmsStorage::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_upsert_and_get_entity() {
        let storage = create_test_storage();
        let now = chrono::Utc::now().timestamp();

        let entity = LineEntity {
            id: "U123456".to_string(),
            entity_type: LineEntityType::User,
            display_name: Some("Test User".to_string()),
            picture_url: None,
            last_message_at: Some(now),
            created_at: now,
            updated_at: now,
        };

        storage.upsert_entity(&entity).unwrap();
        let fetched = storage.get_entity("U123456").unwrap();
        assert!(fetched.is_some());
        assert_eq!(fetched.unwrap().display_name, Some("Test User".to_string()));
    }

    #[test]
    fn test_set_and_get_relationship() {
        let storage = create_test_storage();
        let now = chrono::Utc::now().timestamp();

        // First create an entity
        let entity = LineEntity {
            id: "U123456".to_string(),
            entity_type: LineEntityType::User,
            display_name: Some("Test User".to_string()),
            picture_url: None,
            last_message_at: Some(now),
            created_at: now,
            updated_at: now,
        };
        storage.upsert_entity(&entity).unwrap();

        // Set relationship
        let rel = storage
            .set_relationship("U123456", "client-001", true, Some("Test note"))
            .unwrap();
        assert_eq!(rel.client_id, "client-001");
        assert!(rel.is_manual);
        assert_eq!(rel.notes, Some("Test note".to_string()));
    }

    #[test]
    fn test_remove_relationship() {
        let storage = create_test_storage();
        let now = chrono::Utc::now().timestamp();

        let entity = LineEntity {
            id: "U123456".to_string(),
            entity_type: LineEntityType::User,
            display_name: None,
            picture_url: None,
            last_message_at: None,
            created_at: now,
            updated_at: now,
        };
        storage.upsert_entity(&entity).unwrap();
        storage
            .set_relationship("U123456", "client-001", true, None)
            .unwrap();

        let removed = storage.remove_relationship("U123456").unwrap();
        assert!(removed);

        let fetched = storage.get_relationship("U123456").unwrap();
        assert!(fetched.is_none());
    }

    #[test]
    fn test_entity_filter() {
        let storage = create_test_storage();
        let now = chrono::Utc::now().timestamp();

        // Create test entities
        for (id, etype, name) in [
            ("U111", LineEntityType::User, "User One"),
            ("U222", LineEntityType::User, "User Two"),
            ("C333", LineEntityType::Group, "Group One"),
        ] {
            storage
                .upsert_entity(&LineEntity {
                    id: id.to_string(),
                    entity_type: etype,
                    display_name: Some(name.to_string()),
                    picture_url: None,
                    last_message_at: Some(now),
                    created_at: now,
                    updated_at: now,
                })
                .unwrap();
        }

        // Filter by type
        let filter = EntityFilter {
            entity_type: Some(LineEntityType::User),
            ..Default::default()
        };
        let users = storage.get_entities(&filter).unwrap();
        assert_eq!(users.len(), 2);

        // Filter by search
        let filter = EntityFilter {
            search: Some("One".to_string()),
            ..Default::default()
        };
        let ones = storage.get_entities(&filter).unwrap();
        assert_eq!(ones.len(), 2); // "User One" and "Group One"
    }
}
