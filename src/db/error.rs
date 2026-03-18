//! Database error types

use thiserror::Error;

/// Database operation errors
#[derive(Debug, Error)]
pub enum DbError {
    #[error("Database connection error: {0}")]
    Connection(String),

    #[error("Migration error: {0}")]
    Migration(String),

    #[error("Query error: {0}")]
    Query(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Lock contention timeout")]
    LockTimeout,

    #[error("Unsupported operation: {0}")]
    Unsupported(String),
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Query(e.to_string())
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for DbError {
    fn from(e: sqlx::Error) -> Self {
        DbError::Query(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = DbError::Connection("connection refused".to_string());
        assert_eq!(
            err.to_string(),
            "Database connection error: connection refused"
        );

        let err = DbError::NotFound("user_123".to_string());
        assert_eq!(err.to_string(), "Not found: user_123");
    }

    #[test]
    fn test_from_rusqlite() {
        let err = DbError::from(rusqlite::Error::QueryReturnedNoRows);
        assert!(matches!(err, DbError::Query(_)));
    }
}
