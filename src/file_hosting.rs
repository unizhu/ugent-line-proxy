//! File hosting service for serving outbound artifacts via signed download URLs.
//!
//! Files are saved to a local directory with UUID-based names. Each file has a
//! `.meta.json` sidecar containing the original filename, content type, size,
//! and creation timestamp. Download URLs are protected by HMAC-SHA256 signatures
//! with configurable TTL-based expiry.
//!
//! # URL Format
//!
//! ```text
//! https://<domain>/download?file=<uuid>&code=<hmac_hex>&expires=<unix_ts>
//! ```
//!
//! # Code Generation
//!
//! ```text
//! code = HMAC-SHA256(key, uuid + ":" + expires_timestamp)
//! ```

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use tokio::fs;
use tracing::{debug, error, info, warn};

type HmacSha256 = Hmac<Sha256>;

/// Request parameters for the download endpoint.
#[derive(Debug, Deserialize)]
pub struct DownloadParams {
    /// UUID filename (without extension)
    pub file: String,
    /// HMAC-SHA256 signature code
    pub code: String,
    /// Expiry timestamp (Unix seconds)
    pub expires: Option<i64>,
}

/// Metadata stored alongside each hosted file.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileMeta {
    /// Original filename
    pub original_name: String,
    /// MIME content type
    pub content_type: String,
    /// File size in bytes
    pub size_bytes: u64,
    /// Unix timestamp when the file was created
    pub created_at: i64,
}

/// Result of saving a file.
pub struct SavedFile {
    /// UUID-based filename
    pub uuid_name: String,
    /// The original filename
    pub original_name: String,
}

/// File hosting service.
#[derive(Debug)]
pub struct FileHostingService {
    /// Base directory for storing files
    storage_path: PathBuf,
    /// Public domain for download URLs
    domain: String,
    /// TTL in minutes
    ttl_mins: u64,
    /// HMAC signing key
    hmac_key: Vec<u8>,
}

impl FileHostingService {
    /// Create a new file hosting service.
    pub fn new(storage_path: PathBuf, domain: String, ttl_mins: u64, encryption_key: &str) -> Self {
        Self {
            storage_path,
            domain,
            ttl_mins,
            hmac_key: encryption_key.as_bytes().to_vec(),
        }
    }

    /// Initialize the storage directory if it doesn't exist.
    pub async fn init_storage(&self) -> std::io::Result<()> {
        if !self.storage_path.exists() {
            info!("Creating file hosting directory: {:?}", self.storage_path);
            fs::create_dir_all(&self.storage_path).await?;
        }
        Ok(())
    }

    /// Save a file from a local path into the hosting directory.
    pub async fn save_file(
        &self,
        source_path: &Path,
        original_name: Option<&str>,
        content_type: Option<&str>,
    ) -> Result<SavedFile, FileHostingError> {
        let content = fs::read(source_path).await?;

        let original_name = original_name.unwrap_or_else(|| {
            source_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown")
        });

        let content_type = content_type.unwrap_or_else(|| guess_mime_type(original_name));

        self.save_bytes(&content, original_name, content_type).await
    }

    /// Save raw bytes into the hosting directory.
    pub async fn save_bytes(
        &self,
        content: &[u8],
        original_name: &str,
        content_type: &str,
    ) -> Result<SavedFile, FileHostingError> {
        let uuid_name = uuid::Uuid::new_v4().to_string();
        let file_path = self.storage_path.join(&uuid_name);
        let meta_path = self.storage_path.join(format!("{uuid_name}.meta.json"));

        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let meta = FileMeta {
            original_name: original_name.to_string(),
            content_type: content_type.to_string(),
            size_bytes: content.len() as u64,
            created_at,
        };

        let meta_json =
            serde_json::to_string_pretty(&meta).map_err(FileHostingError::SerializeMeta)?;

        fs::write(&file_path, content).await?;
        fs::write(&meta_path, meta_json.as_bytes()).await?;

        debug!(
            "Saved file: uuid={}, name={}, size={}",
            uuid_name,
            original_name,
            content.len()
        );

        Ok(SavedFile {
            uuid_name,
            original_name: original_name.to_string(),
        })
    }

    /// Generate a signed download URL for a hosted file.
    pub fn generate_download_url(&self, uuid_name: &str, created_at: Option<i64>) -> String {
        let expires = self.calculate_expires(created_at);
        let code = self.generate_code(uuid_name, expires);

        format!(
            "{}/download?file={}&code={}&expires={}",
            self.domain.trim_end_matches('/'),
            uuid_name,
            code,
            expires
        )
    }

    /// Calculate the expires timestamp based on creation time + TTL.
    fn calculate_expires(&self, created_at: Option<i64>) -> i64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        match created_at {
            Some(ts) => ts + (self.ttl_mins as i64 * 60),
            None => now + (self.ttl_mins as i64 * 60),
        }
    }

    /// Generate HMAC-SHA256 code for a file with given expiry.
    pub fn generate_code(&self, file_id: &str, expires: i64) -> String {
        let mut mac =
            HmacSha256::new_from_slice(&self.hmac_key).expect("HMAC can take key of any size");
        mac.update(format!("{file_id}:{expires}").as_bytes());
        let result = mac.finalize();
        hex_encode(result.into_bytes())
    }

    /// Verify a download request's HMAC code and check expiry.
    pub fn verify_request(&self, file: &str, code: &str, expires: i64) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        if expires <= now {
            debug!(
                "Download link expired: file={}, expires={}, now={}",
                file, expires, now
            );
            return false;
        }

        let expected = self.generate_code(file, expires);
        let result = expected == code;

        if !result {
            warn!("Invalid download code for file={}", file);
        }

        result
    }

    /// Read a hosted file's content and metadata.
    pub async fn read_file(
        &self,
        uuid_name: &str,
    ) -> Result<(Vec<u8>, FileMeta), FileHostingError> {
        let file_path = self.storage_path.join(uuid_name);
        let meta_path = self.storage_path.join(format!("{uuid_name}.meta.json"));

        if !file_path.exists() {
            return Err(FileHostingError::FileNotFound(uuid_name.to_string()));
        }

        let content = fs::read(&file_path).await?;

        let meta = if meta_path.exists() {
            let meta_bytes = fs::read(&meta_path).await?;
            serde_json::from_slice::<FileMeta>(&meta_bytes).map_err(FileHostingError::ParseMeta)?
        } else {
            FileMeta {
                original_name: uuid_name.to_string(),
                content_type: "application/octet-stream".to_string(),
                size_bytes: content.len() as u64,
                created_at: 0,
            }
        };

        Ok((content, meta))
    }

    /// Clean up expired files from the storage directory.
    /// Returns the number of files cleaned up.
    pub async fn cleanup_expired(&self) -> Result<usize, FileHostingError> {
        let mut cleaned = 0;
        let mut entries = fs::read_dir(&self.storage_path).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            if !path.is_file() || path.extension().is_some_and(|ext| ext == "json") {
                continue;
            }

            let uuid_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();

            let meta_path = self.storage_path.join(format!("{uuid_name}.meta.json"));

            let should_delete = if meta_path.exists() {
                let meta_bytes = match fs::read(&meta_path).await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                if let Ok(meta) = serde_json::from_slice::<FileMeta>(&meta_bytes) {
                    let expires = meta.created_at + (self.ttl_mins as i64 * 60);
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    now > expires
                } else {
                    false
                }
            } else {
                false
            };

            if should_delete {
                if let Err(e) = fs::remove_file(&path).await {
                    warn!("Failed to delete expired file {:?}: {}", path, e);
                } else {
                    let _ = fs::remove_file(&meta_path).await;
                    debug!("Cleaned up expired file: {}", uuid_name);
                    cleaned += 1;
                }
            }
        }

        if cleaned > 0 {
            info!("Cleaned up {} expired files", cleaned);
        }

        Ok(cleaned)
    }

    /// Get the storage path reference.
    pub fn storage_path(&self) -> &Path {
        &self.storage_path
    }

    /// Get domain reference.
    pub fn domain(&self) -> &str {
        &self.domain
    }
}

/// Serve a file download request given a file hosting service and params.
/// This is the core logic; broker.rs provides the axum handler wrapper.
pub async fn serve_download(
    service: &FileHostingService,
    file: &str,
    code: &str,
    expires: i64,
) -> Response {
    if !service.verify_request(file, code, expires) {
        return (StatusCode::FORBIDDEN, "Invalid or expired download link").into_response();
    }

    let (content, meta) = match service.read_file(file).await {
        Ok(data) => data,
        Err(FileHostingError::FileNotFound(_)) => {
            return (StatusCode::NOT_FOUND, "File not found").into_response();
        }
        Err(e) => {
            error!("Error reading file {}: {}", file, e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response();
        }
    };

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, meta.content_type.as_str())
        .header(
            header::CONTENT_DISPOSITION,
            format!(
                "attachment; filename=\"{}\"",
                meta.original_name.replace('\"', "\\\"")
            ),
        )
        .header(header::CONTENT_LENGTH, meta.size_bytes)
        .body(Body::from(content))
        .unwrap_or_else(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to build response",
            )
                .into_response()
        });

    debug!("Served file download: {} ({})", file, meta.original_name);
    response
}

/// Errors for file hosting operations.
#[derive(Debug, thiserror::Error)]
pub enum FileHostingError {
    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Failed to serialize metadata: {0}")]
    SerializeMeta(serde_json::Error),

    #[error("Failed to parse metadata: {0}")]
    ParseMeta(serde_json::Error),

    #[error("File hosting not configured")]
    NotConfigured,
}

// Compile-time check: FileHostingError is Send
const _: () = {
    fn _assert_send() {
        fn require<T: Send>() {}
        require::<FileHostingError>();
    }
};

// --- Utility functions ---

/// Encode bytes as lowercase hex string.
fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    let bytes = bytes.as_ref();
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{b:02x}").unwrap();
    }
    s
}

/// Guess MIME type from file extension.
fn guess_mime_type(filename: &str) -> &'static str {
    match filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .as_str()
    {
        "pdf" => "application/pdf",
        "txt" | "md" | "markdown" => "text/plain",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "ogg" | "oga" => "audio/ogg",
        "json" => "application/json",
        "zip" => "application/zip",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        "ppt" | "pptx" => "application/vnd.ms-powerpoint",
        "html" | "htm" => "text/html",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode([0xde, 0xad, 0xbe, 0xef]), "deadbeef");
        assert_eq!(hex_encode([]), "");
    }

    #[test]
    fn test_guess_mime_type() {
        assert_eq!(guess_mime_type("test.pdf"), "application/pdf");
        assert_eq!(guess_mime_type("photo.JPG"), "image/jpeg");
        assert_eq!(guess_mime_type("archive.zip"), "application/zip");
        assert_eq!(guess_mime_type("data.xyz"), "application/octet-stream");
        assert_eq!(guess_mime_type("noext"), "application/octet-stream");
    }

    #[tokio::test]
    async fn test_file_hosting_save_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let service = FileHostingService::new(
            dir.path().to_path_buf(),
            "https://example.com".to_string(),
            60,
            "test_encryption_key_at_least_16_bytes",
        );

        service.init_storage().await.unwrap();

        let saved = service
            .save_bytes(b"hello world", "test.txt", "text/plain")
            .await
            .unwrap();

        assert!(!saved.uuid_name.is_empty());
        assert_eq!(saved.original_name, "test.txt");

        let (content, meta) = service.read_file(&saved.uuid_name).await.unwrap();
        assert_eq!(content, b"hello world");
        assert_eq!(meta.original_name, "test.txt");
        assert_eq!(meta.content_type, "text/plain");
        assert_eq!(meta.size_bytes, 11);
        assert!(meta.created_at > 0);
    }

    #[tokio::test]
    async fn test_download_url_generation_and_verification() {
        let dir = tempfile::tempdir().unwrap();
        let service = FileHostingService::new(
            dir.path().to_path_buf(),
            "https://files.example.com".to_string(),
            60,
            "test_encryption_key_at_least_16_bytes",
        );

        service.init_storage().await.unwrap();

        let saved = service
            .save_bytes(b"file content", "doc.pdf", "application/pdf")
            .await
            .unwrap();

        // Read back to get created_at
        let (_, meta) = service.read_file(&saved.uuid_name).await.unwrap();

        let url = service.generate_download_url(&saved.uuid_name, Some(meta.created_at));

        assert!(url.starts_with("https://files.example.com/download?file="));
        assert!(url.contains(&saved.uuid_name));
        assert!(url.contains("&code="));
        assert!(url.contains("&expires="));

        // Parse URL params manually
        let query_str = url.split('?').nth(1).unwrap();
        let mut file_val = String::new();
        let mut code_val = String::new();
        let mut expires_val: i64 = 0;
        for pair in query_str.split('&') {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap();
            let value = parts.next().unwrap_or("");
            match key {
                "file" => file_val = value.to_string(),
                "code" => code_val = value.to_string(),
                "expires" => expires_val = value.parse().unwrap(),
                _ => {}
            }
        }

        assert!(service.verify_request(&file_val, &code_val, expires_val));

        // Verify wrong code fails
        assert!(!service.verify_request(&file_val, "wrong_code", expires_val));
    }

    #[tokio::test]
    async fn test_expired_link() {
        let dir = tempfile::tempdir().unwrap();
        let service = FileHostingService::new(
            dir.path().to_path_buf(),
            "https://example.com".to_string(),
            60,
            "test_encryption_key_at_least_16_bytes",
        );

        // Generate code with past expiry
        let past_expires = 1_000_000_000i64; // Sep 2001
        let code = service.generate_code("test-uuid", past_expires);

        assert!(!service.verify_request("test-uuid", &code, past_expires));
    }

    #[tokio::test]
    async fn test_cleanup_expired() {
        let dir = tempfile::tempdir().unwrap();
        // Use TTL=1 (1 minute) and check after a brief delay
        // The cleanup checks created_at + ttl*60 < now
        let service = FileHostingService::new(
            dir.path().to_path_buf(),
            "https://example.com".to_string(),
            1,
            "test_encryption_key_at_least_16_bytes",
        );

        service.init_storage().await.unwrap();

        // Save a file
        let saved = service
            .save_bytes(b"to be cleaned", "temp.txt", "text/plain")
            .await
            .unwrap();

        // Manually set created_at to 2 minutes ago so it's expired
        let meta_path = dir.path().join(format!("{}.meta.json", saved.uuid_name));
        let meta_bytes = tokio::fs::read_to_string(&meta_path).await.unwrap();
        let mut meta: FileMeta = serde_json::from_str(&meta_bytes).unwrap();
        meta.created_at -= 120; // 2 minutes ago
        tokio::fs::write(&meta_path, serde_json::to_string_pretty(&meta).unwrap())
            .await
            .unwrap();

        let cleaned = service.cleanup_expired().await.unwrap();
        assert_eq!(cleaned, 1);

        // File should be gone
        assert!(service.read_file(&saved.uuid_name).await.is_err());
    }
}
