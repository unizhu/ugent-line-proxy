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

### Nginx Configuration

Create an nginx configuration file at `/etc/nginx/sites-available/ugent-line-proxy`:

```nginx
# UGENT LINE Proxy - Nginx Reverse Proxy Configuration
# Place this file at /etc/nginx/sites-available/ugent-line-proxy
# Then: sudo ln -s /etc/nginx/sites-available/ugent-line-proxy /etc/nginx/sites-enabled/
# Test: sudo nginx -t
# Reload: sudo systemctl reload nginx

server {
    listen 80;
    listen [::]:80;
    server_name your-domain.com;  # Replace with your domain

    # Redirect HTTP to HTTPS
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name your-domain.com;  # Replace with your domain

    # SSL Configuration (use Let's Encrypt certbot for automatic certs)
    ssl_certificate /etc/letsencrypt/live/your-domain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/your-domain.com/privkey.pem;
    ssl_session_timeout 1d;
    ssl_session_cache shared:SSL:50m;
    ssl_session_tickets off;

    # Modern SSL configuration
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_ciphers ECDHE-ECDSA-AES128-GCM-SHA256:ECDHE-RSA-AES128-GCM-SHA256:ECDHE-ECDSA-AES256-GCM-SHA384:ECDHE-RSA-AES256-GCM-SHA384:ECDHE-ECDSA-CHACHA20-POLY1305:ECDHE-RSA-CHACHA20-POLY1305:DHE-RSA-AES128-GCM-SHA256:DHE-RSA-AES256-GCM-SHA384;
    ssl_prefer_server_ciphers off;

    # HSTS
    add_header Strict-Transport-Security "max-age=63072000" always;

    # LINE Webhook Callback (matches LINE_WEBHOOK_PATH in .env.example)
    # URL: https://your-domain.com/line/callback
    location /line/callback {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # Timeout settings for LINE webhooks
        proxy_connect_timeout 10s;
        proxy_send_timeout 10s;
        proxy_read_timeout 10s;
    }

    # WebSocket Endpoint (matches WS_PATH in .env.example)
    # URL: wss://your-domain.com/ws
    location /ws {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket timeout settings
        proxy_connect_timeout 60s;
        proxy_send_timeout 3600s;
        proxy_read_timeout 3600s;
    }

    # Health Check Endpoint
    location /health {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }

    # Deny all other locations
    location / {
        return 404;
    }
}
```

**LINE Developers Console Setup:**
- Webhook URL: `https://your-domain.com/line/callback`
- This matches the `LINE_WEBHOOK_PATH=/line/callback` from `.env.example`

**Enable the site:**
```bash
# Create symlink
sudo ln -s /etc/nginx/sites-available/ugent-line-proxy /etc/nginx/sites-enabled/

# Test configuration
sudo nginx -t

# Reload nginx
sudo systemctl reload nginx

# Get SSL certificate (if using Let's Encrypt)
sudo certbot --nginx -d your-domain.com
```

### Systemd Service

Create the environment file first:
```bash
sudo mkdir -p /etc/ugent-line-proxy
sudo tee /etc/ugent-line-proxy/.env > /dev/null << 'EOF'
LINE_CHANNEL_SECRET=your_secret_here
LINE_CHANNEL_ACCESS_TOKEN=your_token_here
LINE_PROXY_API_KEY=your_api_key_here
LINE_PROXY_BIND_ADDR=0.0.0.0:3000
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

# Security hardening
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```

Install and start:
```bash
# Create directories
sudo mkdir -p /var/lib/ugent-line-proxy
sudo mkdir -p /var/log/ugent-line-proxy

# Copy binary
sudo cp target/release/ugent-line-proxy /usr/bin/
sudo chmod +x /usr/bin/ugent-line-proxy

# Install service
cd ugent-line-proxy
sudo cp ugent-line-proxy.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable ugent-line-proxy
sudo systemctl start ugent-line-proxy

# Check status
sudo systemctl status ugent-line-proxy
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

## Documentation

For detailed documentation, see the [docs](./docs) folder:

- [Architecture](./docs/ARCHITECTURE.md) - System architecture and components
- [Quick Start](./docs/QUICK_START.md) - Getting started guide
- [WebSocket Protocol](./docs/WEBSOCKET_PROTOCOL.md) - WebSocket message specification
- [Features](./docs/FEATURES.md) - Features for UGENT LINE clients
- [API Reference](./docs/API_REFERENCE.md) - Complete API documentation
