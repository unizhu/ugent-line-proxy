//! Relationship Management System (RMS) for LINE Proxy
//!
//! Provides visibility and control over LINE-to-UGENT client relationships:
//! - View connected clients and their owned conversations
//! - View LINE entities (users, groups, rooms)
//! - Manage relationships (manual overrides)
//! - View dispatch rules

pub mod api;
pub mod cli;
pub mod service;
pub mod storage;
pub mod types;

pub use api::rms_routes;
pub use cli::{Cli, run_with_cli};
pub use service::RelationshipManagerService;
pub use storage::RmsStorage;
pub use types::{
    ClientInfo, DispatchRule, EntityFilter, ImportResult, LineEntity, LineEntityType, Relationship,
    RelationshipImport, RmsError, SyncResult, SystemStatus,
};
