//! Database schema and migrations

use crate::storage::StorageError;
use rusqlite::Connection;

/// Schema version for migrations
const SCHEMA_VERSION: i32 = 1;

/// Run all migrations to create/update database schema
pub fn run_migrations(conn: &Connection) -> Result<(), StorageError> {
    // Create version table if not exists
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER PRIMARY KEY
        )",
        [],
    )?;

    // Get current version
    let current_version: i32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);

    // Run migrations if needed
    if current_version < 1 {
        migration_v1(conn)?;
    }

    Ok(())
}

/// Migration v1: Initial schema
fn migration_v1(conn: &Connection) -> Result<(), StorageError> {
    tracing::info!("Running database migration v1");

    // Conversation ownership table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS conversation_ownership (
            conversation_id TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            claimed_at INTEGER NOT NULL,
            last_activity INTEGER NOT NULL
        )",
        [],
    )?;

    // Index for client lookups
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_ownership_client 
         ON conversation_ownership(client_id)",
        [],
    )?;

    // Pending messages table
    conn.execute(
        "CREATE TABLE IF NOT EXISTS pending_messages (
            original_id TEXT PRIMARY KEY,
            conversation_id TEXT NOT NULL,
            reply_token TEXT,
            received_at INTEGER NOT NULL,
            reply_token_expires_at INTEGER,
            webhook_event_id TEXT NOT NULL,
            client_id TEXT
        )",
        [],
    )?;

    // Index for expiration cleanup
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_pending_received 
         ON pending_messages(received_at)",
        [],
    )?;

    // Index for conversation lookups
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_pending_conversation 
         ON pending_messages(conversation_id)",
        [],
    )?;

    // Metrics table (time-series data)
    conn.execute(
        "CREATE TABLE IF NOT EXISTS metrics (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            metric_name TEXT NOT NULL,
            metric_value INTEGER NOT NULL,
            timestamp INTEGER NOT NULL
        )",
        [],
    )?;

    // Index for metric queries
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_metrics_name_time 
         ON metrics(metric_name, timestamp)",
        [],
    )?;

    // Record migration
    conn.execute(
        "INSERT INTO schema_version (version) VALUES (?)",
        [SCHEMA_VERSION],
    )?;

    tracing::info!("Database migration v1 completed");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_migration() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let conn = Connection::open(&db_path).unwrap();

        run_migrations(&conn).unwrap();

        // Verify tables exist
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('conversation_ownership', 'pending_messages', 'metrics')",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(count, 3);
    }
}
