# Bug Report & Enhancement Plan for ugent-line-proxy

**Generated**: 2026-03-11
**Based on**: LINE API 2025 changes, Official SDK comparison, Code review

## Summary

After reviewing the ugent-line-proxy codebase against:
1. LINE Messaging API 2025 updates
2. Official LINE SDKs (Go, Node.js, Rust)
3. Documented features in `/docs`

The following issues and enhancements were identified.

---

## 🔴 Critical Issues (P0)

### 1. Missing `X-Line-Retry-Key` Header Support

**Problem**: The LINE API supports idempotent message sending via `X-Line-Retry-Key` header. This prevents duplicate messages when retrying failed requests.

**Impact**: 
- Network retries can cause duplicate messages
- No protection against 5xx errors

**Official SDK Reference**:
```go
// line-bot-sdk-go
bot.PushMessage(&messaging_api.PushMessageRequest{
    To: "U.......",
    Messages: []messaging_api.MessageInterface{...},
}, "123e4567-e89b-12d3-a456-426614174000") // x-line-retry-key
```

**Fix Required**: Add `X-Line-Retry-Key` header to push_message and multicast APIs.

---

### 2. Missing Rate Limit Handling

**Problem**: No rate limit (429) handling or retry logic. LINE API has strict rate limits:
- Reply: 1000 req/min per bot
- Push: Variable by tier (50-1000 req/min)
- Multicast: Changed April 2025

**Impact**: 
- 429 errors cause message delivery failures
- No exponential backoff
- No `Retry-After` header parsing

**Official Documentation**:
```
HTTP 429 Response Headers:
- Retry-After: seconds to wait
- X-RateLimit-Reset: Unix timestamp when bucket refills
- X-RateLimit-Remaining: tokens left
```

**Fix Required**: Implement rate limit handling with exponential backoff.

---

### 3. Missing Membership Event Types

**Problem**: Recent LINE API added `MembershipEvent` for LINE Official Account membership features, but not implemented.

**Impact**: Cannot handle membership-related events.

**Fix Required**: Add `MembershipEvent` type to Event enum.

---

## 🟡 Medium Issues (P1)

### 4. Signature Verification Uses Non-Constant-Time Comparison

**Location**: `src/webhook/signature.rs:75-80`

**Problem**: Current implementation uses simple XOR loop but comment mentions "consider using `subtle` crate".

**Current Code**:
```rust
let mut result = 0u8;
for (a, b) in signature_bytes.iter().zip(computed_bytes.iter()) {
    result |= a ^ b;
}
```

**Risk**: Timing attacks could theoretically leak signature bytes.

**Fix**: Use `subtle` crate for constant-time comparison.

---

### 5. Custom Database Path Not Used in Storage Initialization

**Problem**: `StorageConfig.path` exists but wasn't being used - storage always used default path.

**Status**: ✅ FIXED - Added `Storage::with_optional_path()` method.

---

### 6. Missing `chat.loading.start` API

**Problem**: LINE's "thinking" indicator API (`POST /v2/bot/chat/loading/start`) not implemented.

**Use Case**: Show typing indicator while UGENT is processing.

**Fix Required**: Add `start_loading()` method to LineApiClient.

---

## 🟢 Low Issues (P2)

### 7. Reply Token Expiration Not Tracked Accurately

**Problem**: Reply tokens expire in ~60 seconds but no proactive tracking.

**Current**: Best effort fallback to push message on error.

**Enhancement**: Track reply token expiration time and preemptively skip expired tokens.

---

### 8. No Webhook Event ID Deduplication Cache

**Problem**: `webhook_event_id` is passed but not used for deduplication.

**Impact**: Redelivered webhooks may be processed multiple times.

**Fix Required**: Add in-memory LRU cache for recent webhook_event_ids.

---

## 📊 Feature Gap Analysis vs Official SDKs

| Feature | ugent-line-proxy | line-bot-sdk-go | Priority |
|---------|-----------------|-----------------|----------|
| X-Line-Retry-Key | ❌ | ✅ | P0 |
| Rate Limit Handling | ❌ | ✅ | P0 |
| Loading Indicator | ❌ | ✅ | P1 |
| Membership Events | ❌ | ✅ | P1 |
| Signature Verification | ✅ (basic) | ✅ (constant-time) | P1 |
| Webhook Dedup | Partial | ✅ | P2 |
| Content Download | ✅ | ✅ | - |
| Profile API | ✅ | ✅ | - |
| Group/Room API | ✅ | ✅ | - |

---

## 🔧 Implementation Plan

### Phase 1: Critical Fixes (This Session)

1. ✅ Add custom database path support (`LINE_PROXY_STORAGE_PATH`)
2. Add `X-Line-Retry-Key` header to push/reply APIs
3. Add basic rate limit error handling

### Phase 2: Robustness (Next Session)

1. Add `subtle` crate for constant-time comparison
2. Implement webhook_event_id deduplication
3. Add `start_loading()` API

### Phase 3: Full Compliance

1. Add MembershipEvent type
2. Implement full rate limit with exponential backoff
3. Add response header parsing (X-RateLimit-*)

---

## API Changes Summary

### New Environment Variables

```bash
# Already added
LINE_PROXY_STORAGE_ENABLED=true
LINE_PROXY_STORAGE_PATH=/custom/path/to/db  # NEW: custom database path
```

### New Code to Add

```rust
// line_api.rs - Add retry key support
pub async fn push_message_with_retry_key(
    &self,
    to: &str,
    messages: Vec<Value>,
    retry_key: Option<&str>,
) -> Result<(), LineApiError> {
    // ...
    if let Some(key) = retry_key {
        request = request.header("X-Line-Retry-Key", key);
    }
    // ...
}
```

---

## Test Coverage Status

- ✅ Signature verification: 7 tests
- ✅ Message parsing: Multiple tests
- ✅ Storage operations: 35 tests total
- ❌ Rate limit handling: No tests
- ❌ Retry key: No tests

---

## References

- [LINE Messaging API Reference](https://developers.line.biz/en/reference/messaging-api/)
- [Retry Failed Requests](https://developers.line.biz/en/docs/messaging-api/retrying-api-request/)
- [line-bot-sdk-go](https://github.com/line/line-bot-sdk-go)
- [line-bot-sdk-rust](https://github.com/nanato12/line-bot-sdk-rust)
