use serde::{Deserialize, Serialize};

/// Execution status for tasks and tool calls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    InProgress,
    Complete,
    Failed,
    Delegated,
    Blocked,
    Denied,
}

/// Priority level for task scheduling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

impl Default for Priority {
    fn default() -> Self {
        Self::Normal
    }
}

/// Classification of agent types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentType {
    /// Plans and decomposes tasks into sub-tasks.
    Planner,
    /// Executes tasks directly using tools.
    Executor,
    /// Specializes in a specific domain.
    Specialist,
}

/// What model backend powers an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelBackend {
    Ollama { model: String, host: String },
    Custom { name: String, endpoint: String },
}

/// Permission required to execute a tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    FileRead,
    FileWrite,
    BashExec,
    NetworkAccess,
    Custom(String),
}

/// A side effect produced by tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideEffect {
    pub kind: String,
    pub description: String,
    pub path: Option<String>,
}
