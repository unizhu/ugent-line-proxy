//! Database schema migration

/// Schema version history
pub const CURRENT_SCHEMA_VERSION: i64 = 2;

/// Migration plan for v2 (new data retention tables)
pub const V2_MIGRATION_SQLITE: &[&str] = &[
    // contacts table
    "CREATE TABLE IF NOT EXISTS contacts (
        line_user_id TEXT PRIMARY KEY,
        display_name TEXT,
        picture_url TEXT,
        status_message TEXT,
        language TEXT,
        first_seen_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        last_seen_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        last_interacted_at INTEGER,
        is_blocked INTEGER NOT NULL DEFAULT 0,
        is_friend INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)
    )",
    "CREATE INDEX IF NOT EXISTS idx_contacts_display_name ON contacts(display_name)",
    "CREATE INDEX IF NOT EXISTS idx_contacts_last_seen_at ON contacts(last_seen_at)",

    // groups table
    "CREATE TABLE IF NOT EXISTS groups (
        line_group_id TEXT PRIMARY KEY,
        group_name TEXT,
        picture_url TEXT,
        member_count INTEGER,
        first_seen_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        last_message_at INTEGER,
        created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)
    )",
    "CREATE INDEX IF NOT EXISTS idx_groups_last_message_at ON groups(last_message_at)",

    // group_members table
    "CREATE TABLE IF NOT EXISTS group_members (
        line_group_id TEXT NOT NULL,
        line_user_id TEXT NOT NULL,
        joined_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        is_bot INTEGER NOT NULL DEFAULT 0,
        PRIMARY KEY (line_group_id, line_user_id),
        FOREIGN KEY (line_group_id) REFERENCES groups(line_group_id) ON DELETE CASCADE,
        FOREIGN KEY (line_user_id) REFERENCES contacts(line_user_id) ON DELETE CASCADE
    )",

    // messages table
    "CREATE TABLE IF NOT EXISTS messages (
        id TEXT PRIMARY KEY,
        direction TEXT NOT NULL CHECK(direction IN ('inbound', 'outbound')),
        conversation_id TEXT NOT NULL,
        source_type TEXT NOT NULL CHECK(source_type IN ('user', 'group', 'room')),
        sender_id TEXT,
        message_type TEXT NOT NULL,
        text_content TEXT,
        message_json TEXT,
        media_content_json TEXT,
        reply_token TEXT,
        quote_token TEXT,
        webhook_event_id TEXT,
        line_timestamp INTEGER,
        received_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        delivered_at INTEGER,
        delivery_status TEXT NOT NULL DEFAULT 'pending' CHECK(delivery_status IN ('pending', 'delivered', 'failed', 'expired')),
        retry_count INTEGER NOT NULL DEFAULT 0,
        last_retry_at INTEGER,
        error_message TEXT,
        ugent_request_id TEXT,
        ugent_correlation_id TEXT,
        created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000)
    )",
    "CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id)",
    "CREATE INDEX IF NOT EXISTS idx_messages_direction ON messages(direction)",
    "CREATE INDEX IF NOT EXISTS idx_messages_delivery_status ON messages(delivery_status)",
    "CREATE INDEX IF NOT EXISTS idx_messages_received_at ON messages(received_at)",
    "CREATE INDEX IF NOT EXISTS idx_messages_webhook_event_id ON messages(webhook_event_id)",

    // outbound_queue table
    "CREATE TABLE IF NOT EXISTS outbound_queue (
        id TEXT PRIMARY KEY,
        message_id TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'processing', 'completed', 'failed')),
        attempt INTEGER NOT NULL DEFAULT 0,
        max_attempts INTEGER NOT NULL DEFAULT 5,
        next_retry_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        locked_by TEXT,
        locked_at INTEGER,
        last_error TEXT,
        created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        updated_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_outbound_queue_status ON outbound_queue(status)",
    "CREATE INDEX IF NOT EXISTS idx_outbound_queue_next_retry_at ON outbound_queue(next_retry_at)",

    // inbound_queue table
    "CREATE TABLE IF NOT EXISTS inbound_queue (
        id TEXT PRIMARY KEY,
        message_id TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending', 'processing', 'completed', 'expired')),
        expires_at INTEGER NOT NULL,
        locked_by TEXT,
        locked_at INTEGER,
        created_at INTEGER NOT NULL DEFAULT (unixepoch('now') * 1000),
        FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
    )",
    "CREATE INDEX IF NOT EXISTS idx_inbound_queue_status ON inbound_queue(status)",
    "CREATE INDEX IF NOT EXISTS idx_inbound_queue_expires_at ON inbound_queue(expires_at)",

    // schema_version update
    "INSERT OR REPLACE INTO schema_version (version, applied_at) VALUES (2, unixepoch('now') * 1000)",
];

/// PostgreSQL migration for v2
#[cfg(feature = "postgres")]
pub const V2_MIGRATION_POSTGRES: &[&str] = &[
    // contacts table
    r#"CREATE TABLE IF NOT EXISTS contacts (
        line_user_id VARCHAR(255) PRIMARY KEY,
        display_name VARCHAR(255),
        picture_url TEXT,
        status_message TEXT,
        language VARCHAR(10),
        first_seen_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        last_seen_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        last_interacted_at BIGINT,
        is_blocked BOOLEAN NOT NULL DEFAULT FALSE,
        is_friend BOOLEAN NOT NULL DEFAULT FALSE,
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000)
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_contacts_display_name ON contacts(display_name)",
    "CREATE INDEX IF NOT EXISTS idx_contacts_last_seen_at ON contacts(last_seen_at)",
    // groups table
    r#"CREATE TABLE IF NOT EXISTS groups (
        line_group_id VARCHAR(255) PRIMARY KEY,
        group_name VARCHAR(255),
        picture_url TEXT,
        member_count INTEGER,
        first_seen_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        last_message_at BIGINT,
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000)
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_groups_last_message_at ON groups(last_message_at)",
    // group_members table
    r#"CREATE TABLE IF NOT EXISTS group_members (
        line_group_id VARCHAR(255) NOT NULL,
        line_user_id VARCHAR(255) NOT NULL,
        joined_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        is_bot BOOLEAN NOT NULL DEFAULT FALSE,
        PRIMARY KEY (line_group_id, line_user_id),
        FOREIGN KEY (line_group_id) REFERENCES groups(line_group_id) ON DELETE CASCADE,
        FOREIGN KEY (line_user_id) REFERENCES contacts(line_user_id) ON DELETE CASCADE
    )"#,
    // messages table
    r#"CREATE TABLE IF NOT EXISTS messages (
        id VARCHAR(255) PRIMARY KEY,
        direction VARCHAR(10) NOT NULL CHECK(direction IN ('inbound', 'outbound')),
        conversation_id VARCHAR(255) NOT NULL,
        source_type VARCHAR(10) NOT NULL CHECK(source_type IN ('user', 'group', 'room')),
        sender_id VARCHAR(255),
        message_type VARCHAR(50) NOT NULL,
        text_content TEXT,
        message_json TEXT,
        media_content_json TEXT,
        reply_token VARCHAR(255),
        quote_token VARCHAR(255),
        webhook_event_id VARCHAR(255),
        line_timestamp BIGINT,
        received_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        delivered_at BIGINT,
        delivery_status VARCHAR(20) NOT NULL DEFAULT 'pending',
        retry_count INTEGER NOT NULL DEFAULT 0,
        last_retry_at BIGINT,
        error_message TEXT,
        ugent_request_id VARCHAR(255),
        ugent_correlation_id VARCHAR(255),
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000)
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id)",
    "CREATE INDEX IF NOT EXISTS idx_messages_direction ON messages(direction)",
    "CREATE INDEX IF NOT EXISTS idx_messages_delivery_status ON messages(delivery_status)",
    "CREATE INDEX IF NOT EXISTS idx_messages_received_at ON messages(received_at)",
    "CREATE INDEX IF NOT EXISTS idx_messages_webhook_event_id ON messages(webhook_event_id)",
    // outbound_queue table
    r#"CREATE TABLE IF NOT EXISTS outbound_queue (
        id VARCHAR(255) PRIMARY KEY,
        message_id VARCHAR(255) NOT NULL,
        status VARCHAR(20) NOT NULL DEFAULT 'pending',
        attempt INTEGER NOT NULL DEFAULT 0,
        max_attempts INTEGER NOT NULL DEFAULT 5,
        next_retry_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        locked_by VARCHAR(255),
        locked_at BIGINT,
        last_error TEXT,
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        updated_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_outbound_queue_status ON outbound_queue(status)",
    "CREATE INDEX IF NOT EXISTS idx_outbound_queue_next_retry_at ON outbound_queue(next_retry_at)",
    // inbound_queue table
    r#"CREATE TABLE IF NOT EXISTS inbound_queue (
        id VARCHAR(255) PRIMARY KEY,
        message_id VARCHAR(255) NOT NULL,
        status VARCHAR(20) NOT NULL DEFAULT 'pending',
        expires_at BIGINT NOT NULL,
        locked_by VARCHAR(255),
        locked_at BIGINT,
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW())::BIGINT * 1000),
        FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
    )"#,
    "CREATE INDEX IF NOT EXISTS idx_inbound_queue_status ON inbound_queue(status)",
    "CREATE INDEX IF NOT EXISTS idx_inbound_queue_expires_at ON inbound_queue(expires_at)",
    // schema_version update
    r#"INSERT INTO schema_version (version, applied_at) VALUES (2, EXTRACT(EPOCH FROM NOW())::BIGINT * 1000)
       ON CONFLICT (version) DO UPDATE SET applied_at = EXCLUDED.applied_at"#,
];
