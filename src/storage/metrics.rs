//! Metrics persistence and tracking

use crate::storage::StorageError;
use parking_lot::Mutex;
use rusqlite::Connection;
use std::sync::Arc;

/// Metric names
pub const METRIC_CONNECTED_CLIENTS: &str = "connected_clients";
pub const METRIC_PENDING_MESSAGES: &str = "pending_messages";
pub const METRIC_OWNED_CONVERSATIONS: &str = "owned_conversations";
pub const METRIC_MESSAGES_RECEIVED: &str = "messages_received";
pub const METRIC_MESSAGES_SENT: &str = "messages_sent";
pub const METRIC_RESPONSES_RECEIVED: &str = "responses_received";
pub const METRIC_OWNERSHIP_CLAIMS: &str = "ownership_claims";
pub const METRIC_OWNERSHIP_RELEASES: &str = "ownership_releases";

/// Metric record
#[derive(Debug, Clone)]
pub struct MetricRecord {
    pub id: i64,
    pub metric_name: String,
    pub metric_value: i64,
    pub timestamp: i64,
}

/// Store for metrics
#[derive(Debug)]
pub struct MetricsStore {
    conn: Arc<Mutex<Connection>>,
}

impl MetricsStore {
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Record a metric value
    pub fn record(&self, name: &str, value: i64) -> Result<(), StorageError> {
        let conn = self.conn.lock();
        let timestamp = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO metrics (metric_name, metric_value, timestamp) VALUES (?1, ?2, ?3)",
            [name, &value.to_string(), &timestamp.to_string()],
        )?;

        Ok(())
    }

    /// Increment a counter metric
    pub fn increment(&self, name: &str) -> Result<i64, StorageError> {
        let conn = self.conn.lock();
        let timestamp = chrono::Utc::now().timestamp();

        conn.execute(
            "INSERT INTO metrics (metric_name, metric_value, timestamp) VALUES (?1, 1, ?2)",
            [name, &timestamp.to_string()],
        )?;

        // Get the sum
        let sum: i64 = conn.query_row(
            "SELECT COALESCE(SUM(metric_value), 0) FROM metrics WHERE metric_name = ?1",
            [name],
            |row| row.get(0),
        )?;

        Ok(sum)
    }

    /// Get the current count for a metric (sum of all values)
    pub fn get_count(&self, name: &str) -> Result<i64, StorageError> {
        let conn = self.conn.lock();

        let sum: i64 = conn.query_row(
            "SELECT COALESCE(SUM(metric_value), 0) FROM metrics WHERE metric_name = ?1",
            [name],
            |row| row.get(0),
        )?;

        Ok(sum)
    }

    /// Get recent metrics (last N hours)
    pub fn get_recent(&self, name: &str, hours: i64) -> Result<Vec<MetricRecord>, StorageError> {
        let conn = self.conn.lock();
        let cutoff = chrono::Utc::now().timestamp() - (hours * 3600);

        let mut stmt = conn.prepare(
            "SELECT id, metric_name, metric_value, timestamp 
             FROM metrics 
             WHERE metric_name = ?1 AND timestamp >= ?2
             ORDER BY timestamp DESC",
        )?;

        let records = stmt
            .query_map(rusqlite::params![name, cutoff], |row| {
                Ok(MetricRecord {
                    id: row.get(0)?,
                    metric_name: row.get(1)?,
                    metric_value: row.get(2)?,
                    timestamp: row.get(3)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Get metrics summary (hourly aggregates for the last N hours)
    pub fn get_hourly_summary(
        &self,
        name: &str,
        hours: i64,
    ) -> Result<Vec<HourlyMetric>, StorageError> {
        let conn = self.conn.lock();
        let cutoff = chrono::Utc::now().timestamp() - (hours * 3600);

        let mut stmt = conn.prepare(
            "SELECT 
                (timestamp / 3600) * 3600 as hour_start,
                SUM(metric_value) as total
             FROM metrics 
             WHERE metric_name = ?1 AND timestamp >= ?2
             GROUP BY (timestamp / 3600)
             ORDER BY hour_start DESC",
        )?;

        let records = stmt
            .query_map(rusqlite::params![name, cutoff], |row| {
                Ok(HourlyMetric {
                    hour_start: row.get(0)?,
                    total: row.get(1)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    /// Delete old metrics (older than N days)
    pub fn delete_old(&self, days: i64) -> Result<usize, StorageError> {
        let conn = self.conn.lock();
        let cutoff = chrono::Utc::now().timestamp() - (days * 86400);

        let rows = conn.execute("DELETE FROM metrics WHERE timestamp < ?1", [cutoff])?;

        if rows > 0 {
            tracing::debug!("Deleted {} old metric records", rows);
        }

        Ok(rows)
    }

    /// Get all metric names
    pub fn get_metric_names(&self) -> Result<Vec<String>, StorageError> {
        let conn = self.conn.lock();

        let mut stmt =
            conn.prepare("SELECT DISTINCT metric_name FROM metrics ORDER BY metric_name")?;

        let names = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(names)
    }
}

/// Hourly aggregated metric
#[derive(Debug, Clone)]
pub struct HourlyMetric {
    pub hour_start: i64,
    pub total: i64,
}

/// Metrics snapshot for reporting
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub connected_clients: i64,
    pub pending_messages: i64,
    pub owned_conversations: i64,
    pub total_messages_received: i64,
    pub total_messages_sent: i64,
    pub total_responses_received: i64,
    pub total_ownership_claims: i64,
    pub total_ownership_releases: i64,
}

impl MetricsStore {
    /// Get a full metrics snapshot
    pub fn get_snapshot(&self) -> Result<MetricsSnapshot, StorageError> {
        Ok(MetricsSnapshot {
            connected_clients: self.get_count(METRIC_CONNECTED_CLIENTS)?,
            pending_messages: self.get_count(METRIC_PENDING_MESSAGES)?,
            owned_conversations: self.get_count(METRIC_OWNED_CONVERSATIONS)?,
            total_messages_received: self.get_count(METRIC_MESSAGES_RECEIVED)?,
            total_messages_sent: self.get_count(METRIC_MESSAGES_SENT)?,
            total_responses_received: self.get_count(METRIC_RESPONSES_RECEIVED)?,
            total_ownership_claims: self.get_count(METRIC_OWNERSHIP_CLAIMS)?,
            total_ownership_releases: self.get_count(METRIC_OWNERSHIP_RELEASES)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Storage;
    use tempfile::tempdir;

    #[test]
    fn test_increment_and_count() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::with_path(&db_path).unwrap();
        let store = storage.metrics();

        // Increment multiple times
        store.increment(METRIC_MESSAGES_RECEIVED).unwrap();
        store.increment(METRIC_MESSAGES_RECEIVED).unwrap();
        store.increment(METRIC_MESSAGES_RECEIVED).unwrap();

        let count = store.get_count(METRIC_MESSAGES_RECEIVED).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_record_and_get_recent() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let storage = Storage::with_path(&db_path).unwrap();
        let store = storage.metrics();

        // Record some metrics
        store.record(METRIC_PENDING_MESSAGES, 5).unwrap();
        store.record(METRIC_PENDING_MESSAGES, 10).unwrap();

        let recent = store.get_recent(METRIC_PENDING_MESSAGES, 1).unwrap();
        assert_eq!(recent.len(), 2);
    }
}
