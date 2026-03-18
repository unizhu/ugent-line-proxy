//! Metric record type

use serde::{Deserialize, Serialize};

/// Metric record (stored in metrics table)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricRecord {
    /// Metric name
    pub name: String,
    /// Metric value
    pub value: i64,
    /// Record timestamp (Unix ms)
    pub recorded_at: i64,
}
