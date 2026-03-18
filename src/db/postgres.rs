//! PostgreSQL database backend
//!
//! Implements DatabaseBackend trait using sqlx for async PostgreSQL operations.
//! Enabled via the `postgres` feature flag.

use crate::db::DatabaseBackend;
use crate::db::config::DataConfig;
use crate::db::contacts::ContactRecord;
use crate::db::error::DbError;
use crate::db::groups::GroupRecord;
use crate::db::inbound_queue::InboundQueueEntry;
use crate::db::messages::{DeliveryStatus, MessageRecord};
use crate::db::metrics::MetricRecord;
use crate::db::migration::V2_MIGRATION_POSTGRES;
use crate::db::outbound_queue::OutboundQueueEntry;
use async_trait::async_trait;
use chrono::Utc;
use sqlx::PgPool;
use sqlx::Row;
use sqlx::postgres::PgPoolOptions;

/// PostgreSQL backend using sqlx
pub struct PostgresBackend {
    pool: PgPool,
}

fn message_record_from_row(row: &sqlx::postgres::PgRow) -> MessageRecord {
    MessageRecord {
        id: row.get("id"),
        direction: row.get("direction"),
        conversation_id: row.get("conversation_id"),
        source_type: row.get("source_type"),
        sender_id: row.get("sender_id"),
        message_type: row.get("message_type"),
        text_content: row.get("text_content"),
        message_json: row.get("message_json"),
        media_content_json: row.get("media_content_json"),
        reply_token: row.get("reply_token"),
        quote_token: row.get("quote_token"),
        webhook_event_id: row.get("webhook_event_id"),
        line_timestamp: row.get("line_timestamp"),
        received_at: row.get("received_at"),
        delivered_at: row.get("delivered_at"),
        delivery_status: {
            let s: String = row.get("delivery_status");
            DeliveryStatus::parse(&s).unwrap_or(DeliveryStatus::Pending)
        },
        retry_count: row.get("retry_count"),
        last_retry_at: row.get("last_retry_at"),
        error_message: row.get("error_message"),
        ugent_request_id: row.get("ugent_request_id"),
        ugent_correlation_id: row.get("ugent_correlation_id"),
        created_at: row.get("created_at"),
    }
}

impl PostgresBackend {
    /// Create from a PostgreSQL connection URL
    pub async fn connect(url: &str) -> Result<Self, DbError> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await
            .map_err(|e| DbError::Connection(format!("Failed to connect to PostgreSQL: {e}")))?;

        let backend = Self { pool };
        backend.init_schema().await?;

        Ok(backend)
    }

    /// Create from DataConfig
    pub async fn from_config(config: &DataConfig) -> Result<Self, DbError> {
        let url = config
            .db_url
            .as_deref()
            .ok_or_else(|| DbError::Config("PostgreSQL DB URL not configured".to_string()))?;
        Self::connect(url).await
    }

    /// Initialize or migrate schema
    async fn init_schema(&self) -> Result<(), DbError> {
        // Ensure schema_version table
        sqlx::query(
            r#"CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at BIGINT NOT NULL
            )"#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))?;

        // Check current version
        let row: Option<(i64,)> =
            sqlx::query_as("SELECT COALESCE(MAX(version), 0) FROM schema_version")
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| DbError::Migration(e.to_string()))?;

        let version = row.map(|(v,)| v).unwrap_or(0);

        if version < 1 {
            self.run_v1_schema().await?;
        }

        if version < 2 {
            for sql in V2_MIGRATION_POSTGRES {
                sqlx::query(sql)
                    .execute(&self.pool)
                    .await
                    .map_err(|e| DbError::Migration(format!("v2 migration failed: {e}")))?;
            }
        }

        Ok(())
    }

    async fn run_v1_schema(&self) -> Result<(), DbError> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS conversation_ownership (
                conversation_id VARCHAR(255) PRIMARY KEY,
                client_id VARCHAR(255) NOT NULL,
                claimed_at BIGINT NOT NULL,
                last_heartbeat BIGINT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS pending_messages (
                response_id VARCHAR(255) PRIMARY KEY,
                conversation_id VARCHAR(255) NOT NULL,
                created_at BIGINT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS metrics (
                id BIGSERIAL PRIMARY KEY,
                name VARCHAR(255) NOT NULL,
                value BIGINT NOT NULL,
                recorded_at BIGINT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS webhook_dedup (
                event_id VARCHAR(255) PRIMARY KEY,
                processed_at BIGINT NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Migration(e.to_string()))?;

        let now = Utc::now().timestamp_millis();
        sqlx::query("INSERT INTO schema_version (version, applied_at) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(1)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Migration(e.to_string()))?;

        Ok(())
    }

    fn now_ms() -> i64 {
        Utc::now().timestamp_millis()
    }
}

#[async_trait]
impl DatabaseBackend for PostgresBackend {
    async fn ping(&self) -> Result<bool, DbError> {
        Ok(sqlx::query("SELECT 1").execute(&self.pool).await.is_ok())
    }

    async fn run_maintenance(&self) -> Result<(), DbError> {
        let now = Self::now_ms();

        sqlx::query(
            "UPDATE inbound_queue SET status = 'expired' WHERE status = 'pending' AND expires_at < $1",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        let one_hour_ago = now - 3600 * 1000;
        sqlx::query(
            "DELETE FROM outbound_queue WHERE status IN ('completed', 'failed') AND updated_at < $1",
        )
        .bind(one_hour_ago)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        sqlx::query(
            "DELETE FROM inbound_queue WHERE status IN ('completed', 'expired') AND created_at < $1",
        )
        .bind(one_hour_ago)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    // ========== Contacts ==========

    async fn upsert_contact(&self, contact: &ContactRecord) -> Result<(), DbError> {
        sqlx::query(
            r#"INSERT INTO contacts (
                line_user_id, display_name, picture_url, status_message, language,
                first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (line_user_id) DO UPDATE SET
                display_name = EXCLUDED.display_name,
                picture_url = EXCLUDED.picture_url,
                status_message = EXCLUDED.status_message,
                language = EXCLUDED.language,
                last_seen_at = EXCLUDED.last_seen_at,
                last_interacted_at = COALESCE(EXCLUDED.last_interacted_at, contacts.last_interacted_at),
                is_blocked = EXCLUDED.is_blocked,
                is_friend = EXCLUDED.is_friend,
                updated_at = EXCLUDED.updated_at"#,
        )
        .bind(&contact.line_user_id)
        .bind(&contact.display_name)
        .bind(&contact.picture_url)
        .bind(&contact.status_message)
        .bind(&contact.language)
        .bind(contact.first_seen_at)
        .bind(contact.last_seen_at)
        .bind(contact.last_interacted_at)
        .bind(contact.is_blocked)
        .bind(contact.is_friend)
        .bind(contact.created_at)
        .bind(contact.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_contact(&self, line_user_id: &str) -> Result<Option<ContactRecord>, DbError> {
        let row: Option<(
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            Option<i64>,
            bool,
            bool,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT line_user_id, display_name, picture_url, status_message, language,
                        first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                        created_at, updated_at
                 FROM contacts WHERE line_user_id = $1",
        )
        .bind(line_user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.map(|r| ContactRecord {
            line_user_id: r.0,
            display_name: r.1,
            picture_url: r.2,
            status_message: r.3,
            language: r.4,
            first_seen_at: r.5,
            last_seen_at: r.6,
            last_interacted_at: r.7,
            is_blocked: r.8,
            is_friend: r.9,
            created_at: r.10,
            updated_at: r.11,
        }))
    }

    async fn list_contacts(&self, offset: u64, limit: u64) -> Result<Vec<ContactRecord>, DbError> {
        let rows: Vec<(
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            Option<i64>,
            bool,
            bool,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT line_user_id, display_name, picture_url, status_message, language,
                        first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                        created_at, updated_at
                 FROM contacts ORDER BY last_seen_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| ContactRecord {
                line_user_id: r.0,
                display_name: r.1,
                picture_url: r.2,
                status_message: r.3,
                language: r.4,
                first_seen_at: r.5,
                last_seen_at: r.6,
                last_interacted_at: r.7,
                is_blocked: r.8,
                is_friend: r.9,
                created_at: r.10,
                updated_at: r.11,
            })
            .collect())
    }

    async fn search_contacts(
        &self,
        query: &str,
        limit: u64,
    ) -> Result<Vec<ContactRecord>, DbError> {
        let pattern = format!("%{query}%");
        let rows: Vec<(
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            i64,
            i64,
            Option<i64>,
            bool,
            bool,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT line_user_id, display_name, picture_url, status_message, language,
                        first_seen_at, last_seen_at, last_interacted_at, is_blocked, is_friend,
                        created_at, updated_at
                 FROM contacts WHERE display_name LIKE $1 ORDER BY last_seen_at DESC LIMIT $2",
        )
        .bind(&pattern)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| ContactRecord {
                line_user_id: r.0,
                display_name: r.1,
                picture_url: r.2,
                status_message: r.3,
                language: r.4,
                first_seen_at: r.5,
                last_seen_at: r.6,
                last_interacted_at: r.7,
                is_blocked: r.8,
                is_friend: r.9,
                created_at: r.10,
                updated_at: r.11,
            })
            .collect())
    }

    // ========== Groups ==========

    async fn upsert_group(&self, group: &GroupRecord) -> Result<(), DbError> {
        sqlx::query(
            r#"INSERT INTO groups (
                line_group_id, group_name, picture_url, member_count,
                first_seen_at, last_message_at, created_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            ON CONFLICT (line_group_id) DO UPDATE SET
                group_name = EXCLUDED.group_name,
                picture_url = EXCLUDED.picture_url,
                member_count = EXCLUDED.member_count,
                last_message_at = COALESCE(EXCLUDED.last_message_at, groups.last_message_at),
                updated_at = EXCLUDED.updated_at"#,
        )
        .bind(&group.line_group_id)
        .bind(&group.group_name)
        .bind(&group.picture_url)
        .bind(group.member_count)
        .bind(group.first_seen_at)
        .bind(group.last_message_at)
        .bind(group.created_at)
        .bind(group.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_group(&self, line_group_id: &str) -> Result<Option<GroupRecord>, DbError> {
        let row: Option<(
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
            Option<i64>,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT line_group_id, group_name, picture_url, member_count,
                        first_seen_at, last_message_at, created_at, updated_at
                 FROM groups WHERE line_group_id = $1",
        )
        .bind(line_group_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.map(|r| GroupRecord {
            line_group_id: r.0,
            group_name: r.1,
            picture_url: r.2,
            member_count: r.3,
            first_seen_at: r.4,
            last_message_at: r.5,
            created_at: r.6,
            updated_at: r.7,
        }))
    }

    async fn add_group_member(&self, group_id: &str, user_id: &str) -> Result<(), DbError> {
        let now = Self::now_ms();
        sqlx::query(
            "INSERT INTO group_members (line_group_id, line_user_id, joined_at) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(group_id)
        .bind(user_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn list_groups(&self, offset: u64, limit: u64) -> Result<Vec<GroupRecord>, DbError> {
        let rows: Vec<(
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            i64,
            Option<i64>,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT line_group_id, group_name, picture_url, member_count,
                        first_seen_at, last_message_at, created_at, updated_at
                 FROM groups ORDER BY last_message_at DESC LIMIT $1 OFFSET $2",
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| GroupRecord {
                line_group_id: r.0,
                group_name: r.1,
                picture_url: r.2,
                member_count: r.3,
                first_seen_at: r.4,
                last_message_at: r.5,
                created_at: r.6,
                updated_at: r.7,
            })
            .collect())
    }

    // ========== Messages ==========

    async fn store_message(&self, msg: &MessageRecord) -> Result<(), DbError> {
        sqlx::query(
            r#"INSERT INTO messages (
                id, direction, conversation_id, source_type, sender_id, message_type,
                text_content, message_json, media_content_json, reply_token, quote_token,
                webhook_event_id, line_timestamp, received_at, delivered_at,
                delivery_status, retry_count, last_retry_at, error_message,
                ugent_request_id, ugent_correlation_id, created_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22)
            ON CONFLICT (id) DO UPDATE SET
                delivery_status = EXCLUDED.delivery_status,
                delivered_at = COALESCE(EXCLUDED.delivered_at, messages.delivered_at),
                error_message = COALESCE(EXCLUDED.error_message, messages.error_message),
                retry_count = GREATEST(messages.retry_count, EXCLUDED.retry_count)"#,
        )
        .bind(&msg.id)
        .bind(&msg.direction)
        .bind(&msg.conversation_id)
        .bind(&msg.source_type)
        .bind(&msg.sender_id)
        .bind(&msg.message_type)
        .bind(&msg.text_content)
        .bind(&msg.message_json)
        .bind(&msg.media_content_json)
        .bind(&msg.reply_token)
        .bind(&msg.quote_token)
        .bind(&msg.webhook_event_id)
        .bind(msg.line_timestamp)
        .bind(msg.received_at)
        .bind(msg.delivered_at)
        .bind(msg.delivery_status.as_str())
        .bind(msg.retry_count)
        .bind(msg.last_retry_at)
        .bind(&msg.error_message)
        .bind(&msg.ugent_request_id)
        .bind(&msg.ugent_correlation_id)
        .bind(msg.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_message(&self, id: &str) -> Result<Option<MessageRecord>, DbError> {
        const SQL: &str =
            "SELECT id, direction, conversation_id, source_type, sender_id, message_type,
                        text_content, message_json, media_content_json, reply_token, quote_token,
                        webhook_event_id, line_timestamp, received_at, delivered_at,
                        delivery_status, retry_count, last_retry_at, error_message,
                        ugent_request_id, ugent_correlation_id, created_at
                 FROM messages WHERE id = $1";

        let row = sqlx::query(SQL)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.map(|r| message_record_from_row(&r)))
    }

    async fn list_messages(
        &self,
        conversation_id: &str,
        direction: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<MessageRecord>, DbError> {
        const SQL_FILTERED: &str =
            "SELECT id, direction, conversation_id, source_type, sender_id, message_type,
                        text_content, message_json, media_content_json, reply_token, quote_token,
                        webhook_event_id, line_timestamp, received_at, delivered_at,
                        delivery_status, retry_count, last_retry_at, error_message,
                        ugent_request_id, ugent_correlation_id, created_at
                 FROM messages WHERE conversation_id = $1 AND direction = $2
                 ORDER BY received_at DESC LIMIT $3 OFFSET $4";
        const SQL_UNFILTERED: &str =
            "SELECT id, direction, conversation_id, source_type, sender_id, message_type,
                        text_content, message_json, media_content_json, reply_token, quote_token,
                        webhook_event_id, line_timestamp, received_at, delivered_at,
                        delivery_status, retry_count, last_retry_at, error_message,
                        ugent_request_id, ugent_correlation_id, created_at
                 FROM messages WHERE conversation_id = $1
                 ORDER BY received_at DESC LIMIT $2 OFFSET $3";

        let rows = if let Some(dir) = direction {
            sqlx::query(SQL_FILTERED)
                .bind(conversation_id)
                .bind(dir)
                .bind(limit as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| DbError::Query(e.to_string()))?
        } else {
            sqlx::query(SQL_UNFILTERED)
                .bind(conversation_id)
                .bind(limit as i64)
                .bind(offset as i64)
                .fetch_all(&self.pool)
                .await
                .map_err(|e| DbError::Query(e.to_string()))?
        };

        Ok(rows
            .into_iter()
            .map(|r| message_record_from_row(&r))
            .collect())
    }

    async fn update_delivery_status(
        &self,
        id: &str,
        status: DeliveryStatus,
        error: Option<&str>,
    ) -> Result<(), DbError> {
        let now = Self::now_ms();
        sqlx::query(
            "UPDATE messages SET delivery_status = $1, error_message = $2, delivered_at = $3 WHERE id = $4",
        )
        .bind(status.as_str())
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn increment_retry_count(&self, id: &str) -> Result<(), DbError> {
        let now = Self::now_ms();
        sqlx::query(
            "UPDATE messages SET retry_count = retry_count + 1, last_retry_at = $1 WHERE id = $2",
        )
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    // ========== Outbound Queue ==========

    async fn enqueue_outbound(&self, entry: &OutboundQueueEntry) -> Result<(), DbError> {
        sqlx::query(
            r#"INSERT INTO outbound_queue (
                id, message_id, status, attempt, max_attempts, next_retry_at,
                locked_by, locked_at, last_error, created_at, updated_at
            ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
            ON CONFLICT (id) DO UPDATE SET
                status = EXCLUDED.status,
                next_retry_at = EXCLUDED.next_retry_at,
                updated_at = EXCLUDED.updated_at"#,
        )
        .bind(&entry.id)
        .bind(&entry.message_id)
        .bind(&entry.status)
        .bind(entry.attempt)
        .bind(entry.max_attempts)
        .bind(entry.next_retry_at)
        .bind(&entry.locked_by)
        .bind(entry.locked_at)
        .bind(&entry.last_error)
        .bind(entry.created_at)
        .bind(entry.updated_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn claim_next_outbound(
        &self,
        worker_id: &str,
        limit: u64,
    ) -> Result<Vec<OutboundQueueEntry>, DbError> {
        let now = Self::now_ms();

        sqlx::query(
            "UPDATE outbound_queue SET status = 'processing', locked_by = $1, locked_at = $2
             WHERE id IN (
                 SELECT id FROM outbound_queue
                 WHERE status = 'pending' AND next_retry_at <= $3
                 ORDER BY next_retry_at ASC LIMIT $4
                 FOR UPDATE SKIP LOCKED
             )",
        )
        .bind(worker_id)
        .bind(now)
        .bind(now)
        .bind(limit as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        let rows: Vec<(
            String,
            String,
            String,
            i64,
            i64,
            i64,
            Option<String>,
            Option<i64>,
            Option<String>,
            i64,
            i64,
        )> = sqlx::query_as(
            "SELECT id, message_id, status, attempt, max_attempts, next_retry_at,
                        locked_by, locked_at, last_error, created_at, updated_at
                 FROM outbound_queue WHERE locked_by = $1 ORDER BY next_retry_at ASC",
        )
        .bind(worker_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| OutboundQueueEntry {
                id: r.0,
                message_id: r.1,
                status: r.2,
                attempt: r.3,
                max_attempts: r.4,
                next_retry_at: r.5,
                locked_by: r.6,
                locked_at: r.7,
                last_error: r.8,
                created_at: r.9,
                updated_at: r.10,
            })
            .collect())
    }

    async fn complete_outbound(
        &self,
        id: &str,
        success: bool,
        error: Option<&str>,
    ) -> Result<(), DbError> {
        let now = Self::now_ms();
        let status = if success { "completed" } else { "failed" };
        sqlx::query(
            "UPDATE outbound_queue SET status = $1, last_error = $2, updated_at = $3, locked_by = NULL, locked_at = NULL WHERE id = $4",
        )
        .bind(status)
        .bind(error)
        .bind(now)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn pending_outbound_count(&self) -> Result<u64, DbError> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM outbound_queue WHERE status = 'pending'")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.0 as u64)
    }

    // ========== Inbound Queue ==========

    async fn enqueue_inbound(&self, entry: &InboundQueueEntry) -> Result<(), DbError> {
        sqlx::query(
            "INSERT INTO inbound_queue (id, message_id, status, expires_at, locked_by, locked_at, created_at)
             VALUES ($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(&entry.id)
        .bind(&entry.message_id)
        .bind(&entry.status)
        .bind(entry.expires_at)
        .bind(&entry.locked_by)
        .bind(entry.locked_at)
        .bind(entry.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn claim_next_inbound(
        &self,
        worker_id: &str,
        limit: u64,
    ) -> Result<Vec<InboundQueueEntry>, DbError> {
        let now = Self::now_ms();

        sqlx::query(
            "UPDATE inbound_queue SET status = 'processing', locked_by = $1, locked_at = $2
             WHERE id IN (
                 SELECT id FROM inbound_queue
                 WHERE status = 'pending' AND expires_at > $3
                 ORDER BY created_at ASC LIMIT $4
                 FOR UPDATE SKIP LOCKED
             )",
        )
        .bind(worker_id)
        .bind(now)
        .bind(now)
        .bind(limit as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        let rows: Vec<(
            String,
            String,
            String,
            i64,
            Option<String>,
            Option<i64>,
            i64,
        )> = sqlx::query_as(
            "SELECT id, message_id, status, expires_at, locked_by, locked_at, created_at
                 FROM inbound_queue WHERE locked_by = $1 ORDER BY created_at ASC",
        )
        .bind(worker_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| InboundQueueEntry {
                id: r.0,
                message_id: r.1,
                status: r.2,
                expires_at: r.3,
                locked_by: r.4,
                locked_at: r.5,
                created_at: r.6,
            })
            .collect())
    }

    async fn complete_inbound(&self, id: &str) -> Result<(), DbError> {
        sqlx::query(
            "UPDATE inbound_queue SET status = 'completed', locked_by = NULL, locked_at = NULL WHERE id = $1",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn pending_inbound_count(&self) -> Result<u64, DbError> {
        let row: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM inbound_queue WHERE status = 'pending'")
                .fetch_one(&self.pool)
                .await
                .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.0 as u64)
    }

    async fn cleanup_expired_inbound(&self) -> Result<u64, DbError> {
        let now = Self::now_ms();
        let result = sqlx::query(
            "UPDATE inbound_queue SET status = 'expired' WHERE status = 'pending' AND expires_at < $1",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(result.rows_affected() as u64)
    }

    // ========== Metrics ==========

    async fn record_metric(&self, name: &str, value: i64) -> Result<(), DbError> {
        sqlx::query("INSERT INTO metrics (name, value, recorded_at) VALUES ($1, $2, $3)")
            .bind(name)
            .bind(value)
            .bind(Self::now_ms())
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_metrics(&self, name: &str, since: i64) -> Result<Vec<MetricRecord>, DbError> {
        let rows: Vec<(String, i64, i64)> = sqlx::query_as(
            "SELECT name, value, recorded_at FROM metrics WHERE name = $1 AND recorded_at >= $2 ORDER BY recorded_at ASC",
        )
        .bind(name)
        .bind(since)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|r| MetricRecord {
                name: r.0,
                value: r.1,
                recorded_at: r.2,
            })
            .collect())
    }

    // ========== Webhook Dedup ==========

    async fn check_and_mark_webhook(&self, event_id: &str) -> Result<bool, DbError> {
        let exists: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM webhook_dedup WHERE event_id = $1")
                .bind(event_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| DbError::Query(e.to_string()))?;

        if exists.0 > 0 {
            return Ok(true);
        }

        let now = Self::now_ms();
        sqlx::query("INSERT INTO webhook_dedup (event_id, processed_at) VALUES ($1, $2)")
            .bind(event_id)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(false)
    }

    async fn cleanup_webhook_dedup(&self, max_age_secs: i64) -> Result<u64, DbError> {
        let now = Self::now_ms();
        let cutoff = now - (max_age_secs * 1000);
        let result = sqlx::query("DELETE FROM webhook_dedup WHERE processed_at < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(result.rows_affected() as u64)
    }

    // ========== Conversation Ownership ==========

    async fn set_conversation_owner(
        &self,
        conversation_id: &str,
        client_id: &str,
    ) -> Result<(), DbError> {
        let now = Self::now_ms();
        sqlx::query(
            r#"INSERT INTO conversation_ownership (conversation_id, client_id, claimed_at, last_heartbeat)
             VALUES ($1, $2, $3, $3)
             ON CONFLICT (conversation_id) DO UPDATE SET
                 client_id = EXCLUDED.client_id,
                 claimed_at = EXCLUDED.claimed_at,
                 last_heartbeat = EXCLUDED.last_heartbeat"#,
        )
        .bind(conversation_id)
        .bind(client_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(())
    }

    async fn get_conversation_owner(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>, DbError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT client_id FROM conversation_ownership WHERE conversation_id = $1",
        )
        .bind(conversation_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(row.map(|r| r.0))
    }

    async fn release_client_conversations(&self, client_id: &str) -> Result<u64, DbError> {
        let result = sqlx::query("DELETE FROM conversation_ownership WHERE client_id = $1")
            .bind(client_id)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(result.rows_affected() as u64)
    }

    async fn cleanup_stale_ownership(&self, max_age_secs: i64) -> Result<u64, DbError> {
        let now = Self::now_ms();
        let cutoff = now - (max_age_secs * 1000);
        let result = sqlx::query("DELETE FROM conversation_ownership WHERE last_heartbeat < $1")
            .bind(cutoff)
            .execute(&self.pool)
            .await
            .map_err(|e| DbError::Query(e.to_string()))?;

        Ok(result.rows_affected() as u64)
    }
}
