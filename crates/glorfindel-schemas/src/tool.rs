use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{SideEffect, Status};

/// A request from an agent to execute a tool, sent over the ZMQ data plane.
///
/// ToolCalls are the mechanism by which agents interact with the outside world.
/// They flow through the ToolExecutor, which checks permissions before execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// The task this tool call is part of.
    pub task_id: Uuid,
    /// The agent making the call.
    pub agent_id: String,
    /// Name of the tool to invoke (e.g., "file.read", "bash.exec").
    pub tool_name: String,
    /// Tool-specific parameters.
    pub parameters: serde_json::Value,
    /// Why the agent chose to invoke this tool (audit trail).
    pub justification: Option<String>,
}

/// The result of executing a tool, returned over the ZMQ data plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    /// The task this result belongs to.
    pub task_id: Uuid,
    /// The tool that was executed.
    pub tool_name: String,
    /// Whether execution succeeded.
    pub status: Status,
    /// The output produced by the tool.
    pub output: serde_json::Value,
    /// Any side effects produced (files modified, processes started, etc.).
    pub side_effects: Vec<SideEffect>,
    /// Error message if status is Failed or Denied.
    pub error: Option<String>,
}

impl ToolResult {
    pub fn success(task_id: Uuid, tool_name: impl Into<String>, output: serde_json::Value) -> Self {
        Self {
            task_id,
            tool_name: tool_name.into(),
            status: Status::Complete,
            output,
            side_effects: Vec::new(),
            error: None,
        }
    }

    pub fn failure(
        task_id: Uuid,
        tool_name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            task_id,
            tool_name: tool_name.into(),
            status: Status::Failed,
            output: serde_json::Value::Null,
            side_effects: Vec::new(),
            error: Some(error.into()),
        }
    }

    pub fn denied(task_id: Uuid, tool_name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            task_id,
            tool_name: tool_name.into(),
            status: Status::Denied,
            output: serde_json::Value::Null,
            side_effects: Vec::new(),
            error: Some(reason.into()),
        }
    }
}
