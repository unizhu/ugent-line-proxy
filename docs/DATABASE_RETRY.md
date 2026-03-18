# Database & Retry System

This document covers the data retention, message persistence, and retry system in `ugent-line-proxy`.

## Overview

The database and retry system provides:

1. **Message Persistence** — Store all inbound/outbound messages for audit and analytics
2. **Contact & Group Storage** — Cache LINE contact and group profiles locally
3. **Inbound Retry** — Queue messages when no client is connected, deliver on reconnect
4. **Outbound Retry** — Retry failed LINE API calls with exponential backoff
5. **Data Retention** — Automatic cleanup of old data

## Database Backend

### Supported Backends

| Backend | Feature Flag | Dependencies | Best For |
|---------|-------------|--------------|----------|
| **SQLite** | `sqlite` (default) | `rusqlite` (bundled) | Single-instance, simple deployments |
| **PostgreSQL** | `postgres` | `sqlx` | Multi-instance, high-availability |

### Configuration

```bash
# SQLite (default, zero config)
LINE_PROXY_DB_TYPE=sqlite

# PostgreSQL
LINE_PROXY_DB_TYPE=postgres
LINE_PROXY_DB_URL=postgresql://user:pass@host:5432/line_proxy
LINE_PROXY_DB_MAX_CONNECTIONS=5
```

### Database Architecture

Both backends implement the `DatabaseBackend` trait:

```rust
pub trait DatabaseBackend: Send + Sync {
    // Messages
    fn store_message(&self, msg: &StoredMessage) -> Result<i64>;
    fn get_messages(&self, filter: &MessageFilter) -> Result<Vec<StoredMessage>>;
    fn get_message_by_webhook_id(&self, webhook_event_id: &str) -> Result<Option<StoredMessage>>;

    // Contacts
    fn store_contact(&self, contact: &StoredContact) -> Result<()>;
    fn get_contact(&self, user_id: &str) -> Result<Option<StoredContact>>;

    // Groups
    fn store_group(&self, group: &StoredGroup) -> Result<()>;
    fn get_group(&self, group_id: &str) -> Result<Option<StoredGroup>>;

    // Queues
    fn enqueue_inbound(&self, msg: &InboundQueueEntry) -> Result<i64>;
    fn get_pending_inbound(&self) -> Result<Vec<InboundQueueEntry>>;
    fn remove_inbound(&self, id: i64) -> Result<()>;

    fn enqueue_outbound(&self, msg: &OutboundQueueEntry) -> Result<i64>;
    fn get_pending_outbound(&self) -> Result<Vec<OutboundQueueEntry>>;
    fn remove_outbound(&self, id: i64) -> Result<()>;

    // Retention
    fn cleanup_old_messages(&self, max_age_days: u32) -> Result<u64>;
    fn get_metrics(&self) -> Result<DbMetrics>;
}
```

### SQLite Implementation

- Uses `rusqlite` with bundled SQLite (no system dependency)
- Access behind `parking_lot::Mutex` for thread safety
- WAL mode enabled for concurrent read performance
- Database file location: `~/.ugent/line-plugin/line-proxy.db`

### PostgreSQL Implementation

- Uses `sqlx` for native async PostgreSQL
- Connection pooling with configurable size
- Requires `LINE_PROXY_DB_URL` environment variable
- Schema migrations run automatically on startup

## Schema

### Messages Table

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER/BIGSERIAL | Primary key |
| `message_type` | TEXT | `inbound` or `outbound` |
| `conversation_id` | TEXT | LINE user/group/room ID |
| `sender_id` | TEXT | Message sender LINE ID |
| `source_type` | TEXT | `user`, `group`, or `room` |
| `content_type` | TEXT | `text`, `image`, `audio`, `video`, etc. |
| `content` | TEXT | Message content/preview |
| `reply_token` | TEXT | LINE reply token (if available) |
| `webhook_event_id` | TEXT | LINE webhook event ID (dedup) |
| `status` | TEXT | `pending`, `delivered`, `failed` |
| `error_message` | TEXT | Error details if failed |
| `created_at` | TEXT/DATETIME | Timestamp |

### Contacts Table

| Column | Type | Description |
|--------|------|-------------|
| `user_id` | TEXT (PK) | LINE user ID |
| `display_name` | TEXT | Display name |
| `picture_url` | TEXT | Profile picture URL |
| `status_message` | TEXT | Status message |
| `last_seen_at` | TEXT/DATETIME | Last interaction timestamp |

### Groups Table

| Column | Type | Description |
|--------|------|-------------|
| `group_id` | TEXT (PK) | LINE group ID |
| `group_name` | TEXT | Group name |
| `picture_url` | TEXT | Group icon URL |
| `member_count` | INTEGER | Number of members |
| `last_seen_at` | TEXT/DATETIME | Last interaction timestamp |

### Inbound Queue Table

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER/BIGSERIAL | Primary key |
| `message_id` | INTEGER | FK to messages table |
| `conversation_id` | TEXT | Target conversation |
| `retry_count` | INTEGER | Current retry count |
| `max_retries` | INTEGER | Maximum retries |
| `next_retry_at` | TEXT/DATETIME | Next retry timestamp |
| `status` | TEXT | `pending`, `processing`, `failed` |
| `created_at` | TEXT/DATETIME | Enqueue timestamp |

### Outbound Queue Table

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER/BIGSERIAL | Primary key |
| `message_id` | INTEGER | FK to messages table |
| `conversation_id` | TEXT | Target conversation |
| `reply_token` | TEXT | LINE reply token |
| `content` | TEXT | Message content |
| `retry_count` | INTEGER | Current retry count |
| `max_retries` | INTEGER | Maximum retries |
| `next_retry_at` | TEXT/DATETIME | Next retry timestamp |
| `error_message` | TEXT | Last error message |
| `status` | TEXT | `pending`, `processing`, `failed`, `dead_letter` |
| `created_at` | TEXT/DATETIME | Enqueue timestamp |

## Retry System

### Inbound Retry

When a LINE message arrives but no UGENT client is connected:

```
1. Message received → stored in messages table
2. No connected client → enqueue in inbound_queue
3. Client connects → retry worker delivers pending messages
4. On failure → increment retry_count, apply backoff
5. On max retries → mark as failed
```

**Configuration:**

| Variable | Default | Description |
|----------|---------|-------------|
| `LINE_PROXY_RETRY_ENABLED` | `false` | Enable retry system |
| `LINE_PROXY_RETRY_MAX_ATTEMPTS` | `5` | Max delivery attempts |
| `LINE_PROXY_RETRY_INITIAL_DELAY_SECS` | `1` | First backoff delay |
| `LINE_PROXY_RETRY_MAX_DELAY_SECS` | `300` | Max backoff delay |

### Outbound Retry

When sending a response to LINE fails:

```
1. UGENT sends response → stored in messages table
2. LINE API call fails → enqueue in outbound_queue
3. Retry worker applies exponential backoff
4. On success → remove from queue
5. On max retries → move to dead letter
```

**Retryable Errors:**
- Network timeouts
- 429 Too Many Requests (rate limit)
- 500/502/503/504 server errors

**Non-Retryable Errors:**
- 400 Bad Request
- 401 Unauthorized (token expired)
- 404 Not Found

### Exponential Backoff Formula

```
delay = min(initial_delay * 2^(attempt-1) + jitter, max_delay)
```

Where `jitter` is a random value in `[0, delay * 0.1]` to prevent thundering herd.

### Example Backoff Sequence

With `initial_delay=1s`, `max_delay=300s`:

| Attempt | Base Delay | With Jitter |
|---------|-----------|-------------|
| 1 | 1s | ~1.0s |
| 2 | 2s | ~2.1s |
| 3 | 4s | ~4.2s |
| 4 | 8s | ~8.5s |
| 5 | 16s | ~16.8s |
| ... | ... | ... |
| 9 | 256s | ~262s |
| 10 | 300s (capped) | ~305s |

## Data Retention

### Configuration

```bash
LINE_PROXY_RETENTION_ENABLED=true
LINE_PROXY_RETENTION_MAX_AGE_DAYS=90
LINE_PROXY_RETENTION_CLEANUP_INTERVAL_SECS=3600
```

### Cleanup Process

1. Background task runs at configured interval
2. Deletes messages older than `MAX_AGE_DAYS`
3. Cleans up associated queue entries
4. Updates storage metrics
5. Logs number of records cleaned

## Metrics

### Database Metrics

```json
{
  "total_messages": 12345,
  "inbound_messages": 8900,
  "outbound_messages": 3445,
  "total_contacts": 456,
  "total_groups": 78,
  "pending_inbound": 3,
  "pending_outbound": 1,
  "failed_outbound": 0
}
```

### Monitoring

Metrics are available via:
- Structured logs (JSON format)
- Database `get_metrics()` API
- Storage metrics module

## Feature Flags

```toml
[features]
default = ["sqlite"]
sqlite = ["dep:rusqlite", "dep:rusqlite bundled"]
postgres = ["dep:sqlx"]
```

## Build Examples

```bash
# SQLite only (default)
cargo build --release

# With PostgreSQL support
cargo build --release --features postgres

# With both backends
cargo build --release --all-features

# Run tests with all features
cargo test --all-features
```

## Performance Notes

### SQLite

- WAL mode provides good concurrent read performance
- Write transactions are serialized (single writer lock)
- Suitable for up to ~1000 messages/second
- Database file can be backed up with standard file tools

### PostgreSQL

- True concurrent access with MVCC
- Connection pooling for efficient resource usage
- Suitable for high-volume deployments
- Use `pg_dump` for backups
