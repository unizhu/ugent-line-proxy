//! RMS CLI - Relationship Management System Command-Line Interface
//!
//! This binary provides CLI access to manage LINE-to-UGENT relationships.

use std::sync::Arc;

use ugent_line_proxy::{
    config::Config,
    line_api::LineApiClient,
    rms::Cli,
    storage::Storage,
    ws_manager::WebSocketManager,
    RelationshipManagerService,
};

use clap::Parser;

#[tokio::main]
async fn main() {
    // Initialize logging
    tracing_subscriber::fmt::init();

    // Load config
    let config = match Config::from_env() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize storage
    let db_path = config.storage.path.clone();
    let storage = match Storage::with_optional_path(db_path) {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("Failed to initialize storage: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize LINE API client
    let line_client = LineApiClient::new(config.line.channel_access_token.clone());

    // Create a dummy WebSocket manager (for storage access only)
    // Note: CLI doesn't have real WebSocket connections
    let ws_manager = Arc::new(WebSocketManager::new(Arc::clone(&config)));

    // Create RMS service
    let rms = RelationshipManagerService::new(storage, ws_manager, line_client);

    // Parse and run CLI
    let cli = Cli::parse();
    if let Err(e) = ugent_line_proxy::rms::run_with_cli(cli, rms).await {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}
