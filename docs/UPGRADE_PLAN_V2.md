# LINE Proxy v2 Upgrade Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Upgrade ugent-line-proxy with targeted ResponseResult delivery, mark-as-read API, typing indicator on inbound, and fix missing LINE API fields — bringing the proxy to full feature parity with the channel-line worker plugin.

**Architecture:** The proxy sits between LINE Platform and UGENT's channel-line worker. Messages flow: `LINE → Webhook → Broker → WebSocket → Worker → Response → Broker → LINE API`. The upgrade targets the broker routing, webhook handler, LINE API client, and type definitions. No architectural changes needed — each task is an additive fix.

**Tech Stack:** Rust (edition 2021 → 2024), axum, reqwest, tokio, serde, thiserror, tracing

---

## Codebase Map (Files Modified)

| File | Lines | Role |
|------|-------|------|
| `src/broker.rs` | 596 | Message routing, response handling, ResponseResult |
| `src/line_api.rs` | 701 | LINE API client (reply, push, download, etc.) |
| `src/types.rs` | 1312 | All type definitions (events, messages, protocol) |
| `src/webhook/mod.rs` | 274 | Webhook handler (signature, parsing, event processing) |
| `src/config.rs` | 412 | Environment-based configuration |
| `src/error.rs` | 25 | Unified error type |
| `Cargo.toml` | 97 | Dependencies, edition, rust-version |

---

## Task 1: Remove Dead Code & Update Cargo.toml

**Why:** `http_client` field has `#[allow(dead_code)]` — violates coding rules. Edition is 2021, should match workspace's 2024.

**Files:**
- Modify: `Cargo.toml:1-5`
- Modify: `src/broker.rs:15-16`

**Step 1: Update Cargo.toml**

```toml
edition = "2024"
rust-version = "1.93"
```

**Step 2: Remove `http_client` field from MessageBroker**

In `src/broker.rs`, remove the field and its two constructors' initialization:

```rust
// DELETE this field:
#[allow(dead_code)]
http_client: Client,

// DELETE from new() constructor:
let http_client = Client::builder()
    .timeout(std::time::Duration::from_secs(30))
    .build()
    .expect("Failed to create HTTP client");
// And the field assignment:
http_client,

// DELETE from with_storage() constructor (same 5 lines)
```

Also remove `use reqwest::Client;` from imports if no longer used elsewhere in the file.

**Step 3: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

Expected: No errors, no warnings about unused imports.

**Step 4: Commit**

```bash
git add ugent-line-proxy/Cargo.toml ugent-line-proxy/src/broker.rs
git commit -m "chore: remove dead http_client field, bump edition to 2024"
```

---

## Task 2: Add `markAsReadToken` to Message Types

**Why:** The 2025 LINE Messaging API added `markAsReadToken` to message events. The proxy needs to capture this so it can call the mark-as-read endpoint after processing.

**Files:**
- Modify: `src/types.rs:258-271` (TextMessage)
- Modify: `src/types.rs:285-295` (ImageMessage)
- Modify: `src/types.rs:300-310` (AudioMessage)
- Modify: `src/types.rs:315-325` (VideoMessage)
- Modify: `src/types.rs:328-335` (FileMessage)
- Modify: `src/types.rs:339-350` (StickerMessage)

**Step 1: Add `mark_as_read_token` field to all LineMessage variants**

Each message struct gets a new optional field:

```rust
/// Read token for mark-as-read API (2025)
#[serde(default, rename = "markAsReadToken")]
pub mark_as_read_token: Option<String>,
```

Add this to: `TextMessage`, `ImageMessage`, `AudioMessage`, `VideoMessage`, `FileMessage`, `StickerMessage`.

**Step 2: Update `LineMessage::id()` and add `mark_as_read_token()` helper**

In the `impl LineMessage` block, add:

```rust
/// Get mark-as-read token if available
pub fn mark_as_read_token(&self) -> Option<String> {
    match self {
        LineMessage::Text(m) => m.mark_as_read_token.clone(),
        LineMessage::Image(m) => m.mark_as_read_token.clone(),
        LineMessage::Audio(m) => m.mark_as_read_token.clone(),
        LineMessage::Video(m) => m.mark_as_read_token.clone(),
        LineMessage::File(m) => m.mark_as_read_token.clone(),
        LineMessage::Sticker(m) => m.mark_as_read_token.clone(),
        LineMessage::Location(m) => None, // Location has no markAsReadToken
    }
}
```

**Step 3: Update `ProxyMessage` to carry mark_as_read_token**

In the `ProxyMessage` struct (around line 1080), add:

```rust
/// Mark-as-read token for LINE API
#[serde(default, skip_serializing_if = "Option::is_none")]
pub mark_as_read_token: Option<String>,
```

And update `ProxyMessage::from_line_event()` to populate it from the message's `mark_as_read_token()`.

**Step 4: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

**Step 5: Commit**

```bash
git add ugent-line-proxy/src/types.rs
git commit -m "feat: add markAsReadToken to LINE message types and ProxyMessage"
```

---

## Task 3: Implement `mark_as_read()` API Method

**Why:** The proxy has direct LINE API access. After the worker responds, the proxy should mark the user's message as read so the LINE UI shows the correct read state.

**Files:**
- Modify: `src/line_api.rs` (add new method)

**Step 1: Add `mark_as_read()` to LineApiClient**

Insert after `start_loading()` method (around line 385):

```rust
/// Mark messages as read using the mark-as-read token.
///
/// This marks all messages prior to the one with the given token as read.
/// Read tokens have no expiration date.
///
/// API endpoint: POST /v2/bot/chat/markAsRead
pub async fn mark_as_read(&self, mark_as_read_token: &str) -> Result<(), LineApiError> {
    let url = format!("{}/chat/markAsRead", API_BASE);
    let body = json!({
        "markAsReadToken": mark_as_read_token
    });

    debug!("Marking messages as read with token: {}...", &mark_as_read_token[..8.min(mark_as_read_token.len())]);

    let response = self
        .client
        .post(&url)
        .header("Authorization", format!("Bearer {}", self.access_token))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await?;

    let status = response.status();
    if status.is_success() {
        info!("Messages marked as read");
        Ok(())
    } else {
        let error_text = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        error!("Failed to mark as read: {} - {}", status, error_text);
        Err(LineApiError::ApiError(status.as_u16(), error_text))
    }
}
```

**Step 2: Add unit test**

In `src/line_api.rs` test module, add:

```rust
#[test]
fn test_mark_as_read_api_url() {
    // Verify the URL is correct
    let url = format!("{}/chat/markAsRead", "https://api.line.me/v2/bot");
    assert_eq!(url, "https://api.line.me/v2/bot/chat/markAsRead");
}
```

**Step 3: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

**Step 4: Commit**

```bash
git add ugent-line-proxy/src/line_api.rs
git commit -m "feat: implement mark_as_read() LINE API method"
```

---

## Task 4: Target ResponseResult to Specific Client (Not Broadcast)

**Why:** Currently `send_response_result()` broadcasts to ALL connected clients. This means non-responding clients receive ResponseResult frames they didn't request. In multi-instance setups this causes confusion. The fix: track which client sent the response and send ResponseResult only to that client.

**Files:**
- Modify: `src/broker.rs:357` (send_response_result)
- Modify: `src/ws_manager.rs:560-600` (handle_socket Response arm)

**Step 1: Add `client_id` parameter to `send_response_result()`**

In `src/broker.rs`, change the method signature:

```rust
/// Send ResponseResult to a specific client
async fn send_response_result(
    &self,
    request_id: Option<String>,
    original_id: String,
    success: bool,
    error: Option<String>,
    target_client_id: Option<&str>,
) -> Result<(), BrokerError> {
    let result = WsProtocol::ResponseResult {
        request_id,
        original_id,
        success,
        error,
    };

    if let Some(client_id) = target_client_id {
        // Send to the specific client that responded
        match self.ws_manager.send_to(client_id, result).await {
            Ok(()) => {
                debug!("ResponseResult sent to client {}", client_id);
                Ok(())
            }
            Err(e) => {
                warn!("Failed to send ResponseResult to client {}: {}", client_id, e);
                Ok(()) // Don't fail the whole operation for this
            }
        }
    } else {
        // Fallback: broadcast (shouldn't happen in normal flow)
        self.ws_manager.broadcast(result).await
    }
}
```

**Step 2: Thread `client_id` through `handle_response()`**

Update `handle_response()` to accept and pass `client_id`:

```rust
pub async fn handle_response(
    &self,
    request_id: Option<String>,
    original_id: String,
    content: String,
    artifacts: Vec<OutboundArtifact>,
    responding_client_id: Option<String>,
) -> Result<(), BrokerError> {
```

Then update all `send_response_result` calls within the method to pass `responding_client_id.as_deref()`:

```rust
self.send_response_result(
    Some(req_id.clone()),
    original_id,
    true,
    None,
    responding_client_id.as_deref(),
)
.await?;
```

**Step 3: Pass client_id from WebSocket handler**

In `src/ws_manager.rs`, in the `WsProtocol::Response` arm (around line 560), pass `client_id` to `broker.handle_response()`:

```rust
if let Err(e) = broker
    .handle_response(
        request_id,
        original_id,
        content,
        artifacts,
        Some(client_id_str.to_string()), // <-- Add this
    )
    .await
```

**Step 4: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

**Step 5: Commit**

```bash
git add ugent-line-proxy/src/broker.rs ugent-line-proxy/src/ws_manager.rs
git commit -m "fix: send ResponseResult to responding client instead of broadcasting"
```

---

## Task 5: Trigger Typing Indicator on Inbound Messages

**Why:** The proxy has `start_loading()` but never calls it. When a user sends a message, they see no feedback until the bot responds. The typing indicator makes the experience feel responsive.

**Files:**
- Modify: `src/webhook/mod.rs:100-115` (process_event, Message arm)
- Modify: `src/config.rs` (add env var for enable/disable)

**Step 1: Add config toggle**

In `src/config.rs`, add to `LineConfig`:

```rust
/// Send typing indicator when message received
#[serde(default = "default_true")]
pub auto_loading_indicator: bool,
```

And in `from_env()`:

```rust
let auto_loading_indicator = std::env::var("LINE_AUTO_LOADING_INDICATOR")
    .map(|v| v != "false" && v != "0")
    .unwrap_or(true);
```

**Step 2: Call `start_loading()` in webhook handler**

In `src/webhook/mod.rs`, in the `Event::Message` arm:

```rust
Event::Message(msg_event) => {
    let proxy_msg = crate::types::ProxyMessage::from_line_event(msg_event, destination);
    let conversation_id = proxy_msg.conversation_id.clone();

    info!(
        "Routing message: channel={}, conversation={}, sender={}",
        proxy_msg.channel, proxy_msg.conversation_id, proxy_msg.sender_id
    );

    // Trigger typing indicator if configured
    if broker.config.line.auto_loading_indicator {
        if let Err(e) = broker.line_client().start_loading(&conversation_id).await {
            // Non-fatal: don't block message routing if indicator fails
            debug!("Failed to start loading indicator: {}", e);
        }
    }

    if let Err(e) = broker.send_to_clients(proxy_msg).await {
        error!("Failed to route message: {}", e);
    }
}
```

**Step 3: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

**Step 4: Commit**

```bash
git add ugent-line-proxy/src/webhook/mod.rs ugent-line-proxy/src/config.rs
git commit -m "feat: send typing indicator on inbound messages (configurable)"
```

---

## Task 6: Auto Mark-as-Read After Response

**Why:** After the proxy sends a response to LINE, it should also mark the user's original message as read. The mark_as_read_token comes from the pending message's proxy message.

**Files:**
- Modify: `src/broker.rs` (handle_response, after send_line_messages)
- Modify: `src/types.rs` (PendingMessage — carry mark_as_read_token)

**Step 1: Add `mark_as_read_token` to PendingMessage**

In `src/types.rs`, in the `PendingMessage` struct:

```rust
/// Mark-as-read token (no expiration)
pub mark_as_read_token: Option<String>,
```

Update `from_proxy_message()`:

```rust
mark_as_read_token: msg.mark_as_read_token.clone(),
```

**Step 2: Call mark_as_read after successful response**

In `src/broker.rs`, in `handle_response()`, after `send_line_messages()` succeeds and before sending ResponseResult:

```rust
let send_result = self.send_line_messages(&pending, messages).await;

// Mark messages as read after successful send
if send_result.is_ok() {
    if let Some(ref token) = pending.mark_as_read_token {
        if let Err(e) = self.line_client.mark_as_read(token).await {
            debug!("Failed to mark as read (non-fatal): {}", e);
        }
    }
}
```

**Step 3: Add config toggle**

In `src/config.rs`, add to `LineConfig`:

```rust
/// Auto mark messages as read after responding
#[serde(default = "default_true")]
pub auto_mark_as_read: bool,
```

And in `from_env()`:

```rust
let auto_mark_as_read = std::env::var("LINE_AUTO_MARK_AS_READ")
    .map(|v| v != "false" && v != "0")
    .unwrap_or(true);
```

Wrap the mark_as_read call with this config check.

**Step 4: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

**Step 5: Commit**

```bash
git add ugent-line-proxy/src/broker.rs ugent-line-proxy/src/types.rs ugent-line-proxy/src/config.rs
git commit -m "feat: auto mark-as-read after sending response (configurable)"
```

---

## Task 7: Fix FollowEvent Structure (add `isUnblocked`)

**Why:** LINE's FollowEvent contains a nested `follow: { isUnblocked: bool }` field. The proxy has a flat structure that will fail to deserialize this field. The worker plugin already fixed this (issue L1 in the API audit).

**Files:**
- Modify: `src/types.rs:547-558` (FollowEvent)

**Step 1: Add FollowDetail struct and update FollowEvent**

```rust
/// Follow detail (nested in FollowEvent)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FollowDetail {
    /// Whether the follow event is due to unblocking
    #[serde(default)]
    pub is_unblocked: bool,
}

/// Follow event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FollowEvent {
    pub source: Source,
    pub timestamp: i64,
    pub mode: WebhookMode,
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Follow detail (contains isUnblocked)
    #[serde(default)]
    pub follow: Option<FollowDetail>,
}
```

**Step 2: Add deserialization test**

```rust
#[test]
fn test_follow_event_with_is_unblocked() {
    let json = r#"{
        "type": "follow",
        "source": { "type": "user", "userId": "U123" },
        "timestamp": 1692251666727,
        "mode": "active",
        "webhookEventId": "01H810YECXQQZ37VAXPF6H9E6T",
        "deliveryContext": { "isRedelivery": false },
        "replyToken": "abc123",
        "follow": { "isUnblocked": true }
    }"#;
    // Parse through WebhookEvent to test full deserialization
    let wrapper = serde_json::json!({
        "destination": "D123",
        "events": [serde_json::from_str::<serde_json::Value>(json).unwrap()]
    });
    let event: WebhookEvent = serde_json::from_value(wrapper).expect("Failed to parse");
    if let Event::Follow(f) = &event.events[0] {
        assert!(f.follow.is_some());
        assert!(f.follow.as_ref().unwrap().is_unblocked);
    } else {
        panic!("Expected follow event");
    }
}

#[test]
fn test_follow_event_without_follow_detail() {
    // Backward compatibility: follow field is optional
    let json = r#"{
        "destination": "D123",
        "events": [{
            "type": "follow",
            "source": { "type": "user", "userId": "U123" },
            "timestamp": 1692251666727,
            "mode": "active",
            "webhookEventId": "01H810YECXQQZ37VAXPF6H9E6T",
            "deliveryContext": { "isRedelivery": false },
            "replyToken": "abc123"
        }]
    }"#;
    let event: WebhookEvent = serde_json::from_str(json).expect("Failed to parse");
    if let Event::Follow(f) = &event.events[0] {
        assert!(f.follow.is_none());
    }
}
```

**Step 3: Run cargo check + test**

```bash
cd ugent-line-proxy && cargo check && cargo test --lib types::tests::test_follow 2>&1
```

**Step 4: Commit**

```bash
git add ugent-line-proxy/src/types.rs
git commit -m "fix: add FollowDetail struct with isUnblocked to FollowEvent"
```

---

## Task 8: Add `quote_token` to StickerMessage

**Why:** LINE API includes quoteToken on sticker messages in groups/rooms. The proxy's `StickerMessage` struct is missing this field, which could cause silent deserialization data loss.

**Files:**
- Modify: `src/types.rs:339-350` (StickerMessage)

**Step 1: Add field**

```rust
/// Quote token (for quote messages in groups)
#[serde(default, rename = "quoteToken")]
pub quote_token: Option<String>,
```

**Step 2: Update `LineMessage::quote_token()` to include Sticker**

```rust
pub fn quote_token(&self) -> Option<String> {
    match self {
        LineMessage::Text(m) => m.quote_token.clone(),
        LineMessage::Image(m) => m.quote_token.clone(),
        LineMessage::Video(m) => m.quote_token.clone(),
        LineMessage::Sticker(m) => m.quote_token.clone(), // <-- Add
        _ => None,
    }
}
```

**Step 3: Run cargo check + test**

```bash
cd ugent-line-proxy && cargo check && cargo test --lib 2>&1
```

**Step 4: Commit**

```bash
git add ugent-line-proxy/src/types.rs
git commit -m "fix: add quote_token to StickerMessage"
```

---

## Task 9: Add `mentionee_type` to Mentionee

**Why:** LINE API added `mentioneeType` field to mentionees. Missing it means we lose data about whether the mention is a user, all, or group.

**Files:**
- Modify: `src/types.rs:273-283` (Mentionee)

**Step 1: Add field**

```rust
/// Mentionee type (user/all)
#[serde(default, rename = "mentioneeType")]
pub mentionee_type: Option<String>,
```

**Step 2: Run cargo check**

```bash
cd ugent-line-proxy && cargo check 2>&1
```

**Step 3: Commit**

```bash
git add ugent-line-proxy/src/types.rs
git commit -m "feat: add mentioneeType to Mentionee struct"
```

---

## Task 10: Quality Gate — Format, Clippy, Test, Build

**Why:** All tasks must pass the strict quality requirements: zero warnings, no dead_code, no unsafe.

**Step 1: cargo fmt**

```bash
cd ugent-line-proxy && cargo fmt --all -- --check 2>&1
```

If any files are unformatted:
```bash
cd ugent-line-proxy && cargo fmt --all
```

**Step 2: cargo clippy**

```bash
cd ugent-line-proxy && cargo clippy --all-targets --all-features -- -D warnings -D clippy::unwrap_used -D clippy::expect_used 2>&1
```

Fix any warnings. Common issues:
- Unnecessary `mut` bindings
- Clone when reference suffices
- Redundant closures

**Step 3: cargo test**

```bash
cd ugent-line-proxy && cargo test --all 2>&1
```

All tests must pass.

**Step 4: cargo build (release)**

```bash
cd ugent-line-proxy && cargo build --release 2>&1
```

No warnings allowed.

**Step 5: Commit any fixes**

```bash
git add -A ugent-line-proxy/src/
git commit -m "chore: quality gate — fmt, clippy, test, build pass clean"
```

---

## Out of Scope (Future Work)

These items are noted but intentionally deferred:

| Item | Reason |
|------|--------|
| Narrowcast API | Not needed for core messaging flow |
| Multicast API | Not needed for core messaging flow |
| Flex Messages / Imagemap | Rich message types — separate feature effort |
| TextV2 (substitution) | Worker-specific feature, proxy just forwards |
| Module channel events (Activated/Deactivated) | Edge case, not in current webhook flow |
| Membership events | New 2025 event type — low priority |
| Artifact staging endpoint | Phase 2 capability |
| Reply token cache TTL configurable | Current 55s is safe |
| Impression measurement API | Analytics, not messaging |

---

## Environment Variables Added

| Variable | Default | Purpose |
|----------|---------|---------|
| `LINE_AUTO_LOADING_INDICATOR` | `true` | Send typing indicator on inbound |
| `LINE_AUTO_MARK_AS_READ` | `true` | Auto mark-as-read after response |

---

## Risk Assessment

| Risk | Mitigation |
|------|------------|
| Edition 2024 requires rust 1.93 | Document in README, CI check |
| `markAsReadToken` field missing in old webhooks | `Option<String>` with `#[serde(default)]` — backward compatible |
| `FollowDetail` field missing in old webhooks | `Option<FollowDetail>` with `#[serde(default)]` — backward compatible |
| Targeted ResponseResult — client disconnects | Fallback to broadcast, log warning |
| Typing indicator API failure | Non-fatal, debug-level log only |
