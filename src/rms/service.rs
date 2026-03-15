//! RMS Service
//!
//! Core business logic for the Relationship Management System.

use std::sync::Arc;
use std::time::Instant;

use tracing::{debug, info};

use super::storage::RmsStorage;
use super::types::*;
use crate::line_api::LineApiClient;
use crate::storage::Storage;
use crate::ws_manager::WebSocketManager;

/// Relationship Manager Service
///
/// Provides unified access to relationship management operations,
/// combining storage, runtime state, and LINE API access.
pub struct RelationshipManagerService {
    /// RMS-specific storage
    rms_storage: RmsStorage,
    /// Main storage reference (for future use with pending messages)
    #[allow(dead_code)]
    storage: Arc<Storage>,
    /// WebSocket manager reference
    ws_manager: Arc<WebSocketManager>,
    /// LINE API client
    line_client: LineApiClient,
    /// Server start time (for uptime calculation)
    start_time: Instant,
}

impl RelationshipManagerService {
    /// Create a new Relationship Manager Service
    pub fn new(
        storage: Arc<Storage>,
        ws_manager: Arc<WebSocketManager>,
        line_client: LineApiClient,
    ) -> Self {
        // Get the connection from storage for RMS operations
        // We need to access the internal connection
        let conn = storage.connection_clone();
        let rms_storage = RmsStorage::new(conn);

        Self {
            rms_storage,
            storage,
            ws_manager,
            line_client,
            start_time: Instant::now(),
        }
    }

    // ========== Query Operations ==========

    /// Get all connected UGENT clients with their owned conversation counts
    pub async fn get_clients(&self) -> Result<Vec<ClientInfo>, RmsError> {
        let connected_ids = self.ws_manager.get_connected_client_ids();
        let mut clients = Vec::new();

        for client_id in connected_ids {
            let info = self.get_client(&client_id).await?;
            if let Some(info) = info {
                clients.push(info);
            }
        }

        // Also include disconnected clients that have relationships
        let relationships = self.rms_storage.get_relationships()?;
        for rel in relationships {
            if !clients.iter().any(|c| c.client_id == rel.client_id) {
                clients.push(ClientInfo {
                    client_id: rel.client_id.clone(),
                    connected: false,
                    connected_at: None,
                    last_activity: rel.updated_at,
                    owned_conversations: self
                        .rms_storage
                        .get_relationships_by_client(&rel.client_id)?
                        .len(),
                    metadata: None,
                });
            }
        }

        Ok(clients)
    }

    /// Get a specific client by ID
    pub async fn get_client(&self, client_id: &str) -> Result<Option<ClientInfo>, RmsError> {
        let is_connected = self.ws_manager.is_client_connected(client_id);

        let owned_conversations = self
            .rms_storage
            .get_relationships_by_client(client_id)?
            .len();

        // Get last activity from dispatch history or relationships
        let last_activity = self.get_client_last_activity(client_id)?;

        Ok(Some(ClientInfo {
            client_id: client_id.to_string(),
            connected: is_connected,
            connected_at: self.ws_manager.get_client_connected_time(client_id),
            last_activity,
            owned_conversations,
            metadata: self.ws_manager.get_client_metadata(client_id),
        }))
    }

    /// Get client's last activity timestamp
    fn get_client_last_activity(&self, client_id: &str) -> Result<i64, RmsError> {
        // Try to get from relationships first
        let rels = self.rms_storage.get_relationships_by_client(client_id)?;
        if let Some(last_rel) = rels.iter().max_by_key(|r| r.updated_at) {
            return Ok(last_rel.updated_at);
        }

        // Fall back to current time if no activity
        Ok(chrono::Utc::now().timestamp())
    }

    /// Get all LINE entities with optional filter
    pub async fn get_entities(&self, filter: EntityFilter) -> Result<Vec<LineEntity>, RmsError> {
        self.rms_storage.get_entities(&filter).map_err(Into::into)
    }

    /// Get entity by ID, optionally refreshing from LINE API
    pub async fn get_entity(&self, entity_id: &str) -> Result<Option<LineEntity>, RmsError> {
        let entity = self.rms_storage.get_entity(entity_id)?;

        if entity.is_some() {
            return Ok(entity);
        }

        // Entity not in DB, try to fetch from LINE API
        self.refresh_entity(entity_id).await
    }

    /// Get all relationships
    pub async fn get_relationships(&self) -> Result<Vec<Relationship>, RmsError> {
        self.rms_storage.get_relationships().map_err(Into::into)
    }

    /// Get relationship for a specific entity
    pub async fn get_relationship(&self, entity_id: &str) -> Result<Option<Relationship>, RmsError> {
        self.rms_storage.get_relationship(entity_id).map_err(Into::into)
    }

    /// Get computed dispatch rules (relationships + runtime state)
    pub async fn get_dispatch_rules(&self) -> Result<Vec<DispatchRule>, RmsError> {
        let relationships = self.rms_storage.get_relationships()?;
        let mut rules = Vec::new();

        for rel in relationships {
            let is_connected = self.ws_manager.is_client_connected(&rel.client_id);
            let last_routed = self
                .rms_storage
                .get_last_dispatch_time(&rel.line_entity_id)?;
            let message_count = self
                .rms_storage
                .get_message_count(&rel.line_entity_id)?;

            rules.push(DispatchRule {
                conversation_id: rel.line_entity_id.clone(),
                entity_type: rel.entity_type,
                assigned_client: Some(rel.client_id.clone()),
                assigned_client_connected: is_connected,
                is_manual: rel.is_manual,
                last_routed_at: last_routed,
                message_count,
            });
        }

        // Also include entities without relationships (broadcast mode)
        let entities = self
            .rms_storage
            .get_entities(&EntityFilter::default())?;
        for entity in entities {
            if !rules.iter().any(|r| r.conversation_id == entity.id) {
                rules.push(DispatchRule {
                    conversation_id: entity.id.clone(),
                    entity_type: entity.entity_type,
                    assigned_client: None,
                    assigned_client_connected: false,
                    is_manual: false,
                    last_routed_at: entity.last_message_at,
                    message_count: 0,
                });
            }
        }

        Ok(rules)
    }

    /// Get dispatch rule for a specific conversation
    pub async fn get_dispatch_rule(
        &self,
        conversation_id: &str,
    ) -> Result<Option<DispatchRule>, RmsError> {
        let rel = self.rms_storage.get_relationship(conversation_id)?;

        match rel {
            Some(r) => {
                let is_connected = self.ws_manager.is_client_connected(&r.client_id);
                let last_routed = self.rms_storage.get_last_dispatch_time(conversation_id)?;
                let message_count = self.rms_storage.get_message_count(conversation_id)?;

                Ok(Some(DispatchRule {
                    conversation_id: conversation_id.to_string(),
                    entity_type: r.entity_type,
                    assigned_client: Some(r.client_id),
                    assigned_client_connected: is_connected,
                    is_manual: r.is_manual,
                    last_routed_at: last_routed,
                    message_count,
                }))
            }
            None => {
                // No relationship, check if entity exists
                let entity = self.rms_storage.get_entity(conversation_id)?;
                Ok(entity.map(|e| DispatchRule {
                    conversation_id: conversation_id.to_string(),
                    entity_type: e.entity_type,
                    assigned_client: None,
                    assigned_client_connected: false,
                    is_manual: false,
                    last_routed_at: e.last_message_at,
                    message_count: 0,
                }))
            }
        }
    }

    /// Get system status summary
    pub async fn get_status(&self) -> Result<SystemStatus, RmsError> {
        let connected_clients = self.ws_manager.client_count();
        let total_entities = self.rms_storage.count_entities()?;
        let total_relationships = self.rms_storage.count_relationships()?;
        let manual_relationships = self.rms_storage.count_manual_relationships()?;
        let auto_relationships = total_relationships - manual_relationships;
        let pending_messages = self.rms_storage.count_pending_messages().unwrap_or(0);
        let uptime_secs = self.start_time.elapsed().as_secs();

        Ok(SystemStatus {
            connected_clients,
            total_entities,
            total_relationships,
            manual_relationships,
            auto_relationships,
            pending_messages: pending_messages as usize,
            uptime_secs,
        })
    }

    // ========== Mutation Operations ==========

    /// Create or update a relationship (manual override)
    pub async fn set_relationship(
        &self,
        entity_id: &str,
        client_id: &str,
        notes: Option<&str>,
    ) -> Result<Relationship, RmsError> {
        // Ensure entity exists
        let entity = self.rms_storage.get_entity(entity_id)?;
        if entity.is_none() {
            // Create entity if it doesn't exist
            self.refresh_entity(entity_id).await?;
        }

        // Set the relationship
        let rel = self
            .rms_storage
            .set_relationship(entity_id, client_id, true, notes)?;

        info!(
            entity_id = %entity_id,
            client_id = %client_id,
            "Relationship set (manual)"
        );

        // Notify the runtime about the change
        self.ws_manager
            .set_conversation_owner(entity_id, client_id);

        Ok(rel)
    }

    /// Remove a relationship (revert to auto-routing)
    pub async fn remove_relationship(&self, entity_id: &str) -> Result<bool, RmsError> {
        let removed = self.rms_storage.remove_relationship(entity_id)?;

        if removed {
            info!(entity_id = %entity_id, "Relationship removed");
            // Clear the runtime ownership
            self.ws_manager.clear_conversation_owner(entity_id);
        }

        Ok(removed)
    }

    /// Refresh entity from LINE API
    pub async fn refresh_entity(&self, entity_id: &str) -> Result<Option<LineEntity>, RmsError> {
        let entity_type = LineEntityType::from_line_id(entity_id);
        let now = chrono::Utc::now().timestamp();

        // Try to get profile from LINE API
        let profile_result = match entity_type {
            LineEntityType::User => self.line_client.get_profile(entity_id).await.ok(),
            LineEntityType::Group | LineEntityType::Room => {
                // Groups/rooms don't have profile API
                None
            }
        };

        let (display_name, picture_url) = match profile_result {
            Some(profile) => (Some(profile.display_name), Some(profile.picture_url)),
            None => (None, None),
        };

        let entity = LineEntity {
            id: entity_id.to_string(),
            entity_type,
            display_name,
            picture_url: picture_url.flatten(),
            last_message_at: None,
            created_at: now,
            updated_at: now,
        };

        self.rms_storage.upsert_entity(&entity)?;

        debug!(entity_id = %entity_id, "Entity refreshed");
        self.rms_storage.get_entity(entity_id).map_err(Into::into)
    }

    /// Sync relationships with runtime ownership state
    pub async fn sync_ownership(&self) -> Result<SyncResult, RmsError> {
        let mut added = 0;
        let mut updated = 0;
        let mut removed = 0;

        // Get all current owners from runtime
        let runtime_owners = self.ws_manager.get_all_conversation_owners();

        // Get all relationships from storage
        let stored_relationships = self.rms_storage.get_relationships()?;
        let stored_map: std::collections::HashMap<_, _> = stored_relationships
            .iter()
            .map(|r| (r.line_entity_id.as_str(), r))
            .collect();

        // Add/update relationships for runtime owners
        for (conv_id, client_id) in &runtime_owners {
            if let Some(stored) = stored_map.get(conv_id.as_str()) {
                if &stored.client_id != client_id && !stored.is_manual {
                    // Update auto-detected relationship
                    self.rms_storage
                        .set_relationship(conv_id, client_id, false, None)?;
                    updated += 1;
                }
            } else {
                // Create new auto-detected relationship
                self.rms_storage
                    .set_relationship(conv_id, client_id, false, None)?;
                added += 1;
            }
        }

        // Remove auto-detected relationships that no longer have runtime owners
        for stored in &stored_relationships {
            if !stored.is_manual && !runtime_owners.contains_key(&stored.line_entity_id) {
                self.rms_storage.remove_relationship(&stored.line_entity_id)?;
                removed += 1;
            }
        }

        info!(added, updated, removed, "Ownership sync complete");

        Ok(SyncResult {
            added,
            updated,
            removed,
        })
    }

    // ========== Batch Operations ==========

    /// Import relationships from JSON
    pub async fn import_relationships(
        &self,
        imports: &[RelationshipImport],
    ) -> Result<ImportResult, RmsError> {
        let mut imported = 0;
        let mut updated = 0;
        let mut errors = Vec::new();

        for import in imports {
            // Ensure entity exists
            if self.rms_storage.get_entity(&import.entity_id)?.is_none() {
                if let Err(e) = self.refresh_entity(&import.entity_id).await {
                    errors.push(format!(
                        "Failed to refresh entity {}: {}",
                        import.entity_id, e
                    ));
                    continue;
                }
            }

            // Check if relationship already exists
            let existing = self.rms_storage.get_relationship(&import.entity_id)?;

            match existing {
                Some(rel) if rel.client_id == import.client_id => {
                    // Same client, skip
                }
                Some(_) => {
                    // Different client, update
                    self.rms_storage.set_relationship(
                        &import.entity_id,
                        &import.client_id,
                        true,
                        import.notes.as_deref(),
                    )?;
                    updated += 1;
                }
                None => {
                    // Create new
                    self.rms_storage.set_relationship(
                        &import.entity_id,
                        &import.client_id,
                        true,
                        import.notes.as_deref(),
                    )?;
                    imported += 1;
                }
            }
        }

        Ok(ImportResult {
            imported,
            updated,
            errors,
        })
    }

    /// Export all relationships to JSON
    pub async fn export_relationships(&self) -> Result<Vec<Relationship>, RmsError> {
        self.rms_storage.get_relationships().map_err(Into::into)
    }

    /// Clear all manual relationships
    pub async fn clear_manual_relationships(&self) -> Result<usize, RmsError> {
        let count = self.rms_storage.clear_manual_relationships()?;
        info!(count, "Manual relationships cleared");
        Ok(count)
    }

    /// Touch entity (update last_message_at)
    pub async fn touch_entity(&self, entity_id: &str) -> Result<(), RmsError> {
        let now = chrono::Utc::now().timestamp();
        self.rms_storage.touch_entity(entity_id, now)?;
        Ok(())
    }

    /// Record a dispatch event
    pub async fn record_dispatch(
        &self,
        conversation_id: &str,
        client_id: &str,
        message_id: Option<&str>,
        success: bool,
    ) -> Result<(), RmsError> {
        self.rms_storage
            .record_dispatch(conversation_id, client_id, message_id, success)?;
        self.touch_entity(conversation_id).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Full integration tests would require mocking Storage, WebSocketManager, and LineApiClient
    // For now, we'll add unit tests for the types and basic logic

    #[test]
    fn test_entity_type_from_line_id() {
        assert_eq!(
            LineEntityType::from_line_id("U1234567890abcdef"),
            LineEntityType::User
        );
        assert_eq!(
            LineEntityType::from_line_id("C1234567890abcdef"),
            LineEntityType::Group
        );
        assert_eq!(
            LineEntityType::from_line_id("R1234567890abcdef"),
            LineEntityType::Room
        );
    }
}
