use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{Permission, Priority};

/// A request to perform a task, published on the DDS control plane.
///
/// TaskRequests are the primary way work enters the system. The orchestrator
/// receives these, matches them against agent capabilities, and routes them
/// to the best available agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRequest {
    /// Unique identifier for this task.
    pub task_id: Uuid,
    /// If this is a sub-task, the parent task that spawned it.
    pub parent_task_id: Option<Uuid>,
    /// Natural language description of what needs to be done.
    pub intent: String,
    /// Contextual information the agent may need (prior messages, file contents, etc.).
    pub context: Vec<ContextEntry>,
    /// Constraints on how the task may be executed.
    pub constraints: TaskConstraints,
    /// DDS topic or ZMQ address where results should be sent.
    pub reply_to: String,
}

/// A piece of context provided with a task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextEntry {
    /// What kind of context this is.
    pub role: ContextRole,
    /// The content of this context entry.
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextRole {
    System,
    User,
    Assistant,
    ToolOutput,
}

/// Constraints governing task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConstraints {
    /// Maximum time (in seconds) the agent may spend on this task.
    pub timeout_secs: Option<u64>,
    /// Tools the agent is permitted to use. Empty means all registered tools.
    pub allowed_tools: Vec<String>,
    /// Permissions granted for this task's tool execution.
    pub granted_permissions: Vec<Permission>,
    /// Priority for scheduling.
    #[serde(default)]
    pub priority: Priority,
    /// Maximum number of agentic loop iterations.
    pub max_iterations: Option<u32>,
}

impl Default for TaskConstraints {
    fn default() -> Self {
        Self {
            timeout_secs: Some(300),
            allowed_tools: Vec::new(),
            granted_permissions: Vec::new(),
            priority: Priority::Normal,
            max_iterations: Some(20),
        }
    }
}

impl TaskRequest {
    pub fn new(intent: impl Into<String>) -> Self {
        Self {
            task_id: Uuid::new_v4(),
            parent_task_id: None,
            intent: intent.into(),
            context: Vec::new(),
            constraints: TaskConstraints::default(),
            reply_to: "glorfindel/tasks/response".into(),
        }
    }
}
