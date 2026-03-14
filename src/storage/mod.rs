//! SQLite storage for LINE proxy persistence
//!
//! Provides persistence for:
//! - Conversation ownership (which client owns a conversation)
//! - Pending messages (messages awaiting response)

mod metrics;
mod ownership;
mod pending;
mod schema;

pub use metrics::{HourlyMetric, MetricRecord, MetricsSnapshot, MetricsStore};
pub use metrics::{
    METRIC_CONNECTED_CLIENTS, METRIC_MESSAGES_RECEIVED, METRIC_MESSAGES_SENT,
    METRIC_OWNED_CONVERSATIONS, METRIC_OWNERSHIP_CLAIMS, METRIC_OWNERSHIP_RELEASES,
    METRIC_PENDING_MESSAGES, METRIC_RESPONSES_RECEIVED,
};
pub use ownership::OwnershipStore;
pub use pending::PendingMessageStore;

use parking_lot::Mutex;
use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Default storage path relative to home directory
pub const DEFAULT_STORAGE_DIR: &str = ".ugent/line-plugin";
pub const DEFAULT_DB_NAME: &str = "line-proxy.db";

/// Storage errors
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Database connection error: {0}")]
    Connection(#[from] rusqlite::Error),

    #[error("Failed to create directory: {0}")]
    DirectoryCreate(String),

    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Combined storage manager
#[derive(Debug)]
pub struct Storage {
    /// Database connection (wrapped in Mutex for thread safety)
    conn: Arc<Mutex<Connection>>,
    /// Ownership store
    ownership: OwnershipStore,
    /// Pending messages store
    pending: PendingMessageStore,
    /// Metrics store
    metrics: MetricsStore,
}

impl Storage {
    /// Create or open storage at the default location (~/.ugent/line-plugin/)
    pub fn new() -> Result<Self, StorageError> {
        let path = Self::default_path()?;
        Self::with_path(&path)
    }

    /// Create or open storage at a specific path
    pub fn with_path(db_path: &PathBuf) -> Result<Self, StorageError> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| StorageError::DirectoryCreate(e.to_string()))?;
            }
        }

        // Open or create database
        let conn = Connection::open(db_path)?;

        // Enable WAL mode for better concurrency
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;",
        )?;

        // Run schema migrations
        schema::run_migrations(&conn)?;

        let conn = Arc::new(Mutex::new(conn));

        Ok(Self {
            ownership: OwnershipStore::new(Arc::clone(&conn)),
            pending: PendingMessageStore::new(Arc::clone(&conn)),
            metrics: MetricsStore::new(Arc::clone(&conn)),
            conn,
        })
    }

    /// Get the default database path
    pub fn default_path() -> Result<PathBuf, StorageError> {
        let home = dirs::home_dir()
            .ok_or_else(|| StorageError::InvalidPath("Cannot determine home directory".into()))?;
        Ok(home.join(DEFAULT_STORAGE_DIR).join(DEFAULT_DB_NAME))
    }

    /// Get ownership store
    pub fn ownership(&self) -> &OwnershipStore {
        &self.ownership
    }

    /// Get pending messages store
    pub fn pending(&self) -> &PendingMessageStore {
        &self.pending
    }

    /// Get metrics store
    pub fn metrics(&self) -> &MetricsStore {
        &self.metrics
    }

    /// Run maintenance tasks (cleanup expired entries, vacuum, etc.)
    pub fn maintenance(&self) -> Result<(), StorageError> {
        let conn = self.conn.lock();

        // Clean up expired pending messages (older than 5 minutes)
        conn.execute(
            "DELETE FROM pending_messages WHERE received_at < strftime('%s', 'now') - 300",
            [],
        )?;

        // Clean up stale ownership (no activity for 30 minutes)
        conn.execute(
            "DELETE FROM conversation_ownership WHERE last_activity < strftime('%s', 'now') - 1800",
            [],
        )?;

        // Update statistics
        conn.execute("ANALYZE", [])?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_storage_creation() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let storage = Storage::with_path(&db_path).unwrap();

        // Verify stores are accessible
        let _ = storage.ownership();
        let _ = storage.pending();
        let _ = storage.metrics();
    }

    #[test]
    fn test_maintenance() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");

        let storage = Storage::with_path(&db_path).unwrap();
        storage.maintenance().unwrap();
    }
}
