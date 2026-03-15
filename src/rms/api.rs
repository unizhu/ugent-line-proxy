//! RMS HTTP API
//!
//! REST API endpoints for the Relationship Management System using Axum 0.8.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Json},
    routing::{delete, get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use super::service::RelationshipManagerService;
use super::types::*;

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
pub fn rms_routes(rms: RmsState) -> Router {
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
        .route(
            "/api/rms/relationships/{entity_id}",
            delete(remove_relationship),
        )
        // Dispatch Rules
        .route("/api/rms/dispatch-rules", get(get_dispatch_rules))
        .route(
            "/api/rms/dispatch-rules/{conv_id}",
            get(get_dispatch_rule),
        )
        // Import/Export
        .route("/api/rms/import", post(import_relationships))
        .route("/api/rms/export", get(export_relationships))
        .route("/api/rms/sync", post(sync_ownership))
        .route("/api/rms/clear", post(clear_manual_relationships))
        .with_state(rms)
}

// ========== Status ==========

async fn get_status(
    State(rms): State<RmsState>,
) -> impl IntoResponse {
    match rms.get_status().await {
        Ok(status) => Json(ApiResponse::success(status)),
        Err(e) => {
            Json(ApiResponse::<SystemStatus>::error(e.to_string()))
        }
    }
}

// ========== Clients ==========

async fn get_clients(
    State(rms): State<RmsState>,
) -> impl IntoResponse {
    match rms.get_clients().await {
        Ok(clients) => Json(ApiResponse::success(clients)),
        Err(e) => Json(ApiResponse::<Vec<ClientInfo>>::error(e.to_string())),
    }
}

async fn get_client(
    State(rms): State<RmsState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match rms.get_client(&id).await {
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
        entity_type: query.entity_type.and_then(|s| LineEntityType::parse_entity_type(&s)),
        has_relationship: query.has_relationship,
        search: query.search,
        limit: query.limit,
        offset: query.offset,
    };

    match rms.get_entities(filter).await {
        Ok(entities) => {
            let total = entities.len();
            Json(ApiResponse::success(PaginatedResponse {
                items: entities,
                total,
                offset: query.offset.unwrap_or(0),
                limit: query.limit,
            }))
        }
        Err(e) => Json(ApiResponse::<PaginatedResponse<LineEntity>>::error(e.to_string())),
    }
}

async fn get_entity(
    State(rms): State<RmsState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match rms.get_entity(&id).await {
        Ok(Some(entity)) => Json(ApiResponse::success(entity)),
        Ok(None) => Json(ApiResponse::<LineEntity>::error("Entity not found")),
        Err(e) => Json(ApiResponse::<LineEntity>::error(e.to_string())),
    }
}

async fn refresh_entity(
    State(rms): State<RmsState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match rms.refresh_entity(&id).await {
        Ok(Some(entity)) => Json(ApiResponse::success(entity)),
        Ok(None) => Json(ApiResponse::<LineEntity>::error("Entity not found")),
        Err(e) => Json(ApiResponse::<LineEntity>::error(e.to_string())),
    }
}

// ========== Relationships ==========

async fn get_relationships(
    State(rms): State<RmsState>,
) -> impl IntoResponse {
    match rms.get_relationships().await {
        Ok(relationships) => Json(ApiResponse::success(relationships)),
        Err(e) => Json(ApiResponse::<Vec<Relationship>>::error(e.to_string())),
    }
}

async fn get_relationship(
    State(rms): State<RmsState>,
    Path(entity_id): Path<String>,
) -> impl IntoResponse {
    match rms.get_relationship(&entity_id).await {
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
    match rms.remove_relationship(&entity_id).await {
        Ok(true) => Json(ApiResponse::success(true)),
        Ok(false) => Json(ApiResponse::<bool>::error("Relationship not found")),
        Err(e) => Json(ApiResponse::<bool>::error(e.to_string())),
    }
}

// ========== Dispatch Rules ==========

async fn get_dispatch_rules(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.get_dispatch_rules().await {
        Ok(rules) => Json(ApiResponse::success(rules)),
        Err(e) => Json(ApiResponse::<Vec<DispatchRule>>::error(e.to_string())),
    }
}

async fn get_dispatch_rule(
    State(rms): State<RmsState>,
    Path(conv_id): Path<String>,
) -> impl IntoResponse {
    match rms.get_dispatch_rule(&conv_id).await {
        Ok(Some(rule)) => Json(ApiResponse::success(rule)),
        Ok(None) => Json(ApiResponse::<DispatchRule>::error("Dispatch rule not found")),
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
    match rms.export_relationships().await {
        Ok(relationships) => Json(ApiResponse::success(relationships)),
        Err(e) => Json(ApiResponse::<Vec<Relationship>>::error(e.to_string())),
    }
}

async fn sync_ownership(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.sync_ownership().await {
        Ok(result) => Json(ApiResponse::success(result)),
        Err(e) => Json(ApiResponse::<SyncResult>::error(e.to_string())),
    }
}

async fn clear_manual_relationships(State(rms): State<RmsState>) -> impl IntoResponse {
    match rms.clear_manual_relationships().await {
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
