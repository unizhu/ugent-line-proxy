//! Integration tests for the database layer
//!
//! Tests SQLite backend CRUD operations, schema migration, and concurrent writes.

use std::time::{SystemTime, UNIX_EPOCH};

use ugent_line_proxy::db::{
    Database, SqliteBackend, contacts::ContactRecord, groups::GroupRecord,
    messages::DeliveryStatus, messages::MessageRecord,
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

/// Helper: create a sample contact record
fn sample_contact(user_id: &str) -> ContactRecord {
    let now = now_ms();
    ContactRecord {
        line_user_id: user_id.to_string(),
        display_name: Some(format!("User {user_id}")),
        picture_url: Some(format!("https://example.com/pic/{user_id}.jpg")),
        status_message: Some("Hello".to_string()),
        language: Some("en".to_string()),
        first_seen_at: now,
        last_seen_at: now,
        last_interacted_at: None,
        is_blocked: false,
        is_friend: true,
        created_at: now,
        updated_at: now,
    }
}

/// Helper: create a sample message record
fn sample_message(id: &str, conv_id: &str, direction: &str) -> MessageRecord {
    let now = now_ms();
    MessageRecord {
        id: id.to_string(),
        direction: direction.to_string(),
        conversation_id: conv_id.to_string(),
        source_type: "user".to_string(),
        sender_id: Some("U123".to_string()),
        message_type: "text".to_string(),
        text_content: Some("Hello world".to_string()),
        message_json: None,
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

/// Helper: create a sample group record
fn sample_group(group_id: &str) -> GroupRecord {
    let now = now_ms();
    GroupRecord {
        line_group_id: group_id.to_string(),
        group_name: Some(format!("Group {group_id}")),
        picture_url: None,
        member_count: Some(5),
        first_seen_at: now,
        last_message_at: Some(now),
        created_at: now,
        updated_at: now,
    }
}

// =========================================================================
// Contact CRUD tests
// =========================================================================

#[tokio::test]
async fn test_contact_crud() {
    let db = setup_test_db();
    let contact = sample_contact("U999");

    // Create
    db.backend()
        .upsert_contact(&contact)
        .await
        .expect("upsert_contact failed");

    // Read
    let fetched = db
        .backend()
        .get_contact("U999")
        .await
        .expect("get_contact failed");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.line_user_id, "U999");
    assert_eq!(fetched.display_name, Some("User U999".to_string()));
    assert!(fetched.is_friend);
    assert!(!fetched.is_blocked);

    // Update
    let mut updated = contact.clone();
    updated.display_name = Some("Updated Name".to_string());
    updated.is_blocked = true;
    updated.updated_at = now_ms();
    db.backend()
        .upsert_contact(&updated)
        .await
        .expect("upsert_contact update failed");

    let fetched = db
        .backend()
        .get_contact("U999")
        .await
        .expect("get_contact after update failed")
        .expect("contact not found after update");
    assert_eq!(fetched.display_name, Some("Updated Name".to_string()));
    assert!(fetched.is_blocked);

    // List
    let contacts = db
        .backend()
        .list_contacts(0, 10)
        .await
        .expect("list_contacts failed");
    assert_eq!(contacts.len(), 1);

    // Search
    let results = db
        .backend()
        .search_contacts("Updated", 10)
        .await
        .expect("search_contacts failed");
    assert_eq!(results.len(), 1);

    let no_results = db
        .backend()
        .search_contacts("nonexistent", 10)
        .await
        .expect("search_contacts no results failed");
    assert!(no_results.is_empty());

    // Not found
    let not_found = db
        .backend()
        .get_contact("U000")
        .await
        .expect("get_contact not found failed");
    assert!(not_found.is_none());
}

// =========================================================================
// Group CRUD tests
// =========================================================================

#[tokio::test]
async fn test_group_crud() {
    let db = setup_test_db();
    let group = sample_group("C111");

    // Create
    db.backend()
        .upsert_group(&group)
        .await
        .expect("upsert_group failed");

    // Read
    let fetched = db
        .backend()
        .get_group("C111")
        .await
        .expect("get_group failed");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.line_group_id, "C111");
    assert_eq!(fetched.group_name, Some("Group C111".to_string()));

    // List groups
    let groups = db
        .backend()
        .list_groups(0, 10)
        .await
        .expect("list_groups failed");
    assert_eq!(groups.len(), 1);
    // Not found
    let not_found = db
        .backend()
        .get_group("C000")
        .await
        .expect("get_group not found failed");
    assert!(not_found.is_none());
}

// =========================================================================
// Message CRUD tests
// =========================================================================

#[tokio::test]
async fn test_message_store_and_retrieve() {
    let db = setup_test_db();
    let msg = sample_message("msg-001", "conv-001", "inbound");

    // Store
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Retrieve
    let fetched = db
        .backend()
        .get_message("msg-001")
        .await
        .expect("get_message failed");
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, "msg-001");
    assert_eq!(fetched.direction, "inbound");
    assert_eq!(fetched.conversation_id, "conv-001");
    assert_eq!(fetched.text_content, Some("Hello world".to_string()));
    assert_eq!(fetched.delivery_status, DeliveryStatus::Pending);

    // List messages
    let messages = db
        .backend()
        .list_messages("conv-001", None, 0, 10)
        .await
        .expect("list_messages failed");
    assert_eq!(messages.len(), 1);

    // List with direction filter
    let inbound = db
        .backend()
        .list_messages("conv-001", Some("inbound"), 0, 10)
        .await
        .expect("list_messages direction filter failed");
    assert_eq!(inbound.len(), 1);

    let outbound = db
        .backend()
        .list_messages("conv-001", Some("outbound"), 0, 10)
        .await
        .expect("list_messages outbound filter failed");
    assert!(outbound.is_empty());
}

#[tokio::test]
async fn test_message_delivery_status_lifecycle() {
    let db = setup_test_db();
    let msg = sample_message("msg-002", "conv-002", "outbound");

    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Increment retry
    db.backend()
        .increment_retry_count("msg-002")
        .await
        .expect("increment_retry_count failed");

    let fetched = db
        .backend()
        .get_message("msg-002")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.retry_count, 1);

    // Update status to delivered
    db.backend()
        .update_delivery_status("msg-002", DeliveryStatus::Delivered, None)
        .await
        .expect("update_delivery_status failed");

    let fetched = db
        .backend()
        .get_message("msg-002")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.delivery_status, DeliveryStatus::Delivered);

    // Update status to failed with error
    db.backend()
        .update_delivery_status("msg-002", DeliveryStatus::Failed, Some("timeout"))
        .await
        .expect("update_delivery_status failed failed");

    let fetched = db
        .backend()
        .get_message("msg-002")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.delivery_status, DeliveryStatus::Failed);
    assert_eq!(fetched.error_message, Some("timeout".to_string()));
}

// =========================================================================
// Schema migration test
// =========================================================================

#[tokio::test]
async fn test_schema_migration_from_scratch() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = dir.path().join("migration_test.db");

    // Verify the file doesn't exist yet
    assert!(!db_path.exists());

    // Open should create the file and run schema migrations
    let backend = SqliteBackend::open(&db_path).expect("failed to open db");
    let db = Database::new(backend);

    // Verify the file was created
    assert!(db_path.exists());

    // Verify we can use the database
    assert!(db.backend().ping().await.expect("ping failed"));

    // Verify tables were created by performing CRUD operations
    let contact = sample_contact("Umigration");
    db.backend()
        .upsert_contact(&contact)
        .await
        .expect("upsert after migration failed");
    let fetched = db
        .backend()
        .get_contact("Umigration")
        .await
        .expect("get after migration failed");
    assert!(fetched.is_some());
}

// =========================================================================
// Concurrent writes test
// =========================================================================

#[tokio::test]
async fn test_concurrent_writes() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = dir.path().join("concurrent_test.db");
    let backend = SqliteBackend::open(&db_path).expect("failed to open db");
    let db = Database::new(backend);

    // Spawn multiple tasks writing contacts concurrently
    let mut handles = Vec::new();
    for i in 0..20 {
        let db_clone = db.clone();
        let handle = tokio::spawn(async move {
            let contact = sample_contact(&format!("Uconcurrent_{i}"));
            db_clone
                .backend()
                .upsert_contact(&contact)
                .await
                .expect("concurrent upsert_contact failed");
        });
        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await.expect("task panicked");
    }

    // Verify all contacts were written
    let contacts = db
        .backend()
        .list_contacts(0, 100)
        .await
        .expect("list_contacts after concurrent writes failed");
    assert_eq!(contacts.len(), 20);

    // Verify we can read each one
    for i in 0..20 {
        let user_id = format!("Uconcurrent_{i}");
        let fetched = db
            .backend()
            .get_contact(&user_id)
            .await
            .expect("get_contact failed")
            .expect("contact not found after concurrent write");
        assert_eq!(fetched.line_user_id, user_id);
    }
}

// =========================================================================
// Webhook dedup test
// =========================================================================

#[tokio::test]
async fn test_webhook_dedup() {
    let db = setup_test_db();

    // First call should return false (not seen)
    let first = db
        .backend()
        .check_and_mark_webhook("evt-001")
        .await
        .expect("check_and_mark_webhook failed");
    assert!(!first);

    // Second call should return true (already seen)
    let second = db
        .backend()
        .check_and_mark_webhook("evt-001")
        .await
        .expect("check_and_mark_webhook failed");
    assert!(second);

    // Different event should return false
    let third = db
        .backend()
        .check_and_mark_webhook("evt-002")
        .await
        .expect("check_and_mark_webhook failed");
    assert!(!third);
}

// =========================================================================
// Conversation ownership test
// =========================================================================

#[tokio::test]
async fn test_conversation_ownership() {
    let db = setup_test_db();

    // No owner initially
    let owner = db
        .backend()
        .get_conversation_owner("conv-own-1")
        .await
        .expect("get_conversation_owner failed");
    assert!(owner.is_none());

    // Set owner
    db.backend()
        .set_conversation_owner("conv-own-1", "client-A")
        .await
        .expect("set_conversation_owner failed");

    let owner = db
        .backend()
        .get_conversation_owner("conv-own-1")
        .await
        .expect("get_conversation_owner failed")
        .expect("owner not found");
    assert_eq!(owner, "client-A");

    // Transfer ownership
    db.backend()
        .set_conversation_owner("conv-own-1", "client-B")
        .await
        .expect("set_conversation_owner transfer failed");

    let owner = db
        .backend()
        .get_conversation_owner("conv-own-1")
        .await
        .expect("get_conversation_owner failed")
        .expect("owner not found after transfer");
    assert_eq!(owner, "client-B");

    // Release client conversations
    let released = db
        .backend()
        .release_client_conversations("client-B")
        .await
        .expect("release_client_conversations failed");
    assert_eq!(released, 1);

    let owner = db
        .backend()
        .get_conversation_owner("conv-own-1")
        .await
        .expect("get_conversation_owner failed");
    assert!(owner.is_none());

    // Release non-existent client should return 0
    let released = db
        .backend()
        .release_client_conversations("client-Z")
        .await
        .expect("release_client_conversations non-existent failed");
    assert_eq!(released, 0);
}

// =========================================================================
// Metrics test
// =========================================================================

#[tokio::test]
async fn test_metrics() {
    let db = setup_test_db();

    // Record some metrics
    db.backend()
        .record_metric("messages_received", 10)
        .await
        .expect("record_metric failed");
    db.backend()
        .record_metric("messages_received", 5)
        .await
        .expect("record_metric 2 failed");
    db.backend()
        .record_metric("messages_sent", 3)
        .await
        .expect("record_metric 3 failed");

    // Get metrics since epoch 0 (all)
    let received = db
        .backend()
        .get_metrics("messages_received", 0)
        .await
        .expect("get_metrics failed");
    assert_eq!(received.len(), 2);

    let sent = db
        .backend()
        .get_metrics("messages_sent", 0)
        .await
        .expect("get_metrics sent failed");
    assert_eq!(sent.len(), 1);
}

// =========================================================================
// Ping test
// =========================================================================

#[tokio::test]
async fn test_ping() {
    let db = setup_test_db();
    let alive = db.backend().ping().await.expect("ping failed");
    assert!(alive);
}

// =========================================================================
// Maintenance test
// =========================================================================

#[tokio::test]
async fn test_maintenance() {
    let db = setup_test_db();
    db.backend()
        .run_maintenance()
        .await
        .expect("run_maintenance failed");
    // If we got here, maintenance ran without error
}
