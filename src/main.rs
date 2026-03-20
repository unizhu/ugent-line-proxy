//! ugent-line-proxy - LINE Messaging API Proxy Server
//!
//! A high-performance proxy server that bridges LINE Platform webhooks
//! with local UGENT instances via WebSocket connections.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    http::{Method, header},
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use ugent_line_proxy::{
    broker::{MessageBroker, handle_file_download},
    config::Config,
    file_hosting, handle_webhook,
    rms::{RelationshipManagerService, rms_routes},
    storage::Storage,
    ws_manager::WebSocketManager,
};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present
    let _ = dotenvy::dotenv();

    // Load configuration
    let config = match Config::from_env() {
        Ok(c) => Arc::new(c),
        Err(e) => {
            eprintln!("Configuration error: {e}");
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
    let (ws_manager, storage_arc) = if config.storage.enabled {
        let storage_path = config.storage.path.clone();
        match Storage::with_optional_path(storage_path) {
            Ok(storage) => {
                if let Some(ref path) = config.storage.path {
                    info!("Persistent storage enabled at {:?}", path);
                } else {
                    info!("Persistent storage enabled at ~/.ugent/line-plugin/");
                }
                let storage_arc = Arc::new(storage);
                // Create ws_manager that shares the storage
                let ws_manager = Arc::new(WebSocketManager::new(config.clone()));
                (ws_manager, Some(storage_arc))
            }
            Err(e) => {
                warn!(
                    "Failed to initialize storage: {}. Falling back to in-memory mode.",
                    e
                );
                (Arc::new(WebSocketManager::new(config.clone())), None)
            }
        }
    } else {
        (Arc::new(WebSocketManager::new(config.clone())), None)
    };
    // Create message broker
    let broker = Arc::new(MessageBroker::new(config.clone(), ws_manager.clone()));

    // Create RMS service for relationship management (optional if storage available)
    let line_client =
        ugent_line_proxy::line_api::LineApiClient::new(config.line.channel_access_token.clone());

    // Build main router
    let mut app = Router::new()
        // Health check
        .route("/health", get(health_check))
        // LINE webhook
        .route(&config.line.webhook_path, post(handle_webhook))
        // WebSocket endpoint
        .route(&config.websocket.path, get(websocket_handler));

    // Add file hosting routes if configured
    if config.file_hosting.is_configured() {
        let fh_service = Arc::new(file_hosting::FileHostingService::new(
            config.file_hosting.storage_path.clone(),
            config.file_hosting.domain.clone(),
            config.file_hosting.ttl_mins,
            &config.file_hosting.encryption_key,
        ));

        // Initialize storage directory
        if let Err(e) = fh_service.init_storage().await {
            error!("Failed to initialize file hosting storage: {e}");
        } else {
            info!(
                "File hosting enabled: domain={}, path={:?}, ttl={}min",
                config.file_hosting.domain,
                config.file_hosting.storage_path,
                config.file_hosting.ttl_mins
            );

            // Start background cleanup task
            let cleanup_service = fh_service.clone();
            let cleanup_interval = config.file_hosting.ttl_mins.max(10);
            tokio::spawn(async move {
                let mut interval =
                    tokio::time::interval(std::time::Duration::from_secs(cleanup_interval * 60));
                loop {
                    interval.tick().await;
                    if let Err(e) = cleanup_service.cleanup_expired().await {
                        tracing::warn!("File hosting cleanup error: {e}");
                    }
                }
            });

            app = app.route("/download", get(handle_file_download));
            // Store file hosting service in broker for artifact handling
            broker.set_file_hosting(fh_service);
        }
    } else if config.file_hosting.enabled {
        warn!(
            "File hosting enabled but not fully configured. Set LINE_FILE_HOSTING_DOMAIN and LINE_FILE_HOSTING_ENCRYPTION_KEY (>= 16 chars)"
        );
    }

    // Add RMS routes if storage is available
    if let Some(storage) = storage_arc {
        let rms_service = Arc::new(RelationshipManagerService::new(
            storage,
            ws_manager.clone(),
            line_client,
        ));
        info!("RMS service initialized");
        app = app.nest("/api/rms", rms_routes().with_state(rms_service));
    } else {
        warn!("RMS service disabled - no storage available");
    }

    // Add CORS and state
    let app = app
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
    use ugent_line_proxy::ws_manager::websocket_handler_with_broker as ws_handler;

    ws_handler(
        ws,
        axum::extract::State(broker.ws_manager()),
        axum::extract::ConnectInfo(addr),
        broker,
    )
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
        () = ctrl_c => {},
        () = terminate => {},
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
