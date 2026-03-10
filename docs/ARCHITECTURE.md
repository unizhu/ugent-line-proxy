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
                                                  │
                                                  │ LINE API
                                                  ▼
                                           ┌──────────────┐
                                           │  LINE API    │
                                           │  Servers     │
                                           └──────────────┘
```

## Core Components

### 1. HTTP Server (Axum)

- **Health Check Endpoint**: `GET /health` - Returns "OK" for monitoring
- **LINE Webhook Endpoint**: `POST /line/callback` - Receives webhooks from LINE
- **WebSocket Endpoint**: `GET /ws` - WebSocket connection for UGENT clients

### 2. Message Broker (`broker.rs`)

The central routing component that:
- Receives messages from LINE webhooks
- Routes messages to connected UGENT clients via WebSocket
- Handles responses from UGENT and sends them back to LINE
- Manages outbound artifacts (files, images, etc.)

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

### 5. Webhook Handler (`webhook/mod.rs`)

Handles incoming LINE webhooks:
- HMAC-SHA256 signature verification
- Webhook event parsing
- Event routing and processing

## Data Flow

### Inbound Message Flow

```
1. LINE Platform sends webhook → POST /line/callback
2. Verify HMAC-SHA256 signature
3. Parse webhook JSON
4. Extract message content and metadata
5. Create ProxyMessage
6. Broadcast to all connected WebSocket clients
7. Return 200 OK to LINE immediately
```

### Outbound Message Flow

```
1. UGENT sends Response via WebSocket
2. Broker receives response with original_id, content, artifacts
3. Split long content into multiple messages (LINE 5000 char limit)
4. Convert artifacts to LINE message format
5. Send via reply token (if valid) or push message
```

## Configuration

| Environment Variable | Description | Default |
|---------------------|-------------|---------|
| `LINE_PROXY_BIND_ADDR` | Server bind address | `0.0.0.0:3000` |
| `LINE_CHANNEL_SECRET` | LINE Channel Secret | (required) |
| `LINE_CHANNEL_ACCESS_TOKEN` | LINE Access Token | (required) |
| `LINE_PROXY_API_KEY` | WebSocket auth key | (optional) |
| `LINE_PROXY_WEBHOOK_PATH` | Webhook path | `/line/callback` |
| `LINE_PROXY_WS_PATH` | WebSocket path | `/ws` |
| `LINE_PROXY_WS_PING_INTERVAL` | Ping interval (secs) | `30` |
| `LINE_PROXY_WS_TIMEOUT` | Connection timeout (secs) | `60` |
| `LINE_PROXY_LOG_LEVEL` | Log level | `info` |
| `LINE_PROXY_LOG_FORMAT` | Log format (json/pretty) | `json` |

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
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/ugent-line-proxy /usr/local/bin/
CMD ["ugent-line-proxy"]
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

## Scalability

- Single proxy can handle multiple UGENT clients
- Messages broadcast to all connected clients
- Horizontal scaling possible with load balancer
- Sticky sessions recommended for WebSocket connections
