use std::collections::{HashMap, HashSet};
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

/// Executes tools on behalf of agents, enforcing permission checks.
///
/// The ToolExecutor is the single point through which all tool invocations
/// must pass. It maintains a registry of available tools and checks that
/// the caller has been granted the necessary permissions before execution.
pub struct ToolExecutor {
    tools: HashMap<String, Box<dyn Tool>>,
}

impl ToolExecutor {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool with the executor.
    pub fn register(&mut self, tool: Box<dyn Tool>) {
        info!(tool = tool.name(), "Registered tool");
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// List all registered tool names.
    pub fn available_tools(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Execute a tool by name, checking permissions first.
    pub async fn execute(
        &self,
        tool_name: &str,
        task_id: Uuid,
        parameters: serde_json::Value,
        granted_permissions: &HashSet<Permission>,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| ToolError::ExecutionFailed(format!("unknown tool: {tool_name}")))?;

        // Check permissions
        for required in tool.required_permissions() {
            if !granted_permissions.contains(&required) {
                warn!(
                    tool = tool_name,
                    permission = ?required,
                    "Permission denied for tool execution"
                );
                return Ok(ToolResult::denied(
                    task_id,
                    tool_name,
                    format!("requires permission: {required:?}"),
                ));
            }
        }

        tool.execute(task_id, parameters).await
    }
}

impl Default for ToolExecutor {
    fn default() -> Self {
        Self::new()
    }
}
