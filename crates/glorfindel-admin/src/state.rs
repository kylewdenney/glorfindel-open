use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use glorfindel_agent::Agent;
use glorfindel_schemas::agent::{AgentResponse, CapabilityManifest};
use glorfindel_schemas::types::{AgentType, Permission, Status};
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefinition {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub agent_type: AgentType,
    pub model: String,
    pub ollama_host: String,
    pub tools: Vec<String>,
    pub domains: Vec<String>,
    pub system_prompt: Option<String>,
    pub default_permissions: Vec<Permission>,
    /// Optional path to a campaign directory for campaign.* tools.
    pub campaign_dir: Option<String>,
    /// Flexible deployment context — tool-specific config (jellyfin creds, rulebook_dir, etc.)
    pub deployed_context: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Deployment {
    Test,
    Prod,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunningAgentInfo {
    pub instance_id: Uuid,
    pub definition_id: Uuid,
    pub definition_name: String,
    pub deployment: Deployment,
    pub capability: CapabilityManifest,
    pub started_at: DateTime<Utc>,
    pub task_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaskRecord {
    pub task_id: Uuid,
    pub agent_instance_id: Uuid,
    pub agent_name: String,
    pub intent: String,
    pub status: Status,
    pub submitted_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub response: Option<AgentResponse>,
    pub error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct TaskEvent {
    pub task_id: Uuid,
    #[serde(flatten)]
    pub kind: TaskEventKind,
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskEventKind {
    Started,
    Complete { response: AgentResponse },
    Failed { message: String },
    /// One step of the Think→Critic→Rules→DM pipeline
    PipelineStep { step: String, body: String },
    /// An agent instance was created as part of the pipeline
    AgentSpawned { name: String, model: String, context: String },
    /// A tool was invoked and returned a result
    ToolCall { tool: String, input: String, output: String },
    /// The server wrote a campaign file
    FileWrite { path: String, bytes: usize },
}

pub struct AppState {
    pub definitions: Arc<RwLock<HashMap<Uuid, AgentDefinition>>>,
    pub running: Arc<RwLock<HashMap<Uuid, RunningAgentInfo>>>,
    pub instances: Arc<RwLock<HashMap<Uuid, Arc<dyn Agent>>>>,
    pub tasks: Arc<RwLock<HashMap<Uuid, TaskRecord>>>,
    pub task_events: broadcast::Sender<TaskEvent>,
    pub data_dir: PathBuf,
}

impl AppState {
    pub fn new(data_dir: PathBuf) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self {
            definitions: Arc::new(RwLock::new(HashMap::new())),
            running: Arc::new(RwLock::new(HashMap::new())),
            instances: Arc::new(RwLock::new(HashMap::new())),
            tasks: Arc::new(RwLock::new(HashMap::new())),
            task_events: tx,
            data_dir,
        }
    }
}
