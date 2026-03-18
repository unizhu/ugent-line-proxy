# Data Retention & Message Retry Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add persistent data retention (contacts, groups, messages) with SQLite/PostgreSQL support and retry logic for both inbound (LINE→UGENT) and outbound (UGENT→LINE) message delivery.

**Architecture:** Abstract database backend via trait (`DatabaseBackend`) with two implementations: `SqliteBackend` (using `rusqlite` for zero-dep embedded) and `PostgresBackend` (using `sqlx` for async PostgreSQL). The existing `Storage` module is refactored from synchronous `rusqlite` to async `DatabaseBackend` trait. Retry logic uses `backon` crate with exponential backoff for both directions. All data persistence is opt-in via config flags — disabled by default for backward compatibility.

**Tech Stack:** `rusqlite 0.32` (SQLite), `sqlx 0.8` (PostgreSQL), `backon 1.6` (retry), existing `tokio 1.44`/`serde 1`/`chrono 0.4`

---

## Current State

### Existing Storage (`src/storage/`)
- Synchronous `rusqlite` behind `parking_lot::Mutex`
- Tables: `conversation_ownership`, `pending_messages`, `metrics`, `webhook_dedup`
- Config: `StorageConfig { enabled: bool, path: Option<PathBuf> }`
- Env vars: `LINE_PROXY_STORAGE_ENABLED`, `LINE_PROXY_STORAGE_PATH`

### Existing Broker (`src/broker.rs`)
- `send_to_clients()`: routes ProxyMessage to WebSocket client(s) — **no persistence on failure**
- `handle_response()`: sends UGENT response to LINE API — **no retry**
- `send_line_messages()`: reply or push — **no retry**
- PendingMessage tracking: in-memory HashMap + optional DB persist (only for reply token tracking, not message content)

### Existing Types (`src/types.rs`)
- `ProxyMessage`: id, channel, direction, conversation_id, sender_id, message, media, timestamp, reply_token, quote_token, mark_as_read_token, webhook_event_id, source_type
- `LineMessage` enum: Text, Image, Audio, Video, File, Sticker, Location
- `Source`: User, Group, Room — with sender_id extraction
- `UserProfile` (in types.rs:1245): userId, displayName, pictureUrl, statusMessage, language
- `Event` enum: Message, Follow, Unfollow, Join, Leave, MemberJoined, MemberLeft, Beacon, AccountLink, Things, Unsend

### Gaps
1. No contact/group data stored (only IDs, no names/profiles)
2. No message content persisted
3. No message retry (inbound or outbound)
4. Storage is sync-only (blocks async runtime via Mutex)
5. No PostgreSQL option

---

## Database Schema Design

### New Tables (v2 migration)

```sql
-- Contacts (LINE users who have interacted with the bot)
CREATE TABLE contacts (
    line_user_id TEXT PRIMARY KEY,
    display_name TEXT,
    picture_url TEXT,
    status_message TEXT,
    language TEXT,
    first_seen_at INTEGER NOT NULL,      -- Unix timestamp ms
    last_seen_at INTEGER NOT NULL,        -- Unix timestamp ms
    last_interacted_at INTEGER,           -- Unix timestamp ms
    is_blocked INTEGER NOT NULL DEFAULT 0,
    is_friend INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_contacts_last_seen ON contacts(last_seen_at);

-- Groups
CREATE TABLE groups (
    line_group_id TEXT PRIMARY KEY,
    group_name TEXT,
    picture_url TEXT,
    member_count INTEGER,
    first_seen_at INTEGER NOT NULL,
    last_message_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX idx_groups_last_msg ON groups(last_message_at);

-- Group members (join table)
CREATE TABLE group_members (
    line_group_id TEXT NOT NULL REFERENCES groups(line_group_id),
    line_user_id TEXT NOT NULL REFERENCES contacts(line_user_id),
    joined_at INTEGER NOT NULL,
    is_bot INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (line_group_id, line_user_id)
);

-- Rooms (multi-person chat without group)
CREATE TABLE rooms (
    line_room_id TEXT PRIMARY KEY,
    first_seen_at INTEGER NOT NULL,
    last_message_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Messages (all inbound + outbound messages)
CREATE TABLE messages (
    id TEXT PRIMARY KEY,                   -- UUID or LINE message ID
    direction TEXT NOT NULL,               -- 'inbound' or 'outbound'
    conversation_id TEXT NOT NULL,         -- user_id, group_id, or room_id
    source_type TEXT NOT NULL,             -- 'user', 'group', 'room'
    sender_id TEXT,                        -- LINE user_id who sent
    message_type TEXT NOT NULL,            -- 'text', 'image', 'audio', 'video', 'file', 'sticker', 'location'
    text_content TEXT,                     -- Text message content (nullable)
    message_json TEXT,                     -- Full LINE message as JSON
    media_content_json TEXT,               -- MediaContent as JSON (nullable)
    reply_token TEXT,
    quote_token TEXT,
    webhook_event_id TEXT,
    line_timestamp INTEGER,                -- LINE's original timestamp
    received_at INTEGER NOT NULL,          -- Proxy received timestamp
    delivered_at INTEGER,                  -- When delivered to LINE/UGENT
    delivery_status TEXT NOT NULL DEFAULT 'pending', -- pending/delivered/failed/expired
    retry_count INTEGER NOT NULL DEFAULT 0,
    last_retry_at INTEGER,
    error_message TEXT,
    ugent_request_id TEXT,                 -- For outbound: UGENT's request ID
    ugent_correlation_id TEXT,             -- For outbound: correlation ID
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id, received_at DESC);
CREATE INDEX idx_messages_status ON messages(delivery_status);
CREATE INDEX idx_messages_retry ON messages(delivery_status, last_retry_at);
CREATE INDEX idx_messages_direction ON messages(direction, conversation_id);

-- Outbound message queue (retry buffer)
CREATE TABLE outbound_queue (
    id TEXT PRIMARY KEY,
    original_message_id TEXT NOT NULL REFERENCES messages(id),
    conversation_id TEXT NOT NULL,
    send_mode TEXT NOT NULL,               -- 'reply' or 'push'
    reply_token TEXT,
    payload_json TEXT NOT NULL,            -- LINE API payload as JSON
    scheduled_at INTEGER NOT NULL,         -- When to attempt send
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 5,
    last_error TEXT,
    created_at INTEGER NOT NULL,
    locked_at INTEGER,                     -- For claim-based processing
    locked_by TEXT                         -- Worker identifier
);
CREATE INDEX idx_outbound_scheduled ON outbound_queue(scheduled_at, delivery_status)
    WHERE delivery_status != 'delivered';
```

### Inbound Message Queue (for when no client connected)

```sql
CREATE TABLE inbound_queue (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES messages(id),
    conversation_id TEXT NOT NULL,
    priority INTEGER NOT NULL DEFAULT 0,   -- Higher = more important
    created_at INTEGER NOT NULL,
    locked_at INTEGER,
    locked_by TEXT
);
CREATE INDEX idx_inbound_pending ON inbound_queue(created_at)
    WHERE locked_at IS NULL;
```

---

## Config Changes

### New Environment Variables

| Env Var | Default | Description |
|---------|---------|-------------|
| `LINE_PROXY_DB_TYPE` | `sqlite` | Database backend: `sqlite` or `postgres` |
| `LINE_PROXY_DB_URL` | (auto) | PostgreSQL connection string (required if `postgres`) |
| `LINE_PROXY_RETENTION_ENABLED` | `false` | Enable data retention |
| `LINE_PROXY_RETENTION_CONTACTS` | `true` | Store contact data (when retention enabled) |
| `LINE_PROXY_RETENTION_MESSAGES` | `true` | Store message data (when retention enabled) |
| `LINE_PROXY_RETENTION_GROUPS` | `true` | Store group data (when retention enabled) |
| `LINE_PROXY_RETRY_ENABLED` | `true` | Enable retry logic (when retention enabled) |
| `LINE_PROXY_RETRY_MAX_ATTEMPTS` | `5` | Max retry attempts for outbound |
| `LINE_PROXY_RETRY_INITIAL_DELAY_MS` | `1000` | Initial retry delay (ms) |
| `LINE_PROXY_RETRY_MAX_DELAY_MS` | `60000` | Max retry delay (ms) |
| `LINE_PROXY_INBOUND_QUEUE_TTL_SECS` | `3600` | TTL for undelivered inbound messages |

### Config Struct Changes

```rust
// StorageConfig expanded → DataConfig
pub struct DataConfig {
    // Existing
    pub enabled: bool,
    pub path: Option<PathBuf>,

    // New: Database backend
    pub db_type: DbType,
    pub db_url: Option<String>,

    // New: Retention flags
    pub retention: RetentionConfig,
    pub retry: RetryConfig,
}

pub enum DbType { Sqlite, Postgres }

pub struct RetentionConfig {
    pub enabled: bool,
    pub contacts: bool,
    pub messages: bool,
    pub groups: bool,
}

pub struct RetryConfig {
    pub enabled: bool,
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
}
```

---

## Architecture

### Module Structure

```
src/
├── db/                          # NEW: Database layer (replaces src/storage/)
│   ├── mod.rs                   # DatabaseBackend trait, Database struct
│   ├── error.rs                 # Database errors
│   ├── config.rs                # DbType, RetentionConfig, RetryConfig
│   ├── migration.rs             # Schema migrations (v1 legacy + v2 new)
│   ├── sqlite.rs                # SqliteBackend implementation
│   ├── postgres.rs              # PostgresBackend implementation
│   ├── contacts.rs              # Contact repository
│   ├── groups.rs                # Group repository
│   ├── messages.rs              # Message repository
│   ├── outbound_queue.rs        # Outbound retry queue
│   ├── inbound_queue.rs         # Inbound retry queue
│   └── metrics.rs               # Metrics (kept from old storage)
├── retry/                       # NEW: Retry logic
│   ├── mod.rs                   # RetryPolicy, RetryConfig
│   ├── outbound.rs              # Outbound message retry worker
│   └── inbound.rs               # Inbound message retry worker
├── broker.rs                    # MODIFIED: Use db layer, call retry
├── config.rs                    # MODIFIED: Add DataConfig
├── line_api.rs                  # UNCHANGED
├── rms/                         # MODIFIED: Use db layer instead of old storage
├── types.rs                     # MINOR: Add delivery status types
├── webhook/                     # MODIFIED: Use db for dedup
└── ws_manager.rs                # MODIFIED: Notify broker on client connect (for inbound queue drain)
```

### DatabaseBackend Trait

```rust
#[async_trait]
pub trait DatabaseBackend: Send + Sync {
    // Lifecycle
    async fn ping(&self) -> Result<bool, DbError>;
    async fn close(&self);

    // Contacts
    async fn upsert_contact(&self, contact: &ContactRecord) -> Result<(), DbError>;
    async fn get_contact(&self, line_user_id: &str) -> Result<Option<ContactRecord>, DbError>;
    async fn list_contacts(&self, offset: u64, limit: u64) -> Result<Vec<ContactRecord>, DbError>;
    async fn search_contacts(&self, query: &str, limit: u64) -> Result<Vec<ContactRecord>, DbError>;

    // Groups
    async fn upsert_group(&self, group: &GroupRecord) -> Result<(), DbError>;
    async fn get_group(&self, line_group_id: &str) -> Result<Option<GroupRecord>, DbError>;
    async fn add_group_member(&self, group_id: &str, user_id: &str) -> Result<(), DbError>;

    // Messages
    async fn store_message(&self, msg: &MessageRecord) -> Result<(), DbError>;
    async fn get_message(&self, id: &str) -> Result<Option<MessageRecord>, DbError>;
    async fn list_messages(
        &self, conversation_id: &str, direction: Option<&str>,
        offset: u64, limit: u64,
    ) -> Result<Vec<MessageRecord>, DbError>;
    async fn update_delivery_status(
        &self, id: &str, status: DeliveryStatus, error: Option<&str>,
    ) -> Result<(), DbError>;

    // Outbound queue
    async fn enqueue_outbound(&self, entry: &OutboundQueueEntry) -> Result<(), DbError>;
    async fn claim_next_outbound(&self, worker_id: &str, limit: u64) -> Result<Vec<OutboundQueueEntry>, DbError>;
    async fn complete_outbound(&self, id: &str, success: bool, error: Option<&str>) -> Result<(), DbError>;

    // Inbound queue
    async fn enqueue_inbound(&self, entry: &InboundQueueEntry) -> Result<(), DbError>;
    async fn claim_next_inbound(&self, worker_id: &str, limit: u64) -> Result<Vec<InboundQueueEntry>, DbError>;
    async fn complete_inbound(&self, id: &str) -> Result<(), DbError>;

    // Metrics
    async fn record_metric(&self, name: &str, value: i64) -> Result<(), DbError>;
    async fn get_metrics(&self, name: &str, since: i64) -> Result<Vec<MetricRecord>, DbError>;

    // Maintenance
    async fn run_maintenance(&self) -> Result<(), DbError>;
}
```

### Retry Flow

#### Inbound (LINE → UGENT) Retry
```
LINE webhook → broker.send_to_clients()
  ├─ Client connected → send via WebSocket ✅
  └─ No client connected →
       ├─ Store message in `messages` table (status=pending)
       ├─ Enqueue in `inbound_queue`
       └─ Background worker polls `inbound_queue`:
            ├─ Client connected → dequeue + send → update status=delivered
            ├─ TTL expired → update status=expired
            └─ No client → wait and retry
```

#### Outbound (UGENT → LINE) Retry
```
UGENT response → broker.handle_response()
  ├─ LINE API success → status=delivered ✅
  └─ LINE API failure (network, 429, 500, expired reply token) →
       ├─ Store message + enqueue in `outbound_queue`
       ├─ If reply token expired → switch to push mode
       └─ Background worker polls `outbound_queue`:
            ├─ Exponential backoff: 1s, 2s, 4s, 8s, 16s (max 60s)
            ├─ Max 5 retries
            ├─ Success → status=delivered, remove from queue
            └─ Max retries exceeded → status=failed
```

#### Client Reconnect → Drain Inbound Queue
```
ws_manager.on_client_connect(client_id)
  → broker.notify_client_connected(client_id)
    → drain_inbound_queue_for_client(client_id)
      → Claim pending inbound messages
      → Send to newly connected client
      → Update delivery status
```

---

## Task Breakdown

### Task 1: Database Error Types and Config

**Files:**
- Create: `src/db/error.rs`
- Create: `src/db/config.rs`
- Create: `src/db/mod.rs` (skeleton)
- Modify: `src/config.rs` (replace `StorageConfig` with `DataConfig`)
- Modify: `Cargo.toml` (add `sqlx`, `backon`, `async-trait`)

**Step 1:** Add new dependencies to `Cargo.toml`

```toml
# Database
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "chrono", "uuid"], optional = true }
async-trait = "0.1"
backon = "1.6"

[features]
default = ["sqlite"]
sqlite = []
postgres = ["dep:sqlx"]
```

**Step 2:** Create `src/db/error.rs`

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DbError {
    #[error("Database connection error: {0}")]
    Connection(String),
    #[error("Migration error: {0}")]
    Migration(String),
    #[error("Query error: {0}")]
    Query(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("Lock contention timeout")]
    LockTimeout,
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Query(e.to_string())
    }
}

impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        DbError::Query(e.to_string())
    }
}
```

**Step 3:** Create `src/db/config.rs` with `DataConfig`, `DbType`, `RetentionConfig`, `RetryConfig`

**Step 4:** Create `src/db/mod.rs` skeleton with `DatabaseBackend` trait, `Database` struct, `DbType` enum

**Step 5:** Update `src/config.rs`: add `DataConfig`, add env vars, keep backward compat with old `StorageConfig` fields

**Step 6:** Run `cargo check` to verify compilation

**Step 7:** Commit

---

### Task 2: SQLite Backend

**Files:**
- Create: `src/db/sqlite.rs`
- Modify: `src/db/mod.rs` (wire up SqliteBackend)

**Step 1:** Create `src/db/sqlite.rs` — `SqliteBackend` struct using `rusqlite` with `tokio::task::spawn_blocking` wrapper for async compatibility

```rust
pub struct SqliteBackend {
    conn: Arc<Mutex<Connection>>,
}
```

**Step 2:** Implement `DatabaseBackend` trait for `SqliteBackend`:
- Each method wraps sync `rusqlite` calls in `tokio::task::spawn_blocking`
- Reuses existing WAL mode + PRAGMA settings from current `storage/mod.rs`

**Step 3:** Write unit tests: creation, ping, contact upsert, message store

**Step 4:** Run `cargo test --features sqlite`

**Step 5:** Commit

---

### Task 3: PostgreSQL Backend

**Files:**
- Create: `src/db/postgres.rs`
- Modify: `src/db/mod.rs` (wire up PostgresBackend)

**Step 1:** Create `src/db/postgres.rs` — `PostgresBackend` struct using `sqlx::PgPool`

```rust
pub struct PostgresBackend {
    pool: PgPool,
}
```

**Step 2:** Implement `DatabaseBackend` trait for `PostgresBackend`:
- Uses `sqlx::query` and `sqlx::query_as` for all operations
- Connection pool from `sqlx::postgres::PgPoolOptions`

**Step 3:** Write unit tests (requires PostgreSQL or use `sqlx::test` fixture)

**Step 4:** Run `cargo check --features postgres`

**Step 5:** Commit

---

### Task 4: Schema Migrations

**Files:**
- Create: `src/db/migration.rs`
- Modify: `src/db/sqlite.rs` (call migrations)
- Modify: `src/db/postgres.rs` (call migrations)

**Step 1:** Create `src/db/migration.rs` with migration system:
- Track schema version in `schema_version` table
- `migration_v1()`: existing tables (ownership, pending_messages, metrics, webhook_dedup) — port from current `storage/schema.rs`
- `migration_v2()`: new tables (contacts, groups, rooms, group_members, messages, outbound_queue, inbound_queue)
- Handle both SQLite (INTEGER) and PostgreSQL (TIMESTAMP) date differences

**Step 2:** Add migration calls to both `SqliteBackend::new()` and `PostgresBackend::new()`

**Step 3:** Test: create DB from scratch → verify all tables exist
- Test: open existing v1 DB → verify migration to v2 succeeds

**Step 4:** Run `cargo test --features sqlite`

**Step 5:** Commit

---

### Task 5: Contact and Group Repositories

**Files:**
- Create: `src/db/contacts.rs`
- Create: `src/db/groups.rs`
- Modify: `src/db/mod.rs` (re-export)

**Step 1:** Create `src/db/contacts.rs`:
- `ContactRecord` struct (maps to contacts table)
- Methods: `upsert_contact`, `get_contact`, `list_contacts`, `search_contacts`
- Called from webhook handler when message received (fetch + cache profile)

**Step 2:** Create `src/db/groups.rs`:
- `GroupRecord` struct (maps to groups table)
- Methods: `upsert_group`, `get_group`, `add_group_member`
- Called from webhook handler on group events

**Step 3:** Modify `src/webhook/mod.rs`:
- After routing message, if retention.contacts enabled: `line_client.get_profile(sender_id)` → `db.upsert_contact()`
- On Group/Join events: `db.upsert_group()`

**Step 4:** Write tests

**Step 5:** Commit

---

### Task 6: Message Repository

**Files:**
- Create: `src/db/messages.rs`
- Modify: `src/db/mod.rs` (re-export)

**Step 1:** Create `src/db/messages.rs`:
- `MessageRecord` struct (maps to messages table)
- `DeliveryStatus` enum: `Pending`, `Delivered`, `Failed`, `Expired`
- Methods: `store_message`, `get_message`, `list_messages`, `update_delivery_status`

**Step 2:** Modify `src/broker.rs::send_to_clients()`:
- Before sending: if retention.messages enabled → `db.store_message(status=Pending)`
- On successful send: `db.update_delivery_status(Delivered)`
- On send failure: `db.update_delivery_status(Failed)` + enqueue to inbound_queue

**Step 3:** Modify `src/broker.rs::handle_response()`:
- Before sending to LINE: if retention.messages enabled → store outbound message (status=Pending)
- On LINE API success: `db.update_delivery_status(Delivered)`
- On LINE API failure: `db.update_delivery_status(Failed)` + enqueue to outbound_queue

**Step 4:** Write tests

**Step 5:** Commit

---

### Task 7: Outbound Retry Worker

**Files:**
- Create: `src/db/outbound_queue.rs`
- Create: `src/retry/mod.rs`
- Create: `src/retry/outbound.rs`
- Modify: `src/broker.rs` (integrate retry)
- Modify: `src/main.rs` (spawn retry worker)

**Step 1:** Create `src/db/outbound_queue.rs`:
- `OutboundQueueEntry` struct
- Methods: `enqueue_outbound`, `claim_next_outbound`, `complete_outbound`
- Claim uses `locked_at/locked_by` for concurrency (SELECT FOR UPDATE SKIP LOCKED pattern)

**Step 2:** Create `src/retry/mod.rs`:
- `RetryPolicy` using `backon::ExponentialBuilder`
- Configurable: max_attempts, initial_delay, max_delay, jitter

**Step 3:** Create `src/retry/outbound.rs`:
- `OutboundRetryWorker` — background tokio task
- Polls `outbound_queue` every 2 seconds
- Claims next batch (limit 10)
- Attempts LINE API send with backon retry per message
- On reply_token expired: switch to push mode
- On success: complete_outbound + update message status
- On max retries: complete_outbound(failed) + update message status

**Step 4:** Integrate into `src/main.rs`:
- If retry.enabled && retention.enabled: spawn `OutboundRetryWorker`

**Step 5:** Write tests (mock LINE API, test retry logic)

**Step 6:** Commit

---

### Task 8: Inbound Queue & Drain

**Files:**
- Create: `src/db/inbound_queue.rs`
- Create: `src/retry/inbound.rs`
- Modify: `src/ws_manager.rs` (notify on connect)
- Modify: `src/broker.rs` (handle drain)
- Modify: `src/main.rs` (spawn inbound worker)

**Step 1:** Create `src/db/inbound_queue.rs`:
- `InboundQueueEntry` struct
- Methods: `enqueue_inbound`, `claim_next_inbound`, `complete_inbound`

**Step 2:** Modify `src/broker.rs::send_to_clients()`:
- When no client connected AND retention.enabled:
  - `db.store_message(status=Pending)`
  - `db.enqueue_inbound()`
  - Return `Ok(())` (don't error — message is queued)

**Step 3:** Create `src/retry/inbound.rs`:
- `InboundRetryWorker` — background tokio task
- Two triggers:
  a) Periodic poll every 5 seconds (for stale messages)
  b) Event-driven: `notify_client_connected` channel
- Claims pending inbound messages
- If client available: send via WebSocket
- If TTL expired: mark as expired

**Step 4:** Modify `src/ws_manager.rs`:
- After successful client auth/registration: call `broker.notify_client_connected(client_id)`
- This triggers immediate drain of inbound queue for that client

**Step 5:** Modify `src/main.rs`:
- If retry.enabled && retention.enabled: spawn `InboundRetryWorker`
- Pass `notify_rx` channel from ws_manager to inbound worker

**Step 6:** Write tests

**Step 7:** Commit

---

### Task 9: Refactor RMS to Use New DB Layer

**Files:**
- Modify: `src/rms/storage.rs` (replace old Storage usage with new Database)
- Modify: `src/rms/service.rs` (use db.contacts, db.groups)
- Modify: `src/rms/api.rs` (use db for queries)
- Remove/Deprecate: `src/storage/` (old sync storage module)

**Step 1:** Create backward compat bridge in `src/db/mod.rs`:
- Re-export old storage types (`OwnershipStore`, `PendingMessageStore`, etc.)
- Provide thin wrappers that delegate to `DatabaseBackend` if old storage was used

**Step 2:** Modify `src/rms/service.rs`:
- `sync_entity()`: use `db.upsert_contact()` / `db.upsert_group()` instead of old storage
- `get_profile()`: check db cache first, then LINE API

**Step 3:** Modify `src/rms/api.rs`:
- REST endpoints use db layer directly

**Step 4:** Add `#[cfg(deprecated)]` to old `src/storage/` module, keep for migration period

**Step 5:** Run all existing RMS tests to verify no regressions

**Step 6:** Commit

---

### Task 10: Broker Integration & ProxyMessage Enhancement

**Files:**
- Modify: `src/broker.rs` (use db everywhere, add sender name enrichment)
- Modify: `src/types.rs` (add DeliveryStatus, enrich ProxyMessage)

**Step 1:** Add `sender_name` field to `ProxyMessage`:

```rust
pub struct ProxyMessage {
    // ... existing fields ...
    /// Sender display name (resolved from cache or LINE API)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    /// Sender profile picture URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_picture_url: Option<String>,
}
```

**Step 2:** Modify `src/webhook/mod.rs::Event::Message`:
- After creating ProxyMessage, if db available: look up sender name from contacts cache
- If not cached: fire-and-forget `line_client.get_profile()` → update contacts cache (don't block message delivery)

**Step 3:** Modify `src/broker.rs`:
- All `send_to_clients` and `handle_response` use db for persistence
- All failures route to retry queues
- Remove old in-memory-only pending message tracking (db is source of truth)

**Step 4:** Run existing tests + new tests

**Step 5:** Commit

---

### Task 11: Integration Tests

**Files:**
- Create: `tests/db_test.rs`
- Create: `tests/retry_test.rs`
- Create: `tests/e2e_test.rs`

**Step 1:** Create `tests/db_test.rs`:
- Test SQLite backend: CRUD for contacts, groups, messages
- Test schema migration from scratch
- Test concurrent writes (spawn_blocking + Mutex)

**Step 2:** Create `tests/retry_test.rs`:
- Test outbound retry: mock LINE API failure → verify retry → success
- Test inbound queue: no client → enqueue → client connects → drain
- Test max retries → verify failed status
- Test TTL expiration → verify expired status

**Step 3:** Create `tests/e2e_test.rs`:
- End-to-end: LINE webhook → broker → db → WebSocket → client
- End-to-end: client response → broker → db → LINE API (mocked)
- Test full retry cycle with mock disconnect

**Step 4:** Run `cargo test`

**Step 5:** Commit

---

### Task 12: Quality Gate

**Files:** All modified files

**Step 1:** `cargo fmt --all`

**Step 2:** `cargo clippy --all-targets --all-features -- -D warnings`

**Step 3:** `cargo test --all-features`

**Step 4:** `cargo build --release --all-features`

**Step 5:** Verify zero warnings, zero `dead_code`, zero `unsafe`

**Step 6:** Commit

---

## Dependency Summary

| Crate | Version | Feature | Purpose |
|-------|---------|---------|---------|
| `rusqlite` | `0.32` | existing `bundled` | SQLite embedded DB |
| `sqlx` | `0.8` | `runtime-tokio, postgres, chrono, uuid` | PostgreSQL async DB |
| `backon` | `1.6` | default (tokio-sleep) | Retry with exponential backoff |
| `async-trait` | `0.1` | — | Async trait for DatabaseBackend |
| `tokio` | `1.44` | existing | Runtime |

## Backward Compatibility

- `LINE_PROXY_STORAGE_ENABLED=false` (default): no database, no retention, no retry — exact current behavior
- `LINE_PROXY_STORAGE_ENABLED=true` with no `LINE_PROXY_DB_TYPE`: defaults to SQLite at `~/.ugent/line-plugin/line-proxy.db`
- Old `StorageConfig` fields (`enabled`, `path`) remain in `DataConfig` for compat
- All new fields use serde defaults → existing `.env` files continue to work
- Old `src/storage/` module kept behind `#[cfg(feature = "legacy-storage")]` for gradual migration

## Implementation Order

Tasks 1-4 (DB foundation) → Tasks 5-6 (data repos) → Tasks 7-8 (retry) → Task 9 (RMS refactor) → Task 10 (broker integration) → Tasks 11-12 (tests + QA)

Estimated: 12 tasks × 30-45 min each = ~8-10 hours total
