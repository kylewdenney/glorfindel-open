use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use uuid::Uuid;

use crate::error::ToolError;

/// A tool that an agent can invoke to interact with the outside world.
///
/// Tools are the mechanism by which agents take actions — reading files,
/// running commands, searching codebases, etc. Each tool declares what
/// permissions it requires, and the ToolExecutor checks these against
/// the task's granted permissions before execution.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Unique name for this tool (e.g., "file.read", "bash.exec").
    fn name(&self) -> &str;

    /// Human-readable description of what this tool does.
    fn description(&self) -> &str;

    /// Permissions required to execute this tool.
    fn required_permissions(&self) -> Vec<Permission>;

    /// Execute the tool with the given parameters.
    async fn execute(
        &self,
        task_id: Uuid,
        parameters: serde_json::Value,
    ) -> Result<ToolResult, ToolError>;
}
