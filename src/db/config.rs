//! Database and retention configuration

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Database backend type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DbType {
    /// Embedded SQLite database
    #[default]
    Sqlite,
    /// PostgreSQL database server
    Postgres,
}

impl DbType {
    /// Parse from environment variable string
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sqlite" => Some(DbType::Sqlite),
            "postgres" | "postgresql" => Some(DbType::Postgres),
            _ => None,
        }
    }
}

impl std::fmt::Display for DbType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbType::Sqlite => write!(f, "sqlite"),
            DbType::Postgres => write!(f, "postgres"),
        }
    }
}

/// Data retention configuration (which data to persist)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionConfig {
    /// Master switch for data retention
    pub enabled: bool,
    /// Store contact/user data
    pub contacts: bool,
    /// Store message content
    pub messages: bool,
    /// Store group data
    pub groups: bool,
}

impl Default for RetentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            contacts: true,
            messages: true,
            groups: true,
        }
    }
}

impl RetentionConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        let enabled = std::env::var("LINE_PROXY_RETENTION_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        Self {
            enabled,
            contacts: std::env::var("LINE_PROXY_RETENTION_CONTACTS")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            messages: std::env::var("LINE_PROXY_RETENTION_MESSAGES")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
            groups: std::env::var("LINE_PROXY_RETENTION_GROUPS")
                .map(|v| v == "true" || v == "1")
                .unwrap_or(true),
        }
    }
}

/// Retry configuration for message delivery
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Master switch for retry logic
    pub enabled: bool,
    /// Maximum number of retry attempts
    pub max_attempts: u32,
    /// Initial delay between retries
    pub initial_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
    /// TTL for undelivered inbound messages
    pub inbound_ttl: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 5,
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            inbound_ttl: Duration::from_secs(3600),
        }
    }
}

impl RetryConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        let enabled = std::env::var("LINE_PROXY_RETRY_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(true);

        let max_attempts = std::env::var("LINE_PROXY_RETRY_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);

        let initial_delay_ms = std::env::var("LINE_PROXY_RETRY_INITIAL_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1000);

        let max_delay_ms = std::env::var("LINE_PROXY_RETRY_MAX_DELAY_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60_000);

        let inbound_ttl_secs = std::env::var("LINE_PROXY_INBOUND_QUEUE_TTL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3600);

        Self {
            enabled,
            max_attempts,
            initial_delay: Duration::from_millis(initial_delay_ms),
            max_delay: Duration::from_millis(max_delay_ms),
            inbound_ttl: Duration::from_secs(inbound_ttl_secs),
        }
    }
}

/// Top-level data configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataConfig {
    /// Enable persistent storage
    pub enabled: bool,
    /// Database file path (for SQLite; default: ~/.ugent/line-plugin/line-proxy.db)
    pub path: Option<PathBuf>,
    /// Database backend type
    pub db_type: DbType,
    /// PostgreSQL connection URL (required when db_type = postgres)
    pub db_url: Option<String>,
    /// Data retention settings
    pub retention: RetentionConfig,
    /// Retry settings
    pub retry: RetryConfig,
}

impl Default for DataConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: None,
            db_type: DbType::Sqlite,
            db_url: None,
            retention: RetentionConfig::default(),
            retry: RetryConfig::default(),
        }
    }
}

impl DataConfig {
    /// Load from environment variables
    pub fn from_env() -> Self {
        let enabled = std::env::var("LINE_PROXY_STORAGE_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let path = std::env::var("LINE_PROXY_STORAGE_PATH")
            .ok()
            .map(PathBuf::from);

        let db_type = std::env::var("LINE_PROXY_DB_TYPE")
            .ok()
            .and_then(|v| DbType::from_str_opt(&v))
            .unwrap_or(DbType::Sqlite);

        let db_url = std::env::var("LINE_PROXY_DB_URL").ok();

        Self {
            enabled,
            path,
            db_type,
            db_url,
            retention: RetentionConfig::from_env(),
            retry: RetryConfig::from_env(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_type_parsing() {
        assert_eq!(DbType::from_str_opt("sqlite"), Some(DbType::Sqlite));
        assert_eq!(DbType::from_str_opt("postgres"), Some(DbType::Postgres));
        assert_eq!(DbType::from_str_opt("postgresql"), Some(DbType::Postgres));
        assert_eq!(DbType::from_str_opt("invalid"), None);
        assert_eq!(DbType::from_str_opt("SQLite"), Some(DbType::Sqlite));
    }

    #[test]
    fn test_db_type_display() {
        assert_eq!(DbType::Sqlite.to_string(), "sqlite");
        assert_eq!(DbType::Postgres.to_string(), "postgres");
    }

    #[test]
    fn test_retention_config_defaults() {
        let config = RetentionConfig::default();
        assert!(!config.enabled);
        assert!(config.contacts);
        assert!(config.messages);
        assert!(config.groups);
    }

    #[test]
    fn test_retry_config_defaults() {
        let config = RetryConfig::default();
        assert!(config.enabled);
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay, Duration::from_secs(1));
        assert_eq!(config.max_delay, Duration::from_secs(60));
        assert_eq!(config.inbound_ttl, Duration::from_secs(3600));
    }

    #[test]
    fn test_data_config_defaults() {
        let config = DataConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.db_type, DbType::Sqlite);
        assert!(config.db_url.is_none());
        assert!(!config.retention.enabled);
    }

    #[test]
    fn test_data_config_from_env() {
        temp_env::with_var("LINE_PROXY_STORAGE_ENABLED", None::<&str>, || {
            temp_env::with_var("LINE_PROXY_DB_TYPE", None::<&str>, || {
                let config = DataConfig::from_env();
                assert!(!config.enabled);
                assert_eq!(config.db_type, DbType::Sqlite);
            })
        });
    }
}
