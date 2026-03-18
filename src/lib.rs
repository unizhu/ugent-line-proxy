//! ugent-line-proxy
//!
//! LINE Messaging API proxy server for UGENT
//!
//! This proxy server bridges LINE Platform webhooks with local UGENT instances
//! via WebSocket connections. It enables running UGENT behind NAT/firewall
//! while still receiving LINE webhooks on a public VPS.
//!
//! # Architecture
//!
//! ```text
//! LINE Platform → ugent-line-proxy (Public VPS) → WebSocket → UGENT (Local)
//! ```
//!
//! # Features
//!
//! - HMAC-SHA256 signature verification
//! - WebSocket-based real-time messaging
//! - Support for all LINE message types (text, image, audio, video, file, sticker, location)
//! - Group chat and P2P chat support
//! - @mention detection in groups
//! - Media content download proxy
//! - Outbound artifact (file/image) sending
//! - Relationship Management System (RMS) for visibility and control
//!
//! # Quick Start
//!
//! ```bash
//! # Set environment variables
//! export LINE_CHANNEL_SECRET=your_secret
//! export LINE_CHANNEL_ACCESS_TOKEN=your_token
//! export LINE_PROXY_API_KEY=your_api_key
//!
//! # Run the proxy
//! ugent-line-proxy
//! ```

pub mod broker;
pub mod config;
pub mod db;
pub mod error;
pub mod line_api;
pub mod retry;
pub mod rms;
pub mod storage;
pub mod types;
pub mod webhook;
pub mod ws_manager;

pub use broker::MessageBroker;
pub use config::Config;
pub use error::ProxyError;
pub use line_api::LineApiClient;
pub use rms::{RelationshipManagerService, RmsError, RmsStorage};
pub use storage::Storage;
pub use types::*;
pub use ws_manager::WebSocketManager;

// Re-exports for convenience
pub use webhook::handle_webhook;
