# API Reference

## HTTP Endpoints

### Health Check

Check if the proxy server is running.

```
GET /health
```

**Response:**
```
HTTP/1.1 200 OK
Content-Type: text/plain

OK
```

### LINE Webhook

Receive webhooks from LINE Platform.

```
POST /line/callback
```

**Headers:**
| Header | Value |
|--------|-------|
| `Content-Type` | `application/json` |
| `x-line-signature` | HMAC-SHA256 signature |

**Request Body:**
```json
{
  "destination": "U8e742f61d673b39c7fff3cecb7536ef0",
  "events": [...]
}
```

**Response:**
```
HTTP/1.1 200 OK
```

**Error Responses:**
| Status | Description |
|--------|-------------|
| 400 | Missing/invalid signature |
| 400 | Invalid JSON |
| 500 | Processing error |

### WebSocket Endpoint

Establish WebSocket connection.

```
GET /ws
```

**Upgrade Request:**
```
GET /ws HTTP/1.1
Host: localhost:3000
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Key: <key>
Sec-WebSocket-Version: 13
```

See [WebSocket Protocol](./WEBSOCKET_PROTOCOL.md) for message formats.

## Configuration API

### Environment Variables

#### Server Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LINE_PROXY_BIND_ADDR` | string | `0.0.0.0:3000` | Server bind address |
| `LINE_PROXY_NAME` | string | `ugent-line-proxy` | Server name for logging |

#### TLS Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LINE_PROXY_TLS_CERT` | path | (none) | TLS certificate file |
| `LINE_PROXY_TLS_KEY` | path | (none) | TLS private key file |

#### LINE Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LINE_CHANNEL_SECRET` | string | (required) | Channel secret for signature |
| `LINE_CHANNEL_ACCESS_TOKEN` | string | (required) | Access token for API |
| `LINE_PROXY_WEBHOOK_PATH` | string | `/line/callback` | Webhook endpoint path |
| `LINE_PROXY_SKIP_SIGNATURE` | bool | `false` | Skip signature verification |
| `LINE_PROXY_PROCESS_REDELIVERIES` | bool | `true` | Process redelivered events |

#### WebSocket Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LINE_PROXY_WS_PATH` | string | `/ws` | WebSocket endpoint path |
| `LINE_PROXY_API_KEY` | string | (none) | Authentication API key |
| `LINE_PROXY_WS_PING_INTERVAL` | number | `30` | Ping interval (seconds) |
| `LINE_PROXY_WS_TIMEOUT` | number | `60` | Connection timeout (seconds) |
| `LINE_PROXY_WS_MAX_MESSAGE_SIZE` | number | `10485760` | Max message size (bytes) |

#### Media Cache Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LINE_PROXY_MEDIA_CACHE_DIR` | path | `$TMP/ugent-line-proxy-cache` | Cache directory |
| `LINE_PROXY_MEDIA_CACHE_MAX_MB` | number | `500` | Max cache size (MB) |
| `LINE_PROXY_MEDIA_CACHE_TTL` | number | `3600` | Cache TTL (seconds) |

#### Logging Configuration

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `LINE_PROXY_LOG_LEVEL` | string | `info` | Log level (trace/debug/info/warn/error) |
| `LINE_PROXY_LOG_FORMAT` | string | `json` | Log format (json/pretty) |
| `LINE_PROXY_LOG_FILE` | path | (none) | Log file path |

## LINE API Methods

### LineApiClient

#### Constructor

```rust
let client = LineApiClient::new(access_token);
```

#### reply_message

Reply to a webhook event.

```rust
async fn reply_message(
    &self,
    reply_token: &str,
    messages: Vec<Value>
) -> Result<(), LineApiError>
```

**Parameters:**
| Name | Type | Description |
|------|------|-------------|
| `reply_token` | &str | Reply token from webhook |
| `messages` | Vec<Value> | LINE message objects (max 5) |

**Errors:**
- `InvalidReplyToken` - Reply token expired or invalid
- `ApiError` - LINE API error

#### push_message

Send a message proactively.

```rust
async fn push_message(
    &self,
    to: &str,
    messages: Vec<Value>
) -> Result<(), LineApiError>
```

**Parameters:**
| Name | Type | Description |
|------|------|-------------|
| `to` | &str | User ID, Group ID, or Room ID |
| `messages` | Vec<Value> | LINE message objects (max 5) |

#### download_content

Download media content.

```rust
async fn download_content(
    &self,
    message_id: &str
) -> Result<(Vec<u8>, String), LineApiError>
```

**Returns:** Tuple of (data, content_type)

#### get_profile

Get user profile.

```rust
async fn get_profile(
    &self,
    user_id: &str
) -> Result<UserProfile, LineApiError>
```

**Returns:**
```rust
struct UserProfile {
    user_id: String,
    display_name: String,
    picture_url: Option<String>,
    status_message: Option<String>,
    language: Option<String>,
}
```

#### get_bot_info

Get bot information.

```rust
async fn get_bot_info(&self) -> Result<BotInfo, LineApiError>
```

**Returns:**
```rust
struct BotInfo {
    user_id: String,
    basic_id: String,
    premium_id: Option<String>,
    display_name: String,
    picture_url: Option<String>,
}
```

#### Group Management

```rust
// Get group summary
async fn get_group_summary(&self, group_id: &str) -> Result<GroupSummary, LineApiError>

// Get group member IDs
async fn get_group_member_ids(&self, group_id: &str) -> Result<MemberIdsResponse, LineApiError>

// Get group member profile
async fn get_group_member_profile(&self, group_id: &str, user_id: &str) -> Result<UserProfile, LineApiError>

// Leave group
async fn leave_group(&self, group_id: &str) -> Result<(), LineApiError>
```

## Message Builders

### build_text_message

```rust
pub fn build_text_message(text: &str) -> Value
```

### build_image_message

```rust
pub fn build_image_message(original_url: &str, preview_url: &str) -> Value
```

### build_video_message

```rust
pub fn build_video_message(original_url: &str, preview_url: &str) -> Value
```

### build_audio_message

```rust
pub fn build_audio_message(original_url: &str, duration_ms: i64) -> Value
```

### build_sticker_message

```rust
pub fn build_sticker_message(package_id: &str, sticker_id: &str) -> Value
```

### build_location_message

```rust
pub fn build_location_message(
    title: &str,
    address: &str,
    latitude: f64,
    longitude: f64
) -> Value
```

## Broker API

### MessageBroker

#### Constructor

```rust
let broker = MessageBroker::new(config, ws_manager);
```

#### send_to_clients

Send message to all connected UGENT clients.

```rust
pub async fn send_to_clients(&self, message: ProxyMessage) -> Result<(), BrokerError>
```

#### handle_response

Handle response from UGENT client.

```rust
pub async fn handle_response(
    &self,
    original_id: &str,
    content: &str,
    artifacts: Vec<OutboundArtifact>
) -> Result<(), BrokerError>
```

#### send_artifact

Send artifact to LINE user.

```rust
pub async fn send_artifact(
    &self,
    conversation_id: &str,
    reply_token: Option<&str>,
    artifact: &OutboundArtifact
) -> Result<(), BrokerError>
```

#### download_media

Download media content from LINE.

```rust
pub async fn download_media(&self, message_id: &str) -> Result<(Vec<u8>, String), BrokerError>
```

#### Accessors

```rust
// Get LINE API client
pub fn line_client(&self) -> &LineApiClient

// Get connected client count
pub fn client_count(&self) -> usize

// Get list of connected clients
pub fn connected_clients(&self) -> Vec<String>

// Get WebSocket manager
pub fn ws_manager(&self) -> Arc<WebSocketManager>
```

## WebSocket Manager API

### WebSocketManager

#### Constructor

```rust
let manager = WebSocketManager::new(config);
```

#### Methods

```rust
// Get client count
pub fn client_count(&self) -> usize

// Check if clients are connected
pub fn has_clients(&self) -> bool

// Broadcast to all clients
pub async fn broadcast(&self, message: WsProtocol) -> Result<(), BroadcastError>

// Send to specific client
pub async fn send_to(&self, client_id: &str, message: WsProtocol) -> Result<(), SendError>

// Get connected client IDs
pub fn get_connected_client_ids(&self) -> Vec<String>

// Register reply token mapping
pub fn register_reply_token(&self, reply_token: &str, client_id: &str)

// Get client for reply token
pub fn get_client_for_reply_token(&self, reply_token: &str) -> Option<String>
```

## Error Types

### ProxyError

```rust
pub enum ProxyError {
    Config(ConfigError),
    Webhook(WebhookError),
    Broker(BrokerError),
    LineApi(LineApiError),
    Io(std::io::Error),
    Server(String),
}
```

### WebhookError

```rust
pub enum WebhookError {
    InvalidSignature,
    MissingSignature,
    InvalidJson(String),
    Processing(String),
}
```

### BrokerError

```rust
pub enum BrokerError {
    Broadcast(BroadcastError),
    Send(SendError),
    LineApi(String),
    Http(String),
    UnsupportedArtifactType,
    PathError(String),
    Io(std::io::Error),
}
```

### LineApiError

```rust
pub enum LineApiError {
    RequestFailed(reqwest::Error),
    ApiError(u16, String),
    DownloadFailed(String),
    Serialization(serde_json::Error),
    InvalidResponse(String),
    InvalidReplyToken,
}
```
