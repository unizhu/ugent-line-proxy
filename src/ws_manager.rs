//! WebSocket manager for LINE proxy
//!
//! Manages WebSocket connections to UGENT clients and broadcasts
//! incoming LINE messages to connected clients.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    extract::{
        ConnectInfo, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    response::IntoResponse,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use parking_lot::RwLock;
use tokio::{sync::mpsc, task::JoinHandle};
use tracing::{debug, error, info, warn};

use crate::broker::MessageBroker;
use crate::config::Config;
use crate::storage::{METRIC_OWNERSHIP_CLAIMS, METRIC_OWNERSHIP_RELEASES, MetricsStore, Storage};
use crate::types::{AuthData, ClientInfo, WsProtocol};

/// WebSocket manager
pub struct WebSocketManager {
    /// Connected clients
    clients: DashMap<String, mpsc::Sender<WsProtocol>>,
    /// Client info storage
    client_infos: RwLock<HashMap<String, ClientInfo>>,
    /// Client count
    client_count: AtomicUsize,
    /// Configuration
    config: Arc<Config>,
    /// Reply token to client mapping
    reply_token_map: RwLock<HashMap<String, String>>,
    /// Conversation ownership: conversation_id -> client_id
    conversation_owners: RwLock<HashMap<String, String>>,
    /// Client conversations: client_id -> Set<conversation_id>
    client_conversations: RwLock<HashMap<String, HashSet<String>>>,
    /// Persistent storage (optional)
    storage: Option<Arc<Storage>>,
}

impl std::fmt::Debug for WebSocketManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebSocketManager")
            .field("client_count", &self.client_count.load(Ordering::Relaxed))
            .field("config", &self.config)
            .field("reply_token_map_len", &self.reply_token_map.read().len())
            .field(
                "conversation_owners_len",
                &self.conversation_owners.read().len(),
            )
            .finish()
    }
}

impl WebSocketManager {
    /// Create a new WebSocket manager
    pub fn new(config: Arc<Config>) -> Self {
        Self {
            clients: DashMap::new(),
            client_infos: RwLock::new(HashMap::new()),
            client_count: AtomicUsize::new(0),
            config,
            reply_token_map: RwLock::new(HashMap::new()),
            conversation_owners: RwLock::new(HashMap::new()),
            client_conversations: RwLock::new(HashMap::new()),
            storage: None,
        }
    }

    /// Create a new WebSocket manager with persistent storage
    pub fn with_storage(config: Arc<Config>, storage: Storage) -> Self {
        Self {
            clients: DashMap::new(),
            client_infos: RwLock::new(HashMap::new()),
            client_count: AtomicUsize::new(0),
            config,
            reply_token_map: RwLock::new(HashMap::new()),
            conversation_owners: RwLock::new(HashMap::new()),
            client_conversations: RwLock::new(HashMap::new()),
            storage: Some(Arc::new(storage)),
        }
    }

    /// Get the storage reference (if enabled)
    pub fn storage(&self) -> Option<&Storage> {
        self.storage.as_ref().map(|s| s.as_ref())
    }

    /// Get the number of connected clients
    pub fn client_count(&self) -> usize {
        self.client_count.load(Ordering::Relaxed)
    }

    /// Check if any clients are connected
    pub fn has_clients(&self) -> bool {
        self.client_count() > 0
    }

    /// Register a reply token to client mapping
    pub fn register_reply_token(&self, reply_token: &str, client_id: &str) {
        let mut map = self.reply_token_map.write();
        map.insert(reply_token.to_string(), client_id.to_string());
    }

    /// Get client ID for a reply token
    pub fn get_client_for_reply_token(&self, reply_token: &str) -> Option<String> {
        let map = self.reply_token_map.read();
        map.get(reply_token).cloned()
    }

    /// Get list of connected client IDs
    pub fn get_connected_client_ids(&self) -> Vec<String> {
        self.clients
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Broadcast a message to all connected clients
    pub async fn broadcast(&self, message: WsProtocol) -> Result<(), BroadcastError> {
        if self.clients.is_empty() {
            warn!("No clients connected to broadcast to");
            return Err(BroadcastError::NoClients);
        }

        let mut failed_count = 0;
        let total_clients = self.clients.len();

        for entry in self.clients.iter() {
            let client_id = entry.key().clone();
            let tx = entry.value().clone();
            if tx.send(message.clone()).await.is_err() {
                warn!("Failed to send message to client: {}", client_id);
                failed_count += 1;
            }
        }

        if failed_count == total_clients {
            error!("All clients failed to receive message");
            return Err(BroadcastError::AllFailed);
        }

        if failed_count > 0 {
            warn!(
                "Broadcast completed with {} failures out of {} clients",
                failed_count,
                self.clients.len()
            );
        }

        Ok(())
    }

    /// Send a message to a specific client
    pub async fn send_to(&self, client_id: &str, message: WsProtocol) -> Result<(), SendError> {
        if let Some(entry) = self.clients.get(client_id) {
            let tx = entry.value().clone();
            tx.send(message)
                .await
                .map_err(|_| SendError::ClientDisconnected)?;
            Ok(())
        } else {
            Err(SendError::ClientNotFound)
        }
    }

    /// Add a client
    fn add_client(&self, client_id: String, info: ClientInfo, tx: mpsc::Sender<WsProtocol>) {
        self.clients.insert(client_id.clone(), tx);
        self.client_infos.write().insert(client_id, info);
        self.client_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Remove a client and release all conversations owned by it
    fn remove_client(&self, client_id: &str) {
        if self.clients.remove(client_id).is_some() {
            self.client_infos.write().remove(client_id);
            self.client_count.fetch_sub(1, Ordering::Relaxed);

            // Release all conversations owned by this client
            self.release_client_conversations(client_id);

            info!("Client disconnected: {}", client_id);
        }
    }

    // =========================================================================
    // Conversation Ownership Methods (for targeted routing)
    // =========================================================================

    /// Claim ownership of a conversation for a client.
    /// Returns true if claim succeeded, false if already owned by another client.
    /// If the same client claims again, ownership is refreshed (returns true).
    pub fn claim_conversation(&self, conversation_id: &str, client_id: &str) -> bool {
        let mut owners = self.conversation_owners.write();

        if let Some(existing_owner) = owners.get(conversation_id)
            && existing_owner != client_id
        {
            // Already owned by another client
            debug!(
                "Conversation {} already owned by {}, cannot claim for {}",
                conversation_id, existing_owner, client_id
            );
            return false;
        }

        // Claim or refresh ownership
        owners.insert(conversation_id.to_string(), client_id.to_string());

        // Track in client's conversation set
        let mut client_convs = self.client_conversations.write();
        client_convs
            .entry(client_id.to_string())
            .or_default()
            .insert(conversation_id.to_string());

        // Persist to storage if enabled
        if let Some(ref storage) = self.storage {
            if let Err(e) = storage.ownership().claim(conversation_id, client_id) {
                warn!("Failed to persist ownership claim: {}", e);
            }
            // Record metric
            if let Err(e) = storage.metrics().increment(METRIC_OWNERSHIP_CLAIMS) {
                warn!("Failed to record ownership claim metric: {}", e);
            }
        }

        info!(
            "Client {} claimed ownership of conversation {}",
            client_id, conversation_id
        );
        true
    }

    /// Get the client that owns a conversation.
    /// Returns None if no owner exists or owner is stale.
    pub fn get_conversation_owner(&self, conversation_id: &str) -> Option<String> {
        let owners = self.conversation_owners.read();
        owners.get(conversation_id).cloned()
    }

    /// Check if a specific client owns a conversation
    pub fn is_conversation_owner(&self, conversation_id: &str, client_id: &str) -> bool {
        let owners = self.conversation_owners.read();
        owners
            .get(conversation_id)
            .map(|o| o == client_id)
            .unwrap_or(false)
    }

    /// Release all conversations owned by a client (on disconnect).
    /// Returns the number of conversations released.
    pub fn release_client_conversations(&self, client_id: &str) -> usize {
        let mut client_convs = self.client_conversations.write();

        if let Some(convs) = client_convs.remove(client_id) {
            let mut owners = self.conversation_owners.write();
            for conv_id in &convs {
                owners.remove(conv_id);
            }

            // Persist to storage if enabled
            if let Some(ref storage) = self.storage {
                if let Err(e) = storage.ownership().release_by_client(client_id) {
                    warn!("Failed to persist ownership release: {}", e);
                }
                // Record metric
                if let Err(e) = storage.metrics().increment(METRIC_OWNERSHIP_RELEASES) {
                    warn!("Failed to record ownership release metric: {}", e);
                }
            }

            info!(
                "Released {} conversations owned by client {}",
                convs.len(),
                client_id
            );
            convs.len()
        } else {
            0
        }
    }

    /// Get the number of conversations with owners
    pub fn owned_conversation_count(&self) -> usize {
        self.conversation_owners.read().len()
    }

    /// Get conversations owned by a specific client
    pub fn get_client_conversations(&self, client_id: &str) -> Vec<String> {
        self.client_conversations
            .read()
            .get(client_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Load ownership state from persistent storage
    /// Call this on startup to restore ownership after restart
    pub fn load_ownership_from_storage(&self) -> Result<usize, crate::storage::StorageError> {
        let storage = self.storage.as_ref().ok_or_else(|| {
            crate::storage::StorageError::InvalidPath("Storage not enabled".into())
        })?;

        let records = storage.ownership().get_all()?;
        let mut owners = self.conversation_owners.write();
        let mut client_convs = self.client_conversations.write();

        let count = records.len();
        for record in records {
            owners.insert(record.conversation_id.clone(), record.client_id.clone());
            client_convs
                .entry(record.client_id)
                .or_default()
                .insert(record.conversation_id);
        }

        info!("Loaded {} ownership records from storage", count);
        Ok(count)
    }

    /// Get the metrics store if storage is enabled
    pub fn metrics(&self) -> Option<&MetricsStore> {
        self.storage.as_ref().map(|s| s.metrics())
    }

    // =========================================================================
    // RMS Integration Methods
    // =========================================================================

    /// Check if a client is currently connected
    pub fn is_client_connected(&self, client_id: &str) -> bool {
        self.clients.contains_key(client_id)
    }

    /// Get the time when a client connected (if connected)
    pub fn get_client_connected_time(&self, client_id: &str) -> Option<i64> {
        let infos = self.client_infos.read();
        infos.get(client_id).map(|info| {
            // Calculate seconds since connection
            let elapsed = info.connected_at.elapsed();
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            now - elapsed.as_secs() as i64
        })
    }

    /// Get client metadata (if available)
    pub fn get_client_metadata(&self, _client_id: &str) -> Option<serde_json::Value> {
        // Currently not storing metadata in client_infos
        None
    }

    /// Set conversation owner (for RMS manual override)
    pub fn set_conversation_owner(&self, conversation_id: &str, client_id: &str) {
        let mut owners = self.conversation_owners.write();
        owners.insert(conversation_id.to_string(), client_id.to_string());

        let mut client_convs = self.client_conversations.write();
        client_convs
            .entry(client_id.to_string())
            .or_default()
            .insert(conversation_id.to_string());

        info!(
            "Set conversation {} owner to {} (manual)",
            conversation_id, client_id
        );
    }

    /// Clear conversation owner (for RMS)
    pub fn clear_conversation_owner(&self, conversation_id: &str) {
        let mut owners = self.conversation_owners.write();
        if let Some(client_id) = owners.remove(conversation_id) {
            let mut client_convs = self.client_conversations.write();
            if let Some(convs) = client_convs.get_mut(&client_id) {
                convs.remove(conversation_id);
            }
            info!(
                "Cleared conversation {} owner (was {})",
                conversation_id, client_id
            );
        }
    }

    /// Get all conversation owners
    pub fn get_all_conversation_owners(&self) -> std::collections::HashMap<String, String> {
        self.conversation_owners.read().clone()
    }
}

/// Broadcast error
#[derive(Debug, thiserror::Error)]
pub enum BroadcastError {
    #[error("No clients connected")]
    NoClients,

    #[error("All clients failed")]
    AllFailed,
}

/// Send error
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("Client not found")]
    ClientNotFound,

    #[error("Client disconnected")]
    ClientDisconnected,
}

/// Handle WebSocket upgrade request
pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(ws_manager): State<Arc<WebSocketManager>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, ws_manager, addr, None))
}

/// Handle WebSocket upgrade request with broker for response handling
pub async fn websocket_handler_with_broker(
    ws: WebSocketUpgrade,
    State(ws_manager): State<Arc<WebSocketManager>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    broker: Arc<MessageBroker>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, ws_manager, addr, Some(broker)))
}

/// Handle WebSocket connection
async fn handle_socket(
    socket: WebSocket,
    ws_manager: Arc<WebSocketManager>,
    addr: SocketAddr,
    broker: Option<Arc<MessageBroker>>,
) {
    info!("New WebSocket connection from: {}", addr);

    let (mut ws_tx, mut ws_rx) = socket.split();

    // Create channel for outgoing messages
    let (tx, mut rx): (mpsc::Sender<WsProtocol>, mpsc::Receiver<WsProtocol>) = mpsc::channel(32);

    // Authentication state
    let mut authenticated = false;
    let mut client_id: Option<String> = None;

    // Task for sending messages
    let send_task: JoinHandle<()> = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let json = match serde_json::to_string(&msg) {
                Ok(j) => j,
                Err(e) => {
                    error!("Failed to serialize message: {}", e);
                    continue;
                }
            };
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Receive loop
    let recv_timeout = Duration::from_secs(ws_manager.config.websocket.timeout_secs);
    let ping_interval = Duration::from_secs(ws_manager.config.websocket.ping_interval_secs);
    let mut last_ping = Instant::now();

    loop {
        match tokio::time::timeout(recv_timeout, ws_rx.next()).await {
            Ok(Some(Ok(msg_result))) => match msg_result {
                Message::Text(text) => {
                    // Parse protocol message
                    let protocol: WsProtocol = match serde_json::from_str(&text) {
                        Ok(p) => p,
                        Err(e) => {
                            warn!("Failed to parse WebSocket message: {}", e);
                            let _ = tx
                                .send(WsProtocol::Error {
                                    code: 400,
                                    message: format!("Invalid message format: {}", e),
                                })
                                .await;
                            continue;
                        }
                    };

                    match protocol {
                        WsProtocol::Auth { data } => {
                            // Authenticate client
                            if !authenticate(&data, &ws_manager.config) {
                                warn!("Authentication failed for client: {}", data.client_id);
                                let _ = tx
                                    .send(WsProtocol::AuthResult {
                                        success: false,
                                        message: "Invalid API key".to_string(),
                                        protocol_version: None,
                                        capabilities: None,
                                    })
                                    .await;
                                continue;
                            }

                            // Add client
                            let info = ClientInfo {
                                client_id: data.client_id.clone(),
                                addr,
                                connected_at: Instant::now(),
                                last_activity: Instant::now(),
                            };

                            ws_manager.add_client(data.client_id.clone(), info, tx.clone());
                            authenticated = true;
                            client_id = Some(data.client_id.clone());

                            info!("Client authenticated: {}", data.client_id);

                            let _ = tx
                                .send(WsProtocol::AuthResult {
                                    success: true,
                                    message: "Authentication successful".to_string(),
                                    protocol_version: Some(crate::broker::MessageBroker::protocol_version()),
                                    capabilities: Some(crate::broker::MessageBroker::capabilities()),
                                })
                                .await;
                        }

                        WsProtocol::Ping => {
                            let _ = tx.send(WsProtocol::Pong).await;
                        }

                        WsProtocol::Pong => {
                            // Pong received, connection is alive
                            debug!("Pong received from {}", addr);
                        }

                        WsProtocol::Message { data } => {
                            if !authenticated {
                                warn!("Unauthenticated message from {}", addr);
                                continue;
                            }

                            // Handle incoming message from UGENT
                            info!(
                                "Received message from {}: {:?}",
                                client_id.as_deref().unwrap_or("unknown"),
                                data
                            );
                        }

                        WsProtocol::Response {
                            request_id,
                            original_id,
                            content,
                            artifacts,
                        } => {
                            if !authenticated {
                                warn!("Unauthenticated response from {}", addr);
                                continue;
                            }

                            let client_id_str = client_id.as_deref().unwrap_or("unknown");

                            info!(
                                "Received response from {}: request_id={:?}, original_id={}, content_len={}, artifacts={}",
                                client_id_str,
                                request_id,
                                original_id,
                                content.len(),
                                artifacts.len()
                            );

                            // Handle response through broker if available
                            if let Some(ref broker) = broker {
                                // Get pending message to find conversation_id for ownership claiming
                                if let Some(pending) = broker.get_pending_message(&original_id) {
                                    // CLAIM OWNERSHIP on first response (first-response-wins)
                                    let claimed = ws_manager.claim_conversation(
                                        &pending.conversation_id,
                                        client_id_str,
                                    );

                                    if claimed {
                                        info!(
                                            "Client {} claimed ownership of conversation {}",
                                            client_id_str, pending.conversation_id
                                        );
                                    } else {
                                        debug!(
                                            "Client {} responded to conversation {} (already owned)",
                                            client_id_str, pending.conversation_id
                                        );
                                    }
                                }

                                if let Err(e) = broker
                                    .handle_response(
                                        request_id,
                                        original_id,
                                        content,
                                        artifacts,
                                        Some(client_id_str.to_string()),
                                    )
                                    .await
                                {
                                    error!("Failed to handle response: {}", e);
                                }
                            }
                        }

                        WsProtocol::Error { code, message } => {
                            warn!("Error from client {}: {} - {}", addr, code, message);
                        }

                        WsProtocol::AuthResult { .. } => {
                            warn!("Unexpected AuthResult from client {}", addr);
                        }

                        WsProtocol::ResponseResult { .. } => {
                            warn!("Unexpected ResponseResult from client {}", addr);
                        }
                    }
                }

                Message::Close(_) => {
                    info!("Client {} closed connection", addr);
                    break;
                }

                Message::Ping(data) => {
                    // Respond with pong through the sender task
                    debug!("Received Ping from {}", addr);
                    let _ = tx.send(WsProtocol::Pong).await;
                    // Note: data is not used but we acknowledge it
                    let _ = data;
                }

                Message::Pong(_) => {
                    // Connection is alive
                    debug!("Pong from {}", addr);
                }

                other => {
                    warn!("Unsupported WebSocket message type: {:?}", other);
                }
            },
            Ok(Some(Err(e))) => {
                error!("WebSocket receive error: {}", e);
                break;
            }
            Ok(None) => {
                // Stream ended
                info!("WebSocket stream ended for {}", addr);
                break;
            }
            Err(_) => {
                // Timeout
                warn!("WebSocket receive timeout for {}", addr);
                break;
            }
        }

        // Send periodic pings
        if last_ping.elapsed() >= ping_interval {
            let _ = tx.send(WsProtocol::Ping).await;
            last_ping = Instant::now();
        }
    }

    // Cleanup
    if let Some(ref id) = client_id {
        ws_manager.remove_client(id);
    }

    send_task.abort();

    info!("WebSocket connection closed: {}", addr);
}

/// Authenticate client
fn authenticate(data: &AuthData, config: &Config) -> bool {
    // If no API key is configured, allow all connections (development mode)
    if !config.websocket.has_api_key() {
        warn!("No API key configured, allowing connection (development mode)");
        return true;
    }

    // Verify API key
    data.api_key == config.websocket.api_key
}

// =============================================================================
// Unit Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> Arc<Config> {
        Arc::new(Config {
            server: crate::config::ServerConfig {
                bind_addr: "0.0.0.0:3000".to_string(),
                name: "test".to_string(),
                tls: None,
            },
            line: crate::config::LineConfig {
                channel_secret: "test_secret".to_string(),
                channel_access_token: "test_token".to_string(),
                webhook_path: "/line/callback".to_string(),
                skip_signature: true,
                process_redeliveries: true,
                auto_loading_indicator: true,
                auto_mark_as_read: true,
            },
            websocket: crate::config::WebSocketConfig {
                path: "/ws".to_string(),
                api_key: "test_api_key".to_string(),
                ping_interval_secs: 30,
                timeout_secs: 60,
                max_message_size: 10 * 1024 * 1024,
            },
            media: crate::config::MediaConfig::default(),
            logging: crate::config::LoggingConfig::default(),
            storage: crate::config::StorageConfig::default(),
        })
    }

    #[test]
    fn test_websocket_manager() {
        let config = create_test_config();
        let manager = WebSocketManager::new(config);

        assert_eq!(manager.client_count(), 0);
        assert!(!manager.has_clients());
    }

    #[test]
    fn test_reply_token_registration() {
        let config = create_test_config();
        let manager = WebSocketManager::new(config);

        manager.register_reply_token("token123", "client1");
        assert_eq!(
            manager.get_client_for_reply_token("token123"),
            Some("client1".to_string())
        );
        assert_eq!(manager.get_client_for_reply_token("nonexistent"), None);
    }

    #[test]
    fn test_authenticate() {
        let config = create_test_config();
        let auth_data = AuthData {
            client_id: "test_client".to_string(),
            api_key: "test_api_key".to_string(),
        };

        assert!(authenticate(&auth_data, &config));

        let wrong_auth_data = AuthData {
            client_id: "test_client".to_string(),
            api_key: "wrong_key".to_string(),
        };

        assert!(!authenticate(&wrong_auth_data, &config));
    }
}
