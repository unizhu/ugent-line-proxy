//! Database abstraction layer
//!
//! Provides a unified async database interface supporting SQLite and PostgreSQL backends.
//! All database operations go through the `DatabaseBackend` trait, allowing transparent
//! switching between storage engines.

pub mod config;
pub mod error;

pub use config::{DataConfig, DbType, RetentionConfig, RetryConfig};
pub use error::DbError;

use async_trait::async_trait;

// Re-exports for convenience
pub mod contacts;
pub mod groups;
pub mod inbound_queue;
pub mod messages;
pub mod metrics;
pub mod migration;
pub mod outbound_queue;

mod sqlite;

#[cfg(feature = "postgres")]
mod postgres;

// Re-export concrete backends
pub use sqlite::SqliteBackend;

#[cfg(feature = "postgres")]
pub use postgres::PostgresBackend;

// Re-export record types
pub use contacts::ContactRecord;
pub use groups::{GroupMemberRecord, GroupRecord};
pub use inbound_queue::InboundQueueEntry;
pub use messages::{DeliveryStatus, MessageRecord};
pub use metrics::MetricRecord;
pub use outbound_queue::OutboundQueueEntry;

use std::sync::Arc;

/// Database trait — implemented by both SQLite and PostgreSQL backends.
///
/// All methods are async to support both sync-backed (SQLite via spawn_blocking)
/// and natively async (PostgreSQL via sqlx) implementations.
#[async_trait]
pub trait DatabaseBackend: Send + Sync {
    // =========================================================================
    // Lifecycle
    // =========================================================================

    /// Check if the database connection is alive
    async fn ping(&self) -> Result<bool, DbError>;

    /// Run periodic maintenance (cleanup expired entries, vacuum, analyze)
    async fn run_maintenance(&self) -> Result<(), DbError>;

    // =========================================================================
    // Contacts
    // =========================================================================

    /// Insert or update a contact record
    async fn upsert_contact(&self, contact: &ContactRecord) -> Result<(), DbError>;

    /// Get a contact by LINE user ID
    async fn get_contact(&self, line_user_id: &str) -> Result<Option<ContactRecord>, DbError>;

    /// List contacts with pagination
    async fn list_contacts(&self, offset: u64, limit: u64) -> Result<Vec<ContactRecord>, DbError>;

    /// Search contacts by display name
    async fn search_contacts(&self, query: &str, limit: u64)
    -> Result<Vec<ContactRecord>, DbError>;

    // =========================================================================
    // Groups
    // =========================================================================

    /// Insert or update a group record
    async fn upsert_group(&self, group: &GroupRecord) -> Result<(), DbError>;

    /// Get a group by LINE group ID
    async fn get_group(&self, line_group_id: &str) -> Result<Option<GroupRecord>, DbError>;

    /// Add a member to a group
    async fn add_group_member(&self, group_id: &str, user_id: &str) -> Result<(), DbError>;

    /// List groups with pagination
    async fn list_groups(&self, offset: u64, limit: u64) -> Result<Vec<GroupRecord>, DbError>;

    // =========================================================================
    // Messages
    // =========================================================================

    /// Store a message record
    async fn store_message(&self, msg: &MessageRecord) -> Result<(), DbError>;

    /// Get a message by ID
    async fn get_message(&self, id: &str) -> Result<Option<MessageRecord>, DbError>;

    /// List messages for a conversation with optional direction filter
    async fn list_messages(
        &self,
        conversation_id: &str,
        direction: Option<&str>,
        offset: u64,
        limit: u64,
    ) -> Result<Vec<MessageRecord>, DbError>;

    /// Update message delivery status
    async fn update_delivery_status(
        &self,
        id: &str,
        status: DeliveryStatus,
        error: Option<&str>,
    ) -> Result<(), DbError>;

    /// Increment retry count on a message
    async fn increment_retry_count(&self, id: &str) -> Result<(), DbError>;

    // =========================================================================
    // Outbound Queue
    // =========================================================================

    /// Enqueue an outbound message for retry
    async fn enqueue_outbound(&self, entry: &OutboundQueueEntry) -> Result<(), DbError>;

    /// Claim next batch of outbound messages for processing
    async fn claim_next_outbound(
        &self,
        worker_id: &str,
        limit: u64,
    ) -> Result<Vec<OutboundQueueEntry>, DbError>;

    /// Mark an outbound queue entry as completed
    async fn complete_outbound(
        &self,
        id: &str,
        success: bool,
        error: Option<&str>,
    ) -> Result<(), DbError>;

    /// Count pending outbound queue entries
    async fn pending_outbound_count(&self) -> Result<u64, DbError>;

    // =========================================================================
    // Inbound Queue
    // =========================================================================

    /// Enqueue an inbound message for later delivery
    async fn enqueue_inbound(&self, entry: &InboundQueueEntry) -> Result<(), DbError>;

    /// Claim next batch of inbound messages for delivery
    async fn claim_next_inbound(
        &self,
        worker_id: &str,
        limit: u64,
    ) -> Result<Vec<InboundQueueEntry>, DbError>;

    /// Mark an inbound queue entry as delivered
    async fn complete_inbound(&self, id: &str) -> Result<(), DbError>;

    /// Count pending inbound queue entries
    async fn pending_inbound_count(&self) -> Result<u64, DbError>;

    /// Clean up expired inbound queue entries
    async fn cleanup_expired_inbound(&self) -> Result<u64, DbError>;

    // =========================================================================
    // Metrics
    // =========================================================================

    /// Record a metric value
    async fn record_metric(&self, name: &str, value: i64) -> Result<(), DbError>;

    /// Get metric values since a timestamp
    async fn get_metrics(&self, name: &str, since: i64) -> Result<Vec<MetricRecord>, DbError>;

    // =========================================================================
    // Webhook Dedup (carried over from old storage)
    // =========================================================================

    /// Check if a webhook event has been seen, and mark it if not
    async fn check_and_mark_webhook(&self, event_id: &str) -> Result<bool, DbError>;

    /// Clean up old webhook dedup entries
    async fn cleanup_webhook_dedup(&self, max_age_secs: i64) -> Result<u64, DbError>;

    // =========================================================================
    // Conversation Ownership (carried over from old storage)
    // =========================================================================

    /// Set conversation ownership
    async fn set_conversation_owner(
        &self,
        conversation_id: &str,
        client_id: &str,
    ) -> Result<(), DbError>;

    /// Get the owner of a conversation
    async fn get_conversation_owner(
        &self,
        conversation_id: &str,
    ) -> Result<Option<String>, DbError>;

    /// Release all conversations owned by a client
    async fn release_client_conversations(&self, client_id: &str) -> Result<u64, DbError>;

    /// Clean up stale conversation ownership
    async fn cleanup_stale_ownership(&self, max_age_secs: i64) -> Result<u64, DbError>;
}

/// Database wrapper that holds the backend implementation
#[derive(Clone)]
pub struct Database {
    backend: Arc<dyn DatabaseBackend>,
}

impl Database {
    /// Create a new database with the given backend
    pub fn new(backend: impl DatabaseBackend + 'static) -> Self {
        Self {
            backend: Arc::new(backend),
        }
    }

    /// Get a reference to the backend
    pub fn backend(&self) -> &dyn DatabaseBackend {
        self.backend.as_ref()
    }

    /// Get an Arc reference to the backend (for sharing across tasks)
    pub fn backend_arc(&self) -> Arc<dyn DatabaseBackend> {
        Arc::clone(&self.backend)
    }
}
