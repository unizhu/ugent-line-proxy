# Quick Start Guide

## Prerequisites

- Rust 1.93 or later (edition 2024)
- LINE Developers account
- LINE Messaging API channel
- Public server (VPS) for webhook reception

## Setup

### 1. Create LINE Channel

1. Go to [LINE Developers Console](https://developers.line.biz/)
2. Create a Provider
3. Create a Messaging API channel
4. Note your Channel Secret and Channel Access Token

### 2. Configure Webhook

1. In LINE Console, go to Messaging API settings
2. Enable webhooks
3. Set webhook URL: `https://your-server.com/line/callback`

### 3. Install ugent-line-proxy

```bash
# Clone and build
git clone https://github.com/your-org/ugent-line-proxy.git
cd ugent-line-proxy
cargo build --release

# Or install with cargo
cargo install --path .
```

### 4. Configure Environment

Create `.env` file:

```bash
# Server
LINE_PROXY_BIND_ADDR=0.0.0.0:3000

# LINE Credentials
LINE_CHANNEL_SECRET=your_channel_secret
LINE_CHANNEL_ACCESS_TOKEN=your_access_token

# WebSocket Auth
LINE_PROXY_API_KEY=your_secure_api_key

# Optional: Logging
LINE_PROXY_LOG_LEVEL=info
LINE_PROXY_LOG_FORMAT=json

# Optional: Storage (enables RMS)
LINE_PROXY_STORAGE_ENABLED=true
LINE_PROXY_STORAGE_PATH=~/.ugent/line-plugin/

# Optional: Database (message persistence & retry)
LINE_PROXY_DB_TYPE=sqlite

# Optional: Auto features
LINE_AUTO_LOADING_INDICATOR=true
LINE_AUTO_MARK_AS_READ=true
LINE_PROXY_PROCESS_REDELIVERIES=true
```

### 5. Run the Proxy

```bash
# Development
cargo run

# Production
./target/release/ugent-line-proxy
```

## Connect UGENT Client

### WebSocket Connection

```python
import asyncio
import websockets
import json
import uuid

async def connect_to_proxy():
    uri = "ws://your-server:3000/ws"
    
    async with websockets.connect(uri) as ws:
        # 1. Authenticate
        auth_msg = {
            "type": "auth",
            "data": {
                "client_id": "ugent-client-001",
                "api_key": "your_secure_api_key"
            }
        }
        await ws.send(json.dumps(auth_msg))
        
        # 2. Wait for auth result
        result = json.loads(await ws.recv())
        if result["type"] != "auth_result" or not result["success"]:
            print("Authentication failed")
            return
        
        print("Connected and authenticated!")
        
        # 3. Message loop
        while True:
            msg = json.loads(await ws.recv())
            
            if msg["type"] == "ping":
                await ws.send(json.dumps({"type": "pong"}))
            
            elif msg["type"] == "message":
                # Handle incoming LINE message
                data = msg["data"]
                print(f"Message from {data['sender_id']}: {data['message']}")
                
                # Send response
                response = {
                    "type": "response",
                    "original_id": data["id"],
                    "content": "Hello from UGENT!",
                    "artifacts": []
                }
                await ws.send(json.dumps(response))

asyncio.run(connect_to_proxy())
```

### Rust Client Example

```rust
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use serde_json::json;

#[tokio::main]
async fn main() {
    let (ws_stream, _) = connect_async("ws://localhost:3000/ws")
        .await
        .expect("Failed to connect");
    
    let (mut write, mut read) = ws_stream.split();
    
    // Authenticate
    let auth = json!({
        "type": "auth",
        "data": {
            "client_id": "ugent-rust-client",
            "api_key": "your_api_key"
        }
    });
    write.send(Message::Text(auth.to_string())).await.unwrap();
    
    // Read messages
    while let Some(msg) = read.next().await {
        let msg = msg.expect("Failed to read message");
        println!("Received: {}", msg);
    }
}
```

## Verify Setup

### 1. Check Health

```bash
curl http://localhost:3000/health
# Should return: OK
```

### 2. Check Logs

```bash
# Look for these log entries:
# - "Starting ugent-line-proxy"
# - "Listening on 0.0.0.0:3000"
# - "Client authenticated: ugent-client-001"
```

### 3. Test Webhook

Add your bot as a LINE friend and send a message. Check logs for:

```
Received webhook: destination=..., events=1
Routing message: channel=line, conversation=..., sender=...
```

## Production Deployment

### Systemd

```bash
# Copy service file
sudo cp ugent-line-proxy.service /etc/systemd/system/

# Edit environment variables
sudo systemctl edit ugent-line-proxy

# Enable and start
sudo systemctl daemon-reload
sudo systemctl enable ugent-line-proxy
sudo systemctl start ugent-line-proxy

# Check status
sudo systemctl status ugent-line-proxy
```

### Docker

```bash
# Build
docker build -t ugent-line-proxy .

# Run with all features
docker run -d \
  -p 3000:3000 \
  -e LINE_CHANNEL_SECRET=your_secret \
  -e LINE_CHANNEL_ACCESS_TOKEN=your_token \
  -e LINE_PROXY_API_KEY=your_key \
  -e LINE_PROXY_STORAGE_ENABLED=true \
  -e LINE_PROXY_DB_TYPE=sqlite \
  -e LINE_AUTO_LOADING_INDICATOR=true \
  -e LINE_AUTO_MARK_AS_READ=true \
  -v ugent-line-data:/data \
  ugent-line-proxy
```

### Nginx Reverse Proxy

```nginx
server {
    listen 443 ssl;
    server_name your-server.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location /line/callback {
        proxy_pass http://127.0.0.1:3000;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }

    location /ws {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }
}
```

## Troubleshooting

### Webhook Not Received

1. Check signature verification:
   ```bash
   LINE_PROXY_SKIP_SIGNATURE=true cargo run
   ```

2. Verify webhook URL in LINE Console

3. Check firewall allows HTTPS

### WebSocket Connection Failed

1. Check API key matches
2. Verify WebSocket path (`/ws`)
3. Check for proxy/firewall blocking WebSocket

### Reply Token Expired

Reply tokens expire after ~1 minute. If you see:
```
Reply token expired, falling back to push message
```

This is normal - the proxy automatically falls back to push messages.

### No Clients Connected

If logs show:
```
No clients connected to broadcast to
```

Ensure your UGENT client is connected and authenticated.

## Next Steps

- Read [Architecture](./ARCHITECTURE.md) for system overview
- See [WebSocket Protocol](./WEBSOCKET_PROTOCOL.md) for message formats
- Check [Features](./FEATURES.md) for available capabilities
- Reference [API Documentation](./API_REFERENCE.md) for details
- See [RMS Guide](./RMS_CLI_API_GUIDE.md) for relationship management
- Check [Database & Retry](./DATABASE_RETRY.md) for persistence and retry features
