# UGENT-LINE-PROXY Relationship Management System

**Enhancement Plan v2.0**  
**Date**: 2026-03-11  
**Status**: Updated with Latest Crate Versions & Rust 2024 Best Practices

---

## 0. Latest Crate Versions (2026-03)

| Crate | Current | Latest | Notes |
|-------|---------|--------|-------|
| **Rust Edition** | 2021 | **2024** | Rust 1.85+ - async closures, RPIT lifetime capture |
| **tokio** | 1.x | **1.50** | Multi-threaded async runtime |
| **axum** | 0.7.x | **0.8.x** | Path syntax changed `/:id` → `/{id}` |
| **tower** | 0.4.x | **0.5.x** | Service abstraction layer |
| **tower-http** | 0.5.x | **0.6.8** | HTTP middleware (trace, cors, compression) |
| **reqwest** | 0.12.x | **0.12.x** | HTTP client with connection pooling |
| **serde** | 1.0.x | **1.0.x** | Serialization framework |
| **serde_json** | 1.0.x | **1.0.149** | JSON with new Żmij float algorithm |
| **rusqlite** | 0.31.x | **0.38.0** | SQLite bindings (statement cache optional now) |
| **tokio-tungstenite** | 0.24.x | **0.28.0** | WebSocket async |
| **subtle** | 2.6.x | **2.6.x** | Constant-time crypto comparison |
| **thiserror** | 1.0.x | **2.0.x** | Error derive macros |
| **tracing** | 0.1.x | **0.1.x** | Structured logging |

### Rust 2024 Edition Key Changes

```rust
// 1. Async Closures (stabilized!)
let fetch_data = async || {
    client.get(url).send().await?.text().await
};

// 2. RPIT Lifetime Capture - impl Trait auto-captures lifetimes
fn get_entity(&self, id: &str) -> impl Future<Output = Result<Entity>> {
    // Lifetime auto-captured in Rust 2024
    async move { ... }
}

// 3. use<...> syntax for explicit lifetime control
fn selective_capture<'a, 'b>(x: &'a str, y: &'b str) 
    -> impl Iterator<Item = &'a str> + use<'a> {
    // Only capture 'a, not 'b
}

// 4. gen keyword reserved (future generators)
```

### Axum 0.8 Breaking Changes

```rust
// OLD (0.7.x)
Router::new().route("/users/:id", get(user_handler))

// NEW (0.8.x)
Router::new().route("/users/{id}", get(user_handler))

// Wildcard
// OLD: /files/*path
// NEW: /files/{*path}
```

---

## 1. Problem Statement

Current ugent-line-proxy uses "first-response-wins" ownership model for routing LINE messages to UGENT clients. However:

- No visibility into current dispatch rules and relationships
- No way to view LINE contacts/groups and their assigned UGENT clients
- No CLI or API to inspect or modify relationships
- Manual intervention requires database access

**Goal**: Build a Relationship Management System (RMS) that provides:

1. Visibility into current state (clients, conversations, ownership)
2. CLI tool for status checking and relationship management
3. HTTP API for web UI integration
4. Manual override capabilities for routing rules

---

## 2. Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                        ugent-line-proxy                              │
├─────────────────────────────────────────────────────────────────────┤
│                                                                      │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────┐   │
│  │  CLI Tool    │    │   HTTP API   │    │    Webhook Handler   │   │
│  │  (rms-cli)   │    │  (/api/rms)  │    │                      │   │
│  └──────┬───────┘    └──────┬───────┘    └──────────┬───────────┘   │
│         │                   │                       │                │
│         └───────────────────┼───────────────────────┘                │
│                             │                                        │
│                             ▼                                        │
│              ┌──────────────────────────────┐                        │
│              │   RelationshipManagerService │                        │
│              │   (Core Business Logic)      │                        │
│              │                              │                        │
│              │  - get_relationships()       │                        │
│              │  - set_relationship()        │                        │
│              │  - remove_relationship()     │                        │
│              │  - get_clients()             │                        │
│              │  - get_conversations()       │                        │
│              │  - get_dispatch_rules()      │                        │
│              │  - sync_with_runtime()       │                        │
│              └──────────────┬───────────────┘                        │
│                             │                                        │
│         ┌───────────────────┼───────────────────┐                    │
│         │                   │                   │                    │
│         ▼                   ▼                   ▼                    │
│  ┌─────────────┐    ┌─────────────┐    ┌─────────────────┐           │
│  │  Storage    │    │ WSManager   │    │  LineApiClient  │           │
│  │  (SQLite)   │    │ (Runtime)   │    │  (LINE Profile) │           │
│  └─────────────┘    └─────────────┘    └─────────────────┘           │
│                                                                      │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 3. Data Model

### 3.1 Core Entities

```rust
/// LINE Entity Types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEntityType {
    User,      // Individual user (1:1 chat)
    Group,     // LINE group
    Room,      // LINE room (multi-person without group)
}

/// LINE Entity (Contact/Group/Room)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineEntity {
    pub id: String,              // LINE ID (Uxxx, Rxxx, Cxxx)
    pub entity_type: LineEntityType,
    pub display_name: Option<String>,  // From LINE API or cached
    pub picture_url: Option<String>,
    pub last_message_at: Option<i64>,  // Unix timestamp
    pub created_at: i64,
    pub updated_at: i64,
}

/// UGENT Client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UgentClient {
    pub client_id: String,       // WebSocket client ID
    pub connected_at: i64,
    pub last_activity: i64,
    pub is_connected: bool,
    pub metadata: Option<serde_json::Value>,  // Client-provided metadata
}

/// Relationship (Routing Rule)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relationship {
    pub id: i64,                 // Auto-increment ID
    pub line_entity_id: String,  // LINE user/group/room ID
    pub entity_type: LineEntityType,
    pub client_id: String,       // Assigned UGENT client
    pub priority: i32,           // For future multi-client support
    pub is_manual: bool,         // true = manually set, false = auto-detected
    pub created_at: i64,
    pub updated_at: i64,
    pub notes: Option<String>,   // Admin notes
}

/// Dispatch Rule (Computed from relationships + runtime state)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchRule {
    pub conversation_id: String,
    pub entity_type: LineEntityType,
    pub assigned_client: Option<String>,
    pub assigned_client_connected: bool,
    pub is_manual: bool,
    pub last_routed_at: Option<i64>,
    pub message_count: i64,
}
```

### 3.2 Database Schema Extensions

```sql
-- LINE entities (contacts, groups, rooms)
CREATE TABLE IF NOT EXISTS line_entities (
    id TEXT PRIMARY KEY,              -- LINE ID
    entity_type TEXT NOT NULL,        -- 'user', 'group', 'room'
    display_name TEXT,
    picture_url TEXT,
    last_message_at INTEGER,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

-- Manual/override relationships
CREATE TABLE IF NOT EXISTS relationships (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    line_entity_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    client_id TEXT NOT NULL,
    priority INTEGER DEFAULT 0,
    is_manual INTEGER DEFAULT 1,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    notes TEXT,
    FOREIGN KEY (line_entity_id) REFERENCES line_entities(id),
    UNIQUE(line_entity_id)
);

-- Dispatch history (for analytics)
CREATE TABLE IF NOT EXISTS dispatch_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    message_id TEXT,
    dispatched_at INTEGER NOT NULL,
    success INTEGER DEFAULT 1
);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_relationships_client ON relationships(client_id);
CREATE INDEX IF NOT EXISTS idx_relationships_entity ON relationships(line_entity_id);
CREATE INDEX IF NOT EXISTS idx_dispatch_conversation ON dispatch_history(conversation_id);
CREATE INDEX IF NOT EXISTS idx_dispatch_time ON dispatch_history(dispatched_at);
```

---

## 4. Service Layer Design

### 4.1 RelationshipManagerService

```rust
// src/rms/mod.rs
pub mod service;
pub mod types;
pub mod api;
pub mod cli;

// src/rms/service.rs
use std::sync::Arc;
use tokio::sync::Mutex;
use crate::storage::Storage;
use crate::ws_manager::WebSocketManager;
use crate::line_api::LineApiClient;

pub struct RelationshipManagerService {
    storage: Arc<Storage>,
    ws_manager: Arc<WebSocketManager>,
    line_client: LineApiClient,
}

impl RelationshipManagerService {
    pub fn new(
        storage: Arc<Storage>,
        ws_manager: Arc<WebSocketManager>,
        line_client: LineApiClient,
    ) -> Self {
        Self { storage, ws_manager, line_client }
    }

    // ========== Query Operations ==========
    
    /// Get all connected UGENT clients
    pub async fn get_clients(&self) -> Result<Vec<ClientInfo>, RmsError>;
    
    /// Get a specific client by ID
    pub async fn get_client(&self, client_id: &str) -> Result<Option<ClientInfo>, RmsError>;
    
    /// Get all LINE entities (contacts, groups, rooms)
    pub async fn get_entities(&self, filter: EntityFilter) -> Result<Vec<LineEntity>, RmsError>;
    
    /// Get entity by ID with profile from LINE API
    pub async fn get_entity(&self, entity_id: &str) -> Result<Option<LineEntity>, RmsError>;
    
    /// Get all relationships
    pub async fn get_relationships(&self) -> Result<Vec<Relationship>, RmsError>;
    
    /// Get relationship for a specific entity
    pub async fn get_relationship(&self, entity_id: &str) -> Result<Option<Relationship>, RmsError>;
    
    /// Get computed dispatch rules (relationships + runtime state)
    pub async fn get_dispatch_rules(&self) -> Result<Vec<DispatchRule>, RmsError>;
    
    /// Get dispatch rule for a specific conversation
    pub async fn get_dispatch_rule(&self, conversation_id: &str) -> Result<Option<DispatchRule>, RmsError>;
    
    /// Get system status summary
    pub async fn get_status(&self) -> Result<SystemStatus, RmsError>;
    
    // ========== Mutation Operations ==========
    
    /// Create or update a relationship (manual override)
    pub async fn set_relationship(
        &self,
        entity_id: &str,
        client_id: &str,
        notes: Option<&str>,
    ) -> Result<Relationship, RmsError>;
    
    /// Remove a relationship (revert to auto-routing)
    pub async fn remove_relationship(&self, entity_id: &str) -> Result<(), RmsError>;
    
    /// Update entity display name (cache from LINE API)
    pub async fn refresh_entity(&self, entity_id: &str) -> Result<LineEntity, RmsError>;
    
    /// Sync relationships with runtime ownership state
    pub async fn sync_ownership(&self) -> Result<SyncResult, RmsError>;
    
    // ========== Batch Operations ==========
    
    /// Import relationships from JSON
    pub async fn import_relationships(&self, data: &[RelationshipImport]) -> Result<ImportResult, RmsError>;
    
    /// Export all relationships to JSON
    pub async fn export_relationships(&self) -> Result<Vec<Relationship>, RmsError>;
    
    /// Clear all manual relationships
    pub async fn clear_manual_relationships(&self) -> Result<usize, RmsError>;
}

// ========== Supporting Types ==========

#[derive(Debug, Clone)]
pub struct EntityFilter {
    pub entity_type: Option<LineEntityType>,
    pub has_relationship: Option<bool>,
    pub search: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub client_id: String,
    pub connected: bool,
    pub connected_at: Option<i64>,
    pub last_activity: i64,
    pub owned_conversations: usize,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemStatus {
    pub connected_clients: usize,
    pub total_entities: usize,
    pub total_relationships: usize,
    pub manual_relationships: usize,
    pub auto_relationships: usize,
    pub pending_messages: usize,
    pub uptime_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncResult {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
}
```

---

## 5. HTTP API Design

### 5.1 REST Endpoints (Axum 0.8 Syntax)

```rust
use axum::{
    Router,
    routing::{get, post, delete},
};

pub fn rms_routes(rms: Arc<RelationshipManagerService>) -> Router {
    Router::new()
        // Status
        .route("/api/rms/status", get(get_status))
        
        // Clients
        .route("/api/rms/clients", get(get_clients))
        .route("/api/rms/clients/{id}", get(get_client))
        
        // Entities
        .route("/api/rms/entities", get(get_entities))
        .route("/api/rms/entities/{id}", get(get_entity))
        .route("/api/rms/entities/{id}/refresh", post(refresh_entity))
        
        // Relationships
        .route("/api/rms/relationships", get(get_relationships))
        .route("/api/rms/relationships/{entity_id}", get(get_relationship))
        .route("/api/rms/relationships", post(set_relationship))
        .route("/api/rms/relationships/{entity_id}", delete(remove_relationship))
        
        // Dispatch Rules
        .route("/api/rms/dispatch-rules", get(get_dispatch_rules))
        .route("/api/rms/dispatch-rules/{conv_id}", get(get_dispatch_rule))
        
        // Import/Export
        .route("/api/rms/import", post(import_relationships))
        .route("/api/rms/export", get(export_relationships))
        .route("/api/rms/sync", post(sync_ownership))
        
        .with_state(rms)
}
```

### 5.2 API Request/Response Examples

```json
// GET /api/rms/status
{
    "connected_clients": 3,
    "total_entities": 150,
    "total_relationships": 45,
    "manual_relationships": 10,
    "auto_relationships": 35,
    "pending_messages": 2,
    "uptime_secs": 86400
}

// GET /api/rms/clients
{
    "clients": [
        {
            "client_id": "ugent-laptop-001",
            "connected": true,
            "connected_at": 1710123456,
            "last_activity": 1710150000,
            "owned_conversations": 15,
            "metadata": {"version": "0.5.0", "platform": "macos"}
        }
    ]
}

// GET /api/rms/entities?has_relationship=true&limit=50
{
    "entities": [
        {
            "id": "U1234567890abcdef",
            "entity_type": "user",
            "display_name": "John Doe",
            "picture_url": "https://...",
            "last_message_at": 1710149000,
            "relationship": {
                "client_id": "ugent-laptop-001",
                "is_manual": true
            }
        }
    ],
    "total": 45,
    "offset": 0,
    "limit": 50
}

// POST /api/rms/relationships
{
    "entity_id": "U1234567890abcdef",
    "client_id": "ugent-desktop-002",
    "notes": "John's primary workstation"
}

// Response
{
    "success": true,
    "relationship": {
        "id": 1,
        "line_entity_id": "U1234567890abcdef",
        "entity_type": "user",
        "client_id": "ugent-desktop-002",
        "is_manual": true,
        "created_at": 1710150000,
        "updated_at": 1710150000,
        "notes": "John's primary workstation"
    }
}

// GET /api/rms/dispatch-rules
{
    "rules": [
        {
            "conversation_id": "U1234567890abcdef",
            "entity_type": "user",
            "assigned_client": "ugent-laptop-001",
            "assigned_client_connected": true,
            "is_manual": false,
            "last_routed_at": 1710149000,
            "message_count": 42
        }
    ]
}
```

---

## 6. CLI Design

### 6.1 Command Structure

```bash
# Binary: rms-cli (or integrated into main binary)

# Status & Overview
rms status                              # Show system status
rms clients                             # List connected clients
rms clients show <client_id>            # Show client details

# Entity Management
rms entities list                       # List all LINE entities
rms entities list --type=user           # Filter by type
rms entities list --assigned            # Only with relationships
rms entities list --unassigned          # Only without relationships
rms entities show <entity_id>           # Show entity details
rms entities refresh <entity_id>        # Refresh from LINE API

# Relationship Management
rms relationships list                  # List all relationships
rms relationships show <entity_id>      # Show relationship
rms relationships set <entity_id> <client_id>  # Create/update
rms relationships set <entity_id> <client_id> --notes "..."
rms relationships remove <entity_id>    # Remove relationship
rms relationships clear                 # Clear all manual

# Dispatch Rules
rms rules list                          # List dispatch rules
rms rules show <conversation_id>        # Show rule for conversation

# Import/Export
rms export > relationships.json         # Export to JSON
rms import < relationships.json         # Import from JSON

# Sync
rms sync                                # Sync with runtime ownership
```

### 6.2 Output Examples

```bash
$ rms status
┌─────────────────────────────────────────────────────┐
│ UGENT-LINE-PROXY Status                             │
├─────────────────────────────────────────────────────┤
│ Connected Clients:     3                            │
│ Total Entities:        150                          │
│ Total Relationships:   45                           │
│   ├─ Manual:           10                           │
│   └─ Auto-detected:    35                           │
│ Pending Messages:      2                            │
│ Uptime:                1d 2h 30m                    │
└─────────────────────────────────────────────────────┘

$ rms clients
┌────────────────────┬───────────┬─────────────────┬───────────┐
│ Client ID          │ Connected │ Last Activity   │ Owned     │
├────────────────────┼───────────┼─────────────────┼───────────┤
│ ugent-laptop-001   │ ✓ Yes     │ 5 mins ago      │ 15        │
│ ugent-desktop-002  │ ✓ Yes     │ 1 hour ago      │ 20        │
│ ugent-mobile-003   │ ✗ No      │ 2 days ago      │ 10        │
└────────────────────┴───────────┴─────────────────┴───────────┘

$ rms entities list --assigned
┌──────────────────────┬──────┬─────────────────┬────────────────────┬───────────┐
│ Entity ID            │ Type │ Display Name    │ Assigned Client    │ Manual?   │
├──────────────────────┼──────┼─────────────────┼────────────────────┼───────────┤
│ U1234567890abcdef    │ user │ John Doe        │ ugent-laptop-001   │ ✓ Yes     │
│ R2345678901bcdef0    │ room │ Project Alpha   │ ugent-desktop-002  │ ✗ Auto    │
│ C3456789012cdef01    │ group│ Team Meeting    │ ugent-laptop-001   │ ✗ Auto    │
└──────────────────────┴──────┴─────────────────┴────────────────────┴───────────┘

$ rms relationships set U1234567890abcdef ugent-desktop-002 --notes "John's workstation"
✓ Relationship created/updated
  Entity:     U1234567890abcdef (user)
  Client:     ugent-desktop-002
  Type:       Manual
  Notes:      John's workstation

$ rms rules list
┌──────────────────────┬────────────────────┬───────────┬────────────┐
│ Conversation         │ Assigned Client    │ Connected │ Route Type │
├──────────────────────┼────────────────────┼───────────┼────────────┤
│ U1234567890abcdef    │ ugent-laptop-001   │ ✓ Yes     │ Manual     │
│ R2345678901bcdef0    │ ugent-desktop-002  │ ✓ Yes     │ Auto       │
│ C3456789012cdef01    │ ugent-laptop-001   │ ✓ Yes     │ Auto       │
│ U9999999999zzzzzz    │ (none)             │ -         │ Broadcast  │
└──────────────────────┴────────────────────┴───────────┴────────────┘
```

---

## 7. Cargo.toml Dependencies (Updated 2026-03)

```toml
[package]
name = "ugent-line-proxy"
version = "0.1.0"
edition = "2024"  # Rust 2024 Edition
rust-version = "1.85"

[dependencies]
# Async Runtime
tokio = { version = "1.50", features = ["full"] }

# Web Framework (Axum 0.8 with new path syntax)
axum = { version = "0.8", features = ["macros", "ws"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["cors", "trace", "compression-gzip", "request-id"] }

# HTTP Client
reqwest = { version = "0.12", features = ["json", "rustls-tls"], default-features = false }

# WebSocket
tokio-tungstenite = { version = "0.28", features = ["rustls-tls-webpki-roots"] }
futures-util = "0.3"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Database
rusqlite = { version = "0.38", features = ["bundled"] }

# Security
subtle = "2.6"

# Error Handling
thiserror = "2.0"
anyhow = "1.0"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# CLI (for rms-cli binary)
clap = { version = "4.5", features = ["derive"] }
tabled = "0.18"
comfy-table = "7.1"

# Time
chrono = { version = "0.4", features = ["serde"] }

# Utils
uuid = { version = "1.7", features = ["v4", "serde"] }

[dev-dependencies]
tokio-test = "0.4"
tempfile = "3.10"

[[bin]]
name = "rms-cli"
path = "src/bin/rms-cli.rs"
```

---

## 8. Integration Points

### 8.1 WebSocket Protocol Extensions

```json
// Client metadata on connect
{
    "type": "authenticate",
    "api_key": "...",
    "metadata": {
        "version": "0.5.0",
        "platform": "macos",
        "hostname": "john-macbook"
    }
}

// Server sends relationship updates to clients
{
    "type": "relationship_changed",
    "entity_id": "U1234567890abcdef",
    "old_client_id": "ugent-laptop-001",
    "new_client_id": "ugent-desktop-002"
}
```

### 8.2 Webhook Integration

- Auto-create/update `line_entities` on message received
- Respect manual relationships over auto-detected ones
- Log dispatch history for analytics

### 8.3 Environment Variables

```bash
# RMS Configuration
RMS_API_ENABLED=true           # Enable HTTP API endpoints
RMS_API_AUTH_REQUIRED=true     # Require API key for RMS API
RMS_CLI_ENABLED=true           # Enable CLI (via stdin commands)
RMS_AUTO_SYNC_INTERVAL=300     # Auto-sync interval in seconds (0=disabled)
```

---

## 9. Implementation Plan

### Phase 1: Core Service (2-3 days)
1. Create `src/rms/` module structure
2. Implement database schema migrations
3. Implement `RelationshipManagerService` core logic
4. Add entity auto-creation on webhook receive
5. Unit tests for service layer

### Phase 2: HTTP API (1-2 days)
1. Create `/api/rms/*` routes with Axum 0.8
2. Implement authentication/authorization
3. Add OpenAPI documentation
4. Integration tests

### Phase 3: CLI Tool (1-2 days)
1. Create `src/bin/rms-cli.rs`
2. Implement all CLI commands with clap 4.5
3. Add pretty table formatting with comfy-table
4. Shell completion scripts

### Phase 4: Integration (1 day)
1. Integrate with WebSocketManager
2. Add relationship override in MessageBroker
3. Update ownership sync logic
4. End-to-end testing

### Phase 5: Web UI Support (Future)
1. WebSocket events for real-time updates
2. Bulk operations API
3. Audit logging

---

## 10. Code Structure

```
ugent-line-proxy/
├── src/
│   ├── rms/
│   │   ├── mod.rs              # Module exports
│   │   ├── service.rs          # Core service logic
│   │   ├── types.rs            # Data types (LineEntity, Relationship, etc.)
│   │   ├── api.rs              # HTTP API handlers
│   │   ├── cli.rs              # CLI command handlers
│   │   └── storage.rs          # Storage operations (extends main storage)
│   ├── storage/
│   │   ├── rms.rs              # RMS-specific storage operations
│   │   └── ...                 # Existing storage modules
│   └── ...
└── src/bin/
    └── rms-cli.rs              # Standalone CLI binary
```

---

## 11. Security Considerations

1. **API Authentication**: Require API key for all RMS endpoints
2. **Authorization**: Consider role-based access (view vs modify)
3. **Audit Logging**: Log all relationship changes
4. **Rate Limiting**: Prevent abuse of refresh/sync operations
5. **Input Validation**: Validate entity IDs, client IDs

---

## 12. Open Questions

1. **Multi-Client Priority**: Should we support multiple clients per entity with priority order?
2. **Fallback Behavior**: When assigned client disconnects, should we:
   - Auto-assign to another client?
   - Broadcast to all?
   - Queue messages?
3. **Bulk Operations**: What batch size limits for import/export?
4. **Web UI**: Should this be a separate project or embedded?

---

## 13. Success Criteria

- [ ] Can view all LINE entities and their relationships via CLI
- [ ] Can view all connected UGENT clients via CLI
- [ ] Can manually assign/reassign relationships via CLI
- [ ] Can view dispatch rules and routing status
- [ ] HTTP API provides same functionality as CLI
- [ ] Manual relationships override auto-detected ones
- [ ] All operations logged for audit
- [ ] Documentation complete
- [ ] All code passes `cargo clippy -- -D warnings`
- [ ] All tests pass with `cargo test`

---

**Next Steps**: Review this plan and provide feedback on:
1. Any missing features or use cases
2. Priority of implementation phases
3. Answers to open questions
4. Any architecture concerns
5. Rust 2024 edition migration strategy
