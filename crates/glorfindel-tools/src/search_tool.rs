use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

/// Searches file contents using grep-like pattern matching.
pub struct SearchTool;

#[async_trait]
impl Tool for SearchTool {
    fn name(&self) -> &str {
        "search.grep"
    }

    fn description(&self) -> &str {
        "Search for a pattern in files within a directory"
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::FileRead]
    }

    async fn execute(
        &self,
        task_id: Uuid,
        parameters: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let pattern = parameters
            .get("pattern")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("pattern".into()))?;

        let path = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or(".");

        let mut cmd = tokio::process::Command::new("grep");
        cmd.args(["-rn", "--include=*", pattern, path]);

        match cmd.output().await {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let lines: Vec<&str> = stdout.lines().take(100).collect();

                Ok(ToolResult::success(
                    task_id,
                    "search.grep",
                    serde_json::json!({
                        "matches": lines,
                        "pattern": pattern,
                        "path": path,
                        "truncated": stdout.lines().count() > 100,
                    }),
                ))
            }
            Err(e) => Ok(ToolResult::failure(task_id, "search.grep", e.to_string())),
        }
    }
}
