# WebSocket Protocol Specification

## Connection

### Endpoint

```
ws://host:port/ws
wss://host:port/ws (with TLS)
```

### Connection Flow

```
┌──────────┐                          ┌──────────┐
│  Client  │                          │  Server  │
└────┬─────┘                          └────┬─────┘
     │                                     │
     │  1. WebSocket Upgrade Request       │
     │ ──────────────────────────────────► │
     │                                     │
     │  2. WebSocket Upgrade Response      │
     │ ◄────────────────────────────────── │
     │                                     │
     │  3. Auth Message                    │
     │ ──────────────────────────────────► │
     │                                     │
     │  4. AuthResult Message              │
     │ ◄────────────────────────────────── │
     │                                     │
     │  5. Message/Response/Ping Exchange  │
     │ ◄─────────────────────────────────► │
     │                                     │
     │  6. Close Frame                     │
     │ ◄─────────────────────────────────► │
     │                                     │
```

## Protocol Messages

All messages are JSON-encoded with a `type` field for message identification.

### 1. Authentication

#### Auth (Client → Server)

Authenticates the WebSocket client.

```json
{
  "type": "auth",
  "data": {
    "client_id": "ugent-client-001",
    "api_key": "your-api-key"
  }
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `client_id` | string | Yes | Unique client identifier |
| `api_key` | string | Yes | API key for authentication |

**Note**: If no API key is configured on server, any key is accepted (development mode).

#### AuthResult (Server → Client)

Response to authentication attempt.

```json
{
  "type": "auth_result",
  "success": true,
  "message": "Authentication successful"
}
```

```json
{
  "type": "auth_result",
  "success": false,
  "message": "Invalid API key"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `success` | boolean | Whether authentication succeeded |
| `message` | string | Human-readable status message |

### 2. Inbound Messages

#### Message (Server → Client)

Delivers an incoming LINE message to the UGENT client.

```json
{
  "type": "message",
  "data": {
    "id": "550e8400-e29b-41d4-a716-446655440000",
    "channel": "line",
    "direction": "inbound",
    "conversation_id": "U4af4980629...",
    "sender_id": "U4af4980629...",
    "message": {
      "type": "text",
      "id": "468789577898262530",
      "text": "Hello, Bot!",
      "mention": null
    },
    "media": null,
    "timestamp": 1692251666727,
    "reply_token": "38ef843bde154d9b91c21320ffd17a0f",
    "quote_token": null,
    "mark_as_read_token": null,
    "webhook_event_id": "01H810YECXQQZ37VAXPF6H9E6T",
    "source_type": "user",
    "sender_name": "John Doe",
    "sender_picture_url": null
  }
}
```

### 3. Outbound Messages

#### Response (Client → Server)

Sends a response from UGENT back to LINE.

```json
{
  "type": "response",
  "original_id": "550e8400-e29b-41d4-a716-446655440000",
  "content": "Hello! How can I help you today?",
  "artifacts": []
}
```

With artifacts:

```json
{
  "type": "response",
  "original_id": "550e8400-e29b-41d4-a716-446655440000",
  "content": "Here's the image you requested:",
  "artifacts": [
    {
      "file_name": "image.png",
      "content_type": "image/png",
      "kind": "image",
      "data": "base64-encoded-data",
      "local_path": "https://example.com/image.png"
    }
  ]
}
```

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `original_id` | UUID | Yes | ID of the original ProxyMessage |
| `content` | string | Yes | Text response content |
| `artifacts` | array | Yes | List of files/images to send |

### 4. Heartbeat

#### Ping (Client → Server)

```json
{
  "type": "ping"
}
```

#### Pong (Server → Client)

```json
{
  "type": "pong"
}
```

**Note**: Server also sends periodic pings (default: 30 seconds).

### 5. Error Messages

#### Error (Server → Client)

```json
{
  "type": "error",
  "code": 400,
  "message": "Invalid message format: missing required field"
}
```

| Code | Description |
|------|-------------|
| 400 | Bad request / Invalid format |
| 401 | Authentication required |
| 403 | Forbidden / Invalid API key |
| 500 | Internal server error |

## Data Types

### ProxyMessage

The main message envelope for LINE communication.

| Field | Type | Description |
|-------|------|-------------|
| `id` | UUID | Unique message ID |
| `channel` | string | Source channel: `line`, `wechat`, `wecom` |
| `direction` | string | `inbound` or `outbound` |
| `conversation_id` | string | User ID, Group ID, or Room ID |
| `sender_id` | string | User ID who sent the message |
| `message` | object | LINE message content (see below) |
| `media` | object | Media metadata (for media messages) |
| `timestamp` | number | Unix milliseconds |
| `reply_token` | string | LINE reply token (valid ~1 minute) |
| `quote_token` | string | Quote/reply token |
| `mark_as_read_token` | string | Mark-as-read token (LINE-specific) |
| `webhook_event_id` | string | Unique webhook event ID |
| `source_type` | string | `user`, `group`, or `room` |
| `sender_name` | string | Sender display name (optional) |
| `sender_picture_url` | string | Sender profile picture URL (optional) |

### LineMessageContent Types

#### Text Message

```json
{
  "type": "text",
  "id": "468789577898262530",
  "text": "Hello, Bot!",
  "mention": {
    "mentionees": [
      {
        "index": 0,
        "length": 4,
        "user_id": "U8e742f61d673b39c7fff3cecb7536ef0",
        "is_self": true
      }
    ]
  }
}
```

#### Image Message

```json
{
  "type": "image",
  "id": "468789577898262531",
  "content_provider": {
    "type": "line",
    "original_content_url": null,
    "preview_image_url": null
  }
}
```

#### Audio Message

```json
{
  "type": "audio",
  "id": "468789577898262532",
  "duration": 60000,
  "content_provider": {
    "type": "line"
  }
}
```

#### Video Message

```json
{
  "type": "video",
  "id": "468789577898262533",
  "duration": 120000,
  "content_provider": {
    "type": "external",
    "original_content_url": "https://example.com/video.mp4",
    "preview_image_url": "https://example.com/preview.jpg"
  }
}
```

#### File Message

```json
{
  "type": "file",
  "id": "468789577898262534",
  "file_name": "document.pdf",
  "file_size": 102400
}
```

#### Sticker Message

```json
{
  "type": "sticker",
  "id": "468789577898262535",
  "package_id": "446",
  "sticker_id": "1988",
  "sticker_resource_type": "STATIC"
}
```

#### Location Message

```json
{
  "type": "location",
  "id": "468789577898262536",
  "title": "Tokyo Station",
  "address": "Tokyo, Japan",
  "latitude": 35.6812,
  "longitude": 139.7671
}
```

### MediaContent Types

```json
// Image
{
  "type": "image",
  "message_id": "468789577898262531",
  "url": "https://example.com/image.jpg"
}

// Audio
{
  "type": "audio",
  "message_id": "468789577898262532",
  "duration_ms": 60000,
  "format": "m4a"
}

// Video
{
  "type": "video",
  "message_id": "468789577898262533",
  "duration_ms": 120000,
  "format": "mp4"
}

// File
{
  "type": "file",
  "message_id": "468789577898262534",
  "file_name": "document.pdf",
  "size_bytes": 102400
}
```

### OutboundArtifact

For sending files/images from UGENT to LINE.

```json
{
  "file_name": "image.png",
  "content_type": "image/png",
  "kind": "image",
  "data": "base64-encoded-data",
  "local_path": "https://example.com/image.png"
}
```

| Field | Type | Description |
|-------|------|-------------|
| `file_name` | string | Original file name |
| `content_type` | string | MIME type |
| `kind` | string | `image`, `audio`, `video`, `file` |
| `data` | string | Base64-encoded file data |
| `local_path` | string | Public URL (for LINE media) |

**Note**: LINE requires public URLs for images/videos. The `local_path` must be a publicly accessible URL.

## Connection Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| Ping Interval | 30s | Server sends ping every 30s |
| Timeout | 60s | Connection closed if no activity |
| Max Message Size | 10MB | Maximum WebSocket message size |
| Channel Buffer | 32 | Outbound message buffer size |

## Example Session

```
Client connects to ws://localhost:3000/ws

→ {"type":"auth","data":{"client_id":"ugent-001","api_key":"secret"}}
← {"type":"auth_result","success":true,"message":"Authentication successful"}

← {"type":"ping"}
→ {"type":"pong"}

← {"type":"message","data":{"id":"...","channel":"line","direction":"inbound",...}}

→ {"type":"response","original_id":"...","content":"Hello!","artifacts":[]}

Client disconnects
```
