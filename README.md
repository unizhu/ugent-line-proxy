# ugent-line-proxy

LINE Messaging API proxy server for UGENT. This proxy enables running UGENT behind NAT/firewall while still receiving LINE webhooks on a public VPS.

## Architecture

```
LINE Platform → ugent-line-proxy (Public VPS) → WebSocket → UGENT (Local)
```

## Features

- HMAC-SHA256 signature verification
- WebSocket-based real-time messaging
- Support for all LINE message types (text, image, audio, video, file, sticker, location)
- Group chat and P2P chat support
- @mention detection in groups
- Media content download proxy
- Outbound artifact (file/image) sending

## Configuration

### Environment Variables

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_CHANNEL_SECRET` | Yes | - | LINE Channel Secret for signature verification |
| `LINE_CHANNEL_ACCESS_TOKEN` | Yes | - | LINE Channel Access Token for API calls |
| `LINE_PROXY_API_KEY` | No | - | WebSocket API key for client authentication |
| `LINE_PROXY_BIND_ADDR` | No | `0.0.0.0:3000` | Server bind address |
| `LINE_PROXY_WEBHOOK_PATH` | No | `/line/callback` | Webhook endpoint path |
| `LINE_PROXY_WS_PATH` | No | `/ws` | WebSocket endpoint path |
| `LINE_PROXY_LOG_LEVEL` | No | `info` | Log level (trace/debug/info/warn/error) |
| `LINE_PROXY_LOG_FORMAT` | No | `json` | Log format (json/pretty) |

### TLS Configuration (Optional)

| Variable | Required | Default | Description |
|----------|----------|---------|-------------|
| `LINE_PROXY_TLS_CERT` | No | - | Path to TLS certificate file |
| `LINE_PROXY_TLS_KEY` | No | - | Path to TLS private key file |

## Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/line/callback` | POST | LINE webhook receiver |
| `/ws` | GET | WebSocket endpoint |

## Building

```bash
cd ugent-line-proxy
cargo build --release
```

## Running

```bash
# Set environment variables
export LINE_CHANNEL_SECRET=your_secret
export LINE_CHANNEL_ACCESS_TOKEN=your_token
export LINE_PROXY_API_KEY=your_api_key

# Run the proxy
./target/release/ugent-line-proxy
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
    "message": {
      "type": "text",
      "id": "468789577898262530",
      "text": "Hello, Bot!"
    },
    "timestamp": 1692251666727,
    "reply_token": "38ef843bde154d9b91c21320ffd17a0f",
    "source_type": "user"
  }
}
```

### Outgoing Response (from UGENT)

```json
{
  "type": "response",
  "original_id": "uuid",
  "content": "Hello! How can I help you?",
  "artifacts": []
}
```

## Deployment

### Systemd Service

```ini
[Unit]
Description=UGENT LINE Proxy
After=network.target

[Service]
Type=simple
User=ugent
WorkingDirectory=/opt/ugent-line-proxy
Environment="LINE_CHANNEL_SECRET=your_secret"
Environment="LINE_CHANNEL_ACCESS_TOKEN=your_token"
Environment="LINE_PROXY_API_KEY=your_api_key"
ExecStart=/opt/ugent-line-proxy/ugent-line-proxy
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

### Docker

```dockerfile
FROM rust:1.85-slim as builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
COPY --from=builder /app/target/release/ugent-line-proxy /usr/local/bin/
CMD ["ugent-line-proxy"]
```

## License

MIT
