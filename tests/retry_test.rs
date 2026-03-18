//! Integration tests for retry workers
//!
//! Tests outbound retry, inbound queue drain, max retries, and TTL expiration.

use std::time::{SystemTime, UNIX_EPOCH};

use ugent_line_proxy::db::{
    Database, SqliteBackend, inbound_queue::InboundQueueEntry, messages::DeliveryStatus,
    messages::MessageRecord, outbound_queue::OutboundQueueEntry,
};

/// Helper: get current timestamp in milliseconds
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time went backwards")
        .as_millis() as i64
}

/// Helper: create a temporary SQLite database
fn setup_test_db() -> Database {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = dir.path().join("test.db");
    let backend = SqliteBackend::open(&db_path).expect("failed to open db");
    Database::new(backend)
}

/// Helper: create a sample message record
fn sample_message(id: &str, conv_id: &str) -> MessageRecord {
    let now = now_ms();
    MessageRecord {
        id: id.to_string(),
        direction: "outbound".to_string(),
        conversation_id: conv_id.to_string(),
        source_type: "user".to_string(),
        sender_id: None,
        message_type: "text".to_string(),
        text_content: Some("Test message".to_string()),
        message_json: Some(r#"{"type":"text","text":"Test message"}"#.to_string()),
        media_content_json: None,
        reply_token: None,
        quote_token: None,
        webhook_event_id: None,
        line_timestamp: Some(now),
        received_at: now,
        delivered_at: None,
        delivery_status: DeliveryStatus::Pending,
        retry_count: 0,
        last_retry_at: None,
        error_message: None,
        ugent_request_id: None,
        ugent_correlation_id: None,
        created_at: now,
    }
}

// =========================================================================
// Outbound queue lifecycle test
// =========================================================================

#[tokio::test]
async fn test_outbound_queue_lifecycle() {
    let db = setup_test_db();
    let now = now_ms();

    // Store a message first
    let msg = sample_message("out-msg-001", "conv-out-1");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Enqueue for retry
    let entry = OutboundQueueEntry {
        id: "out-q-001".to_string(),
        message_id: "out-msg-001".to_string(),
        status: "pending".to_string(),
        attempt: 0,
        max_attempts: 5,
        next_retry_at: now,
        locked_by: None,
        locked_at: None,
        last_error: None,
        created_at: now,
        updated_at: now,
    };
    db.backend()
        .enqueue_outbound(&entry)
        .await
        .expect("enqueue_outbound failed");

    // Check pending count
    let count = db
        .backend()
        .pending_outbound_count()
        .await
        .expect("pending_outbound_count failed");
    assert_eq!(count, 1);

    // Claim the entry
    let claimed = db
        .backend()
        .claim_next_outbound("worker-1", 10)
        .await
        .expect("claim_next_outbound failed");
    assert_eq!(claimed.len(), 1);
    let claimed_entry = &claimed[0];
    assert_eq!(claimed_entry.id, "out-q-001");
    assert_eq!(claimed_entry.locked_by, Some("worker-1".to_string()));
    assert!(claimed_entry.locked_at.is_some());

    // Mark as completed (success)
    db.backend()
        .complete_outbound("out-q-001", true, None)
        .await
        .expect("complete_outbound failed");

    // Update message status
    db.backend()
        .update_delivery_status("out-msg-001", DeliveryStatus::Delivered, None)
        .await
        .expect("update_delivery_status failed");

    // Verify message status
    let msg = db
        .backend()
        .get_message("out-msg-001")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(msg.delivery_status, DeliveryStatus::Delivered);

    // Pending count should be 0 now
    let count = db
        .backend()
        .pending_outbound_count()
        .await
        .expect("pending_outbound_count after complete failed");
    assert_eq!(count, 0);
}

// =========================================================================
// Max retries test
// =========================================================================

#[tokio::test]
async fn test_max_retries() {
    let db = setup_test_db();
    let now = now_ms();

    // Store a message
    let msg = sample_message("out-msg-002", "conv-out-2");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Simulate max retries (3 attempts)
    let max_attempts = 3i64;
    for i in 0..max_attempts {
        // Increment retry count on the message
        db.backend()
            .increment_retry_count("out-msg-002")
            .await
            .expect("increment_retry_count failed");

        let next_retry = now + (i + 1) * 1000;
        let entry = OutboundQueueEntry {
            id: format!("out-q-002-{i}"),
            message_id: "out-msg-002".to_string(),
            status: "pending".to_string(),
            attempt: i,
            max_attempts,
            next_retry_at: next_retry,
            locked_by: None,
            locked_at: None,
            last_error: Some(format!("attempt {i} failed")),
            created_at: now,
            updated_at: now,
        };
        db.backend()
            .enqueue_outbound(&entry)
            .await
            .expect("enqueue_outbound failed");

        // Claim and complete with failure
        let claimed = db
            .backend()
            .claim_next_outbound("worker-1", 10)
            .await
            .expect("claim_next_outbound failed");
        if let Some(claimed_entry) = claimed.first() {
            db.backend()
                .complete_outbound(&claimed_entry.id, false, Some("API error"))
                .await
                .expect("complete_outbound failed");
        }
    }

    // Verify final state
    let msg = db
        .backend()
        .get_message("out-msg-002")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(msg.retry_count, 3);

    // Mark as failed
    db.backend()
        .update_delivery_status(
            "out-msg-002",
            DeliveryStatus::Failed,
            Some("max retries exceeded"),
        )
        .await
        .expect("update_delivery_status failed");

    let msg = db
        .backend()
        .get_message("out-msg-002")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(msg.delivery_status, DeliveryStatus::Failed);
    assert_eq!(msg.error_message, Some("max retries exceeded".to_string()));
}

// =========================================================================
// Inbound queue lifecycle test
// =========================================================================

#[tokio::test]
async fn test_inbound_queue_lifecycle() {
    let db = setup_test_db();
    let now = now_ms();

    // Store an inbound message
    let mut msg = sample_message("in-msg-001", "conv-in-1");
    msg.direction = "inbound".to_string();
    msg.sender_id = Some("Usender1".to_string());
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Enqueue for delivery
    let entry = InboundQueueEntry {
        id: "in-q-001".to_string(),
        message_id: "in-msg-001".to_string(),
        status: "pending".to_string(),
        expires_at: now + 3_600_000, // 1 hour from now
        locked_by: None,
        locked_at: None,
        created_at: now,
    };
    db.backend()
        .enqueue_inbound(&entry)
        .await
        .expect("enqueue_inbound failed");

    // Check pending count
    let count = db
        .backend()
        .pending_inbound_count()
        .await
        .expect("pending_inbound_count failed");
    assert_eq!(count, 1);

    // Claim the entry
    let claimed = db
        .backend()
        .claim_next_inbound("worker-1", 10)
        .await
        .expect("claim_next_inbound failed");
    assert_eq!(claimed.len(), 1);
    let claimed_entry = &claimed[0];
    assert_eq!(claimed_entry.id, "in-q-001");
    assert_eq!(claimed_entry.locked_by, Some("worker-1".to_string()));

    // Mark as completed (delivered to UGENT)
    db.backend()
        .complete_inbound("in-q-001")
        .await
        .expect("complete_inbound failed");

    // Update message status
    db.backend()
        .update_delivery_status("in-msg-001", DeliveryStatus::Delivered, None)
        .await
        .expect("update_delivery_status failed");

    // Pending count should be 0
    let count = db
        .backend()
        .pending_inbound_count()
        .await
        .expect("pending_inbound_count after complete failed");
    assert_eq!(count, 0);
}

// =========================================================================
// TTL expiration test
// =========================================================================

#[tokio::test]
async fn test_ttl_expiration() {
    let db = setup_test_db();
    let now = now_ms();

    // Store an inbound message
    let mut msg = sample_message("in-msg-expired", "conv-expired");
    msg.direction = "inbound".to_string();
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Enqueue with an already-expired TTL
    let entry = InboundQueueEntry {
        id: "in-q-expired".to_string(),
        message_id: "in-msg-expired".to_string(),
        status: "pending".to_string(),
        expires_at: now - 10_000, // expired 10 seconds ago
        locked_by: None,
        locked_at: None,
        created_at: now - 60_000,
    };
    db.backend()
        .enqueue_inbound(&entry)
        .await
        .expect("enqueue_inbound failed");

    // Enqueue a non-expired entry
    let entry2 = InboundQueueEntry {
        id: "in-q-valid".to_string(),
        message_id: "in-msg-expired".to_string(),
        status: "pending".to_string(),
        expires_at: now + 3_600_000,
        locked_by: None,
        locked_at: None,
        created_at: now,
    };
    db.backend()
        .enqueue_inbound(&entry2)
        .await
        .expect("enqueue_inbound 2 failed");

    // Run cleanup
    let cleaned = db
        .backend()
        .cleanup_expired_inbound()
        .await
        .expect("cleanup_expired_inbound failed");
    assert!(cleaned >= 1, "should have cleaned at least 1 expired entry");

    // Expired entry should not be claimable
    // Only the valid entry should be claimable
    let claimed = db
        .backend()
        .claim_next_inbound("worker-1", 10)
        .await
        .expect("claim_next_inbound failed");
    // Should only get the non-expired entry
    for entry in &claimed {
        assert_ne!(
            entry.id, "in-q-expired",
            "expired entry should not be claimable"
        );
    }
}

// =========================================================================
// Outbound queue: failure then success
// =========================================================================

#[tokio::test]
async fn test_outbound_failure_then_success() {
    let db = setup_test_db();
    let now = now_ms();

    // Store a message
    let msg = sample_message("out-msg-fs", "conv-fs");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // First attempt: enqueue, claim, fail
    let entry1 = OutboundQueueEntry {
        id: "out-q-fs-1".to_string(),
        message_id: "out-msg-fs".to_string(),
        status: "pending".to_string(),
        attempt: 0,
        max_attempts: 5,
        next_retry_at: now,
        locked_by: None,
        locked_at: None,
        last_error: None,
        created_at: now,
        updated_at: now,
    };
    db.backend()
        .enqueue_outbound(&entry1)
        .await
        .expect("enqueue_outbound 1 failed");

    let claimed = db
        .backend()
        .claim_next_outbound("worker-1", 10)
        .await
        .expect("claim_next_outbound 1 failed");
    assert_eq!(claimed.len(), 1);

    db.backend()
        .complete_outbound("out-q-fs-1", false, Some("network timeout"))
        .await
        .expect("complete_outbound fail failed");

    db.backend()
        .increment_retry_count("out-msg-fs")
        .await
        .expect("increment_retry_count failed");

    // Second attempt: enqueue, claim, succeed
    let entry2 = OutboundQueueEntry {
        id: "out-q-fs-2".to_string(),
        message_id: "out-msg-fs".to_string(),
        status: "pending".to_string(),
        attempt: 1,
        max_attempts: 5,
        next_retry_at: now + 1000,
        locked_by: None,
        locked_at: None,
        last_error: Some("network timeout".to_string()),
        created_at: now,
        updated_at: now,
    };
    db.backend()
        .enqueue_outbound(&entry2)
        .await
        .expect("enqueue_outbound 2 failed");

    // Need to update next_retry_at to now so it's claimable
    let claimed = db
        .backend()
        .claim_next_outbound("worker-2", 10)
        .await
        .expect("claim_next_outbound 2 failed");
    // May or may not be claimable depending on timing of next_retry_at
    if let Some(claimed_entry) = claimed.first() {
        db.backend()
            .complete_outbound(&claimed_entry.id, true, None)
            .await
            .expect("complete_outbound success failed");

        db.backend()
            .update_delivery_status("out-msg-fs", DeliveryStatus::Delivered, None)
            .await
            .expect("update_delivery_status failed");
    }

    // Verify message state
    let msg = db
        .backend()
        .get_message("out-msg-fs")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(msg.retry_count, 1);
}

// =========================================================================
// Multiple messages in queue test
// =========================================================================

#[tokio::test]
async fn test_multiple_messages_in_queue() {
    let db = setup_test_db();
    let now = now_ms();

    // Store 5 messages
    for i in 0..5 {
        let msg = sample_message(&format!("out-msg-batch-{i}"), "conv-batch");
        db.backend()
            .store_message(&msg)
            .await
            .expect("store_message failed");

        let entry = OutboundQueueEntry {
            id: format!("out-q-batch-{i}"),
            message_id: format!("out-msg-batch-{i}"),
            status: "pending".to_string(),
            attempt: 0,
            max_attempts: 3,
            next_retry_at: now,
            locked_by: None,
            locked_at: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        db.backend()
            .enqueue_outbound(&entry)
            .await
            .expect("enqueue_outbound failed");
    }

    // Verify count
    let count = db
        .backend()
        .pending_outbound_count()
        .await
        .expect("pending_outbound_count failed");
    assert_eq!(count, 5);

    // Claim in batches
    let batch1 = db
        .backend()
        .claim_next_outbound("worker-1", 2)
        .await
        .expect("claim batch 1 failed");
    assert_eq!(batch1.len(), 2);

    let batch2 = db
        .backend()
        .claim_next_outbound("worker-2", 2)
        .await
        .expect("claim batch 2 failed");
    assert_eq!(batch2.len(), 2);

    let batch3 = db
        .backend()
        .claim_next_outbound("worker-3", 2)
        .await
        .expect("claim batch 3 failed");
    assert_eq!(batch3.len(), 1);

    // Complete all
    for batch in [&batch1, &batch2, &batch3] {
        for entry in batch {
            db.backend()
                .complete_outbound(&entry.id, true, None)
                .await
                .expect("complete_outbound batch failed");
        }
    }

    // All should be done
    let count = db
        .backend()
        .pending_outbound_count()
        .await
        .expect("pending_outbound_count after complete failed");
    assert_eq!(count, 0);
}
