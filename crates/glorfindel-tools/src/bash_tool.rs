use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::{Permission, SideEffect};
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

/// Executes shell commands in a sandboxed environment.
pub struct BashTool {
    /// Maximum execution time in seconds.
    timeout_secs: u64,
}

impl BashTool {
    pub fn new(timeout_secs: u64) -> Self {
        Self { timeout_secs }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new(30)
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash.exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output"
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::BashExec]
    }

    async fn execute(
        &self,
        task_id: Uuid,
        parameters: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let command = parameters
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("command".into()))?;

        let working_dir = parameters
            .get("working_dir")
            .and_then(|v| v.as_str());

        let timeout = std::time::Duration::from_secs(self.timeout_secs);

        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(command);

        if let Some(dir) = working_dir {
            cmd.current_dir(dir);
        }

        let result = tokio::time::timeout(timeout, cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                let exit_code = output.status.code().unwrap_or(-1);

                let mut tool_result = ToolResult::success(
                    task_id,
                    "bash.exec",
                    serde_json::json!({
                        "stdout": stdout,
                        "stderr": stderr,
                        "exit_code": exit_code,
                    }),
                );

                tool_result.side_effects.push(SideEffect {
                    kind: "command_executed".into(),
                    description: format!("Executed: {command}"),
                    path: working_dir.map(String::from),
                });

                if exit_code != 0 {
                    tool_result.status = glorfindel_schemas::types::Status::Failed;
                    tool_result.error = Some(format!("exit code {exit_code}: {stderr}"));
                }

                Ok(tool_result)
            }
            Ok(Err(e)) => Ok(ToolResult::failure(task_id, "bash.exec", e.to_string())),
            Err(_) => Ok(ToolResult::failure(
                task_id,
                "bash.exec",
                format!("command timed out after {}s", self.timeout_secs),
            )),
        }
    }
}
