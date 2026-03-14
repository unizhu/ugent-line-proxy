# UGENT-LINE-PROXY Bugfix Plan

Author: UGENT Agent  
Date: 2026-03-11  
Status: Draft for Review  
Priority: P0 - Critical for Multi-Instance Deployment

---

## Executive Summary

This document outlines the critical bugfixes required for `ugent-line-proxy` to properly support multiple UGENT instances. The current implementation **broadcasts all inbound messages to all connected clients**, which causes duplicate responses and breaks conversation isolation when multiple UGENT instances connect to the same proxy.

---

## Problem Statement

### Current Behavior (BROKEN)

```
┌─────────────────────────────────────────────────────────────┐
│                    ugent-line-proxy                         │
│                                                             │
│  LINE Webhook ──▶ Message ──▶ BROADCAST to ALL clients      │
│                                      │                      │
│                          ┌───────────┼───────────┐          │
│                          ▼           ▼           ▼          │
│                    UGENT-1      UGENT-2      UGENT-3        │
│                    (Home)       (Office)     (Dev)          │
│                          │           │           │          │
│                          ▼           ▼           ▼          │
│                    Responds     Responds     Responds       │
│                    ❌          ❌          ❌                │
│                                                             │
│  RESULT: User receives 3 duplicate replies!                │
└─────────────────────────────────────────────────────────────┘
```

### Expected Behavior (FIXED)

```
┌─────────────────────────────────────────────────────────────┐
│                    ugent-line-proxy                         │
│                                                             │
│  LINE Webhook ──▶ Message ──▶ ROUTE to OWNING client        │
│                                      │                      │
│                                      ▼                      │
│                                 UGENT-1 (Home)              │
│                                 (owns this conversation)    │
│                                      │                      │
│                                      ▼                      │
│                                 Responds                    │
│                                 ✅                          │
│                                                             │
│  RESULT: User receives exactly 1 reply!                    │
└─────────────────────────────────────────────────────────────┘
```

---

## Root Cause Analysis

### Code Evidence

**File: `src/broker.rs:74-107`**
```rust
/// Send a message to all connected UGENT clients and track it
pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError> {
    // ... pending message tracking ...
    
    // Broadcast to all clients  <-- BUG: Should route to specific client
    self.ws_manager.broadcast(ws_msg).await?;
    
    Ok(())
}
```

**File: `src/ws_manager.rs:100-134`**
```rust
/// Broadcast a message to all connected clients
pub async fn broadcast(&self, message: WsProtocol) -> Result<(), BroadcastError> {
    for entry in self.clients.iter() {
        let tx = entry.value().clone();
        if tx.send(message.clone()).await.is_err() {
            failed_count += 1;
        }
    }
    Ok(())
}
```

**File: `src/types.rs:965-997`**
```rust
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            response_result: true,
            artifact_staging: false,
            push_fallback: true,
            targeted_routing: false,  // <-- BUG: Not implemented!
        }
    }
}
```

---

## Bugfix Plan

### Phase 1: Targeted Client Routing (P0 - Critical)

#### 1.1 Add Conversation Ownership Data Structure

**File: `src/types.rs`** (Add new struct)

```rust
/// Conversation ownership binding
#[derive(Debug, Clone)]
pub struct ConversationOwnership {
    /// Conversation ID (LINE user/group/room ID)
    pub conversation_id: String,
    /// Client ID that owns this conversation
    pub client_id: String,
    /// When ownership was claimed
    pub claimed_at: Instant,
    /// Last activity timestamp (for stale detection)
    pub last_activity: Instant,
}
```

#### 1.2 Add Ownership Manager to WebSocketManager

**File: `src/ws_manager.rs`** (Add to WebSocketManager struct)

```rust
pub struct WebSocketManager {
    clients: DashMap<String, mpsc::Sender<WsProtocol>>,
    client_infos: RwLock<HashMap<String, ClientInfo>>,
    client_count: AtomicUsize,
    config: Arc<Config>,
    reply_token_map: RwLock<HashMap<String, String>>,
    
    // NEW: Conversation ownership tracking
    conversation_owners: RwLock<HashMap<String, String>>,  // conversation_id -> client_id
    client_conversations: RwLock<HashMap<String, HashSet<String>>>,  // client_id -> Set<conversation_id>
}
```

#### 1.3 Implement Ownership Claim on First Response

**File: `src/ws_manager.rs`** (Add new methods)

```rust
impl WebSocketManager {
    /// Claim ownership of a conversation for a client
    /// Returns true if claim succeeded, false if already owned by another client
    pub fn claim_conversation(&self, conversation_id: &str, client_id: &str) -> bool {
        let mut owners = self.conversation_owners.write();
        
        if let Some(existing_owner) = owners.get(conversation_id) {
            if existing_owner != client_id {
                // Already owned by another client
                return false;
            }
        }
        
        // Claim or refresh ownership
        owners.insert(conversation_id.to_string(), client_id.to_string());
        
        // Track in client's conversation set
        let mut client_convs = self.client_conversations.write();
        client_convs
            .entry(client_id.to_string())
            .or_insert_with(HashSet::new)
            .insert(conversation_id.to_string());
        
        true
    }
    
    /// Get the client that owns a conversation
    pub fn get_conversation_owner(&self, conversation_id: &str) -> Option<String> {
        self.conversation_owners.read().get(conversation_id).cloned()
    }
    
    /// Release all conversations owned by a client (on disconnect)
    pub fn release_client_conversations(&self, client_id: &str) {
        if let Some(convs) = self.client_conversations.write().remove(client_id) {
            let mut owners = self.conversation_owners.write();
            for conv_id in convs {
                owners.remove(&conv_id);
            }
        }
    }
}
```

#### 1.4 Replace Broadcast with Targeted Routing

**File: `src/broker.rs`** (Modify send_to_clients)

```rust
/// Route a message to the appropriate UGENT client
pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError> {
    let conversation_id = message.conversation_id.clone();
    
    // Check if conversation has an owner
    if let Some(owner_client_id) = self.ws_manager.get_conversation_owner(&conversation_id) {
        // Route to owning client
        let ws_msg = WsProtocol::Message {
            data: Box::new(message.clone()),
        };
        
        match self.ws_manager.send_to(&owner_client_id, ws_msg).await {
            Ok(()) => {
                info!("Routed message to owning client: {}", owner_client_id);
                return Ok(());
            }
            Err(SendError::ClientDisconnected) | Err(SendError::ClientNotFound) => {
                warn!("Owner client disconnected, releasing ownership");
                self.ws_manager.release_client_conversations(&owner_client_id);
                // Fall through to broadcast fallback
            }
        }
    }
    
    // No owner or owner disconnected - broadcast to all (will be claimed on first response)
    let pending = PendingMessage::from_proxy_message(&message);
    {
        let mut pending_map = self.pending_messages.write();
        pending_map.insert(message.id.clone(), pending);
    }
    
    let ws_msg = WsProtocol::Message {
        data: Box::new(message),
    };
    
    // Broadcast for first-response-wins
    self.ws_manager.broadcast(ws_msg).await?;
    
    Ok(())
}
```

#### 1.5 Claim Ownership on Response

**File: `src/ws_manager.rs`** (In handle_socket)

```rust
WsProtocol::Response {
    request_id,
    original_id,
    content,
    artifacts,
} => {
    if !authenticated {
        continue;
    }
    
    let client_id_str = client_id.as_deref().unwrap_or("unknown");
    
    // Get pending message to find conversation_id
    if let Some(broker) = &broker {
        if let Some(pending) = broker.get_pending_message(&original_id) {
            // CLAIM OWNERSHIP on first response
            let claimed = self.ws_manager.claim_conversation(
                &pending.conversation_id,
                client_id_str,
            );
            
            if claimed {
                info!(
                    "Client {} claimed ownership of conversation {}",
                    client_id_str, pending.conversation_id
                );
            } else {
                info!(
                    "Client {} responded to conversation {} (already owned)",
                    client_id_str, pending.conversation_id
                );
            }
        }
        
        // Handle response
        if let Err(e) = broker.handle_response(
            request_id,
            original_id,
            content,
            artifacts,
        ).await {
            error!("Failed to handle response: {}", e);
        }
    }
}
```

#### 1.6 Release Ownership on Client Disconnect

**File: `src/ws_manager.rs`** (In remove_client)

```rust
fn remove_client(&self, client_id: &str) {
    // Release all conversations owned by this client
    self.release_client_conversations(client_id);
    
    // Remove client from maps
    if self.clients.remove(client_id).is_some() {
        self.client_infos.write().remove(client_id);
        self.client_count.fetch_sub(1, Ordering::Relaxed);
        info!("Client disconnected and conversations released: {}", client_id);
    }
}
```

#### 1.7 Update Capabilities Flag

**File: `src/types.rs`**

```rust
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            response_result: true,
            artifact_staging: false,
            push_fallback: true,
            targeted_routing: true,  // CHANGED: Now implemented
        }
    }
}
```

---

### Phase 2: Additional Bugfixes (P1 - Important)

#### 2.1 Add Pending Message Getter

**File: `src/broker.rs`** (Add method)

```rust
/// Get a pending message by original_id (for ownership claiming)
pub fn get_pending_message(&self, original_id: &str) -> Option<PendingMessage> {
    self.pending_messages.read().get(original_id).cloned()
}
```

#### 2.2 Implement SQLite Persistence (Optional but Recommended)

**File: `src/storage/mod.rs`** (New file)

```rust
pub mod sqlite;

pub use sqlite::PendingMessageStore;
```

**File: `src/storage/sqlite.rs`** (New file)

```rust
use rusqlite::{Connection, params};
use std::path::Path;

pub struct PendingMessageStore {
    conn: Connection,
}

impl PendingMessageStore {
    pub fn new(path: &Path) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.execute(
            "CREATE TABLE IF NOT EXISTS pending_messages (
                original_id TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL,
                reply_token TEXT,
                source_type TEXT NOT NULL,
                sender_id TEXT,
                received_at INTEGER NOT NULL,
                reply_token_expires_at INTEGER
            )",
            [],
        )?;
        Ok(Self { conn })
    }
    
    pub fn save(&self, msg: &PendingMessage) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "INSERT OR REPLACE INTO pending_messages 
             (original_id, conversation_id, reply_token, source_type, sender_id, received_at, reply_token_expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                msg.original_id,
                msg.conversation_id,
                msg.reply_token,
                msg.source_type.to_string(),
                msg.sender_id,
                msg.received_at,
                msg.reply_token_expires_at,
            ],
        )?;
        Ok(())
    }
    
    pub fn get(&self, original_id: &str) -> Result<Option<PendingMessage>, rusqlite::Error> {
        // ... implementation
    }
    
    pub fn remove(&self, original_id: &str) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "DELETE FROM pending_messages WHERE original_id = ?1",
            params![original_id],
        )?;
        Ok(())
    }
}
```

#### 2.3 Add Bounded Channels

**File: `src/ws_manager.rs`**

```rust
// Change from unbounded to bounded
let (tx, mut rx): (mpsc::Sender<WsProtocol>, mpsc::Receiver<WsProtocol>) = 
    mpsc::channel(32);  // Already bounded at 32, good!
```

Add overflow handling:

```rust
// In broadcast method
if tx.try_send(message.clone()).is_err() {
    warn!("Client {} queue full, message dropped", client_id);
    failed_count += 1;
}
```

---

### Phase 3: Observability (P2 - Recommended)

#### 3.1 Add Metrics Endpoint

**File: `src/main.rs`**

```rust
use axum::routing::get;

async fn metrics_handler() -> impl IntoResponse {
    // Return Prometheus-style metrics
    format!(
        "# HELP line_proxy_clients Number of connected clients\n\
         # TYPE line_proxy_clients gauge\n\
         line_proxy_clients {}\n\
         # HELP line_proxy_pending Number of pending messages\n\
         # TYPE line_proxy_pending gauge\n\
         line_proxy_pending {}\n\
         # HELP line_proxy_conversations Number of tracked conversations\n\
         # TYPE line_proxy_conversations gauge\n\
         line_proxy_conversations {}\n",
        ws_manager.client_count(),
        broker.pending_count(),
        ws_manager.conversation_count(),
    )
}

// Add route
Router::new()
    .route("/metrics", get(metrics_handler))
    // ... other routes
```

#### 3.2 Add Structured Logging

Already using `tracing` - add more spans:

```rust
#[instrument(skip(self))]
pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError> {
    // ... 
}
```

---

## Testing Plan

### Unit Tests

1. **Test ownership claim**
   - First client claims successfully
   - Second client cannot claim already-owned conversation

2. **Test ownership release**
   - Client disconnect releases all conversations
   - Conversations become available for new claims

3. **Test routing logic**
   - Owned conversation routes to owner
   - Unowned conversation broadcasts

### Integration Tests

1. **Multi-client simulation**
   - Start proxy
   - Connect 2+ mock clients
   - Send webhook for new conversation
   - Verify only one client responds
   - Verify ownership is claimed

2. **Failover test**
   - Owner client disconnects
   - New message arrives
   - Verify re-broadcast and new ownership

---

## Migration Path

### Backward Compatibility

The fix is **backward compatible**:
- Single-client deployments work unchanged
- New `targeted_routing: true` capability is informational
- Clients that don't check capabilities continue to work

### Deployment Steps

1. Deploy updated proxy
2. Reconnect UGENT instances (or let them reconnect automatically)
3. Monitor logs for "claimed ownership" messages
4. Verify no duplicate responses in LINE conversations

---

## Rollback Plan

If issues arise:

1. Set `targeted_routing: false` in capabilities (hotfix)
2. Or revert to previous proxy version
3. Broadcast mode will resume (duplicate responses return)

---

## Acceptance Criteria

- [ ] New conversation routes to exactly one client
- [ ] First response claims ownership
- [ ] Owner disconnect releases all conversations
- [ ] Owner reconnect can reclaim conversations
- [ ] No duplicate LINE responses in multi-client setup
- [ ] Single-client setup continues to work
- [ ] All unit tests pass
- [ ] Integration tests pass
- [ ] `cargo clippy` passes with no warnings
- [ ] `cargo test` passes

---

## Estimated Effort

| Phase | Tasks | Est. Time |
|-------|-------|-----------|
| Phase 1 | Targeted routing implementation | 4-6 hours |
| Phase 2 | SQLite persistence | 2-3 hours |
| Phase 3 | Metrics & observability | 1-2 hours |
| Testing | Unit + integration tests | 2-3 hours |
| **Total** | | **9-14 hours** |

---

## References

- Original design: `ugent/docs/plans/line-proxy-server-design.md`
- Enhancement plan: `ugent/docs/plans/ugent-line-proxy-enhancement-plan.md`
- Current implementation: `ugent-line-proxy/src/`

---

## Sign-off

- [ ] Implementation reviewed
- [ ] Tests passing
- [ ] Documentation updated
- [ ] Deployed to staging
- [ ] Verified in production
