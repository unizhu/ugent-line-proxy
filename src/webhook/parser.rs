//! Webhook event parsing
//!
//! Provides functions to parse LINE webhook JSON into typed structures.

use axum::body::Bytes;
use serde_json;
use tracing::debug;

use crate::types::WebhookEvent;

/// Parse webhook body into WebhookEvent
pub fn parse_webhook(body: &Bytes) -> Result<WebhookEvent, ParseError> {
    debug!("Parsing webhook body: {} bytes", body.len());

    // First, try to parse as UTF-8
    let json_str = std::str::from_utf8(body).map_err(|e| ParseError::InvalidUtf8(e.to_string()))?;

    // Then parse as JSON
    let event: WebhookEvent = serde_json::from_str(json_str).map_err(|e| {
        debug!(
            "JSON parse error at line {}, column {}",
            e.line(),
            e.column()
        );
        ParseError::JsonError(e.to_string())
    })?;

    Ok(event)
}

/// Parse error types
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("Invalid UTF-8: {0}")]
    InvalidUtf8(String),

    #[error("JSON parse error: {0}")]
    JsonError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_message() {
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

        let bytes = Bytes::from(json);
        let event = parse_webhook(&bytes).expect("Failed to parse");

        assert_eq!(event.destination, "U8e742f61d673b39c7fff3cecb7536ef0");
        assert_eq!(event.events.len(), 1);
    }

    #[test]
    fn test_parse_invalid_json() {
        let json = r#"{ "invalid": "#;
        let bytes = Bytes::from(json);

        assert!(parse_webhook(&bytes).is_err());
    }

    #[test]
    fn test_parse_invalid_utf8() {
        let invalid_bytes = Bytes::from(vec![0xff, 0xfe, 0xfd]);

        assert!(matches!(
            parse_webhook(&invalid_bytes),
            Err(ParseError::InvalidUtf8(_))
        ));
    }
}
