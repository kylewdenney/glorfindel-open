use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use glorfindel_agent::Agent;
use glorfindel_schemas::task::{TaskConstraints, TaskRequest};
use glorfindel_schemas::types::{Permission, Status};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::ApiError;
use crate::state::{AppState, TaskEvent, TaskEventKind, TaskRecord};

#[derive(Debug, Deserialize)]
pub struct SubmitTaskBody {
    pub agent_instance_id: Uuid,
    pub intent: String,
    pub permissions: Vec<Permission>,
    pub max_iterations: Option<u32>,
}

#[derive(Serialize)]
pub struct SubmitTaskResponse {
    pub task_id: Uuid,
}

pub async fn list(
    State(state): State<Arc<AppState>>,
) -> Json<Vec<TaskRecord>> {
    let tasks = state.tasks.read().await;
    let mut list: Vec<TaskRecord> = tasks.values().cloned().collect();
    list.sort_by(|a, b| b.submitted_at.cmp(&a.submitted_at));
    Json(list)
}

pub async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<TaskRecord>, ApiError> {
    let tasks = state.tasks.read().await;
    tasks
        .get(&id)
        .cloned()
        .map(Json)
        .ok_or_else(|| ApiError::NotFound(format!("task {id} not found")))
}

pub async fn submit(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubmitTaskBody>,
) -> Result<Json<SubmitTaskResponse>, ApiError> {
    let agent: Arc<dyn Agent> = {
        let instances = state.instances.read().await;
        instances
            .get(&body.agent_instance_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("agent {} not found", body.agent_instance_id)))?
    };

    let agent_name = {
        let running = state.running.read().await;
        running
            .get(&body.agent_instance_id)
            .map(|a| a.definition_name.clone())
            .unwrap_or_else(|| "Unknown".into())
    };

    let task_id = Uuid::new_v4();
    let task_request = TaskRequest {
        task_id,
        parent_task_id: None,
        intent: body.intent.clone(),
        context: vec![],
        constraints: TaskConstraints {
            granted_permissions: body.permissions,
            max_iterations: body.max_iterations.or(Some(20)),
            ..Default::default()
        },
        reply_to: "admin".into(),
    };

    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(
            task_id,
            TaskRecord {
                task_id,
                agent_instance_id: body.agent_instance_id,
                agent_name: agent_name.clone(),
                intent: body.intent,
                status: Status::InProgress,
                submitted_at: Utc::now(),
                completed_at: None,
                response: None,
                error: None,
            },
        );
    }

    // Increment task count
    {
        let mut running = state.running.write().await;
        if let Some(info) = running.get_mut(&body.agent_instance_id) {
            info.task_count += 1;
        }
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        let result = agent.handle_task(task_request).await;

        let mut tasks = state_clone.tasks.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            match result {
                Ok(response) => {
                    record.status = Status::Complete;
                    record.completed_at = Some(Utc::now());
                    record.response = Some(response.clone());
                    let _ = state_clone.task_events.send(TaskEvent {
                        task_id,
                        kind: TaskEventKind::Complete { response },
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    record.status = Status::Failed;
                    record.completed_at = Some(Utc::now());
                    record.error = Some(msg.clone());
                    let _ = state_clone.task_events.send(TaskEvent {
                        task_id,
                        kind: TaskEventKind::Failed { message: msg },
                    });
                }
            }
        }
    });

    Ok(Json(SubmitTaskResponse { task_id }))
}
