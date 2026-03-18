//! RMS HTTP API
//!
//! REST API endpoints for the Relationship Management System using Axum 0.8.

use axum::{
    Router,
    extract::{Path, Query, State},
    response::{IntoResponse, Json},
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::service::RelationshipManagerService;
use super::types::{RelationshipImport, SystemStatus, ClientInfo, EntityFilter, LineEntityType, LineEntity, Relationship, DispatchRule, ImportResult, SyncResult};

/// RMS API state
pub type RmsState = Arc<RelationshipManagerService>;

/// Query parameters for list endpoints
#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Filter by entity type
    #[serde(rename = "type")]
    pub entity_type: Option<String>,
    /// Filter by relationship status
    pub has_relationship: Option<bool>,
    /// Search string
    pub search: Option<String>,
    /// Limit results
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
}

/// Request body for setting a relationship
#[derive(Debug, Deserialize)]
pub struct SetRelationshipRequest {
    /// LINE entity ID
    pub entity_id: String,
    /// Client ID to assign
    pub client_id: String,
    /// Admin notes
    pub notes: Option<String>,
}

/// Request body for importing relationships
#[derive(Debug, Deserialize)]
pub struct ImportRequest {
    pub relationships: Vec<RelationshipImport>,
}

/// Generic API response wrapper
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg.into()),
        }
    }
}

/// Paginated list response
#[derive(Debug, Serialize)]
pub struct PaginatedResponse<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub offset: usize,
    pub limit: Option<usize>,
}

/// Create RMS API routes (Axum 0.8 syntax with {id} path params)
/// Note: Caller must call .with_state(rms) on the returned router
pub fn rms_routes() -> Router<RmsState> {
    Router::new()
        // Status
        .route("/status", get(get_status))
        // Clients
        .route("/clients", get(get_clients))
        .route("/clients/{id}", get(get_client))
        // Entities
        .route("/entities", get(get_entities))
        .route("/entities/{id}", get(get_entity))
        .route("/entities/{id}/refresh", post(refresh_entity))
        // Relationships
        .route("/relationships", get(get_relationships))
        .route("/relationships", post(set_relationship))
        .route("/relationships/{entity_id}", get(get_relationship))
        .route("/relationships/{entity_id}", delete(remove_relationship))
        // Dispatch Rules
        .route("/dispatch-rules", get(get_dispatch_rules))
        .route("/dispatch-rules/{conv_id}", get(get_dispatch_rule))
        // Import/Export
        .route("/import", post(import_relationships))
        .route("/export", get(export_relationships))
        .route("/sync", post(sync_ownership))
        .route("/clear", post(clear_manual_relationships))
}

// ========== Status ==========

async fn get_status(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.get_status() {
        Ok(status) => Json(ApiResponse::success(status)),
        Err(e) => Json(ApiResponse::<SystemStatus>::error(e.to_string())),
    }
}

// ========== Clients ==========

async fn get_clients(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.get_clients() {
        Ok(clients) => Json(ApiResponse::success(clients)),
        Err(e) => Json(ApiResponse::<Vec<ClientInfo>>::error(e.to_string())),
    }
}

async fn get_client(State(rms): State<RmsState>, Path(id): Path<String>) -> impl IntoResponse {
    match rms.get_client(&id) {
        Ok(Some(client)) => Json(ApiResponse::success(client)),
        Ok(None) => Json(ApiResponse::<ClientInfo>::error("Client not found")),
        Err(e) => Json(ApiResponse::<ClientInfo>::error(e.to_string())),
    }
}

// ========== Entities ==========

async fn get_entities(
    State(rms): State<RmsState>,
    Query(query): Query<ListQuery>,
) -> impl IntoResponse {
    let filter = EntityFilter {
        entity_type: query
            .entity_type
            .and_then(|s| LineEntityType::parse_entity_type(&s)),
        has_relationship: query.has_relationship,
        search: query.search,
        limit: query.limit,
        offset: query.offset,
    };

    match rms.get_entities(&filter) {
        Ok(entities) => {
            let total = entities.len();
            Json(ApiResponse::success(PaginatedResponse {
                items: entities,
                total,
                offset: query.offset.unwrap_or(0),
                limit: query.limit,
            }))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<LineEntity>>::error(
            e.to_string(),
        )),
    }
}

async fn get_entity(State(rms): State<RmsState>, Path(id): Path<String>) -> impl IntoResponse {
    match rms.get_entity(&id).await {
        Ok(Some(entity)) => Json(ApiResponse::success(entity)),
        Ok(None) => Json(ApiResponse::<LineEntity>::error("Entity not found")),
        Err(e) => Json(ApiResponse::<LineEntity>::error(e.to_string())),
    }
}

async fn refresh_entity(State(rms): State<RmsState>, Path(id): Path<String>) -> impl IntoResponse {
    match rms.refresh_entity(&id).await {
        Ok(Some(entity)) => Json(ApiResponse::success(entity)),
        Ok(None) => Json(ApiResponse::<LineEntity>::error("Entity not found")),
        Err(e) => Json(ApiResponse::<LineEntity>::error(e.to_string())),
    }
}

// ========== Relationships ==========

async fn get_relationships(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.get_relationships() {
        Ok(relationships) => Json(ApiResponse::success(relationships)),
        Err(e) => Json(ApiResponse::<Vec<Relationship>>::error(e.to_string())),
    }
}

async fn get_relationship(
    State(rms): State<RmsState>,
    Path(entity_id): Path<String>,
) -> impl IntoResponse {
    match rms.get_relationship(&entity_id) {
        Ok(Some(rel)) => Json(ApiResponse::success(rel)),
        Ok(None) => Json(ApiResponse::<Relationship>::error("Relationship not found")),
        Err(e) => Json(ApiResponse::<Relationship>::error(e.to_string())),
    }
}

async fn set_relationship(
    State(rms): State<RmsState>,
    Json(req): Json<SetRelationshipRequest>,
) -> impl IntoResponse {
    match rms
        .set_relationship(&req.entity_id, &req.client_id, req.notes.as_deref())
        .await
    {
        Ok(rel) => Json(ApiResponse::success(rel)),
        Err(e) => Json(ApiResponse::<Relationship>::error(e.to_string())),
    }
}

async fn remove_relationship(
    State(rms): State<RmsState>,
    Path(entity_id): Path<String>,
) -> impl IntoResponse {
    match rms.remove_relationship(&entity_id) {
        Ok(true) => Json(ApiResponse::success(true)),
        Ok(false) => Json(ApiResponse::<bool>::error("Relationship not found")),
        Err(e) => Json(ApiResponse::<bool>::error(e.to_string())),
    }
}

// ========== Dispatch Rules ==========

async fn get_dispatch_rules(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.get_dispatch_rules() {
        Ok(rules) => Json(ApiResponse::success(rules)),
        Err(e) => Json(ApiResponse::<Vec<DispatchRule>>::error(e.to_string())),
    }
}

async fn get_dispatch_rule(
    State(rms): State<RmsState>,
    Path(conv_id): Path<String>,
) -> impl IntoResponse {
    match rms.get_dispatch_rule(&conv_id) {
        Ok(Some(rule)) => Json(ApiResponse::success(rule)),
        Ok(None) => Json(ApiResponse::<DispatchRule>::error(
            "Dispatch rule not found",
        )),
        Err(e) => Json(ApiResponse::<DispatchRule>::error(e.to_string())),
    }
}

// ========== Import/Export ==========

async fn import_relationships(
    State(rms): State<RmsState>,
    Json(req): Json<ImportRequest>,
) -> impl IntoResponse {
    match rms.import_relationships(&req.relationships).await {
        Ok(result) => Json(ApiResponse::success(result)),
        Err(e) => Json(ApiResponse::<ImportResult>::error(e.to_string())),
    }
}

async fn export_relationships(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.export_relationships() {
        Ok(relationships) => Json(ApiResponse::success(relationships)),
        Err(e) => Json(ApiResponse::<Vec<Relationship>>::error(e.to_string())),
    }
}

async fn sync_ownership(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.sync_ownership() {
        Ok(result) => Json(ApiResponse::success(result)),
        Err(e) => Json(ApiResponse::<SyncResult>::error(e.to_string())),
    }
}

async fn clear_manual_relationships(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.clear_manual_relationships() {
        Ok(count) => Json(ApiResponse::success(count)),
        Err(e) => Json(ApiResponse::<usize>::error(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_response_serialization() {
        let resp = ApiResponse::success(SystemStatus {
            connected_clients: 1,
            total_entities: 10,
            total_relationships: 5,
            manual_relationships: 2,
            auto_relationships: 3,
            pending_messages: 0,
            uptime_secs: 100,
        });
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("\"success\":true"));
    }
}
