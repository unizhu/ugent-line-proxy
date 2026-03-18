//! Outbound retry worker
//!
//! Background worker that processes outbound message queue entries with
//! exponential backoff retry logic. Claims pending entries from the database,
//! attempts LINE API delivery, and marks them complete or failed.

use std::sync::Arc;
use std::time::Duration;

use backon::ExponentialBuilder;
use backon::Retryable;
use chrono::Utc;
use tokio::sync::Notify;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::db::{Database, OutboundQueueEntry};
use crate::line_api::LineApiClient;

/// Default poll interval when queue is empty
const POLL_INTERVAL_EMPTY: Duration = Duration::from_secs(5);

/// Default poll interval when queue has entries
const POLL_INTERVAL_ACTIVE: Duration = Duration::from_millis(500);

/// Maximum entries to claim per batch
const BATCH_SIZE: u64 = 10;

/// Worker ID for this process
fn worker_id() -> String {
    format!("worker-{}", std::process::id())
}

/// Outbound retry worker configuration
#[derive(Debug, Clone)]
pub struct OutboundWorkerConfig {
    /// Enable the worker
    pub enabled: bool,
    /// Maximum retry attempts
    pub max_attempts: u32,
    /// Initial backoff delay
    pub initial_delay: Duration,
    /// Maximum backoff delay
    pub max_delay: Duration,
    /// Poll interval when queue is empty
    pub poll_interval_empty: Duration,
    /// Poll interval when queue has entries
    pub poll_interval_active: Duration,
    /// Batch size per claim
    pub batch_size: u64,
}

impl Default for OutboundWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            poll_interval_empty: POLL_INTERVAL_EMPTY,
            poll_interval_active: POLL_INTERVAL_ACTIVE,
            batch_size: BATCH_SIZE,
        }
    }
}

impl OutboundWorkerConfig {
    /// Build from database retry config
    pub fn from_db_retry_config(retry: &crate::db::RetryConfig) -> Self {
        Self {
            enabled: retry.enabled,
            max_attempts: retry.max_attempts,
            initial_delay: retry.initial_delay,
            max_delay: retry.max_delay,
            ..Default::default()
        }
    }
}

/// Outbound retry worker
///
/// Processes the outbound message queue, attempting to deliver messages
/// to LINE API with exponential backoff on failures.
pub struct OutboundRetryWorker {
    config: OutboundWorkerConfig,
    database: Arc<Database>,
    line_client: LineApiClient,
    shutdown: Arc<Notify>,
}

impl OutboundRetryWorker {
    /// Create a new outbound retry worker
    pub fn new(
        config: OutboundWorkerConfig,
        database: Arc<Database>,
        line_client: LineApiClient,
    ) -> Self {
        Self {
            config,
            database,
            line_client,
            shutdown: Arc::new(Notify::new()),
        }
    }

    /// Get a handle for shutting down the worker
    pub fn shutdown_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.shutdown)
    }

    /// Run the worker loop until shutdown is signaled
    pub async fn run(&self) {
        if !self.config.enabled {
            info!("Outbound retry worker disabled by configuration");
            return;
        }

        info!(
            "Outbound retry worker started (max_attempts={}, batch_size={})",
            self.config.max_attempts, self.config.batch_size
        );

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    info!("Outbound retry worker shutting down");
                    break;
                }
                _ = self.process_batch() => {}
            }
        }
    }

    /// Process a single batch of outbound queue entries
    async fn process_batch(&self) {
        let backend = self.database.backend();
        let wid = worker_id();

        // Claim next batch
        let entries = match backend
            .claim_next_outbound(&wid, self.config.batch_size)
            .await
        {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to claim outbound entries: {}", e);
                time::sleep(self.config.poll_interval_empty).await;
                return;
            }
        };

        if entries.is_empty() {
            time::sleep(self.config.poll_interval_empty).await;
            return;
        }

        debug!("Claimed {} outbound entries for processing", entries.len());

        for entry in entries {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    return;
                }
                result = self.process_entry(&entry) => {
                    match result {
                        Ok(()) => debug!("Outbound entry {} completed successfully", entry.id),
                        Err(e) => warn!("Outbound entry {} failed: {}", entry.id, e),
                    }
                }
            }
        }

        // Brief pause before next batch
        time::sleep(self.config.poll_interval_active).await;
    }

    /// Process a single outbound queue entry with retry
    async fn process_entry(&self, entry: &OutboundQueueEntry) -> Result<(), String> {
        let backend = self.database.backend();

        // Fetch the stored message
        let message = backend
            .get_message(&entry.message_id)
            .await
            .map_err(|e| format!("Failed to fetch message {}: {}", entry.message_id, e))?
            .ok_or_else(|| format!("Message {} not found", entry.message_id))?;

        // Check if max attempts exceeded
        if (entry.attempt as u32) >= self.config.max_attempts {
            warn!(
                "Outbound entry {} exceeded max attempts ({})",
                entry.id, entry.attempt
            );
            let _ = backend
                .complete_outbound(&entry.id, false, Some("Max retry attempts exceeded"))
                .await;
            let _ = backend
                .update_delivery_status(
                    &entry.message_id,
                    crate::db::DeliveryStatus::Failed,
                    Some("Max retry attempts exceeded"),
                )
                .await;
            return Err("Max retry attempts exceeded".to_string());
        }

        // Attempt delivery via LINE API
        let reply_token = message.reply_token.as_deref().unwrap_or("");
        let text = message.text_content.as_deref().unwrap_or("");

        let result = self.deliver_with_retry(reply_token, text).await;

        match result {
            Ok(()) => {
                // Mark as completed
                let _ = backend.complete_outbound(&entry.id, true, None).await;
                let _ = backend
                    .update_delivery_status(
                        &entry.message_id,
                        crate::db::DeliveryStatus::Delivered,
                        None,
                    )
                    .await;
                info!(
                    "Message {} delivered successfully (attempt {}/{})",
                    entry.message_id,
                    entry.attempt + 1,
                    self.config.max_attempts
                );
                Ok(())
            }
            Err(err_msg) => {
                // Mark as failed, the queue will pick it up on next retry
                let _ = backend
                    .complete_outbound(&entry.id, false, Some(&err_msg))
                    .await;
                let _ = backend.increment_retry_count(&entry.message_id).await;

                // Re-enqueue for next attempt if under max
                if (entry.attempt as u32) + 1 < self.config.max_attempts {
                    let next_retry = calculate_next_retry(
                        entry.attempt as u32 + 1,
                        self.config.initial_delay,
                        self.config.max_delay,
                    );
                    let requeue = OutboundQueueEntry {
                        id: uuid::Uuid::new_v4().to_string(),
                        message_id: entry.message_id.clone(),
                        status: "pending".to_string(),
                        attempt: entry.attempt + 1,
                        max_attempts: self.config.max_attempts as i64,
                        next_retry_at: next_retry,
                        locked_by: None,
                        locked_at: None,
                        last_error: Some(err_msg.clone()),
                        created_at: Utc::now().timestamp_millis(),
                        updated_at: Utc::now().timestamp_millis(),
                    };
                    let _ = backend.enqueue_outbound(&requeue).await;
                } else {
                    let _ = backend
                        .update_delivery_status(
                            &entry.message_id,
                            crate::db::DeliveryStatus::Failed,
                            Some(&err_msg),
                        )
                        .await;
                }

                Err(err_msg)
            }
        }
    }

    /// Deliver a message to LINE with exponential backoff retry
    async fn deliver_with_retry(&self, reply_token: &str, text: &str) -> Result<(), String> {
        if reply_token.is_empty() {
            return Err("No reply token available".to_string());
        }

        let token = reply_token.to_string();
        let text = text.to_string();
        let client = self.line_client.clone();
        let initial_delay = self.config.initial_delay;
        let max_delay = self.config.max_delay;

        let result = (|| async {
            client
                .reply_message(&token, vec![crate::line_api::build_text_message(&text)])
                .await
                .map_err(|e| e.to_string())
        })
        .retry(
            ExponentialBuilder::new()
                .with_min_delay(initial_delay)
                .with_max_delay(max_delay)
                .with_max_times(3usize),
        )
        .await;

        result.map_err(|e| format!("LINE API delivery failed: {}", e))
    }
}

/// Calculate the next retry timestamp using exponential backoff
fn calculate_next_retry(attempt: u32, initial_delay: Duration, max_delay: Duration) -> i64 {
    let delay_ms = if attempt == 0 {
        initial_delay.as_millis() as u64
    } else {
        (initial_delay.as_millis() as u64)
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1)))
    };
    let capped_delay = delay_ms.min(max_delay.as_millis() as u64);
    Utc::now().timestamp_millis() + capped_delay as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_next_retry() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(60);

        let now = Utc::now().timestamp_millis();
        let t1 = calculate_next_retry(1, initial, max);
        let t2 = calculate_next_retry(2, initial, max);
        let t3 = calculate_next_retry(3, initial, max);

        let d1 = t1 - now;
        let d2 = t2 - now;
        let d3 = t3 - now;

        // Each delay should be roughly: initial * 2^(attempt-1)
        // attempt 1: 1s, attempt 2: 2s, attempt 3: 4s
        assert!(
            (900..=1100).contains(&d1),
            "attempt 1 delay ~1s, got {}",
            d1
        );
        assert!(
            (1900..=2100).contains(&d2),
            "attempt 2 delay ~2s, got {}",
            d2
        );
        assert!(
            (3900..=4100).contains(&d3),
            "attempt 3 delay ~4s, got {}",
            d3
        );

        // Verify exponential growth
        assert!(d2 > d1);
        assert!(d3 > d2);
    }

    #[test]
    fn test_calculate_next_retry_caps_at_max() {
        let initial = Duration::from_secs(1);
        let max = Duration::from_secs(10);

        // With high attempt number, delay should cap at max
        let now = Utc::now().timestamp_millis();
        let t = calculate_next_retry(100, initial, max);
        let delay = t - now;

        // Should not exceed max + some tolerance for processing
        assert!(delay <= max.as_millis() as i64 + 100);
    }

    #[test]
    fn test_worker_id_format() {
        let wid = worker_id();
        assert!(wid.starts_with("worker-"));
        assert!(wid.len() > "worker-".len());
    }

    #[test]
    fn test_default_config() {
        let config = OutboundWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.batch_size, 10);
    }
}
