//! HMAC-SHA256 signature verification for LINE webhooks
//!
//! LINE Platform signs all webhook requests with HMAC-SHA256 using the Channel Secret.
//! This module provides functions to verify these signatures.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

/// Type alias for HMAC-SHA256
type HmacSha256 = Hmac<Sha256>;

/// Verify LINE webhook signature
///
/// # Arguments
/// * `body` - Raw request body bytes (must not be modified)
/// * `signature` - Base64-encoded signature from x-line-signature header
/// * `channel_secret` - LINE Channel Secret
///
/// # Returns
/// `true` if signature is valid, `false` otherwise
///
/// # Security Notes
/// - Uses constant-time comparison to prevent timing attacks
/// - MUST verify signature BEFORE parsing/modifying the body
/// - The body must be exactly as received from LINE
pub fn verify_signature(body: &[u8], signature: &str, channel_secret: &str) -> bool {
    // Empty secret means verification is disabled
    if channel_secret.is_empty() {
        tracing::warn!("Channel secret is empty, signature verification will fail");
        return false;
    }

    // Decode the base64 signature
    let signature_bytes = match STANDARD.decode(signature) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::error!("Failed to decode signature: {}", e);
            return false;
        }
    };

    // Compute HMAC-SHA256
    let Ok(mut mac) = HmacSha256::new_from_slice(channel_secret.as_bytes()) else {
        tracing::error!("Invalid key length for HMAC");
        return false;
    };
    mac.update(body);
    let result = mac.finalize();
    let computed_bytes = result.into_bytes();

    // Constant-time comparison using subtle crate
    // This prevents timing attacks that could leak signature bytes
    signature_bytes.ct_eq(computed_bytes.as_slice()).into()
}

/// Compute signature for testing
///
/// This is useful for generating test signatures in unit tests.
#[cfg(test)]
pub fn compute_signature(body: &[u8], channel_secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(channel_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(body);
    let result = mac.finalize();
    STANDARD.encode(result.into_bytes())
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test_channel_secret";
    const TEST_BODY: &str = r#"{"destination":"U123","events":[]}"#;

    #[test]
    fn test_verify_valid_signature() {
        let signature = compute_signature(TEST_BODY.as_bytes(), TEST_SECRET);
        assert!(verify_signature(
            TEST_BODY.as_bytes(),
            &signature,
            TEST_SECRET
        ));
    }

    #[test]
    fn test_verify_invalid_signature() {
        let valid_signature = compute_signature(TEST_BODY.as_bytes(), TEST_SECRET);
        let invalid_signature = valid_signature.replace('a', "b");

        // If no replacement happened, try a different approach
        let invalid_signature = if invalid_signature == valid_signature {
            "invalid_signature_xyz".to_string()
        } else {
            invalid_signature
        };

        assert!(!verify_signature(
            TEST_BODY.as_bytes(),
            &invalid_signature,
            TEST_SECRET
        ));
    }

    #[test]
    fn test_verify_modified_body() {
        let signature = compute_signature(TEST_BODY.as_bytes(), TEST_SECRET);
        let modified_body = r#"{"destination":"U456","events":[]}"#;

        assert!(!verify_signature(
            modified_body.as_bytes(),
            &signature,
            TEST_SECRET
        ));
    }

    #[test]
    fn test_verify_empty_secret() {
        let signature = compute_signature(TEST_BODY.as_bytes(), TEST_SECRET);
        assert!(!verify_signature(TEST_BODY.as_bytes(), &signature, ""));
    }

    #[test]
    fn test_verify_empty_signature() {
        assert!(!verify_signature(TEST_BODY.as_bytes(), "", TEST_SECRET));
    }

    #[test]
    fn test_verify_invalid_base64() {
        assert!(!verify_signature(
            TEST_BODY.as_bytes(),
            "not valid base64!!!",
            TEST_SECRET
        ));
    }

    #[test]
    fn test_verify_unicode_body() {
        let unicode_body = r#"{"destination":"U123","events":[{"type":"message","message":{"text":"你好世界 🌍"}}]}"#;
        let signature = compute_signature(unicode_body.as_bytes(), TEST_SECRET);

        assert!(verify_signature(
            unicode_body.as_bytes(),
            &signature,
            TEST_SECRET
        ));
    }

    #[test]
    fn test_verify_large_body() {
        // Test with a larger body
        let large_body = "x".repeat(100_000);
        let signature = compute_signature(large_body.as_bytes(), TEST_SECRET);

        assert!(verify_signature(
            large_body.as_bytes(),
            &signature,
            TEST_SECRET
        ));
    }
}
