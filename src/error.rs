//! Error types for ugent-line-proxy

use thiserror::Error;

/// Main error type
#[derive(Debug, Error)]
pub enum ProxyError {
    #[error("Configuration error: {0}")]
    Config(#[from] crate::config::ConfigError),

    #[error("Webhook error: {0}")]
    Webhook(#[from] crate::webhook::WebhookError),

    #[error("Broker error: {0}")]
    Broker(#[from] crate::broker::BrokerError),

    #[error("LINE API error: {0}")]
    LineApi(#[from] crate::line_api::LineApiError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Server error: {0}")]
    Server(String),
}
