//! LINE webhook handling
//!
//! Provides:
//! - HMAC-SHA256 signature verification
//! - Webhook event parsing
//! - Webhook handler implementation

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use tracing::{debug, error, info, warn};

use crate::broker::MessageBroker;
use crate::types::{Event, WebhookEvent, WebhookMode};

pub mod parser;
pub mod signature;

pub use parser::parse_webhook;
pub use signature::verify_signature;

/// Webhook handler error
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    #[error("Invalid signature")]
    InvalidSignature,

    #[error("Missing signature header")]
    MissingSignature,

    #[error("Invalid JSON: {0}")]
    InvalidJson(String),

    #[error("Processing error: {0}")]
    Processing(String),
}

impl IntoResponse for WebhookError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            WebhookError::InvalidSignature => {
                (StatusCode::BAD_REQUEST, "Invalid signature".to_string())
            }
            WebhookError::MissingSignature => {
                (StatusCode::BAD_REQUEST, "Missing signature".to_string())
            }
            WebhookError::InvalidJson(msg) => (StatusCode::BAD_REQUEST, msg),
            WebhookError::Processing(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };
        (status, message).into_response()
    }
}

/// Handle LINE webhook
pub async fn handle_webhook(
    State(broker): State<std::sync::Arc<MessageBroker>>,
    headers: HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    // 1. Extract signature from header
    let signature = headers
        .get("x-line-signature")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| {
            warn!("Missing x-line-signature header");
            WebhookError::MissingSignature
        })?;

    debug!("Received webhook with signature: {}", &signature[..20]);

    // 2. Verify signature (MUST be first, before any parsing)
    if !broker.config.line.skip_signature {
        if !verify_signature(&body, signature, &broker.config.line.channel_secret) {
            warn!("Invalid signature");
            return Err(WebhookError::InvalidSignature);
        }
    } else {
        warn!("Signature verification skipped (testing mode)");
    }

    // 3. Parse webhook event
    let event: WebhookEvent = match parse_webhook(&body) {
        Ok(e) => e,
        Err(e) => {
            error!("Failed to parse webhook: {}", e);
            return Err(WebhookError::InvalidJson(e.to_string()));
        }
    };

    info!(
        "Received webhook: destination={}, events={}",
        event.destination,
        event.events.len()
    );

    // 4. Process each event
    for evt in event.events.iter() {
        if let Err(e) = process_event(&broker, evt, &event.destination).await {
            error!("Failed to process event: {}", e);
            // Continue processing other events even if one fails
        }
    }

    // 5. Return 200 OK immediately (LINE expects quick response)
    Ok(())
}

/// Process a single webhook event
async fn process_event(
    broker: &std::sync::Arc<MessageBroker>,
    event: &Event,
    destination: &str,
) -> Result<(), WebhookError> {
    // Skip standby mode events
    if event.mode() == WebhookMode::Standby {
        debug!("Skipping standby mode event");
        return Ok(());
    }

    // Skip redelivered events if configured
    if event.is_redelivery() && !broker.config.line.process_redeliveries {
        debug!("Skipping redelivered event");
        return Ok(());
    }

    match event {
        Event::Message(msg_event) => {
            // Create proxy message and route to connected clients
            let proxy_msg = crate::types::ProxyMessage::from_line_event(msg_event, destination);

            info!(
                "Routing message: channel={}, conversation={}, sender={}",
                proxy_msg.channel, proxy_msg.conversation_id, proxy_msg.sender_id
            );

            if let Err(e) = broker.send_to_clients(proxy_msg).await {
                error!("Failed to route message: {}", e);
            }
        }
        Event::Follow(follow_event) => {
            info!("User followed bot: {:?}", follow_event.source);
            // Could send a welcome message here
        }
        Event::Unfollow(unfollow_event) => {
            info!("User unfollowed bot: {:?}", unfollow_event.source);
        }
        Event::Join(join_event) => {
            info!("Bot joined group/room: {:?}", join_event.source);
            // Could send a greeting message here
        }
        Event::Leave(leave_event) => {
            info!("Bot left group/room: {:?}", leave_event.source);
        }
        Event::MemberJoined(member_event) => {
            info!("Member joined: {:?}", member_event.source);
        }
        Event::MemberLeft(member_event) => {
            info!("Member left: {:?}", member_event.source);
        }
        Event::Unsend(unsend_event) => {
            debug!("Message unsent: {}", unsend_event.unsend_message_id);
        }
        Event::Postback(postback_event) => {
            info!("Postback received: {}", postback_event.postback.data);
            // Handle postback like a message
            // TODO: Create a special message type for postbacks
        }
        Event::VideoPlayComplete(video_event) => {
            debug!(
                "Video play complete: {}",
                video_event.video_play_complete.tracking_id
            );
        }
        Event::Beacon(beacon_event) => {
            info!("Beacon event: hwid={}", beacon_event.beacon.hwid);
        }
        Event::AccountLink(link_event) => {
            info!("Account link: result={}", link_event.link.result);
        }
        Event::Things(things_event) => {
            info!("Things event: device={}", things_event.things.device_id);
        }
    }

    Ok(())
}

/// Extension trait for Event to get mode
trait EventExt {
    fn mode(&self) -> WebhookMode;
    fn is_redelivery(&self) -> bool;
}

impl EventExt for Event {
    fn mode(&self) -> WebhookMode {
        match self {
            Event::Message(e) => e.mode,
            Event::Unsend(e) => e.mode,
            Event::Follow(e) => e.mode,
            Event::Unfollow(e) => e.mode,
            Event::Join(e) => e.mode,
            Event::Leave(e) => e.mode,
            Event::MemberJoined(e) => e.mode,
            Event::MemberLeft(e) => e.mode,
            Event::Postback(e) => e.mode,
            Event::VideoPlayComplete(e) => e.mode,
            Event::Beacon(e) => e.mode,
            Event::AccountLink(e) => e.mode,
            Event::Things(e) => e.mode,
        }
    }

    fn is_redelivery(&self) -> bool {
        match self {
            Event::Message(e) => e.delivery_context.is_redelivery,
            Event::Unsend(e) => e.delivery_context.is_redelivery,
            Event::Follow(e) => e.delivery_context.is_redelivery,
            Event::Unfollow(e) => e.delivery_context.is_redelivery,
            Event::Join(e) => e.delivery_context.is_redelivery,
            Event::Leave(e) => e.delivery_context.is_redelivery,
            Event::MemberJoined(e) => e.delivery_context.is_redelivery,
            Event::MemberLeft(e) => e.delivery_context.is_redelivery,
            Event::Postback(e) => e.delivery_context.is_redelivery,
            Event::VideoPlayComplete(e) => e.delivery_context.is_redelivery,
            Event::Beacon(e) => e.delivery_context.is_redelivery,
            Event::AccountLink(e) => e.delivery_context.is_redelivery,
            Event::Things(e) => e.delivery_context.is_redelivery,
        }
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn test_event_ext() {
        // This is more of a compile-time check
        // The trait implementation should work for all event types
    }
}
