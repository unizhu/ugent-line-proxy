//! Webhook event deduplication store
//!
//! Prevents processing the same webhook event multiple times.
//! LINE may redeliver webhooks if acknowledgment is delayed.

use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::Arc;
use tracing::debug;

/// Deduplication window in seconds (24 hours)
const DEDUP_WINDOW_SECS: i64 = 24 * 60 * 60;

/// Webhook event deduplication store
#[derive(Debug)]
pub struct WebhookDedupStore {
    conn: Arc<Mutex<Connection>>,
}

impl WebhookDedupStore {
    /// Create a new dedup store
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Check if an event has been seen and mark it as seen
    /// Returns true if the event is a duplicate (already processed)
    pub fn check_and_mark(&self, webhook_event_id: &str) -> bool {
        let conn = self.conn.lock();

        // First, clean up old entries
        let _ = conn.execute(
            "DELETE FROM webhook_dedup WHERE seen_at < strftime('%s', 'now') - ?",
            [DEDUP_WINDOW_SECS],
        );

        // Check if event exists
        let exists: bool = conn
            .query_row(
                "SELECT 1 FROM webhook_dedup WHERE webhook_event_id = ?",
                [webhook_event_id],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if exists {
            debug!("Duplicate webhook event detected: {}", webhook_event_id);
            return true;
        }

        // Mark as seen
        let _ = conn.execute(
            "INSERT OR IGNORE INTO webhook_dedup (webhook_event_id, seen_at) VALUES (?, strftime('%s', 'now'))",
            [webhook_event_id],
        );

        false
    }

    /// Check if an event has been seen without marking it
    pub fn is_seen(&self, webhook_event_id: &str) -> bool {
        let conn = self.conn.lock();
        conn.query_row(
            "SELECT 1 FROM webhook_dedup WHERE webhook_event_id = ?",
            [webhook_event_id],
            |_| Ok(true),
        )
        .unwrap_or(false)
    }

    /// Mark an event as seen explicitly
    pub fn mark_seen(&self, webhook_event_id: &str) {
        let conn = self.conn.lock();
        let _ = conn.execute(
            "INSERT OR IGNORE INTO webhook_dedup (webhook_event_id, seen_at) VALUES (?, strftime('%s', 'now'))",
            [webhook_event_id],
        );
    }

    /// Get count of tracked events
    pub fn count(&self) -> usize {
        let conn = self.conn.lock();
        conn.query_row("SELECT COUNT(*) FROM webhook_dedup", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap_or(0) as usize
    }

    /// Clear all tracked events
    pub fn clear(&self) {
        let conn = self.conn.lock();
        let _ = conn.execute("DELETE FROM webhook_dedup", []);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn create_test_store() -> WebhookDedupStore {
        let conn = Connection::open_in_memory().unwrap();

        // Create table
        conn.execute(
            "CREATE TABLE IF NOT EXISTS webhook_dedup (
                webhook_event_id TEXT PRIMARY KEY,
                seen_at INTEGER NOT NULL
            )",
            [],
        )
        .unwrap();

        WebhookDedupStore::new(Arc::new(Mutex::new(conn)))
    }

    #[test]
    fn test_check_and_mark() {
        let store = create_test_store();
        let event_id = "event-123";

        // First check should return false (not duplicate)
        assert!(!store.check_and_mark(event_id));

        // Second check should return true (duplicate)
        assert!(store.check_and_mark(event_id));
    }

    #[test]
    fn test_is_seen() {
        let store = create_test_store();
        let event_id = "event-456";

        // Not seen initially
        assert!(!store.is_seen(event_id));

        // Mark as seen
        store.mark_seen(event_id);

        // Now seen
        assert!(store.is_seen(event_id));
    }

    #[test]
    fn test_count() {
        let store = create_test_store();

        assert_eq!(store.count(), 0);

        store.mark_seen("event-1");
        store.mark_seen("event-2");
        store.mark_seen("event-3");

        assert_eq!(store.count(), 3);
    }
}
