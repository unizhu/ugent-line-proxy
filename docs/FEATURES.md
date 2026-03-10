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
  "content": "Your response text here",
  "artifacts": []
}
```

**With Artifacts (Images/Audio/Video):**
```json
{
  "type": "response",
  "original_id": "message-uuid",
  "content": "Here's the image:",
  "artifacts": [
    {
      "file_name": "image.png",
      "content_type": "image/png",
      "kind": "image",
      "data": "base64-data",
      "local_path": "https://public-url.com/image.png"
    }
  ]
}
```

### 8. Message Deduplication

Each message includes `webhook_event_id` for deduplication.

**Use Cases:**
- Handle webhook redeliveries
- Prevent duplicate processing
- Implement idempotent handlers

### 9. Event Type Support

Beyond messages, the proxy handles:

| Event | Description |
|-------|-------------|
| `Follow` | User added bot as friend |
| `Unfollow` | User blocked bot |
| `Join` | Bot joined group/room |
| `Leave` | Bot left group/room |
| `MemberJoined` | New member joined group |
| `MemberLeft` | Member left group |
| `Postback` | Template button tap |
| `Beacon` | LINE Beacon detection |
| `AccountLink` | Account link event |

## LINE API Features

The proxy provides access to LINE Messaging API:

### Reply Message

Respond to a webhook event using reply token:

```rust
line_client.reply_message(reply_token, messages).await?;
```

- Max 5 messages per reply
- Reply token expires in ~1 minute

### Push Message

Send proactive messages:

```rust
line_client.push_message(to, messages).await?;
```

- Max 5 messages per push
- Can send to user/group/room

### User Profile

Get user information:

```rust
let profile = line_client.get_profile(user_id).await?;
// profile.display_name, profile.picture_url, profile.status_message
```

### Group Management

- `get_group_summary(group_id)` - Get group info
- `get_group_member_ids(group_id)` - List members
- `get_group_member_profile(group_id, user_id)` - Get member profile
- `leave_group(group_id)` - Leave a group

### Bot Info

Get bot information:

```rust
let info = line_client.get_bot_info().await?;
// info.user_id, info.display_name, info.picture_url
```

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
