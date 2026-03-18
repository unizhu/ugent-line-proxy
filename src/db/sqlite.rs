//! SQLite database backend
//!
//! Implements DatabaseBackend trait using rusqlite with spawn_blocking
//! for async compatibility.

use crate::db::DatabaseBackend;
use crate::db::config::DataConfig;
use crate::db::contacts::ContactRecord;
use crate::db::error::DbError;
use crate::db::groups::GroupRecord;
use crate::db::inbound_queue::InboundQueueEntry;
use crate::db::messages::{DeliveryStatus, MessageRecord};
use crate::db::metrics::MetricRecord;
use crate::db::migration::V2_MIGRATION_SQLITE;
use crate::db::outbound_queue::OutboundQueueEntry;
use async_trait::async_trait;
use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::OpenFlags;
use std::path::Path;
use std::sync::Arc;

/// SQLite backend using rusqlite behind a Mutex
pub struct SqliteBackend {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SqliteBackend {
    /// Open or create a SQLite database at the given path
    pub fn open(path: &Path) -> Result<Self, DbError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;

        let conn = rusqlite::Connection::open_with_flags(path, flags)
            .map_err(|e| DbError::Connection(e.to_string()))?;

        conn.execute_batch("PRAGMA journal_mode=WAL")
            .map_err(|e| DbError::Connection(e.to_string()))?;
        conn.execute_batch("PRAGMA foreign_keys=ON")
            .map_err(|e| DbError::Connection(e.to_string()))?;
        conn.execute_batch("PRAGMA busy_timeout=5000")
            .map_err(|e| DbError::Connection(e.to_string()))?;

        let backend = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        backend.init_schema()?;

        Ok(backend)
    }

    /// Create from DataConfig
    pub fn from_config(config: &DataConfig) -> Result<Self, DbError> {
        let path = match &config.path {
            Some(p) => p.clone(),
            None => {
                let base = dirs::data_local_dir()
                    .or_else(dirs::home_dir)
                    .unwrap_or_else(|| Path::new(".").to_path_buf());
                base.join(".ugent")
                    .join("line-plugin")
                    .join("line-proxy.db")
            }
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Config(format!("Cannot create DB directory: {e}")))?;
        }

        Self::open(&path)
    }

    /// Initialize or migrate schema
    fn init_schema(&self) -> Result<(), DbError> {
        let conn = self.conn.lock();

        // Ensure schema_version table exists
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            )",
        )
        .map_err(|e| DbError::Migration(e.to_string()))?;

        // Check current version
        let version: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Run legacy v1 tables if needed (backward compat)
        if version < 1 {
            self.run_v1_schema(&conn)?;
        }

        // Run v2 tables if needed
        if version < 2 {
            for sql in V2_MIGRATION_SQLITE {
                conn.execute_batch(sql)
                    .map_err(|e| DbError::Migration(format!("v2 migration failed: {e}")))?;
            }
        }

        Ok(())
    }

    /// Create v1 legacy tables (for backward compatibility with existing installs)
    fn run_v1_schema(&self, conn: &rusqlite::Connection) -> Result<(), DbError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversation_ownership (
                conversation_id TEXT PRIMARY KEY,
                client_id TEXT NOT NULL,
                claimed_at INTEGER NOT NULL,
                last_heartbeat INTEGER NOT NULL
            )",
        )
        .map_err(|e| DbError::Migration(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS pending_messages (
                response_id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                created_at INTEGER NOT NULL
            )",
        )
        .map_err(|e| DbError::Migration(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS metrics (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL,
                value INTEGER NOT NULL,
                recorded_at INTEGER NOT NULL
            )",
        )
        .map_err(|e| DbError::Migration(e.to_string()))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS webhook_dedup (
                event_id TEXT PRIMARY KEY,
                processed_at INTEGER NOT NULL
            )",
        )
        .map_err(|e| DbError::Migration(e.to_string()))?;

        conn.execute_batch(
            "INSERT OR IGNORE INTO schema_version (version, applied_at) VALUES (1, unixepoch('now') * 1000)",
        )
        .map_err(|e| DbError::Migration(e.to_string()))?;

        Ok(())
    }

    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }
}

#[async_trait]
impl DatabaseBackend for SqliteBackend {
    async fn ping(&self) -> Result<bool, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute_batch("SELECT 1").is_ok()
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))
    }

    async fn run_maintenance(&self) -> Result<(), DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();

            // Clean expired inbound queue entries
            let expired = conn
                .execute(
                    "UPDATE inbound_queue SET status = 'expired' WHERE status = 'pending' AND expires_at < ?1",
                    rusqlite::params![now],
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            if expired > 0 {
                tracing::info!(count = expired, "Cleaned expired inbound queue entries");
            }

            // Clean completed outbound queue entries older than 1 hour
            let one_hour_ago = now - 3600 * 1000;
            conn.execute(
                "DELETE FROM outbound_queue WHERE status IN ('completed', 'failed') AND updated_at < ?1",
                rusqlite::params![one_hour_ago],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

            // Clean expired inbound entries older than 1 hour
            conn.execute(
                "DELETE FROM inbound_queue WHERE status IN ('completed', 'expired') AND created_at < ?1",
                rusqlite::params![one_hour_ago],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Contacts ==========

    async fn upsert_contact(&self, contact: &ContactRecord) -> Result<(), DbError> {
        let contact = contact.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO contacts (
                    line_user_id, display_name, picture_url, status_message, language,
                    first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                    created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(line_user_id) DO UPDATE SET
                    display_name = excluded.display_name,
                    picture_url = excluded.picture_url,
                    status_message = excluded.status_message,
                    language = excluded.language,
                    last_seen_at = excluded.last_seen_at,
                    last_interacted_at = COALESCE(excluded.last_interacted_at, contacts.last_interacted_at),
                    is_blocked = excluded.is_blocked,
                    is_friend = excluded.is_friend,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    contact.line_user_id,
                    contact.display_name,
                    contact.picture_url,
                    contact.status_message,
                    contact.language,
                    contact.first_seen_at,
                    contact.last_seen_at,
                    contact.last_interacted_at,
                    contact.is_blocked,
                    contact.is_friend,
                    contact.created_at,
                    contact.updated_at,
                ],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn get_contact(&self, line_user_id: &str) -> Result<Option<ContactRecord>, DbError> {
        let uid = line_user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT line_user_id, display_name, picture_url, status_message, language,
                            first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                            created_at, updated_at
                     FROM contacts WHERE line_user_id = ?1",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let result = stmt
                .query_row(rusqlite::params![uid], |row| {
                    Ok(ContactRecord {
                        line_user_id: row.get(0)?,
                        display_name: row.get(1)?,
                        picture_url: row.get(2)?,
                        status_message: row.get(3)?,
                        language: row.get(4)?,
                        first_seen_at: row.get(5)?,
                        last_seen_at: row.get(6)?,
                        last_interacted_at: row.get(7)?,
                        is_blocked: row.get::<_, i64>(8)? != 0,
                        is_friend: row.get::<_, i64>(9)? != 0,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                })
                .ok();

            Ok(result)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn list_contacts(&self, offset: u64, limit: u64) -> Result<Vec<ContactRecord>, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT line_user_id, display_name, picture_url, status_message, language,
                            first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                            created_at, updated_at
                     FROM contacts ORDER BY last_seen_at DESC LIMIT ?1 OFFSET ?2",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![limit, offset], |row| {
                    Ok(ContactRecord {
                        line_user_id: row.get(0)?,
                        display_name: row.get(1)?,
                        picture_url: row.get(2)?,
                        status_message: row.get(3)?,
                        language: row.get(4)?,
                        first_seen_at: row.get(5)?,
                        last_seen_at: row.get(6)?,
                        last_interacted_at: row.get(7)?,
                        is_blocked: row.get::<_, i64>(8)? != 0,
                        is_friend: row.get::<_, i64>(9)? != 0,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                })
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut contacts = Vec::new();
            for row in rows {
                contacts.push(row.map_err(|e| DbError::Query(e.to_string()))?);
            }
            Ok(contacts)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn search_contacts(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<ContactRecord>, DbError> {
        // Escape LIKE wildcards to prevent LIKE injection
        let escaped = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let query = format!("%{escaped}%");
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT line_user_id, display_name, picture_url, status_message, language,
                            first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                            created_at, updated_at
                     FROM contacts WHERE display_name LIKE ?1 ESCAPE '\\' ORDER BY last_seen_at DESC LIMIT ?2",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![query, limit], |row| {
                    Ok(ContactRecord {
                        line_user_id: row.get(0)?,
                        display_name: row.get(1)?,
                        picture_url: row.get(2)?,
                        status_message: row.get(3)?,
                        language: row.get(4)?,
                        first_seen_at: row.get(5)?,
                        last_seen_at: row.get(6)?,
                        last_interacted_at: row.get(7)?,
                        is_blocked: row.get::<_, i64>(8)? != 0,
                        is_friend: row.get::<_, i64>(9)? != 0,
                        created_at: row.get(10)?,
                        updated_at: row.get(11)?,
                    })
                })
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut contacts = Vec::new();
            for row in rows {
                contacts.push(row.map_err(|e| DbError::Query(e.to_string()))?);
            }
            Ok(contacts)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Groups ==========

    async fn upsert_group(&self, group: &GroupRecord) -> Result<(), DbError> {
        let group = group.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO groups (
                    line_group_id, group_name, picture_url, member_count,
                    first_seen_at, last_message_at, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                ON CONFLICT(line_group_id) DO UPDATE SET
                    group_name = excluded.group_name,
                    picture_url = excluded.picture_url,
                    member_count = excluded.member_count,
                    last_message_at = COALESCE(excluded.last_message_at, groups.last_message_at),
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    group.line_group_id,
                    group.group_name,
                    group.picture_url,
                    group.member_count,
                    group.first_seen_at,
                    group.last_message_at,
                    group.created_at,
                    group.updated_at,
                ],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn get_group(&self, line_group_id: &str) -> Result<Option<GroupRecord>, DbError> {
        let gid = line_group_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let result = conn
                .query_row(
                    "SELECT line_group_id, group_name, picture_url, member_count,
                            first_seen_at, last_message_at, created_at, updated_at
                     FROM groups WHERE line_group_id = ?1",
                    rusqlite::params![gid],
                    |row| {
                        Ok(GroupRecord {
                            line_group_id: row.get(0)?,
                            group_name: row.get(1)?,
                            picture_url: row.get(2)?,
                            member_count: row.get(3)?,
                            first_seen_at: row.get(4)?,
                            last_message_at: row.get(5)?,
                            created_at: row.get(6)?,
                            updated_at: row.get(7)?,
                        })
                    },
                )
                .ok();

            Ok(result)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn add_group_member(&self, group_id: &str, user_id: &str) -> Result<(), DbError> {
        let gid = group_id.to_string();
        let uid = user_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT OR IGNORE INTO group_members (line_group_id, line_user_id, joined_at)
                 VALUES (?1, ?2, unixepoch('now') * 1000)",
                rusqlite::params![gid, uid],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn list_groups(&self, offset: u64, limit: u64) -> Result<Vec<GroupRecord>, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT line_group_id, group_name, picture_url, member_count,
                            first_seen_at, last_message_at, created_at, updated_at
                     FROM groups ORDER BY last_message_at DESC LIMIT ?1 OFFSET ?2",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![limit, offset], |row| {
                    Ok(GroupRecord {
                        line_group_id: row.get(0)?,
                        group_name: row.get(1)?,
                        picture_url: row.get(2)?,
                        member_count: row.get(3)?,
                        first_seen_at: row.get(4)?,
                        last_message_at: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut groups = Vec::new();
            for row in rows {
                groups.push(row.map_err(|e| DbError::Query(e.to_string()))?);
            }
            Ok(groups)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Messages ==========

    async fn store_message(&self, msg: &MessageRecord) -> Result<(), DbError> {
        let msg = msg.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO messages (
                    id, direction, conversation_id, source_type, sender_id, message_type,
                    text_content, message_json, media_content_json, reply_token, quote_token,
                    webhook_event_id, line_timestamp, received_at, delivered_at,
                    delivery_status, retry_count, last_retry_at, error_message,
                    ugent_request_id, ugent_correlation_id, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
                ON CONFLICT(id) DO UPDATE SET
                    delivery_status = excluded.delivery_status,
                    delivered_at = COALESCE(excluded.delivered_at, messages.delivered_at),
                    error_message = COALESCE(excluded.error_message, messages.error_message),
                    retry_count = MAX(messages.retry_count, excluded.retry_count)",
                rusqlite::params![
                    msg.id,
                    msg.direction,
                    msg.conversation_id,
                    msg.source_type,
                    msg.sender_id,
                    msg.message_type,
                    msg.text_content,
                    msg.message_json,
                    msg.media_content_json,
                    msg.reply_token,
                    msg.quote_token,
                    msg.webhook_event_id,
                    msg.line_timestamp,
                    msg.received_at,
                    msg.delivered_at,
                    msg.delivery_status.as_str(),
                    msg.retry_count,
                    msg.last_retry_at,
                    msg.error_message,
                    msg.ugent_request_id,
                    msg.ugent_correlation_id,
                    msg.created_at,
                ],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn get_message(&self, id: &str) -> Result<Option<MessageRecord>, DbError> {
        let msg_id = id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let result = conn
                .query_row(
                    "SELECT id, direction, conversation_id, source_type, sender_id, message_type,
                            text_content, message_json, media_content_json, reply_token, quote_token,
                            webhook_event_id, line_timestamp, received_at, delivered_at,
                            delivery_status, retry_count, last_retry_at, error_message,
                            ugent_request_id, ugent_correlation_id, created_at
                     FROM messages WHERE id = ?1",
                    rusqlite::params![msg_id],
                    |row| {
                        let status_str: String = row.get(15)?;
                        Ok(MessageRecord {
                            id: row.get(0)?,
                            direction: row.get(1)?,
                            conversation_id: row.get(2)?,
                            source_type: row.get(3)?,
                            sender_id: row.get(4)?,
                            message_type: row.get(5)?,
                            text_content: row.get(6)?,
                            message_json: row.get(7)?,
                            media_content_json: row.get(8)?,
                            reply_token: row.get(9)?,
                            quote_token: row.get(10)?,
                            webhook_event_id: row.get(11)?,
                            line_timestamp: row.get(12)?,
                            received_at: row.get(13)?,
                            delivered_at: row.get(14)?,
                            delivery_status: DeliveryStatus::parse(&status_str)
                                .unwrap_or(DeliveryStatus::Pending),
                            retry_count: row.get(16)?,
                            last_retry_at: row.get(17)?,
                            error_message: row.get(18)?,
                            ugent_request_id: row.get(19)?,
                            ugent_correlation_id: row.get(20)?,
                            created_at: row.get(21)?,
                        })
                    },
                )
                .ok();

            Ok(result)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn list_messages(
        &self,
        conversation_id: &str,
        direction: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<MessageRecord>, DbError> {
        let conv_id = conversation_id.to_string();
        let dir = direction.map(|d| d.to_string());
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let (sql, with_dir) =
                if dir.is_some() {
                    ("SELECT id, direction, conversation_id, source_type, sender_id, message_type,
                        text_content, message_json, media_content_json, reply_token, quote_token,
                        webhook_event_id, line_timestamp, received_at, delivered_at,
                        delivery_status, retry_count, last_retry_at, error_message,
                        ugent_request_id, ugent_correlation_id, created_at
                 FROM messages WHERE conversation_id = ?1 AND direction = ?2
                 ORDER BY received_at DESC LIMIT ?3 OFFSET ?4", true)
                } else {
                    ("SELECT id, direction, conversation_id, source_type, sender_id, message_type,
                        text_content, message_json, media_content_json, reply_token, quote_token,
                        webhook_event_id, line_timestamp, received_at, delivered_at,
                        delivery_status, retry_count, last_retry_at, error_message,
                        ugent_request_id, ugent_correlation_id, created_at
                 FROM messages WHERE conversation_id = ?1
                 ORDER BY received_at DESC LIMIT ?2 OFFSET ?3", false)
                };

            let mut stmt = conn
                .prepare(sql)
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut messages = Vec::new();
            if with_dir {
                let rows = stmt
                    .query_map(
                        rusqlite::params![conv_id, dir, limit, offset],
                        parse_message_row,
                    )
                    .map_err(|e| DbError::Query(e.to_string()))?;
                for row in rows {
                    messages.push(row.map_err(|e| DbError::Query(e.to_string()))?);
                }
            } else {
                let rows = stmt
                    .query_map(rusqlite::params![conv_id, limit, offset], parse_message_row)
                    .map_err(|e| DbError::Query(e.to_string()))?;
                for row in rows {
                    messages.push(row.map_err(|e| DbError::Query(e.to_string()))?);
                }
            }
            Ok(messages)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn update_delivery_status(
        &self,
        id: &str,
        status: DeliveryStatus,
        error: Option<&str>,
    ) -> Result<(), DbError> {
        let msg_id = id.to_string();
        let err_msg = error.map(|e| e.to_string());
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            conn.execute(
                "UPDATE messages SET delivery_status = ?1, error_message = ?2, delivered_at = ?3
                 WHERE id = ?4",
                rusqlite::params![status.as_str(), err_msg, now, msg_id],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn increment_retry_count(&self, id: &str) -> Result<(), DbError> {
        let msg_id = id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            conn.execute(
                "UPDATE messages SET retry_count = retry_count + 1, last_retry_at = ?1
                 WHERE id = ?2",
                rusqlite::params![now, msg_id],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Outbound Queue ==========

    async fn enqueue_outbound(&self, entry: &OutboundQueueEntry) -> Result<(), DbError> {
        let entry = entry.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO outbound_queue (
                    id, message_id, status, attempt, max_attempts, next_retry_at,
                    locked_by, locked_at, last_error, created_at, updated_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(id) DO UPDATE SET
                    status = excluded.status,
                    next_retry_at = excluded.next_retry_at,
                    updated_at = excluded.updated_at",
                rusqlite::params![
                    entry.id,
                    entry.message_id,
                    entry.status,
                    entry.attempt,
                    entry.max_attempts,
                    entry.next_retry_at,
                    entry.locked_by,
                    entry.locked_at,
                    entry.last_error,
                    entry.created_at,
                    entry.updated_at,
                ],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn claim_next_outbound(
        &self,
        worker_id: &str,
        limit: u64,
    ) -> Result<Vec<OutboundQueueEntry>, DbError> {
        let wid = worker_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();

            // Claim entries: atomically update status and lock
            conn.execute(
                "UPDATE outbound_queue SET status = 'processing', locked_by = ?1, locked_at = ?2
                 WHERE id IN (
                     SELECT id FROM outbound_queue
                     WHERE status = 'pending' AND next_retry_at <= ?3
                     ORDER BY next_retry_at ASC LIMIT ?4
                 )",
                rusqlite::params![wid, now, now, limit],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

            // Read claimed entries back
            let mut stmt = conn
                .prepare(
                    "SELECT id, message_id, status, attempt, max_attempts, next_retry_at,
                            locked_by, locked_at, last_error, created_at, updated_at
                     FROM outbound_queue WHERE locked_by = ?1 ORDER BY next_retry_at ASC",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![wid], |row| {
                    Ok(OutboundQueueEntry {
                        id: row.get(0)?,
                        message_id: row.get(1)?,
                        status: row.get(2)?,
                        attempt: row.get(3)?,
                        max_attempts: row.get(4)?,
                        next_retry_at: row.get(5)?,
                        locked_by: row.get(6)?,
                        locked_at: row.get(7)?,
                        last_error: row.get(8)?,
                        created_at: row.get(9)?,
                        updated_at: row.get(10)?,
                    })
                })
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row.map_err(|e| DbError::Query(e.to_string()))?);
            }
            Ok(entries)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn complete_outbound(
        &self,
        id: &str,
        success: bool,
        error: Option<&str>,
    ) -> Result<(), DbError> {
        let eid = id.to_string();
        let err_msg = error.map(|e| e.to_string());
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            let status = if success { "completed" } else { "failed" };
            conn.execute(
                "UPDATE outbound_queue SET status = ?1, last_error = ?2, updated_at = ?3, locked_by = NULL, locked_at = NULL
                 WHERE id = ?4",
                rusqlite::params![status, err_msg, now, eid],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn pending_outbound_count(&self) -> Result<u64, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM outbound_queue WHERE status = 'pending'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(count as u64)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Inbound Queue ==========

    async fn enqueue_inbound(&self, entry: &InboundQueueEntry) -> Result<(), DbError> {
        let entry = entry.clone();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO inbound_queue (id, message_id, status, expires_at, locked_by, locked_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![
                    entry.id,
                    entry.message_id,
                    entry.status,
                    entry.expires_at,
                    entry.locked_by,
                    entry.locked_at,
                    entry.created_at,
                ],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn claim_next_inbound(
        &self,
        worker_id: &str,
        limit: u64,
    ) -> Result<Vec<InboundQueueEntry>, DbError> {
        let wid = worker_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();

            conn.execute(
                "UPDATE inbound_queue SET status = 'processing', locked_by = ?1, locked_at = ?2
                 WHERE id IN (
                     SELECT id FROM inbound_queue
                     WHERE status = 'pending' AND expires_at > ?3
                     ORDER BY created_at ASC LIMIT ?4
                 )",
                rusqlite::params![wid, now, now, limit],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;

            let mut stmt = conn
                .prepare(
                    "SELECT id, message_id, status, expires_at, locked_by, locked_at, created_at
                     FROM inbound_queue WHERE locked_by = ?1 ORDER BY created_at ASC",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![wid], |row| {
                    Ok(InboundQueueEntry {
                        id: row.get(0)?,
                        message_id: row.get(1)?,
                        status: row.get(2)?,
                        expires_at: row.get(3)?,
                        locked_by: row.get(4)?,
                        locked_at: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                })
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row.map_err(|e| DbError::Query(e.to_string()))?);
            }
            Ok(entries)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn complete_inbound(&self, id: &str) -> Result<(), DbError> {
        let eid = id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "UPDATE inbound_queue SET status = 'completed', locked_by = NULL, locked_at = NULL
                 WHERE id = ?1",
                rusqlite::params![eid],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn pending_inbound_count(&self) -> Result<u64, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM inbound_queue WHERE status = 'pending'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            Ok(count as u64)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn cleanup_expired_inbound(&self) -> Result<u64, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            let deleted = conn
                .execute(
                    "UPDATE inbound_queue SET status = 'expired' WHERE status = 'pending' AND expires_at < ?1",
                    rusqlite::params![now],
                )
                .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(deleted as u64)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Metrics ==========

    async fn record_metric(&self, name: &str, value: i64) -> Result<(), DbError> {
        let mname = name.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            conn.execute(
                "INSERT INTO metrics (name, value, recorded_at) VALUES (?1, ?2, unixepoch('now') * 1000)",
                rusqlite::params![mname, value],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn get_metrics(&self, name: &str, since: i64) -> Result<Vec<MetricRecord>, DbError> {
        let mname = name.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let mut stmt = conn
                .prepare(
                    "SELECT name, value, recorded_at FROM metrics WHERE name = ?1 AND recorded_at >= ?2 ORDER BY recorded_at ASC",
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            let rows = stmt
                .query_map(rusqlite::params![mname, since], |row| {
                    Ok(MetricRecord {
                        name: row.get(0)?,
                        value: row.get(1)?,
                        recorded_at: row.get(2)?,
                    })
                })
                .map_err(|e| DbError::Query(e.to_string()))?;

            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(|e| DbError::Query(e.to_string()))?);
            }
            Ok(records)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Webhook Dedup ==========

    async fn check_and_mark_webhook(&self, event_id: &str) -> Result<bool, DbError> {
        let eid = event_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();

            // Try INSERT OR IGNORE (atomic check-and-mark)
            let inserted = conn
                .execute(
                    "INSERT OR IGNORE INTO webhook_dedup (event_id, processed_at) VALUES (?1, ?2)",
                    rusqlite::params![eid, now],
                )
                .map_err(|e| DbError::Query(e.to_string()))?;

            Ok(inserted == 0) // 0 rows inserted means already existed
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn cleanup_webhook_dedup(&self, max_age_secs: i64) -> Result<u64, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            let cutoff = now - (max_age_secs * 1000);
            let deleted = conn
                .execute(
                    "DELETE FROM webhook_dedup WHERE processed_at < ?1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(deleted as u64)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    // ========== Conversation Ownership ==========

    async fn set_conversation_owner(
        &self,
        conversation_id: &str,
        client_id: &str,
    ) -> Result<(), DbError> {
        let cid = conversation_id.to_string();
        let clid = client_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            conn.execute(
                "INSERT INTO conversation_ownership (conversation_id, client_id, claimed_at, last_heartbeat)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(conversation_id) DO UPDATE SET
                     client_id = excluded.client_id,
                     claimed_at = excluded.claimed_at,
                     last_heartbeat = excluded.last_heartbeat",
                rusqlite::params![cid, clid, now],
            )
            .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(())
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn get_conversation_owner(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>, DbError> {
        let cid = conversation_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let result = conn
                .query_row(
                    "SELECT client_id FROM conversation_ownership WHERE conversation_id = ?1",
                    rusqlite::params![cid],
                    |row| row.get::<_, String>(0),
                )
                .ok();
            Ok(result)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn release_client_conversations(&self, client_id: &str) -> Result<u64, DbError> {
        let clid = client_id.to_string();
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let deleted = conn
                .execute(
                    "DELETE FROM conversation_ownership WHERE client_id = ?1",
                    rusqlite::params![clid],
                )
                .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(deleted as u64)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }

    async fn cleanup_stale_ownership(&self, max_age_secs: i64) -> Result<u64, DbError> {
        let conn = Arc::clone(&self.conn);
        tokio::task::spawn_blocking(move || {
            let conn = conn.lock();
            let now = Self::now_ms();
            let cutoff = now - (max_age_secs * 1000);
            let deleted = conn
                .execute(
                    "DELETE FROM conversation_ownership WHERE last_heartbeat < ?1",
                    rusqlite::params![cutoff],
                )
                .map_err(|e| DbError::Query(e.to_string()))?;
            Ok(deleted as u64)
        })
        .await
        .map_err(|e| DbError::Connection(e.to_string()))?
    }
}

/// Helper to parse a message row from SQLite
fn parse_message_row(row: &rusqlite::Row<'_>) -> Result<MessageRecord, rusqlite::Error> {
    let status_str: String = row.get(15)?;
    Ok(MessageRecord {
        id: row.get(0)?,
        direction: row.get(1)?,
        conversation_id: row.get(2)?,
        source_type: row.get(3)?,
        sender_id: row.get(4)?,
        message_type: row.get(5)?,
        text_content: row.get(6)?,
        message_json: row.get(7)?,
        media_content_json: row.get(8)?,
        reply_token: row.get(9)?,
        quote_token: row.get(10)?,
        webhook_event_id: row.get(11)?,
        line_timestamp: row.get(12)?,
        received_at: row.get(13)?,
        delivered_at: row.get(14)?,
        delivery_status: DeliveryStatus::parse(&status_str).unwrap_or(DeliveryStatus::Pending),
        retry_count: row.get(16)?,
        last_retry_at: row.get(17)?,
        error_message: row.get(18)?,
        ugent_request_id: row.get(19)?,
        ugent_correlation_id: row.get(20)?,
        created_at: row.get(21)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn setup_db() -> (SqliteBackend, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let backend = SqliteBackend::open(tmp.path()).unwrap();
        (backend, tmp)
    }

    #[tokio::test]
    async fn test_ping() {
        let (db, _tmp) = setup_db();
        assert!(db.ping().await.unwrap());
    }

    #[tokio::test]
    async fn test_contact_crud() {
        let (db, _tmp) = setup_db();

        let contact = ContactRecord {
            line_user_id: "U123".to_string(),
            display_name: Some("Test User".to_string()),
            picture_url: None,
            status_message: None,
            language: Some("en".to_string()),
            first_seen_at: 1000,
            last_seen_at: 1000,
            last_interacted_at: None,
            is_blocked: false,
            is_friend: true,
            created_at: 1000,
            updated_at: 1000,
        };

        db.upsert_contact(&contact).await.unwrap();

        let fetched = db.get_contact("U123").await.unwrap().unwrap();
        assert_eq!(fetched.display_name.as_deref(), Some("Test User"));
        assert!(fetched.is_friend);
    }

    #[tokio::test]
    async fn test_contact_not_found() {
        let (db, _tmp) = setup_db();
        let result = db.get_contact("U999").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_message_store_and_retrieve() {
        let (db, _tmp) = setup_db();

        let msg = MessageRecord {
            id: "msg_001".to_string(),
            direction: "inbound".to_string(),
            conversation_id: "U123".to_string(),
            source_type: "user".to_string(),
            sender_id: Some("U123".to_string()),
            message_type: "text".to_string(),
            text_content: Some("Hello!".to_string()),
            message_json: None,
            media_content_json: None,
            reply_token: None,
            quote_token: None,
            webhook_event_id: None,
            line_timestamp: Some(1000),
            received_at: 1000,
            delivered_at: None,
            delivery_status: DeliveryStatus::Pending,
            retry_count: 0,
            last_retry_at: None,
            error_message: None,
            ugent_request_id: None,
            ugent_correlation_id: None,
            created_at: 1000,
        };

        db.store_message(&msg).await.unwrap();
        let fetched = db.get_message("msg_001").await.unwrap().unwrap();
        assert_eq!(fetched.text_content.as_deref(), Some("Hello!"));
        assert_eq!(fetched.delivery_status, DeliveryStatus::Pending);
    }

    #[tokio::test]
    async fn test_webhook_dedup() {
        let (db, _tmp) = setup_db();

        // First check should return false (not seen)
        assert!(!db.check_and_mark_webhook("evt_001").await.unwrap());

        // Second check should return true (already seen)
        assert!(db.check_and_mark_webhook("evt_001").await.unwrap());

        // Different event should return false
        assert!(!db.check_and_mark_webhook("evt_002").await.unwrap());
    }

    #[tokio::test]
    async fn test_conversation_ownership() {
        let (db, _tmp) = setup_db();

        db.set_conversation_owner("conv_001", "client_1")
            .await
            .unwrap();
        let owner = db.get_conversation_owner("conv_001").await.unwrap();
        assert_eq!(owner.as_deref(), Some("client_1"));

        // Release
        let released = db.release_client_conversations("client_1").await.unwrap();
        assert_eq!(released, 1);

        // Should be None now
        let owner = db.get_conversation_owner("conv_001").await.unwrap();
        assert!(owner.is_none());
    }

    #[tokio::test]
    async fn test_outbound_queue_lifecycle() {
        let (db, _tmp) = setup_db();

        // Store a message first
        let msg = MessageRecord {
            id: "msg_002".to_string(),
            direction: "outbound".to_string(),
            conversation_id: "U123".to_string(),
            source_type: "user".to_string(),
            sender_id: None,
            message_type: "text".to_string(),
            text_content: Some("Reply".to_string()),
            message_json: None,
            media_content_json: None,
            reply_token: None,
            quote_token: None,
            webhook_event_id: None,
            line_timestamp: None,
            received_at: 1000,
            delivered_at: None,
            delivery_status: DeliveryStatus::Pending,
            retry_count: 0,
            last_retry_at: None,
            error_message: None,
            ugent_request_id: None,
            ugent_correlation_id: None,
            created_at: 1000,
        };
        db.store_message(&msg).await.unwrap();

        // Enqueue
        let entry = OutboundQueueEntry {
            id: "oq_001".to_string(),
            message_id: "msg_002".to_string(),
            status: "pending".to_string(),
            attempt: 0,
            max_attempts: 5,
            next_retry_at: 1000,
            locked_by: None,
            locked_at: None,
            last_error: None,
            created_at: 1000,
            updated_at: 1000,
        };
        db.enqueue_outbound(&entry).await.unwrap();

        let count = db.pending_outbound_count().await.unwrap();
        assert_eq!(count, 1);

        // Claim
        let claimed = db.claim_next_outbound("worker_1", 10).await.unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].locked_by.as_deref(), Some("worker_1"));

        // Complete
        db.complete_outbound("oq_001", true, None).await.unwrap();
        let count = db.pending_outbound_count().await.unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_inbound_queue_lifecycle() {
        let (db, _tmp) = setup_db();

        let msg = MessageRecord {
            id: "msg_003".to_string(),
            direction: "inbound".to_string(),
            conversation_id: "U456".to_string(),
            source_type: "user".to_string(),
            sender_id: Some("U456".to_string()),
            message_type: "text".to_string(),
            text_content: Some("Queued".to_string()),
            message_json: None,
            media_content_json: None,
            reply_token: None,
            quote_token: None,
            webhook_event_id: None,
            line_timestamp: None,
            received_at: 1000,
            delivered_at: None,
            delivery_status: DeliveryStatus::Pending,
            retry_count: 0,
            last_retry_at: None,
            error_message: None,
            ugent_request_id: None,
            ugent_correlation_id: None,
            created_at: 1000,
        };
        db.store_message(&msg).await.unwrap();

        let entry = InboundQueueEntry {
            id: "iq_001".to_string(),
            message_id: "msg_003".to_string(),
            status: "pending".to_string(),
            expires_at: i64::MAX / 2, // far future
            locked_by: None,
            locked_at: None,
            created_at: 1000,
        };
        db.enqueue_inbound(&entry).await.unwrap();

        let count = db.pending_inbound_count().await.unwrap();
        assert_eq!(count, 1);

        let claimed = db.claim_next_inbound("drain_worker", 10).await.unwrap();
        assert_eq!(claimed.len(), 1);

        db.complete_inbound("iq_001").await.unwrap();
        let count = db.pending_inbound_count().await.unwrap();
        assert_eq!(count, 0);
    }
}
