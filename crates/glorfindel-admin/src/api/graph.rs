use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::State,
    Json,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::api::ApiError;
use crate::state::AppState;

// ── Shared markdown helpers (mirrors graph-stack's persistence.py) ────────────

fn parse_frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let mut meta = HashMap::new();
    if !text.starts_with("---\n") {
        return (meta, text.to_string());
    }
    let rest = &text[4..];
    let end = match rest.find("\n---\n") {
        Some(i) => i,
        None => return (meta, text.to_string()),
    };
    let front = &rest[..end];
    let body = rest[end + 5..].trim_start_matches('\n').to_string();
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(": ") {
            meta.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    (meta, body)
}

fn format_frontmatter(meta: &HashMap<String, String>, body: &str) -> String {
    const KEY_ORDER: &[&str] = &["id", "type", "name", "created", "from", "to", "relationship"];
    let mut yaml = String::from("---\n");
    for key in KEY_ORDER {
        if let Some(val) = meta.get(*key) {
            yaml.push_str(&format!("{key}: {val}\n"));
        }
    }
    for (k, v) in meta {
        if !KEY_ORDER.contains(&k.as_str()) {
            yaml.push_str(&format!("{k}: {v}\n"));
        }
    }
    yaml.push_str("---\n");
    if !body.trim().is_empty() {
        yaml.push('\n');
        yaml.push_str(body.trim());
        yaml.push('\n');
    }
    yaml
}

fn meta_to_json(meta: HashMap<String, String>, body: String) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    for (k, v) in meta {
        obj.insert(k, serde_json::Value::String(v));
    }
    obj.insert("body".to_string(), serde_json::Value::String(body));
    serde_json::Value::Object(obj)
}

// ── GET /api/graph/nodes ──────────────────────────────────────────────────────

pub async fn list_nodes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let nodes_dir = state.data_dir.join("nodes");
    let _ = tokio::fs::create_dir_all(&nodes_dir).await;

    let mut nodes = Vec::new();
    let mut rd = match tokio::fs::read_dir(&nodes_dir).await {
        Ok(r) => r,
        Err(e) => return Err(ApiError::Internal(format!("read nodes dir: {e}"))),
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            let (meta, body) = parse_frontmatter(&text);
            nodes.push(meta_to_json(meta, body));
        }
    }
    nodes.sort_by_key(|n| n.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string());
    Ok(Json(nodes))
}

// ── GET /api/graph/edges ──────────────────────────────────────────────────────

pub async fn list_edges(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    let edges_dir = state.data_dir.join("edges");
    let _ = tokio::fs::create_dir_all(&edges_dir).await;

    let mut edges = Vec::new();
    let mut rd = match tokio::fs::read_dir(&edges_dir).await {
        Ok(r) => r,
        Err(e) => return Err(ApiError::Internal(format!("read edges dir: {e}"))),
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            let (meta, body) = parse_frontmatter(&text);
            edges.push(meta_to_json(meta, body));
        }
    }
    edges.sort_by_key(|e| e.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string());
    Ok(Json(edges))
}

// ── POST /api/graph/nodes ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateNodeBody {
    pub node_id: String,
    #[serde(rename = "type")]
    pub node_type: Option<String>,
    pub name: Option<String>,
    pub body: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

pub async fn create_node(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateNodeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let nodes_dir = state.data_dir.join("nodes");
    tokio::fs::create_dir_all(&nodes_dir)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let node_id = body.node_id.trim().to_string();
    if node_id.is_empty() {
        return Err(ApiError::BadRequest("node_id is required".into()));
    }

    let mut meta: HashMap<String, String> = HashMap::new();
    meta.insert("id".to_string(), node_id.clone());
    if let Some(t) = &body.node_type {
        meta.insert("type".to_string(), t.clone());
    }
    if let Some(n) = &body.name {
        meta.insert("name".to_string(), n.clone());
    }
    meta.insert("created".to_string(), Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string());
    for (k, v) in &body.extra {
        if let Some(s) = v.as_str() {
            meta.insert(k.clone(), s.to_string());
        }
    }

    let body_text = body.body.clone().unwrap_or_default();
    let name_display = meta.get("name").cloned().unwrap_or_else(|| node_id.clone());
    let full_body = if body_text.is_empty() {
        format!("# {name_display}\n")
    } else {
        format!("# {name_display}\n\n{body_text}")
    };

    let content = format_frontmatter(&meta, &full_body);
    let path = nodes_dir.join(format!("{node_id}.md"));
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(meta_to_json(meta, full_body)))
}

// ── POST /api/graph/edges ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateEdgeBody {
    pub edge_id: Option<String>,
    pub from_id: String,
    pub to_id: String,
    pub relationship: Option<String>,
    pub body: Option<String>,
}

pub async fn create_edge(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateEdgeBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let edges_dir = state.data_dir.join("edges");
    tokio::fs::create_dir_all(&edges_dir)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let edge_id = body
        .edge_id
        .clone()
        .unwrap_or_else(|| format!("edge-{}", Uuid::new_v4().simple()));
    let relationship = body.relationship.clone().unwrap_or_else(|| "related".into());

    let mut meta: HashMap<String, String> = HashMap::new();
    meta.insert("id".to_string(), edge_id.clone());
    meta.insert("from".to_string(), body.from_id.clone());
    meta.insert("to".to_string(), body.to_id.clone());
    meta.insert("relationship".to_string(), relationship.clone());
    meta.insert("created".to_string(), Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string());

    let body_text = body.body.clone().unwrap_or_default();
    let content = format_frontmatter(&meta, &body_text);
    let path = edges_dir.join(format!("{edge_id}.md"));
    tokio::fs::write(&path, content)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(meta_to_json(meta, body_text)))
}

// ── POST /api/graph/ingest ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct IngestBody {
    pub filename: String,
    pub content: String,
    /// Agent domain to route to; defaults to "file-classification".
    pub domain: Option<String>,
    /// If set, used as the task intent verbatim instead of the default excerpt format.
    pub intent_override: Option<String>,
    /// Override the default max_iterations (12).
    pub max_iterations: Option<u32>,
}

#[derive(Serialize)]
pub struct IngestResponse {
    pub task_id: Uuid,
    pub agent_instance_id: Uuid,
}

pub async fn ingest_file(
    State(state): State<Arc<AppState>>,
    Json(body): Json<IngestBody>,
) -> Result<Json<IngestResponse>, ApiError> {
    let domain = body.domain.as_deref().unwrap_or("file-classification");

    let (instance_id, agent, agent_name, definition_id) = {
        let running = state.running.read().await;
        let instances = state.instances.read().await;
        running
            .iter()
            .find(|(_, info)| info.capability.domains.iter().any(|d| d == domain))
            .and_then(|(id, info)| {
                instances.get(id).map(|a| {
                    (*id, a.clone(), info.definition_name.clone(), info.definition_id)
                })
            })
            .ok_or_else(|| ApiError::BadRequest(
                format!("No running agent for domain '{domain}'. Spawn it first."),
            ))?
    };

    let model = {
        let defs = state.definitions.read().await;
        defs.get(&definition_id)
            .map(|d| d.model.clone())
            .unwrap_or_else(|| "unknown".into())
    };

    let task_id = Uuid::new_v4();
    let intent = if let Some(override_intent) = body.intent_override.clone() {
        override_intent
    } else {
        let excerpt: String = body.content.chars().take(600).collect();
        format!("File: {}\n\nExcerpt:\n{}", body.filename, excerpt)
    };

    use glorfindel_schemas::task::{TaskConstraints, TaskRequest};
    use glorfindel_schemas::types::{Permission, Status};
    use crate::state::{TaskEvent, TaskEventKind, TaskRecord};

    let task_request = TaskRequest {
        task_id,
        parent_task_id: None,
        intent: intent.clone(),
        context: vec![],
        constraints: TaskConstraints {
            granted_permissions: vec![Permission::Custom("graph.write".into())],
            max_iterations: Some(body.max_iterations.unwrap_or(12)),
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
                agent_instance_id: instance_id,
                agent_name: agent_name.clone(),
                intent,
                status: Status::InProgress,
                submitted_at: Utc::now(),
                completed_at: None,
                response: None,
                error: None,
            },
        );
    }

    let state_clone = state.clone();
    let filename_clone = body.filename.clone();
    tokio::spawn(async move {
        // DDS control-plane: broadcast agent assignment before inference begins.
        let _ = state_clone.task_events.send(TaskEvent {
            task_id,
            kind: TaskEventKind::AgentSpawned {
                name: agent_name.clone(),
                model: model.clone(),
                context: format!("classifying: {filename_clone}"),
            },
        });

        let result = agent.handle_task(task_request).await;

        // Update task record; drop lock before emitting further events.
        {
            let mut tasks = state_clone.tasks.write().await;
            if let Some(record) = tasks.get_mut(&task_id) {
                match &result {
                    Ok(response) => {
                        record.status = Status::Complete;
                        record.completed_at = Some(Utc::now());
                        record.response = Some(response.clone());
                    }
                    Err(e) => {
                        record.status = Status::Failed;
                        record.completed_at = Some(Utc::now());
                        record.error = Some(e.to_string());
                    }
                }
            }
        }

        // Emit ZMQ data-plane events: one per tool call, then terminal event.
        match result {
            Ok(response) => {
                for action in &response.actions_taken {
                    let input: String = serde_json::to_string(&action.tool_call.parameters)
                        .unwrap_or_default()
                        .chars()
                        .take(300)
                        .collect();
                    let output: String = if let Some(err) = &action.tool_result.error {
                        format!("error: {err}")
                    } else {
                        serde_json::to_string(&action.tool_result.output)
                            .unwrap_or_default()
                            .chars()
                            .take(300)
                            .collect()
                    };
                    let _ = state_clone.task_events.send(TaskEvent {
                        task_id,
                        kind: TaskEventKind::ToolCall {
                            tool: action.tool_call.tool_name.clone(),
                            input,
                            output,
                        },
                    });
                }
                let _ = state_clone.task_events.send(TaskEvent {
                    task_id,
                    kind: TaskEventKind::Complete { response },
                });
            }
            Err(e) => {
                let _ = state_clone.task_events.send(TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed { message: e.to_string() },
                });
            }
        }
    });

    Ok(Json(IngestResponse { task_id, agent_instance_id: instance_id }))
}
