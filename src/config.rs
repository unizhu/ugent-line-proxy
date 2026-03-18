//! Configuration management for LINE proxy
//!
//! Handles loading configuration from environment variables and files.

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Configuration errors
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    MissingEnv(&'static str),

    #[error("Invalid bind address: {0}")]
    InvalidBindAddr(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Main configuration structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Server configuration
    pub server: ServerConfig,
    /// LINE API configuration
    pub line: LineConfig,
    /// WebSocket configuration
    pub websocket: WebSocketConfig,
    /// Media cache configuration
    #[serde(default)]
    pub media: MediaConfig,
    /// Logging configuration
    #[serde(default)]
    pub logging: LoggingConfig,
    /// Storage configuration
    #[serde(default)]
    pub storage: StorageConfig,
}

impl Config {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let server = ServerConfig::from_env()?;
        let line = LineConfig::from_env()?;
        let websocket = WebSocketConfig::from_env()?;
        let media = MediaConfig::from_env();
        let logging = LoggingConfig::from_env();
        let storage = StorageConfig::from_env();

        Ok(Self {
            server,
            line,
            websocket,
            media,
            logging,
            storage,
        })
    }

    /// Get the bind address as SocketAddr
    pub fn bind_addr(&self) -> SocketAddr {
        self.server.bind_addr.parse().unwrap_or_else(|_| {
            tracing::warn!("Invalid bind address, using default 0.0.0.0:3000");
            "0.0.0.0:3000".parse().unwrap()
        })
    }

    /// Check if LINE is properly configured
    pub fn is_line_configured(&self) -> bool {
        !self.line.channel_secret.is_empty() && !self.line.channel_access_token.is_empty()
    }
}

/// Server configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Bind address (e.g., "0.0.0.0:3000")
    pub bind_addr: String,
    /// Server name for logging
    #[serde(default = "default_server_name")]
    pub name: String,
    /// Enable TLS
    #[serde(default)]
    pub tls: Option<TlsConfig>,
}

fn default_server_name() -> String {
    "ugent-line-proxy".to_string()
}

impl ServerConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let bind_addr =
            std::env::var("LINE_PROXY_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3000".to_string());

        let name = std::env::var("LINE_PROXY_NAME").unwrap_or_else(|_| default_server_name());

        let tls = if std::env::var("LINE_PROXY_TLS_CERT").is_ok() {
            Some(TlsConfig::from_env()?)
        } else {
            None
        };

        Ok(Self {
            bind_addr,
            name,
            tls,
        })
    }
}

/// TLS configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    /// Path to TLS certificate file
    pub cert_path: PathBuf,
    /// Path to TLS private key file
    pub key_path: PathBuf,
}

impl TlsConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let cert_path = std::env::var("LINE_PROXY_TLS_CERT")
            .map_err(|_| ConfigError::MissingEnv("LINE_PROXY_TLS_CERT"))?
            .into();

        let key_path = std::env::var("LINE_PROXY_TLS_KEY")
            .map_err(|_| ConfigError::MissingEnv("LINE_PROXY_TLS_KEY"))?
            .into();

        Ok(Self {
            cert_path,
            key_path,
        })
    }
}

/// LINE API configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineConfig {
    /// LINE Channel Secret (for signature verification)
    pub channel_secret: String,
    /// LINE Channel Access Token
    pub channel_access_token: String,
    /// Webhook path
    #[serde(default = "default_webhook_path")]
    pub webhook_path: String,
    /// Skip signature verification (for testing only)
    #[serde(default)]
    pub skip_signature: bool,
    /// Process redelivered events
    #[serde(default = "default_true")]
    pub process_redeliveries: bool,
    /// Automatically send typing indicator when messages are received
    #[serde(default = "default_true")]
    pub auto_loading_indicator: bool,
    /// Automatically mark messages as read after response
    #[serde(default = "default_true")]
    pub auto_mark_as_read: bool,
}

fn default_webhook_path() -> String {
    "/line/callback".to_string()
}

fn default_true() -> bool {
    true
}

/// Ensure path starts with /
fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

impl LineConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let channel_secret = std::env::var("LINE_CHANNEL_SECRET").unwrap_or_default();

        let channel_access_token = std::env::var("LINE_CHANNEL_ACCESS_TOKEN").unwrap_or_default();

        let webhook_path =
            std::env::var("LINE_PROXY_WEBHOOK_PATH").unwrap_or_else(|_| default_webhook_path());
        let webhook_path = ensure_leading_slash(&webhook_path);

        let skip_signature = std::env::var("LINE_PROXY_SKIP_SIGNATURE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let process_redeliveries = std::env::var("LINE_PROXY_PROCESS_REDELIVERIES")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let auto_loading_indicator = std::env::var("LINE_AUTO_LOADING_INDICATOR")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let auto_mark_as_read = std::env::var("LINE_AUTO_MARK_AS_READ")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        Ok(Self {
            channel_secret,
            channel_access_token,
            webhook_path,
            skip_signature,
            process_redeliveries,
            auto_loading_indicator,
            auto_mark_as_read,
        })
    }
}

/// WebSocket configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketConfig {
    /// WebSocket path
    #[serde(default = "default_ws_path")]
    pub path: String,
    /// API key for client authentication
    pub api_key: String,
    /// Ping interval in seconds
    #[serde(default = "default_ping_interval")]
    pub ping_interval_secs: u64,
    /// Connection timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Maximum message size in bytes
    #[serde(default = "default_max_message_size")]
    pub max_message_size: usize,
}

fn default_ws_path() -> String {
    "/ws".to_string()
}

fn default_ping_interval() -> u64 {
    30
}

fn default_timeout() -> u64 {
    60
}

fn default_max_message_size() -> usize {
    10 * 1024 * 1024 // 10MB
}

impl WebSocketConfig {
    fn from_env() -> Result<Self, ConfigError> {
        let path = std::env::var("LINE_PROXY_WS_PATH").unwrap_or_else(|_| default_ws_path());
        let path = ensure_leading_slash(&path);

        let api_key = std::env::var("LINE_PROXY_API_KEY").unwrap_or_default();

        let ping_interval_secs = std::env::var("LINE_PROXY_WS_PING_INTERVAL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_ping_interval());

        let timeout_secs = std::env::var("LINE_PROXY_WS_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_timeout());

        let max_message_size = std::env::var("LINE_PROXY_WS_MAX_MESSAGE_SIZE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_max_message_size());

        Ok(Self {
            path,
            api_key,
            ping_interval_secs,
            timeout_secs,
            max_message_size,
        })
    }

    /// Check if API key is configured
    pub fn has_api_key(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// Media cache configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MediaConfig {
    /// Cache directory path
    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,
    /// Maximum cache size in MB
    #[serde(default = "default_cache_size")]
    pub max_size_mb: u64,
    /// Cache TTL in seconds
    #[serde(default = "default_cache_ttl")]
    pub ttl_secs: u64,
}

fn default_cache_dir() -> PathBuf {
    std::env::temp_dir().join("ugent-line-proxy-cache")
}

fn default_cache_size() -> u64 {
    500 // 500MB
}

fn default_cache_ttl() -> u64 {
    3600 // 1 hour
}

impl MediaConfig {
    fn from_env() -> Self {
        let cache_dir = std::env::var("LINE_PROXY_MEDIA_CACHE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| default_cache_dir());

        let max_size_mb = std::env::var("LINE_PROXY_MEDIA_CACHE_MAX_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_cache_size());

        let ttl_secs = std::env::var("LINE_PROXY_MEDIA_CACHE_TTL")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default_cache_ttl());

        Self {
            cache_dir,
            max_size_mb,
            ttl_secs,
        }
    }
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoggingConfig {
    /// Log level
    #[serde(default = "default_log_level")]
    pub level: String,
    /// Log format (json/pretty)
    #[serde(default = "default_log_format")]
    pub format: String,
    /// Log to file
    #[serde(default)]
    pub file: Option<PathBuf>,
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_format() -> String {
    "json".to_string()
}

impl LoggingConfig {
    fn from_env() -> Self {
        let level = std::env::var("LINE_PROXY_LOG_LEVEL").unwrap_or_else(|_| default_log_level());

        let format =
            std::env::var("LINE_PROXY_LOG_FORMAT").unwrap_or_else(|_| default_log_format());

        let file = std::env::var("LINE_PROXY_LOG_FILE").ok().map(PathBuf::from);

        Self {
            level,
            format,
            file,
        }
    }
}

// =============================================================================
// Storage Configuration
// =============================================================================

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StorageConfig {
    /// Enable persistent storage
    #[serde(default)]
    pub enabled: bool,
    /// Storage path (default: ~/.ugent/line-plugin/)
    #[serde(default)]
    pub path: Option<PathBuf>,
}

impl StorageConfig {
    fn from_env() -> Self {
        let enabled = std::env::var("LINE_PROXY_STORAGE_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let path = std::env::var("LINE_PROXY_STORAGE_PATH")
            .ok()
            .map(PathBuf::from);

        Self { enabled, path }
    }
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        // Test that default values are reasonable
        assert_eq!(default_webhook_path(), "/line/callback");
        assert_eq!(default_ws_path(), "/ws");
        assert!(default_true());
        assert_eq!(default_ping_interval(), 30);
        assert_eq!(default_cache_size(), 500);
    }
}
