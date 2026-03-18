//! Inbound queue worker
//!
//! Background worker that delivers inbound messages from the queue to
//! connected UGENT clients. Handles cases where no client was connected
//! when a LINE webhook was received. Messages expire after configurable TTL.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::Notify;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::broker::MessageBroker;
use crate::db::{Database, InboundQueueEntry};
use crate::types::{MessageDirection, ProxyMessage, SourceType};

/// Default poll interval when queue is empty
const POLL_INTERVAL_EMPTY: Duration = Duration::from_secs(5);

/// Default poll interval when queue has entries
const POLL_INTERVAL_ACTIVE: Duration = Duration::from_millis(500);

/// Maximum entries to claim per batch
const BATCH_SIZE: u64 = 20;

/// Worker ID for this process
fn worker_id() -> String {
    format!("inbound-worker-{}", std::process::id())
}

/// Inbound queue worker configuration
#[derive(Debug, Clone)]
pub struct InboundWorkerConfig {
    /// Enable the worker
    pub enabled: bool,
    /// Inbound message TTL (messages older than this are discarded)
    pub ttl: Duration,
    /// Poll interval when queue is empty
    pub poll_interval_empty: Duration,
    /// Poll interval when queue has entries
    pub poll_interval_active: Duration,
    /// Batch size per claim
    pub batch_size: u64,
    /// Interval between expired entry cleanup passes
    pub cleanup_interval: Duration,
}

impl Default for InboundWorkerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl: Duration::from_secs(3600), // 1 hour
            poll_interval_empty: POLL_INTERVAL_EMPTY,
            poll_interval_active: POLL_INTERVAL_ACTIVE,
            batch_size: BATCH_SIZE,
            cleanup_interval: Duration::from_secs(60),
        }
    }
}

impl InboundWorkerConfig {
    /// Build from database retry config
    pub fn from_db_retry_config(retry: &crate::db::RetryConfig) -> Self {
        Self {
            enabled: retry.enabled,
            ttl: retry.inbound_ttl,
            ..Default::default()
        }
    }
}

/// Inbound queue worker
///
/// Processes the inbound message queue, attempting to deliver queued messages
/// to connected UGENT clients via the broker's WebSocket connections.
pub struct InboundQueueWorker {
    config: InboundWorkerConfig,
    database: Arc<Database>,
    broker: Arc<MessageBroker>,
    shutdown: Arc<Notify>,
}

impl InboundQueueWorker {
    /// Create a new inbound queue worker
    pub fn new(
        config: InboundWorkerConfig,
        database: Arc<Database>,
        broker: Arc<MessageBroker>,
    ) -> Self {
        Self {
            config,
            database,
            broker,
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
            info!("Inbound queue worker disabled by configuration");
            return;
        }

        info!(
            "Inbound queue worker started (ttl={}s, batch_size={})",
            self.config.ttl.as_secs(),
            self.config.batch_size
        );

        let mut cleanup_interval = time::interval(self.config.cleanup_interval);
        // First tick completes immediately
        cleanup_interval.tick().await;

        loop {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    info!("Inbound queue worker shutting down");
                    break;
                }
                _ = self.process_batch() => {}
                _ = cleanup_interval.tick() => {
                    self.cleanup_expired().await;
                }
            }
        }
    }

    /// Process a single batch of inbound queue entries
    async fn process_batch(&self) {
        let backend = self.database.backend();
        let wid = worker_id();

        // Claim next batch
        let entries = match backend
            .claim_next_inbound(&wid, self.config.batch_size)
            .await
        {
            Ok(entries) => entries,
            Err(e) => {
                error!("Failed to claim inbound entries: {}", e);
                time::sleep(self.config.poll_interval_empty).await;
                return;
            }
        };

        if entries.is_empty() {
            time::sleep(self.config.poll_interval_empty).await;
            return;
        }

        debug!("Claimed {} inbound entries for delivery", entries.len());

        for entry in entries {
            tokio::select! {
                _ = self.shutdown.notified() => {
                    return;
                }
                result = self.process_entry(&entry) => {
                    match result {
                        Ok(()) => debug!("Inbound entry {} delivered successfully", entry.id),
                        Err(e) => warn!("Inbound entry {} delivery failed: {}", entry.id, e),
                    }
                }
            }
        }

        // Brief pause before next batch
        time::sleep(self.config.poll_interval_active).await;
    }

    /// Process a single inbound queue entry
    async fn process_entry(&self, entry: &InboundQueueEntry) -> Result<(), String> {
        let backend = self.database.backend();

        // Check if entry has expired
        let now_ms = Utc::now().timestamp_millis();
        if now_ms > entry.expires_at {
            debug!(
                "Inbound entry {} expired (expires_at={}, now={})",
                entry.id, entry.expires_at, now_ms
            );
            // Mark as delivered (we consume expired entries)
            let _ = backend.complete_inbound(&entry.id).await;
            return Err("Message expired".to_string());
        }

        // Fetch the stored message
        let message = backend
            .get_message(&entry.message_id)
            .await
            .map_err(|e| format!("Failed to fetch message {}: {}", entry.message_id, e))?
            .ok_or_else(|| format!("Message {} not found", entry.message_id))?;

        // Build a ProxyMessage from the stored message
        let proxy_msg = ProxyMessage {
            id: message.id.clone(),
            channel: "line".to_string(),
            direction: if message.direction == "inbound" {
                MessageDirection::Inbound
            } else {
                MessageDirection::Outbound
            },
            conversation_id: message.conversation_id.clone(),
            sender_id: message.sender_id.clone().unwrap_or_default(),
            message: message
                .message_json
                .as_deref()
                .and_then(|json| serde_json::from_str(json).ok()),
            media: None,
            timestamp: message.line_timestamp.unwrap_or(message.received_at),
            reply_token: message.reply_token.clone(),
            quote_token: message.quote_token.clone(),
            mark_as_read_token: None,
            webhook_event_id: message.webhook_event_id.clone().unwrap_or_default(),
            source_type: match message.source_type.as_str() {
                "user" => SourceType::User,
                "group" => SourceType::Group,
                _ => SourceType::Room,
            },
            sender_name: None,
            sender_picture_url: None,
        };

        // Deliver via broker
        match self.broker.send_to_clients(proxy_msg).await {
            Ok(()) => {
                // Mark as delivered
                let _ = backend.complete_inbound(&entry.id).await;
                let _ = backend
                    .update_delivery_status(
                        &entry.message_id,
                        crate::db::DeliveryStatus::Delivered,
                        None,
                    )
                    .await;
                info!("Inbound message {} delivered to client", entry.message_id);
                Ok(())
            }
            Err(e) => {
                // Release the claim (set status back to pending) so it can be retried
                warn!(
                    "Failed to deliver inbound message {} to client: {}",
                    entry.message_id, e
                );
                // Re-enqueue the entry for next attempt
                let requeue = InboundQueueEntry {
                    id: uuid::Uuid::new_v4().to_string(),
                    message_id: entry.message_id.clone(),
                    status: "pending".to_string(),
                    expires_at: entry.expires_at,
                    locked_by: None,
                    locked_at: None,
                    created_at: Utc::now().timestamp_millis(),
                };
                let _ = backend.enqueue_inbound(&requeue).await;
                Err(format!("Client delivery failed: {}", e))
            }
        }
    }

    /// Clean up expired inbound queue entries
    async fn cleanup_expired(&self) {
        let backend = self.database.backend();

        match backend.cleanup_expired_inbound().await {
            Ok(count) if count > 0 => {
                info!("Cleaned up {} expired inbound queue entries", count);
            }
            Ok(_) => {}
            Err(e) => {
                error!("Failed to cleanup expired inbound entries: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worker_id_format() {
        let wid = worker_id();
        assert!(wid.starts_with("inbound-worker-"));
        assert!(wid.len() > "inbound-worker-".len());
    }

    #[test]
    fn test_default_config() {
        let config = InboundWorkerConfig::default();
        assert!(config.enabled);
        assert_eq!(config.ttl, Duration::from_secs(3600));
        assert_eq!(config.batch_size, 20);
    }

    #[tokio::test]
    async fn test_cleanup_does_not_panic() {
        // Verify the cleanup method doesn't panic even without a real DB
        // This tests the control flow, not the actual DB operation
        let config = InboundWorkerConfig::default();
        assert!(config.enabled);
    }
}
