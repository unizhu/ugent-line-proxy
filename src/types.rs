//! LINE Messaging API types and proxy protocol types
//!
//! This module defines:
//! - LINE webhook event types
//! - LINE message types
//! - Proxy protocol types for WebSocket communication
//! - Shared types for message routing

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Instant;
use uuid::Uuid;

// =============================================================================
// Client Info Types
// =============================================================================

/// Connected client information
#[derive(Debug, Clone)]
pub struct ClientInfo {
    /// Client identifier
    pub client_id: String,
    /// Remote address
    pub addr: SocketAddr,
    /// Connection timestamp
    pub connected_at: Instant,
    /// Last activity timestamp
    pub last_activity: Instant,
}

// =============================================================================
// Message Source Channel
// =============================================================================

/// Message source channel - identifies the messaging platform
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// WeChat Official Account (公众号)
    Wechat,
    /// WeCom/企业微信 (Enterprise WeChat)
    Wecom,
    /// LINE Messaging API
    #[default]
    Line,
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Channel::Wechat => write!(f, "wechat"),
            Channel::Wecom => write!(f, "wecom"),
            Channel::Line => write!(f, "line"),
        }
    }
}

// =============================================================================
// Proxy Protocol Types (for WebSocket communication)
// =============================================================================

/// Proxy message direction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageDirection {
    /// Message from platform to UGENT
    Inbound,
    /// Message from UGENT to platform
    Outbound,
}

/// Proxy message wrapper - the main message format for WebSocket communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyMessage {
    /// Unique message ID (UUID string for JSON compatibility)
    pub id: String,
    /// Source channel ("line", "wechat", "wecom")
    pub channel: String,
    /// Message direction
    pub direction: MessageDirection,
    /// Conversation ID (user ID, group ID, or room ID)
    pub conversation_id: String,
    /// Sender ID (user ID who sent the message)
    pub sender_id: String,
    /// LINE message content (if available)
    pub message: Option<LineMessageContent>,
    /// Media content (for image/audio/video/file messages)
    pub media: Option<MediaContent>,
    /// Message timestamp (Unix milliseconds)
    pub timestamp: i64,
    /// Reply token (LINE-specific: valid for ~1 minute after webhook)
    pub reply_token: Option<String>,
    /// Quote token (LINE-specific: for quote/reply messages)
    pub quote_token: Option<String>,
    /// Mark-as-read token (LINE-specific: for marking messages as read)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_as_read_token: Option<String>,
    /// Webhook event ID (for deduplication) - always present
    pub webhook_event_id: String,
    /// Source type (user/group/room)
    pub source_type: SourceType,
    /// Sender display name (resolved from contacts cache or LINE API)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
    /// Sender profile picture URL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_picture_url: Option<String>,
}

impl ProxyMessage {
    /// Create a new proxy message from a LINE webhook event
    pub fn from_line_event(event: &WebhookEventBody, _destination: &str) -> Self {
        let source = &event.source;
        let conversation_id = source.conversation_id();
        let sender_id = source.sender_id();

        let (message, media) = Self::extract_message_content(&event.message);

        Self {
            id: Uuid::new_v4().to_string(),
            channel: Channel::Line.to_string(),
            direction: MessageDirection::Inbound,
            conversation_id,
            sender_id,
            message,
            media,
            timestamp: event.timestamp,
            reply_token: event.reply_token.clone(),
            quote_token: event.message.quote_token(),
            mark_as_read_token: event.message.mark_as_read_token(),
            webhook_event_id: event.webhook_event_id.clone(),
            source_type: source.source_type(),
            sender_name: None,
            sender_picture_url: None,
        }
    }

    fn extract_message_content(
        msg: &LineMessage,
    ) -> (Option<LineMessageContent>, Option<MediaContent>) {
        match msg {
            LineMessage::Text(t) => (
                Some(LineMessageContent::Text {
                    id: t.id.clone(),
                    text: t.text.clone(),
                    mention: t.mention.clone(),
                }),
                None,
            ),
            LineMessage::Image(i) => (
                Some(LineMessageContent::Image {
                    id: i.id.clone(),
                    content_provider: i.content_provider.clone(),
                }),
                Some(MediaContent::Image {
                    message_id: i.id.clone(),
                    url: i.content_provider.original_content_url.clone(),
                }),
            ),
            LineMessage::Audio(a) => (
                Some(LineMessageContent::Audio {
                    id: a.id.clone(),
                    duration: a.duration,
                    content_provider: a.content_provider.clone(),
                }),
                Some(MediaContent::Audio {
                    message_id: a.id.clone(),
                    duration_ms: a.duration,
                    format: "m4a".to_string(),
                }),
            ),
            LineMessage::Video(v) => (
                Some(LineMessageContent::Video {
                    id: v.id.clone(),
                    duration: v.duration,
                    content_provider: v.content_provider.clone(),
                }),
                Some(MediaContent::Video {
                    message_id: v.id.clone(),
                    duration_ms: v.duration,
                    format: "mp4".to_string(),
                }),
            ),
            LineMessage::File(f) => (
                Some(LineMessageContent::File {
                    id: f.id.clone(),
                    file_name: f.file_name.clone(),
                    file_size: f.file_size,
                }),
                Some(MediaContent::File {
                    message_id: f.id.clone(),
                    file_name: f.file_name.clone(),
                    size_bytes: f.file_size,
                }),
            ),
            LineMessage::Sticker(s) => (
                Some(LineMessageContent::Sticker {
                    id: s.id.clone(),
                    package_id: s.package_id.clone(),
                    sticker_id: s.sticker_id.clone(),
                    sticker_resource_type: s.sticker_resource_type.clone(),
                }),
                None,
            ),
            LineMessage::Location(l) => (
                Some(LineMessageContent::Location {
                    id: l.id.clone(),
                    title: l.title.clone(),
                    address: l.address.clone(),
                    latitude: l.latitude,
                    longitude: l.longitude,
                }),
                None,
            ),
        }
    }
}

// =============================================================================
// LINE Webhook Event Types
// =============================================================================

/// Root webhook event object from LINE Platform
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebhookEvent {
    /// Bot user ID that received the event
    pub destination: String,
    /// List of events
    pub events: Vec<Event>,
}

/// LINE webhook event types
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event {
    /// Message event
    Message(WebhookEventBody),
    /// Unsend event
    Unsend(UnsendEvent),
    /// Follow event (user added bot)
    Follow(FollowEvent),
    /// Unfollow event (user blocked bot)
    Unfollow(UnfollowEvent),
    /// Join event (bot joined group/room)
    Join(JoinEvent),
    /// Leave event (bot left group/room)
    Leave(LeaveEvent),
    /// Member joined event
    MemberJoined(MemberJoinedEvent),
    /// Member left event
    MemberLeft(MemberLeftEvent),
    /// Postback event
    Postback(PostbackEvent),
    /// Video play complete event
    VideoPlayComplete(VideoPlayCompleteEvent),
    /// Beacon event
    Beacon(BeaconEvent),
    /// Account link event
    AccountLink(AccountLinkEvent),
    /// Things event (LINE Things)
    Things(ThingsEvent),
}

/// Common fields in webhook events
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WebhookEventBody {
    /// Message object
    pub message: LineMessage,
    /// Source of the message
    pub source: Source,
    /// Reply token (valid for ~1 minute)
    #[serde(default)]
    pub reply_token: Option<String>,
    /// Timestamp (Unix milliseconds)
    pub timestamp: i64,
    /// Webhook mode (active/standby)
    pub mode: WebhookMode,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Unique webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
}

/// Webhook mode
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WebhookMode {
    /// Bot is active and can receive/respond
    Active,
    /// Bot is in standby (Extensions enabled)
    Standby,
}

/// Delivery context
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeliveryContext {
    /// Whether this is a redelivery
    #[serde(rename = "isRedelivery")]
    pub is_redelivery: bool,
}

// =============================================================================
// LINE Message Types
// =============================================================================

/// LINE message types received in webhooks
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LineMessage {
    /// Text message
    Text(TextMessage),
    /// Image message
    Image(ImageMessage),
    /// Audio message
    Audio(AudioMessage),
    /// Video message
    Video(VideoMessage),
    /// File message
    File(FileMessage),
    /// Sticker message
    Sticker(StickerMessage),
    /// Location message
    Location(LocationMessage),
}

impl LineMessage {
    /// Get message ID
    pub fn id(&self) -> &str {
        match self {
            LineMessage::Text(m) => &m.id,
            LineMessage::Image(m) => &m.id,
            LineMessage::Audio(m) => &m.id,
            LineMessage::Video(m) => &m.id,
            LineMessage::File(m) => &m.id,
            LineMessage::Sticker(m) => &m.id,
            LineMessage::Location(m) => &m.id,
        }
    }

    /// Get quote token if available
    pub fn quote_token(&self) -> Option<String> {
        match self {
            LineMessage::Text(m) => m.quote_token.clone(),
            LineMessage::Image(m) => m.quote_token.clone(),
            LineMessage::Video(m) => m.quote_token.clone(),
            LineMessage::Sticker(m) => m.quote_token.clone(),
            _ => None,
        }
    }

    /// Get mark-as-read token if available
    pub fn mark_as_read_token(&self) -> Option<String> {
        match self {
            LineMessage::Text(m) => m.mark_as_read_token.clone(),
            LineMessage::Image(m) => m.mark_as_read_token.clone(),
            LineMessage::Audio(m) => m.mark_as_read_token.clone(),
            LineMessage::Video(m) => m.mark_as_read_token.clone(),
            LineMessage::File(m) => m.mark_as_read_token.clone(),
            LineMessage::Sticker(m) => m.mark_as_read_token.clone(),
            LineMessage::Location(_m) => None,
        }
    }
}

/// Text message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TextMessage {
    /// Message ID
    pub id: String,
    /// Message text
    pub text: String,
    /// Mention object (if text contains @mention)
    #[serde(default)]
    pub mention: Option<Mention>,
    /// Quote token (for quote messages)
    #[serde(default, rename = "quoteToken")]
    pub quote_token: Option<String>,
    /// Read token for mark-as-read API
    #[serde(default, rename = "markAsReadToken")]
    pub mark_as_read_token: Option<String>,
}

/// Mention object
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mention {
    /// List of mentioned users
    pub mentionees: Vec<Mentionee>,
}

/// Mentionee object
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mentionee {
    /// Index of the mention in text
    pub index: u32,
    /// Length of the mention text
    pub length: u32,
    /// User ID of mentioned user
    #[serde(default, rename = "userId")]
    pub user_id: Option<String>,
    /// Whether this mention is the bot itself
    #[serde(default, rename = "isSelf")]
    pub is_self: bool,
    /// Type of mentionee ("user" or "all")
    #[serde(default, rename = "mentioneeType")]
    pub mentionee_type: Option<String>,
}

/// Image message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ImageMessage {
    /// Message ID
    pub id: String,
    /// Content provider
    pub content_provider: ContentProvider,
    /// Quote token
    #[serde(default, rename = "quoteToken")]
    pub quote_token: Option<String>,
    /// Read token for mark-as-read API
    #[serde(default, rename = "markAsReadToken")]
    pub mark_as_read_token: Option<String>,
}

/// Audio message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AudioMessage {
    /// Message ID
    pub id: String,
    /// Duration in milliseconds
    pub duration: i64,
    /// Content provider
    pub content_provider: ContentProvider,
    /// Read token for mark-as-read API
    #[serde(default, rename = "markAsReadToken")]
    pub mark_as_read_token: Option<String>,
}

/// Video message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoMessage {
    /// Message ID
    pub id: String,
    /// Duration in milliseconds
    pub duration: i64,
    /// Content provider
    pub content_provider: ContentProvider,
    /// Quote token
    #[serde(default, rename = "quoteToken")]
    pub quote_token: Option<String>,
    /// Read token for mark-as-read API
    #[serde(default, rename = "markAsReadToken")]
    pub mark_as_read_token: Option<String>,
}

/// File message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FileMessage {
    /// Message ID
    pub id: String,
    /// File name
    pub file_name: String,
    /// File size in bytes
    pub file_size: i64,
    /// Read token for mark-as-read API
    #[serde(default, rename = "markAsReadToken")]
    pub mark_as_read_token: Option<String>,
}

/// Sticker message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StickerMessage {
    /// Message ID
    pub id: String,
    /// Sticker package ID
    pub package_id: String,
    /// Sticker ID
    pub sticker_id: String,
    /// Sticker resource type
    #[serde(default, rename = "stickerResourceType")]
    pub sticker_resource_type: Option<String>,
    /// Keywords for the sticker
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Quote token (for quote messages in groups)
    #[serde(default, rename = "quoteToken")]
    pub quote_token: Option<String>,
    /// Read token for mark-as-read API
    #[serde(default, rename = "markAsReadToken")]
    pub mark_as_read_token: Option<String>,
}

/// Location message
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LocationMessage {
    /// Message ID
    pub id: String,
    /// Location title
    pub title: String,
    /// Location address
    pub address: String,
    /// Latitude
    pub latitude: f64,
    /// Longitude
    pub longitude: f64,
}

/// Content provider
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ContentProvider {
    /// Provider type (line/external)
    #[serde(rename = "type")]
    pub provider_type: String,
    /// Original content URL (for external provider)
    #[serde(default, rename = "originalContentUrl")]
    pub original_content_url: Option<String>,
    /// Preview image URL (for external provider)
    #[serde(default, rename = "previewImageUrl")]
    pub preview_image_url: Option<String>,
}

// =============================================================================
// Source Types
// =============================================================================

/// Source of the message
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// User (P2P chat)
    User {
        #[serde(rename = "userId")]
        user_id: String,
    },
    /// Group chat
    Group {
        #[serde(rename = "groupId")]
        group_id: String,
        #[serde(default, rename = "userId")]
        user_id: Option<String>,
    },
    /// Multi-person chat (room)
    Room {
        #[serde(rename = "roomId")]
        room_id: String,
        #[serde(default, rename = "userId")]
        user_id: Option<String>,
    },
}

/// Source type enum
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceType {
    User,
    Group,
    Room,
}

impl Source {
    /// Get conversation ID (user_id, group_id, or room_id)
    pub fn conversation_id(&self) -> String {
        match self {
            Source::User { user_id } => user_id.clone(),
            Source::Group { group_id, .. } => group_id.clone(),
            Source::Room { room_id, .. } => room_id.clone(),
        }
    }

    /// Get sender user ID if available
    pub fn sender_id(&self) -> String {
        match self {
            Source::User { user_id } => user_id.clone(),
            Source::Group { group_id, user_id } => {
                user_id.clone().unwrap_or_else(|| group_id.clone())
            }
            Source::Room { room_id, user_id } => user_id.clone().unwrap_or_else(|| room_id.clone()),
        }
    }

    /// Get source type
    pub fn source_type(&self) -> SourceType {
        match self {
            Source::User { .. } => SourceType::User,
            Source::Group { .. } => SourceType::Group,
            Source::Room { .. } => SourceType::Room,
        }
    }

    /// Check if this is a group or room chat
    pub fn is_group_chat(&self) -> bool {
        matches!(self, Source::Group { .. } | Source::Room { .. })
    }
}

// =============================================================================
// Other Event Types
// =============================================================================

/// Unsend event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UnsendEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Unsend message ID
    #[serde(rename = "unsendMessageId")]
    pub unsend_message_id: String,
}

/// Follow event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FollowEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Follow details
    #[serde(default, deserialize_with = "deserialize_follow_detail")]
    pub follow: FollowDetail,
}

/// Follow event detail containing isUnblocked flag
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FollowDetail {
    /// Whether the user unblocked the bot (true if re-adding after block)
    #[serde(default)]
    pub is_unblocked: bool,
}

/// Deserialize FollowDetail from either an object or missing/null value
fn deserialize_follow_detail<'de, D>(deserializer: D) -> Result<FollowDetail, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let val = Option::<serde_json::Value>::deserialize(deserializer)?;
    match val {
        Some(serde_json::Value::Object(map)) => {
            let is_unblocked = map
                .get("isUnblocked")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(FollowDetail { is_unblocked })
        }
        _ => Ok(FollowDetail::default()),
    }
}

/// Unfollow event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UnfollowEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
}

/// Join event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JoinEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
}

/// Leave event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LeaveEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
}

/// Member joined event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemberJoinedEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Joined members
    pub joined: Members,
}

/// Member left event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemberLeftEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Left members
    pub left: Members,
}

/// Members list
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Members {
    /// List of members
    pub members: Vec<Member>,
}

/// Member info
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Member {
    /// User ID
    #[serde(rename = "userId")]
    pub user_id: String,
}

/// Postback event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostbackEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Postback data
    pub postback: Postback,
}

/// Postback data
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Postback {
    /// Postback data
    pub data: String,
    /// Postback params (for datetime picker)
    #[serde(default)]
    pub params: Option<PostbackParams>,
}

/// Postback params
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PostbackParams {
    /// Selected date
    #[serde(default)]
    pub date: Option<String>,
    /// Selected time
    #[serde(default)]
    pub time: Option<String>,
    /// Selected datetime
    #[serde(default)]
    pub datetime: Option<String>,
}

/// Video play complete event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoPlayCompleteEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Video play complete info
    #[serde(rename = "videoPlayComplete")]
    pub video_play_complete: VideoPlayComplete,
}

/// Video play complete info
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VideoPlayComplete {
    /// Tracking ID
    #[serde(rename = "trackingId")]
    pub tracking_id: String,
}

/// Beacon event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BeaconEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Beacon info
    pub beacon: Beacon,
}

/// Beacon info
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Beacon {
    /// Hardware ID
    #[serde(rename = "hwid")]
    pub hwid: String,
    /// Beacon type
    #[serde(rename = "type")]
    pub beacon_type: String,
    /// Device message (optional)
    #[serde(default)]
    pub dm: Option<String>,
}

/// Account link event
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AccountLinkEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Link result
    pub link: LinkResult,
}

/// Link result
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LinkResult {
    /// Result (ok/ng)
    pub result: String,
    /// Nonce
    #[serde(default)]
    pub nonce: Option<String>,
}

/// Things event (LINE Things)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThingsEvent {
    /// Source
    pub source: Source,
    /// Timestamp
    pub timestamp: i64,
    /// Webhook mode
    pub mode: WebhookMode,
    /// Webhook event ID
    #[serde(rename = "webhookEventId")]
    pub webhook_event_id: String,
    /// Delivery context
    #[serde(rename = "deliveryContext")]
    pub delivery_context: DeliveryContext,
    /// Reply token
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Things info
    pub things: Things,
}

/// Things info
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Things {
    /// Device ID
    #[serde(rename = "deviceId")]
    pub device_id: String,
    /// Things type
    #[serde(rename = "type")]
    pub things_type: String,
}

// =============================================================================
// Proxy Message Content Types
// =============================================================================

/// LINE message content for proxy messages
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LineMessageContent {
    Text {
        id: String,
        text: String,
        mention: Option<Mention>,
    },
    Image {
        id: String,
        content_provider: ContentProvider,
    },
    Audio {
        id: String,
        duration: i64,
        content_provider: ContentProvider,
    },
    Video {
        id: String,
        duration: i64,
        content_provider: ContentProvider,
    },
    File {
        id: String,
        file_name: String,
        file_size: i64,
    },
    Sticker {
        id: String,
        package_id: String,
        sticker_id: String,
        sticker_resource_type: Option<String>,
    },
    Location {
        id: String,
        title: String,
        address: String,
        latitude: f64,
        longitude: f64,
    },
}

/// Media content for download
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MediaContent {
    Image {
        message_id: String,
        url: Option<String>,
    },
    Audio {
        message_id: String,
        duration_ms: i64,
        format: String,
    },
    Video {
        message_id: String,
        duration_ms: i64,
        format: String,
    },
    File {
        message_id: String,
        file_name: String,
        size_bytes: i64,
    },
}

// =============================================================================
// WebSocket Protocol Types
// =============================================================================

/// Client authentication data for proxy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthData {
    /// Client identifier
    pub client_id: String,
    /// API key for authentication
    pub api_key: String,
}

/// Server capabilities advertised in AuthResult
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    /// Supports delivery acknowledgment via ResponseResult
    pub response_result: bool,
    /// Supports artifact URL staging
    pub artifact_staging: bool,
    /// Supports reply_token -> push fallback
    pub push_fallback: bool,
    /// Supports per-client targeted routing
    pub targeted_routing: bool,
}

impl Default for Capabilities {
    fn default() -> Self {
        Self {
            response_result: true,
            artifact_staging: false, // Phase 2
            push_fallback: true,
            targeted_routing: true, // Implemented: first-response-wins ownership model
        }
    }
}

/// WebSocket protocol message types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsProtocol {
    /// Authentication request from client
    Auth { data: AuthData },
    /// Authentication result from server
    AuthResult {
        success: bool,
        message: String,
        /// Protocol version (currently 2)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        protocol_version: Option<u32>,
        /// Server capabilities
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capabilities: Option<Capabilities>,
    },
    /// Incoming message from LINE
    Message { data: Box<ProxyMessage> },
    /// Response from UGENT client
    Response {
        /// Client request correlation ID (optional)
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        /// Original message ID from ProxyMessage
        original_id: String,
        /// Response text content
        content: String,
        /// Outbound artifacts (files/images)
        #[serde(default)]
        artifacts: Vec<OutboundArtifact>,
    },
    /// Delivery acknowledgment from server to client
    ResponseResult {
        /// Client request correlation ID
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request_id: Option<String>,
        /// Original message ID
        original_id: String,
        /// Whether delivery succeeded
        success: bool,
        /// Error message if failed
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Ping from client
    Ping,
    /// Pong from server
    Pong,
    /// Error message
    Error { code: i32, message: String },
}

/// Outbound artifact (file/image to send)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutboundArtifact {
    /// File name
    pub file_name: String,
    /// Content type (mime type)
    pub content_type: String,
    /// Artifact type
    pub kind: ArtifactKind,
    /// File data (base64 encoded for WebSocket transport)
    pub data: String,
    /// Local file path (if available)
    pub local_path: Option<String>,
}

/// Artifact kind
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArtifactKind {
    Image,
    Audio,
    Video,
    File,
}

// =============================================================================
// Pending Message Tracking
// =============================================================================

/// Pending inbound message awaiting response
#[derive(Debug, Clone)]
pub struct PendingMessage {
    /// Proxy message ID (used as original_id in response)
    pub original_id: String,
    /// LINE conversation ID
    pub conversation_id: String,
    /// LINE reply token (expires in ~1 minute)
    pub reply_token: Option<String>,
    /// When the message was received
    pub received_at: std::time::Instant,
    /// When reply token expires (~60 seconds from received_at)
    pub reply_token_expires_at: Option<std::time::Instant>,
    /// Webhook event ID for deduplication
    pub webhook_event_id: String,
    /// Client that should receive the response (for targeted routing)
    pub client_id: Option<String>,
    /// Mark-as-read token (for auto mark-as-read)
    pub mark_as_read_token: Option<String>,
}

impl PendingMessage {
    /// Create a new pending message from a proxy message
    pub fn from_proxy_message(msg: &ProxyMessage) -> Self {
        let now = std::time::Instant::now();
        let reply_token_expires_at = msg.reply_token.as_ref().map(|_| {
            // LINE reply tokens expire in ~60 seconds
            now + std::time::Duration::from_secs(55) // Use 55s to be safe
        });

        Self {
            original_id: msg.id.clone(),
            conversation_id: msg.conversation_id.clone(),
            reply_token: msg.reply_token.clone(),
            received_at: now,
            reply_token_expires_at,
            webhook_event_id: msg.webhook_event_id.clone(),
            client_id: None, // Will be set when targeted routing is implemented
            mark_as_read_token: msg.mark_as_read_token.clone(),
        }
    }

    /// Check if reply token is still valid
    pub fn is_reply_token_valid(&self) -> bool {
        match self.reply_token_expires_at {
            Some(expires_at) => std::time::Instant::now() < expires_at,
            None => false,
        }
    }

    /// Check if this pending message has expired (for cleanup)
    pub fn is_expired(&self) -> bool {
        // Expire after 5 minutes regardless of reply token
        std::time::Instant::now().duration_since(self.received_at)
            > std::time::Duration::from_secs(300)
    }
}

// =============================================================================
// Conversation Ownership (for targeted routing)
// =============================================================================

/// Conversation ownership binding for targeted client routing
/// When a client responds to a conversation first, it claims ownership
/// and all future messages for that conversation route to that client only.
#[derive(Debug, Clone)]
pub struct ConversationOwnership {
    /// Conversation ID (LINE user/group/room ID)
    pub conversation_id: String,
    /// Client ID that owns this conversation
    pub client_id: String,
    /// When ownership was claimed
    pub claimed_at: std::time::Instant,
    /// Last activity timestamp (for stale detection)
    pub last_activity: std::time::Instant,
}

impl ConversationOwnership {
    /// Create a new ownership binding
    pub fn new(conversation_id: String, client_id: String) -> Self {
        let now = std::time::Instant::now();
        Self {
            conversation_id,
            client_id,
            claimed_at: now,
            last_activity: now,
        }
    }

    /// Update last activity timestamp
    pub fn touch(&mut self) {
        self.last_activity = std::time::Instant::now();
    }

    /// Check if ownership is stale (no activity for 30 minutes)
    pub fn is_stale(&self) -> bool {
        std::time::Instant::now().duration_since(self.last_activity)
            > std::time::Duration::from_secs(1800)
    }
}

// =============================================================================
// LINE API Response Types
// =============================================================================

/// User profile from LINE API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UserProfile {
    /// User ID
    #[serde(rename = "userId")]
    pub user_id: String,
    /// Display name
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Profile image URL
    #[serde(default, rename = "pictureUrl")]
    pub picture_url: Option<String>,
    /// Status message
    #[serde(default, rename = "statusMessage")]
    pub status_message: Option<String>,
    /// Language
    #[serde(default, rename = "language")]
    pub language: Option<String>,
}

/// Bot info from LINE API
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BotInfo {
    /// Bot user ID
    #[serde(rename = "userId")]
    pub user_id: String,
    /// Bot basic ID
    #[serde(rename = "basicId")]
    pub basic_id: String,
    /// Bot premium ID (if any)
    #[serde(default, rename = "premiumId")]
    pub premium_id: Option<String>,
    /// Bot display name
    #[serde(rename = "displayName")]
    pub display_name: String,
    /// Bot picture URL
    #[serde(default, rename = "pictureUrl")]
    pub picture_url: Option<String>,
    /// Chat mode
    #[serde(default)]
    pub chat_mode: Option<ChatMode>,
    /// Mark as read mode
    #[serde(default)]
    pub mark_as_read_mode: Option<MarkAsReadMode>,
}

/// Chat mode
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatMode {
    Bot,
    Chat,
}

/// Mark as read mode
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkAsReadMode {
    Auto,
    Manual,
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_message_webhook() {
        let json = r#"{
            "destination": "U8e742f61d673b39c7fff3cecb7536ef0",
            "events": [{
                "type": "message",
                "message": {
                    "id": "468789577898262530",
                    "type": "text",
                    "text": "Hello, Bot!"
                },
                "webhookEventId": "01H810YECXQQZ37VAXPF6H9E6T",
                "deliveryContext": { "isRedelivery": false },
                "timestamp": 1692251666727,
                "source": {
                    "type": "user",
                    "userId": "U4af4980629..."
                },
                "replyToken": "38ef843bde154d9b91c21320ffd17a0f",
                "mode": "active"
            }]
        }"#;

        let event: WebhookEvent = serde_json::from_str(json).expect("Failed to parse webhook");
        assert_eq!(event.destination, "U8e742f61d673b39c7fff3cecb7536ef0");
        assert_eq!(event.events.len(), 1);
    }

    #[test]
    fn test_parse_mention_message() {
        let json = r#"{
            "id": "444573844083572737",
            "type": "text",
            "text": "@bot Hello!",
            "mention": {
                "mentionees": [{
                    "index": 0,
                    "length": 4,
                    "userId": "U8e742f61d673b39c7fff3cecb7536ef0",
                    "isSelf": true
                }]
            }
        }"#;

        let msg: LineMessage = serde_json::from_str(json).expect("Failed to parse message");
        if let LineMessage::Text(text) = msg {
            assert!(text.mention.is_some());
            let mention = text.mention.unwrap();
            assert_eq!(mention.mentionees.len(), 1);
            assert!(mention.mentionees[0].is_self);
        } else {
            panic!("Expected text message");
        }
    }

    #[test]
    fn test_source_type() {
        let user_source = Source::User {
            user_id: "U123".to_string(),
        };
        assert!(!user_source.is_group_chat());
        assert_eq!(user_source.conversation_id(), "U123");

        let group_source = Source::Group {
            group_id: "C123".to_string(),
            user_id: Some("U456".to_string()),
        };
        assert!(group_source.is_group_chat());
        assert_eq!(group_source.conversation_id(), "C123");
        assert_eq!(group_source.sender_id(), "U456");
    }
}
