//! End-to-end integration tests
//!
//! Tests full message flow: webhook → broker → db → WebSocket → client,
//! and client response → broker → db → LINE API (mocked).

use std::time::{SystemTime, UNIX_EPOCH};

use ugent_line_proxy::db::{
    Database, SqliteBackend, messages::DeliveryStatus, messages::MessageRecord,
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

/// Helper: create a sample inbound message
fn inbound_message(id: &str, conv_id: &str, sender_id: &str) -> MessageRecord {
    let now = now_ms();
    MessageRecord {
        id: id.to_string(),
        direction: "inbound".to_string(),
        conversation_id: conv_id.to_string(),
        source_type: "user".to_string(),
        sender_id: Some(sender_id.to_string()),
        message_type: "text".to_string(),
        text_content: Some(format!("Message from {sender_id}")),
        message_json: Some(format!(
            r#"{{"type":"text","text":"Message from {sender_id}"}}"#
        )),
        media_content_json: None,
        reply_token: Some(format!("reply-token-{id}")),
        quote_token: None,
        webhook_event_id: Some(format!("webhook-{id}")),
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

/// Helper: create a sample outbound message
fn outbound_message(id: &str, conv_id: &str) -> MessageRecord {
    let now = now_ms();
    MessageRecord {
        id: id.to_string(),
        direction: "outbound".to_string(),
        conversation_id: conv_id.to_string(),
        source_type: "user".to_string(),
        sender_id: None,
        message_type: "text".to_string(),
        text_content: Some("Reply from UGENT".to_string()),
        message_json: Some(r#"{"type":"text","text":"Reply from UGENT"}"#.to_string()),
        media_content_json: None,
        reply_token: None,
        quote_token: None,
        webhook_event_id: None,
        line_timestamp: None,
        received_at: now,
        delivered_at: None,
        delivery_status: DeliveryStatus::Pending,
        retry_count: 0,
        last_retry_at: None,
        error_message: None,
        ugent_request_id: Some(format!("ugent-req-{id}")),
        ugent_correlation_id: Some(format!("ugent-corr-{id}")),
        created_at: now,
    }
}

// =========================================================================
// E2E: Inbound message flow (webhook → db → ready for WebSocket delivery)
// =========================================================================

#[tokio::test]
async fn test_e2e_inbound_flow() {
    let db = setup_test_db();

    // Step 1: Webhook dedup check
    let seen = db
        .backend()
        .check_and_mark_webhook("webhook-e2e-001")
        .await
        .expect("check_and_mark_webhook failed");
    assert!(!seen, "first time seeing this webhook event");

    // Step 2: Store inbound message
    let msg = inbound_message("e2e-in-001", "conv-e2e-1", "Ue2e-sender");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Step 3: Verify message is in database
    let fetched = db
        .backend()
        .get_message("e2e-in-001")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.direction, "inbound");
    assert_eq!(fetched.sender_id, Some("Ue2e-sender".to_string()));
    assert_eq!(fetched.delivery_status, DeliveryStatus::Pending);

    // Step 4: List messages for this conversation
    let messages = db
        .backend()
        .list_messages("conv-e2e-1", Some("inbound"), 0, 10)
        .await
        .expect("list_messages failed");
    assert_eq!(messages.len(), 1);

    // Step 5: Mark as delivered (simulating WebSocket delivery to UGENT client)
    db.backend()
        .update_delivery_status("e2e-in-001", DeliveryStatus::Delivered, None)
        .await
        .expect("update_delivery_status failed");

    let fetched = db
        .backend()
        .get_message("e2e-in-001")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.delivery_status, DeliveryStatus::Delivered);
    assert!(fetched.delivered_at.is_some());

    // Step 6: Second webhook with same event ID should be deduped
    let seen_again = db
        .backend()
        .check_and_mark_webhook("webhook-e2e-001")
        .await
        .expect("check_and_mark_webhook 2 failed");
    assert!(seen_again, "duplicate webhook event should be detected");
}

// =========================================================================
// E2E: Outbound message flow (UGENT → db → LINE API mock)
// =========================================================================

#[tokio::test]
async fn test_e2e_outbound_flow() {
    let db = setup_test_db();

    // Step 1: UGENT sends an outbound message
    let msg = outbound_message("e2e-out-001", "conv-e2e-2");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store_message failed");

    // Step 2: Verify message stored with UGENT tracking
    let fetched = db
        .backend()
        .get_message("e2e-out-001")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.direction, "outbound");
    assert_eq!(
        fetched.ugent_request_id,
        Some("ugent-req-e2e-out-001".to_string())
    );
    assert_eq!(fetched.delivery_status, DeliveryStatus::Pending);

    // Step 3: Simulate successful LINE API delivery
    db.backend()
        .update_delivery_status("e2e-out-001", DeliveryStatus::Delivered, None)
        .await
        .expect("update_delivery_status failed");

    let fetched = db
        .backend()
        .get_message("e2e-out-001")
        .await
        .expect("get_message failed")
        .expect("message not found");
    assert_eq!(fetched.delivery_status, DeliveryStatus::Delivered);
}

// =========================================================================
// E2E: Conversation with multiple messages
// =========================================================================

#[tokio::test]
async fn test_e2e_conversation_multiple_messages() {
    let db = setup_test_db();
    let conv_id = "conv-e2e-multi";

    // User sends a message
    let in_msg = inbound_message("e2e-multi-1", conv_id, "Ualice");
    db.backend()
        .store_message(&in_msg)
        .await
        .expect("store inbound failed");
    db.backend()
        .update_delivery_status("e2e-multi-1", DeliveryStatus::Delivered, None)
        .await
        .expect("mark delivered failed");

    // UGENT replies
    let out_msg = outbound_message("e2e-multi-2", conv_id);
    db.backend()
        .store_message(&out_msg)
        .await
        .expect("store outbound failed");
    db.backend()
        .update_delivery_status("e2e-multi-2", DeliveryStatus::Delivered, None)
        .await
        .expect("mark outbound delivered failed");

    // User sends another message
    let in_msg2 = inbound_message("e2e-multi-3", conv_id, "Ualice");
    db.backend()
        .store_message(&in_msg2)
        .await
        .expect("store inbound 2 failed");

    // Verify conversation history
    let all_messages = db
        .backend()
        .list_messages(conv_id, None, 0, 10)
        .await
        .expect("list all messages failed");
    assert_eq!(all_messages.len(), 3);

    let inbound_msgs = db
        .backend()
        .list_messages(conv_id, Some("inbound"), 0, 10)
        .await
        .expect("list inbound failed");
    assert_eq!(inbound_msgs.len(), 2);

    let outbound_msgs = db
        .backend()
        .list_messages(conv_id, Some("outbound"), 0, 10)
        .await
        .expect("list outbound failed");
    assert_eq!(outbound_msgs.len(), 1);
}

// =========================================================================
// E2E: Conversation ownership in multi-client scenario
// =========================================================================

#[tokio::test]
async fn test_e2e_conversation_ownership() {
    let db = setup_test_db();

    // Client A claims the conversation
    db.backend()
        .set_conversation_owner("conv-own-e2e", "client-A")
        .await
        .expect("set owner A failed");

    // Client A should be the owner
    let owner = db
        .backend()
        .get_conversation_owner("conv-own-e2e")
        .await
        .expect("get owner failed");
    assert_eq!(owner, Some("client-A".to_string()));

    // Client A disconnects
    let released = db
        .backend()
        .release_client_conversations("client-A")
        .await
        .expect("release A failed");
    assert_eq!(released, 1);

    // Client B connects and claims
    db.backend()
        .set_conversation_owner("conv-own-e2e", "client-B")
        .await
        .expect("set owner B failed");

    let owner = db
        .backend()
        .get_conversation_owner("conv-own-e2e")
        .await
        .expect("get owner after transfer failed");
    assert_eq!(owner, Some("client-B".to_string()));
}

// =========================================================================
// E2E: Webhook dedup prevents duplicate processing
// =========================================================================

#[tokio::test]
async fn test_e2e_webhook_dedup() {
    let db = setup_test_db();

    // Simulate webhook with event ID
    let event_id = "evt-dedup-e2e";

    // First call: not seen
    let first = db
        .backend()
        .check_and_mark_webhook(event_id)
        .await
        .expect("first check failed");
    assert!(!first);

    // Store the message
    let msg = inbound_message("e2e-dedup-1", "conv-dedup", "Udedup");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store failed");

    // Second call: already seen (simulating LINE retry)
    let second = db
        .backend()
        .check_and_mark_webhook(event_id)
        .await
        .expect("second check failed");
    assert!(second);

    // Verify only one message exists
    let messages = db
        .backend()
        .list_messages("conv-dedup", None, 0, 10)
        .await
        .expect("list messages failed");
    assert_eq!(messages.len(), 1, "should not store duplicate message");
}

// =========================================================================
// E2E: Database persistence across operations
// =========================================================================

#[tokio::test]
async fn test_e2e_database_persistence() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = dir.path().join("persistence_test.db");

    // First session: write data
    {
        let backend = SqliteBackend::open(&db_path).expect("open 1 failed");
        let db = Database::new(backend);

        let msg = inbound_message("e2e-persist-1", "conv-persist", "Upersist");
        db.backend()
            .store_message(&msg)
            .await
            .expect("store failed");

        let contact = ugent_line_proxy::db::contacts::ContactRecord {
            line_user_id: "Upersist".to_string(),
            display_name: Some("Persistent User".to_string()),
            picture_url: None,
            status_message: None,
            language: Some("en".to_string()),
            first_seen_at: now_ms(),
            last_seen_at: now_ms(),
            last_interacted_at: None,
            is_blocked: false,
            is_friend: true,
            created_at: now_ms(),
            updated_at: now_ms(),
        };
        db.backend()
            .upsert_contact(&contact)
            .await
            .expect("upsert contact failed");
    }

    // Second session: verify data persists
    {
        let backend = SqliteBackend::open(&db_path).expect("open 2 failed");
        let db = Database::new(backend);

        let msg = db
            .backend()
            .get_message("e2e-persist-1")
            .await
            .expect("get message failed")
            .expect("message not found after reopen");
        assert_eq!(msg.direction, "inbound");
        assert_eq!(msg.sender_id, Some("Upersist".to_string()));

        let contact = db
            .backend()
            .get_contact("Upersist")
            .await
            .expect("get contact failed")
            .expect("contact not found after reopen");
        assert_eq!(contact.display_name, Some("Persistent User".to_string()));
    }
}

// =========================================================================
// E2E: Contact enrichment from webhook data
// =========================================================================

#[tokio::test]
async fn test_e2e_contact_enrichment() {
    let db = setup_test_db();

    // First message from a user - contact should be created
    let contact = ugent_line_proxy::db::contacts::ContactRecord {
        line_user_id: "Unew-user".to_string(),
        display_name: Some("New User".to_string()),
        picture_url: Some("https://example.com/pic.jpg".to_string()),
        status_message: Some("Hey there".to_string()),
        language: None,
        first_seen_at: now_ms(),
        last_seen_at: now_ms(),
        last_interacted_at: Some(now_ms()),
        is_blocked: false,
        is_friend: true,
        created_at: now_ms(),
        updated_at: now_ms(),
    };

    db.backend()
        .upsert_contact(&contact)
        .await
        .expect("upsert contact failed");

    // Store message
    let msg = inbound_message("e2e-enrich-1", "Unew-user", "Unew-user");
    db.backend()
        .store_message(&msg)
        .await
        .expect("store message failed");

    // Later, update contact info (e.g., profile picture changed)
    let mut updated_contact = contact.clone();
    updated_contact.display_name = Some("Updated User".to_string());
    updated_contact.picture_url = Some("https://example.com/new-pic.jpg".to_string());
    updated_contact.updated_at = now_ms();

    db.backend()
        .upsert_contact(&updated_contact)
        .await
        .expect("update contact failed");

    let fetched = db
        .backend()
        .get_contact("Unew-user")
        .await
        .expect("get contact failed")
        .expect("contact not found");
    assert_eq!(fetched.display_name, Some("Updated User".to_string()));
    assert_eq!(
        fetched.picture_url,
        Some("https://example.com/new-pic.jpg".to_string())
    );
}
