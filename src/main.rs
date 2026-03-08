//! ugent-line-proxy - LINE Messaging API Proxy Server
//!
//! A high-performance proxy server that bridges LINE Platform webhooks
//! with local UGENT instances via WebSocket connections.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    http::{header, Method},
    routing::{get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use ugent_line_proxy::{
    broker::MessageBroker, config::Config, handle_webhook, ws_manager::WebSocketManager,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present
    let _ = dotenvy::dotenv();

    // Load configuration
    let config = match Config::from_env() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("Configuration error: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize logging
    init_logging(&config);

    info!("Starting ugent-line-proxy v{}", env!("CARGO_PKG_VERSION"));
    info!("Bind address: {}", config.server.bind_addr);
    info!("LINE webhook path: {}", config.line.webhook_path);
    info!("WebSocket path: {}", config.websocket.path);

    // Check LINE configuration
    if !config.is_line_configured() {
        warn!("LINE credentials not configured - webhook verification will fail!");
        warn!("Set LINE_CHANNEL_SECRET and LINE_CHANNEL_ACCESS_TOKEN");
    }

    // Check API key configuration
    if !config.websocket.has_api_key() {
        warn!("No WebSocket API key configured - connections will not be authenticated!");
        warn!("Set LINE_PROXY_API_KEY for production use");
    }

    // Create WebSocket manager
    let ws_manager = Arc::new(WebSocketManager::new(config.clone()));

    // Create message broker
    let broker = Arc::new(MessageBroker::new(config.clone(), ws_manager.clone()));

    // Build router
    let app = Router::new()
        // Health check
        .route("/health", get(health_check))
        // LINE webhook
        .route(&config.line.webhook_path, post(handle_webhook))
        // WebSocket endpoint
        .route(&config.websocket.path, get(websocket_handler))
        // CORS
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]),
        )
        // State
        .with_state(broker);

    // Create TCP listener
    let addr = config.bind_addr();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("Listening on {}", addr);

    // Start server
    info!("Server ready - waiting for connections...");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    info!("Server shutdown complete");
    Ok(())
}

/// Health check endpoint
async fn health_check() -> &'static str {
    "OK"
}

/// WebSocket handler (delegated to ws_manager)
async fn websocket_handler(
    ws: axum::extract::ws::WebSocketUpgrade,
    axum::extract::State(broker): axum::extract::State<Arc<MessageBroker>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<SocketAddr>,
) -> impl axum::response::IntoResponse {
    use ugent_line_proxy::ws_manager::websocket_handler as ws_handler;

    ws_handler(
        ws,
        axum::extract::State(broker.ws_manager()),
        axum::extract::ConnectInfo(addr),
    )
    .await
}

/// Shutdown signal handler
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("Failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Shutdown signal received");
}

/// Initialize logging based on configuration
fn init_logging(config: &Config) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(&config.logging.level));

    if config.logging.format == "json" {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().json())
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(tracing_subscriber::fmt::layer().pretty())
            .init();
    }
}
