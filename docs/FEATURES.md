# Features for UGENT LINE Client

This document describes the features and capabilities that `ugent-line-proxy` provides to UGENT LINE client plugins.

## Core Features

### 1. Real-time Message Reception

Receive LINE messages in real-time via WebSocket connection.

**Supported Message Types:**
- ✅ Text messages (with @mention support)
- ✅ Image messages
- ✅ Audio messages
- ✅ Video messages
- ✅ File messages
- ✅ Sticker messages
- ✅ Location messages

### 2. Message Context Extraction

Each message includes rich context:

| Field | Description |
|-------|-------------|
| `conversation_id` | User ID, Group ID, or Room ID |
| `sender_id` | User ID of message sender |
| `source_type` | `user` (P2P), `group`, or `room` |
| `timestamp` | Unix milliseconds |
| `reply_token` | Valid for ~1 minute after message |
| `quote_token` | For quote/reply functionality |
| `webhook_event_id` | For deduplication |

### 3. Group Chat Support

Full support for LINE group chats:

- **Group ID extraction**: Messages from groups include `group_id`
- **Sender identification**: Know who sent the message in the group
- **@mention detection**: Detect when bot is mentioned
- **Room support**: Multi-person chat (room) support

### 4. @Mention Detection

Text messages include mention information:

```json
{
  "message": {
    "type": "text",
    "text": "@bot Help me!",
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
}
```

Use `is_self: true` to detect bot mentions.

### 5. Reply Token Management

LINE provides a reply token valid for ~1 minute after a message.

**Features:**
- Reply token included in every message event
- Automatic fallback: reply token → push message
- Reply token tracking for routing responses

**Best Practice:** Respond quickly using reply token for better UX.

### 6. Media Content Download

Download media content from LINE messages:

```rust
// Via broker
let (data, content_type) = broker.download_media(message_id).await?;
```

**Supported Media:**
- Images (JPEG, PNG)
- Audio (M4A)
- Video (MP4)
- Files (any type)

### 7. Outbound Message Sending

Send messages back to LINE users:

**Text Messages:**
```json
{
  "type": "response",
  "original_id": "message-uuid",
  "content": "Hello!",
  "artifacts": []
}
```

**With Artifacts (Images, Audio, Video):**
```json
{
  "type": "response",
  "original_id": "message-uuid",
  "content": "Here's the image:",
  "artifacts": [
    {
      "kind": "image",
      "content_type": "image/png",
      "file_name": "photo.png",
      "local_path": "https://example.com/photo.png"
    }
  ]
}
```

**Artifact Kinds:** `image`, `audio`, `video`, `file`

### 8. Auto Loading Indicator

When a message is received from a LINE user, the proxy can automatically send a typing indicator to show the user that the bot is processing their request.

**Configuration:**
```bash
LINE_AUTO_LOADING_INDICATOR=true   # default: true
```

### 9. Auto Mark as Read

After a response is sent back to a LINE user, the proxy can automatically mark the message as read in the LINE chat.

**Configuration:**
```bash
LINE_AUTO_MARK_AS_READ=true        # default: true
```

### 10. Redelivery Handling

LINE may redeliver webhook events. The proxy supports deduplication via `webhook_event_id` and configurable redelivery processing.

**Configuration:**
```bash
LINE_PROXY_PROCESS_REDELIVERIES=true  # default: true
```

## Data Retention & Persistence

### 11. Message Database

All messages (inbound and outbound) can be persisted to a database for audit, analytics, and replay.

**Supported Backends:**
- **SQLite** (default, zero-config): Embedded database, no external dependencies
- **PostgreSQL** (feature flag `postgres`): For production deployments with existing PG infrastructure

**Configuration:**
```bash
LINE_PROXY_DB_TYPE=sqlite          # or "postgres"
LINE_PROXY_DB_URL=postgresql://... # required for postgres
```

### 12. Contact & Group Storage

Store and retrieve LINE contact profiles and group information:
- User display names, profile pictures
- Group summaries and member lists
- Updated automatically on message receipt

### 13. Data Retention Policies

Configure automatic cleanup of old data:

| Variable | Default | Description |
|----------|---------|-------------|
| `LINE_PROXY_RETENTION_ENABLED` | `false` | Enable data retention cleanup |
| `LINE_PROXY_RETENTION_MAX_AGE_DAYS` | `90` | Max message age in days |
| `LINE_PROXY_RETENTION_CLEANUP_INTERVAL_SECS` | `3600` | Cleanup check interval |

## Message Retry

### 14. Inbound Message Retry

If a UGENT client is not connected when a LINE message arrives, the message is stored in an inbound queue. When a client reconnects, pending messages are automatically delivered.

**Configuration:**

| Variable | Default | Description |
|----------|---------|-------------|
| `LINE_PROXY_RETRY_ENABLED` | `false` | Enable retry system |
| `LINE_PROXY_RETRY_MAX_ATTEMPTS` | `5` | Max retry attempts |
| `LINE_PROXY_RETRY_INITIAL_DELAY_SECS` | `1` | Initial backoff delay |
| `LINE_PROXY_RETRY_MAX_DELAY_SECS` | `300` | Maximum backoff delay |

### 15. Outbound Message Retry

If sending a response to LINE fails (network error, rate limit, etc.), the message is queued for retry with exponential backoff.

**Features:**
- Exponential backoff with configurable limits
- Automatic retry on transient failures
- Dead letter handling after max retries exceeded

## Relationship Management (RMS)

### 16. Entity-Client Mapping

The Relationship Management System (RMS) allows controlling which UGENT client handles which LINE conversation.

**Features:**
- View connected clients and their status
- Map LINE entities (users/groups) to specific clients
- Import/export relationship configurations
- REST API and CLI for management

See [RMS CLI & API Guide](./RMS_CLI_API_GUIDE.md) for details.

## Integration Features

### 1. Multiple Client Support

The proxy supports multiple UGENT clients:
- Messages broadcast to all connected clients
- Each client receives all incoming messages
- Client deduplication is client's responsibility

### 2. Connection Management

- Automatic ping/pong heartbeat
- Configurable connection timeout
- Graceful reconnection support

### 3. Security

- HMAC-SHA256 webhook signature verification
- API key authentication for WebSocket
- Secure credential storage

### 4. Error Handling

- Structured error responses
- Retry mechanisms for LINE API
- Timeout handling

## Message Flow Examples

### Example 1: Simple Text Response

```
1. User sends: "Hello, Bot!"
2. Proxy → UGENT: {"type":"message","data":{...}}
3. UGENT → Proxy: {"type":"response","content":"Hi there!","artifacts":[]}
4. Proxy → LINE: Reply via reply token
5. User receives: "Hi there!"
```

### Example 2: Image with Text

```
1. User sends: "Show me a cat"
2. Proxy → UGENT: {"type":"message","data":{...}}
3. UGENT → Proxy: {
     "type":"response",
     "content":"Here's a cute cat!",
     "artifacts":[{
       "kind":"image",
       "local_path":"https://example.com/cat.jpg",
       ...
     }]
   }
4. Proxy → LINE: Reply with text + image
5. User receives: Text + Cat image
```

### Example 3: Group Chat with @Mention

```
1. User in group sends: "@bot Help"
2. Proxy → UGENT: {
     "type":"message",
     "data":{
       "source_type":"group",
       "message":{"mention":{"mentionees":[{"is_self":true}]}},
       ...
     }
   }
3. UGENT detects is_self=true, responds
4. User receives response in group
```

### Example 4: Message Retry Flow

```
1. LINE message arrives
2. No UGENT client connected
3. Message stored in inbound queue
4. UGENT client connects
5. Pending messages delivered from queue
6. Message processed normally
```

## Limitations

1. **Media URLs**: LINE requires public URLs for images/videos. The proxy cannot send local files directly.
2. **Reply Token Expiry**: Reply tokens expire in ~1 minute. Long processing should use push messages.
3. **Message Size**: Text messages limited to 5000 characters (auto-split by proxy).
4. **Rate Limits**: LINE API has rate limits. The proxy does not implement rate limiting.

## Best Practices

1. **Respond Quickly**: Use reply token within 1 minute for best UX
2. **Handle Deduplication**: Use `webhook_event_id` to prevent duplicate processing
3. **Check @Mentions**: In groups, only respond when bot is mentioned
4. **Provide Public URLs**: For images/videos, upload to public URL first
5. **Implement Fallbacks**: Handle expired reply tokens by falling back to push messages
6. **Enable Retry**: Set `LINE_PROXY_RETRY_ENABLED=true` for production reliability
7. **Enable Storage**: Set `LINE_PROXY_STORAGE_ENABLED=true` for persistence and RMS support
