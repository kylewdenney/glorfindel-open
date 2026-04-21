use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use glorfindel_agent::{Agent, OllamaAgent};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::ApiError;
use crate::state::{AppState, Deployment, RunningAgentInfo};
use crate::tools::build_executor;

#[derive(Debug, Deserialize)]
pub struct SpawnBody {
    pub definition_id: Uuid,
    pub deployment: Deployment,
}

#[derive(Serialize)]
pub struct StopResponse {
    pub stopped: bool,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<RunningAgentInfo>> {
    let running = state.running.read().await;
    let mut list: Vec<RunningAgentInfo> = running.values().cloned().collect();
    list.sort_by_key(|a| a.started_at);
    Json(list)
}

pub async fn spawn(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SpawnBody>,
) -> Result<Json<RunningAgentInfo>, ApiError> {
    let def = {
        let defs = state.definitions.read().await;
        defs.get(&body.definition_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("definition {} not found", body.definition_id)))?
    };

    let instance_id = Uuid::new_v4();
    let defs_snapshot = state.definitions.read().await.clone();
    let executor = build_executor(&def.tools, def.campaign_dir.as_deref(), def.deployed_context.as_ref(), Some(&defs_snapshot)).await;

    let mut agent = OllamaAgent::new(
        instance_id.to_string(),
        &def.model,
        &def.ollama_host,
        executor,
        def.domains.clone(),
    )
    .with_name(&def.name)
    .with_agent_type(def.agent_type.clone());

    if let Some(ref prompt) = def.system_prompt {
        agent = agent.with_system_prompt(prompt);
    }

    let capability = agent.capability();
    let info = RunningAgentInfo {
        instance_id,
        definition_id: def.id,
        definition_name: def.name.clone(),
        deployment: body.deployment,
        capability,
        started_at: Utc::now(),
        task_count: 0,
    };

    state
        .instances
        .write()
        .await
        .insert(instance_id, Arc::new(agent));
    state.running.write().await.insert(instance_id, info.clone());

    Ok(Json(info))
}

pub async fn stop(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<StopResponse>, ApiError> {
    let removed = state.running.write().await.remove(&id).is_some();
    state.instances.write().await.remove(&id);
    if removed {
        Ok(Json(StopResponse { stopped: true }))
    } else {
        Err(ApiError::NotFound(format!("agent {id} not found")))
    }
}
