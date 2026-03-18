# ugent-line-proxy

LINE Messaging API proxy server for UGENT. This proxy enables running UGENT behind NAT/firewall while still receiving LINE webhooks on a public VPS.

## Architecture

```
LINE Platform → ugent-line-proxy (Public VPS) → WebSocket → UGENT (Local)
```

The proxy handles:
- Receiving LINE webhooks and forwarding to UGENT via WebSocket
- Sending UGENT responses back to LINE (reply → push fallback)
- Managing conversation ownership for targeted client routing
- SQLite-backed persistence for dedup, metrics, and pending messages

## Features

- HMAC-SHA256 signature verification
- WebSocket-based real-time messaging (protocol v2)
- All LINE message types: text, image, audio, video, file, sticker, location
- Group chat and P2P chat support
- @mention detection in groups (including `@all` via `mentioneeType`)
- Typing indicator (loading animation) on inbound messages
- Auto mark-as-read after response delivery
- Targeted ResponseResult routing (ack sent to the responding client, not broadcast)
- Media content download proxy with disk cache
- Outbound artifact (file/image) sending to LINE
- Conversation ownership: first-response-wins for targeted routing
- Event deduplication via `webhookEventId`
- SQLite persistence for pending messages, dedup, metrics, and ownership
- Relationship Management Service (RMS) REST API + CLI
- Configurable via environment variables or TOML config file

## Configuration

### Environment Variables

#### Server

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_PROXY_BIND_ADDR` | No | `0.0.0.0:3000` | Server bind address |
| `LINE_PROXY_NAME` | No | `ugent-line-proxy` | Server display name |
| `LINE_PROXY_LOG_LEVEL` | No | `info` | Log level (trace/debug/info/warn/error) |
| `LINE_PROXY_LOG_FORMAT` | No | `json` | Log format (json/pretty) |

#### LINE API

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_CHANNEL_SECRET` | Yes | - | LINE Channel Secret for signature verification |
| `LINE_CHANNEL_ACCESS_TOKEN` | Yes | - | LINE Channel Access Token for API calls |
| `LINE_PROXY_WEBHOOK_PATH` | No | `/line/callback` | Webhook endpoint path |
| `LINE_PROXY_SKIP_SIGNATURE` | No | `false` | Skip HMAC signature verification (testing only) |
| `LINE_PROXY_PROCESS_REDELIVERIES` | No | `true` | Process redelivered webhook events |
| `LINE_AUTO_LOADING_INDICATOR` | No | `true` | Send typing indicator on inbound messages |
| `LINE_AUTO_MARK_AS_READ` | No | `true` | Mark messages as read after successful response |

#### WebSocket

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_PROXY_API_KEY` | No | - | WebSocket API key for client authentication |
| `LINE_PROXY_WS_PATH` | No | `/ws` | WebSocket endpoint path |
| `LINE_PROXY_WS_PING_INTERVAL` | No | `30` | WebSocket ping interval (seconds) |
| `LINE_PROXY_WS_TIMEOUT` | No | `60` | WebSocket timeout (seconds) |
| `LINE_PROXY_WS_MAX_MESSAGE_SIZE` | No | `16777216` | Max WebSocket message size (bytes) |

#### TLS (Optional)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_PROXY_TLS_CERT` | No | - | Path to TLS certificate file |
| `LINE_PROXY_TLS_KEY` | No | - | Path to TLS private key file |

#### Media Cache

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_PROXY_MEDIA_CACHE_DIR` | No | `$TEMP/ugent-line-proxy-cache` | Directory for cached media files |
| `LINE_PROXY_MEDIA_CACHE_MAX_MB` | No | `500` | Maximum cache size (MB) |
| `LINE_PROXY_MEDIA_CACHE_TTL` | No | `3600` | Cache TTL (seconds) |

### Boolean Environment Variables

Variables `LINE_PROXY_SKIP_SIGNATURE`, `LINE_PROXY_PROCESS_REDELIVERIES`, `LINE_AUTO_LOADING_INDICATOR`, and `LINE_AUTO_MARK_AS_READ` accept `false` or `0` to disable; any other value (or unset) enables them.

## Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/line/callback` | POST | LINE webhook receiver |
| `/ws` | GET | WebSocket endpoint for UGENT clients |

## Building

```bash
cd ugent-line-proxy
cargo build --release
```

## Running

```bash
# Set required environment variables
export LINE_CHANNEL_SECRET=your_secret
export LINE_CHANNEL_ACCESS_TOKEN=your_token

# Optional: protect WebSocket with API key
export LINE_PROXY_API_KEY=your_api_key

# Run the proxy
./target/release/ugent-line-proxy
```

### RMS CLI

The `rms-cli` binary provides relationship management commands:

```bash
# Entity management
./target/release/rms-cli entity list
./target/release/rms-cli entity show <line_id>
./target/release/rms-cli entity refresh <line_id>

# Relationship management
./target/release/rms-cli relationship list
./target/release/rms-cli relationship set <entity_id> <client_id> [--priority N] [--manual]
./target/release/rms-cli relationship remove <entity_id>
./target/release/rms-cli relationship find <client_id>

# Dispatch history
./target/release/rms-cli dispatch history [--conversation CONV_ID] [--limit N]
./target/release/rms-cli dispatch record <conversation_id> <entity_id>

# Maintenance
./target/release/rms-cli maintenance cleanup --days N
./target/release/rms-cli stats
```

## WebSocket Protocol

### Authentication

```json
{
  "type": "auth",
  "data": {
    "client_id": "ugent-client-1",
    "api_key": "your_api_key"
  }
}
```

### Incoming Message (from LINE)

```json
{
  "type": "message",
  "data": {
    "id": "uuid",
    "channel": "line",
    "direction": "inbound",
    "conversation_id": "U1234567890",
    "sender_id": "U1234567890",
    "message": { "type": "text", "id": "468789577898262530", "text": "Hello, Bot!" },
    "timestamp": 1692251666727,
    "reply_token": "38ef843bde154d9b91c21320ffd17a0f",
    "quote_token": "...",
    "mark_as_read_token": "...",
    "webhook_event_id": "event-uuid",
    "source_type": "user"
  }
}
```

### Outgoing Response (from UGENT)

```json
{
  "type": "response",
  "request_id": "optional-uuid-for-tracking",
  "original_id": "uuid-from-message",
  "content": "Hello! How can I help you?",
  "artifacts": []
}
```

### Response Result (acknowledgment)

After processing, the proxy sends a `ResponseResult` back to the responding client:

```json
{
  "type": "response_result",
  "request_id": "uuid-if-provided",
  "original_id": "uuid-from-message",
  "success": true,
  "error": null
}
```

## Message Processing

1. LINE webhook received → signature verified → deduplicated
2. Typing indicator sent to user (if enabled)
3. ProxyMessage forwarded to UGENT client(s) via WebSocket
4. First client to respond claims conversation ownership
5. UGENT response sent to LINE (reply token first, push as fallback)
6. Messages marked as read (if enabled and send succeeded)
7. ResponseResult sent to responding client (targeted, not broadcast)

## Deployment

### Nginx Configuration

Create `/etc/nginx/sites-available/ugent-line-proxy`:

```nginx
server {
    listen 80;
    listen [::]:80;
    server_name your-domain.com;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name your-domain.com;

    ssl_certificate /etc/letsencrypt/live/your-domain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/your-domain.com/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;

    location /line/callback {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_connect_timeout 10s;
        proxy_send_timeout 10s;
        proxy_read_timeout 10s;
    }

    location /ws {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_connect_timeout 60s;
        proxy_send_timeout 3600s;
        proxy_read_timeout 3600s;
    }

    location /health {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
    }

    location / {
        return 404;
    }
}
```

### Systemd Service

Environment file at `/etc/ugent-line-proxy/.env`:

```bash
sudo mkdir -p /etc/ugent-line-proxy
sudo tee /etc/ugent-line-proxy/.env > /dev/null << 'EOF'
LINE_CHANNEL_SECRET=your_secret_here
LINE_CHANNEL_ACCESS_TOKEN=your_token_here
LINE_PROXY_API_KEY=your_api_key_here
LINE_PROXY_BIND_ADDR=0.0.0.0:3000
LINE_AUTO_LOADING_INDICATOR=true
LINE_AUTO_MARK_AS_READ=true
EOF
```

Service file at `/etc/systemd/system/ugent-line-proxy.service`:

```ini
[Unit]
Description=UGENT LINE Proxy Server
After=network.target
Wants=network-online.target

[Service]
Type=simple
User=root
WorkingDirectory=/var/lib/ugent-line-proxy
EnvironmentFile=/etc/ugent-line-proxy/.env
ExecStart=/usr/bin/ugent-line-proxy
Restart=on-failure
RestartSec=5
TimeoutStopSec=30
LimitNOFILE=65536
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

```bash
sudo mkdir -p /var/lib/ugent-line-proxy /var/log/ugent-line-proxy
sudo cp target/release/ugent-line-proxy /usr/bin/
sudo systemctl daemon-reload
sudo systemctl enable --now ugent-line-proxy
sudo systemctl status ugent-line-proxy
```

### Docker

```dockerfile
FROM rust:1.93-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/ugent-line-proxy /usr/local/bin/
CMD ["ugent-line-proxy"]
```

## Documentation

For detailed documentation, see the [docs](./docs) folder:

- [Architecture](./docs/ARCHITECTURE.md) - System architecture and components
- [Quick Start](./docs/QUICK_START.md) - Getting started guide
- [WebSocket Protocol](./docs/WEBSOCKET_PROTOCOL.md) - WebSocket message specification
- [Features](./docs/FEATURES.md) - Features for UGENT LINE clients
- [API Reference](./docs/API_REFERENCE.md) - Complete API documentation
- [RMS CLI Guide](./docs/RMS_CLI_API_GUIDE.md) - Relationship management CLI reference

## License

MIT
