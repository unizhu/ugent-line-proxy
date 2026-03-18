# ugent-line-proxy Architecture

## Overview

`ugent-line-proxy` is a high-performance proxy server that bridges LINE Platform webhooks with local UGENT instances via WebSocket connections. It enables running UGENT behind NAT/firewall while still receiving LINE webhooks on a public VPS.

## Architecture Diagram

```
┌─────────────────┐     HTTPS Webhook      ┌──────────────────────┐     WebSocket      ┌─────────────────┐
│                 │  ──────────────────►   │                      │  ───────────────►  │                 │
│  LINE Platform  │                        │  ugent-line-proxy    │                    │  UGENT (Local)  │
│                 │  ◄──────────────────   │  (Public VPS)        │  ◄───────────────  │                 │
└─────────────────┘    Push Messages       └──────────────────────┘    Responses       └─────────────────┘
                                                  │                    │
                                                  │ LINE API           │ Queue (if offline)
                                                  ▼                    ▼
                                           ┌──────────────┐   ┌──────────────────┐
                                           │  LINE API    │   │  Inbound Queue   │
                                           │  Servers     │   │  (SQLite/PG)      │
                                           └──────────────┘   └──────────────────┘
                                                                      │
                                                   ┌──────────────────┼──────────────────┐
                                                   │                  │                  │
                                            ┌──────▼──────┐   ┌─────▼──────┐   ┌─────▼──────┐
                                            │  Database   │   │   Retry    │   │  Storage    │
                                            │  (db/)      │   │  (retry/)  │   │  (storage/) │
                                            └─────────────┘   └────────────┘   └────────────┘
```

## Core Components

### 1. HTTP Server (Axum)

- **Health Check Endpoint**: `GET /health` - Returns "OK" for monitoring
- **LINE Webhook Endpoint**: `POST /line/callback` - Receives webhooks from LINE
- **WebSocket Endpoint**: `GET /ws` - WebSocket connection for UGENT clients
- **RMS REST API**: `GET/POST /api/rms/*` - Relationship management (when storage enabled)

### 2. Message Broker (`broker.rs`)

The central routing component that:
- Receives messages from LINE webhooks
- Routes messages to connected UGENT clients via WebSocket
- Handles responses from UGENT and sends them back to LINE
- Manages outbound artifacts (files, images, etc.)
- Stores messages in database when persistence is enabled
- Queues messages for retry when clients are offline

### 3. WebSocket Manager (`ws_manager.rs`)

Manages WebSocket connections:
- Client authentication via API key
- Connection lifecycle management
- Message broadcasting to all clients
- Ping/pong heartbeat mechanism
- Reply token tracking

### 4. LINE API Client (`line_api.rs`)

Provides methods for:
- Sending reply messages (using reply token)
- Sending push messages (proactive)
- Downloading media content (image/audio/video)
- Getting user profiles
- Group/room management
- Sending typing indicators (loading indicator)
- Marking messages as read

### 5. Webhook Handler (`webhook/mod.rs`)

Handles incoming LINE webhooks:
- HMAC-SHA256 signature verification
- Webhook event parsing
- Event routing and processing
- Deduplication via webhook event ID

### 6. Database Module (`db/`)

Persistent storage for messages, contacts, and groups.

**Files:**
| File | Description |
|------|-------------|
| `mod.rs` | `DatabaseBackend` trait, unified API |
| `sqlite.rs` | SQLite backend (default, sync via `rusqlite`) |
| `postgres.rs` | PostgreSQL backend (async via `sqlx`, feature flag) |
| `config.rs` | Database configuration from env vars |
| `types.rs` | Shared database types |
| `messages.rs` | Message CRUD operations |
| `contacts.rs` | Contact profile storage |
| `groups.rs` | Group profile storage |
| `metrics.rs` | Database metrics |
| `migration.rs` | Schema migrations (SQLite & PG) |
| `inbound_queue.rs` | Pending inbound messages queue |
| `outbound_queue.rs` | Failed outbound messages queue |
| `error.rs` | Database error types |

**Architecture:**
- `DatabaseBackend` trait provides sync API (behind `parking_lot::Mutex`)
- `SqliteBackend` uses `rusqlite` with bundled SQLite
- `PostgresBackend` uses `sqlx` with native async (behind `postgres` feature)
- Feature flags: `default = ["sqlite"]`, `postgres = ["dep:sqlx"]`

### 7. Retry Module (`retry/`)

Automatic retry with exponential backoff for failed message delivery.

**Files:**
| File | Description |
|------|-------------|
| `mod.rs` | Public exports |
| `inbound.rs` | Retry worker for inbound messages (deliver to offline clients) |
| `outbound.rs` | Retry worker for outbound messages (send to LINE API) |

**Features:**
- Exponential backoff with configurable limits
- Inbound retry: re-deliver queued messages when client reconnects
- Outbound retry: retry failed LINE API calls (network errors, rate limits)
- Configurable max attempts, initial delay, max delay

### 8. Storage Module (`storage/`)

SQLite-based storage for RMS (Relationship Management System).

**Files:**
| File | Description |
|------|-------------|
| `mod.rs` | Storage facade and API |
| `schema.rs` | SQLite schema for RMS tables |
| `pending.rs` | Pending message tracking |
| `ownership.rs` | Entity-client ownership mapping |
| `dedup.rs` | Message deduplication |
| `metrics.rs` | Storage metrics tracking |

### 9. RMS Module (`rms/`)

Relationship Management System for entity-client mapping.

### 10. Configuration (`config.rs`)

All configuration via environment variables with sensible defaults. See `.env.example` for full list.

## Data Flow

### Inbound Message Flow

```
1. LINE Platform sends webhook → POST /line/callback
2. Verify HMAC-SHA256 signature
3. Parse webhook JSON, deduplicate via webhook_event_id
4. Extract message content and metadata
5. Create ProxyMessage
6. Store message in database (if persistence enabled)
7. Send auto loading indicator (if enabled)
8. Broadcast to all connected WebSocket clients
9. Queue in inbound queue if no clients connected (if retry enabled)
10. Return 200 OK to LINE immediately
```

### Outbound Message Flow

```
1. UGENT sends Response via WebSocket
2. Broker receives response with original_id, content, artifacts
3. Split long content into multiple messages (LINE 5000 char limit)
4. Convert artifacts to LINE message format
5. Store outbound message in database (if persistence enabled)
6. Send via reply token (if valid) or push message
7. On failure: queue in outbound queue for retry (if retry enabled)
8. Auto mark message as read (if enabled)
```

### Retry Flow

```
Inbound Retry:
1. Message arrives, no client connected
2. Store in inbound_queue with retry_count=0
3. Retry worker checks queue periodically
4. When client connects, deliver pending messages
5. Increment retry_count, apply backoff on failure
6. Remove from queue after successful delivery or max retries

Outbound Retry:
1. LINE API call fails (network/rate limit)
2. Store in outbound_queue with error info
3. Retry worker applies exponential backoff
4. Retry up to max attempts
5. Move to dead letter after max retries exceeded
```

## Configuration

### Server

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_BIND_ADDR` | Server bind address | `0.0.0.0:3000` |
| `LINE_PROXY_NAME` | Server name | `ugent-line-proxy` |
| `LINE_PROXY_TLS_CERT` | TLS cert path | (none) |
| `LINE_PROXY_TLS_KEY` | TLS key path | (none) |

### LINE

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_CHANNEL_SECRET` | LINE Channel Secret | (required) |
| `LINE_CHANNEL_ACCESS_TOKEN` | LINE Access Token | (required) |
| `LINE_PROXY_WEBHOOK_PATH` | Webhook path | `/line/callback` |
| `LINE_PROXY_SKIP_SIGNATURE` | Skip sig verification (testing) | `false` |
| `LINE_PROXY_PROCESS_REDELIVERIES` | Process redelivered events | `true` |
| `LINE_AUTO_LOADING_INDICATOR` | Auto send typing indicator | `true` |
| `LINE_AUTO_MARK_AS_READ` | Auto mark messages as read | `true` |

### WebSocket

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_API_KEY` | WebSocket auth key | (optional) |
| `LINE_PROXY_WS_PATH` | WebSocket path | `/ws` |
| `LINE_PROXY_WS_PING_INTERVAL` | Ping interval (secs) | `30` |
| `LINE_PROXY_WS_TIMEOUT` | Connection timeout (secs) | `60` |
| `LINE_PROXY_WS_MAX_MESSAGE_SIZE` | Max message size (bytes) | `10MB` |

### Database

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_DB_TYPE` | Database type | `sqlite` |
| `LINE_PROXY_DB_URL` | PostgreSQL URL | (none) |
| `LINE_PROXY_DB_MAX_CONNECTIONS` | PG max connections | `5` |

### Storage (RMS)

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_STORAGE_ENABLED` | Enable persistent storage | `false` |
| `LINE_PROXY_STORAGE_PATH` | Storage file path | `~/.ugent/line-plugin/` |

### Retry

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_RETRY_ENABLED` | Enable retry system | `false` |
| `LINE_PROXY_RETRY_MAX_ATTEMPTS` | Max retry attempts | `5` |
| `LINE_PROXY_RETRY_INITIAL_DELAY_SECS` | Initial backoff delay | `1` |
| `LINE_PROXY_RETRY_MAX_DELAY_SECS` | Maximum backoff delay | `300` |

### Data Retention

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_RETENTION_ENABLED` | Enable data retention cleanup | `false` |
| `LINE_PROXY_RETENTION_MAX_AGE_DAYS` | Max message age (days) | `90` |
| `LINE_PROXY_RETENTION_CLEANUP_INTERVAL_SECS` | Cleanup interval (secs) | `3600` |

### Logging

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_LOG_LEVEL` | Log level | `info` |
| `LINE_PROXY_LOG_FORMAT` | Log format (json/pretty) | `json` |
| `LINE_PROXY_LOG_FILE` | Log file path | (none) |

### Media Cache

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_MEDIA_CACHE_DIR` | Cache directory | `/tmp/ugent-line-proxy-cache/` |
| `LINE_PROXY_MEDIA_CACHE_MAX_MB` | Max cache size (MB) | `500` |
| `LINE_PROXY_MEDIA_CACHE_TTL` | Cache TTL (secs) | `3600` |

## Security

### Signature Verification

All webhooks are verified using HMAC-SHA256:
1. LINE signs request body with channel secret
2. Signature sent in `x-line-signature` header
3. Server verifies signature before processing

### WebSocket Authentication

1. Client connects to `/ws`
2. Client sends `Auth` message with `client_id` and `api_key`
3. Server validates API key
4. Server sends `AuthResult`
5. Only authenticated clients receive messages

## Deployment

### Systemd Service

A systemd service file is provided at `ugent-line-proxy.service`:

```bash
# Install
sudo cp ugent-line-proxy.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable ugent-line-proxy
sudo systemctl start ugent-line-proxy
```

### Docker

```dockerfile
FROM rust:1.93 AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/ugent-line-proxy /usr/local/bin/
CMD ["ugent-line-proxy"]
```

### Feature Flags

| Flag | Default | Description |
|------|---------|-------------|
| `sqlite` | ✅ | SQLite database backend |
| `postgres` | ❌ | PostgreSQL database backend |

```bash
# Build with PostgreSQL support
cargo build --release --features postgres

# Build with both
cargo build --release --all-features
```

## Monitoring

### Health Check

```bash
curl http://localhost:3000/health
# Returns: OK
```

### Logs

Structured JSON logging with fields:
- `client_id`: Connected client identifier
- `conversation_id`: LINE conversation ID
- `sender_id`: Message sender ID
- `event_type`: Webhook event type

## Source Code Layout

```
src/
├── main.rs              # Entry point, server setup
├── lib.rs               # Module declarations, public API
├── broker.rs            # Message routing & orchestration
├── config.rs            # Environment-based configuration
├── error.rs             # Error types
├── types.rs             # Shared types & protocol messages
├── line_api.rs          # LINE Messaging API client
├── ws_manager.rs        # WebSocket connection management
├── bin/
│   └── rms-cli.rs       # RMS CLI binary
├── db/                  # Database abstraction & backends
│   ├── mod.rs           # DatabaseBackend trait
│   ├── sqlite.rs        # SQLite implementation
│   ├── postgres.rs      # PostgreSQL implementation
│   ├── config.rs        # DB configuration
│   ├── types.rs         # DB types
│   ├── messages.rs      # Message persistence
│   ├── contacts.rs      # Contact profiles
│   ├── groups.rs        # Group profiles
│   ├── migration.rs     # Schema migrations
│   ├── inbound_queue.rs # Inbound message queue
│   ├── outbound_queue.rs# Outbound message queue
│   ├── metrics.rs       # DB metrics
│   └── error.rs         # DB errors
├── retry/               # Retry with exponential backoff
│   ├── mod.rs
│   ├── inbound.rs       # Inbound retry worker
│   └── outbound.rs      # Outbound retry worker
├── storage/             # RMS storage (SQLite)
│   ├── mod.rs
│   ├── schema.rs
│   ├── pending.rs
│   ├── ownership.rs
│   ├── dedup.rs
│   └── metrics.rs
├── rms/                 # Relationship Management System
└── webhook/             # Webhook handler & processing
```

## Scalability

- Single proxy can handle multiple UGENT clients
- Messages broadcast to all connected clients
- Horizontal scaling possible with load balancer
- Sticky sessions recommended for WebSocket connections
- Database-backed persistence for reliability
- Retry system ensures message delivery
