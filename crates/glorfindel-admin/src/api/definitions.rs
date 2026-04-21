use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use glorfindel_schemas::types::{AgentType, Permission};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::ApiError;
use crate::persist;
use crate::state::{AgentDefinition, AppState};

#[derive(Debug, Deserialize)]
pub struct DefinitionBody {
    pub name: String,
    pub description: String,
    pub agent_type: AgentType,
    pub model: String,
    pub ollama_host: String,
    pub tools: Vec<String>,
    pub domains: Vec<String>,
    pub system_prompt: Option<String>,
    pub default_permissions: Vec<Permission>,
    pub campaign_dir: Option<String>,
    pub deployed_context: Option<serde_json::Value>,
}

#[derive(Serialize)]
pub struct DeleteResponse {
    pub deleted: bool,
}

pub async fn list(State(state): State<Arc<AppState>>) -> Json<Vec<AgentDefinition>> {
    let defs = state.definitions.read().await;
    let mut list: Vec<AgentDefinition> = defs.values().cloned().collect();
    list.sort_by_key(|d| d.created_at);
    Json(list)
}

pub async fn create(
    State(state): State<Arc<AppState>>,
    Json(body): Json<DefinitionBody>,
) -> Result<Json<AgentDefinition>, ApiError> {
    if body.name.trim().is_empty() {
        return Err(ApiError::BadRequest("name is required".into()));
    }
    let now = Utc::now();
    let def = AgentDefinition {
        id: Uuid::new_v4(),
        name: body.name,
        description: body.description,
        agent_type: body.agent_type,
        model: body.model,
        ollama_host: body.ollama_host,
        tools: body.tools,
        domains: body.domains,
        system_prompt: body.system_prompt,
        default_permissions: body.default_permissions,
        campaign_dir: body.campaign_dir,
        deployed_context: body.deployed_context,
        created_at: now,
        updated_at: now,
    };
    state.definitions.write().await.insert(def.id, def.clone());
    persist::save_definition(&state.data_dir, &def)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(def))
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<AgentDefinition>, ApiError> {
    let defs = state.definitions.read().await;
    defs.get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("definition {id} not found")))
}

pub async fn update(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(body): Json<DefinitionBody>,
) -> Result<Json<AgentDefinition>, ApiError> {
    let def = {
        let mut defs = state.definitions.write().await;
        let def = defs
            .get_mut(&id)
            .ok_or_else(|| ApiError::NotFound(format!("definition {id} not found")))?;
        def.name = body.name;
        def.description = body.description;
        def.agent_type = body.agent_type;
        def.model = body.model;
        def.ollama_host = body.ollama_host;
        def.tools = body.tools;
        def.domains = body.domains;
        def.system_prompt = body.system_prompt;
        def.default_permissions = body.default_permissions;
        def.campaign_dir = body.campaign_dir;
        def.deployed_context = body.deployed_context;
        def.updated_at = Utc::now();
        def.clone()
    };
    persist::save_definition(&state.data_dir, &def)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(def))
}

pub async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<DeleteResponse>, ApiError> {
    let removed = state.definitions.write().await.remove(&id).is_some();
    if !removed {
        return Err(ApiError::NotFound(format!("definition {id} not found")));
    }
    persist::delete_definition(&state.data_dir, id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    Ok(Json(DeleteResponse { deleted: true }))
}
