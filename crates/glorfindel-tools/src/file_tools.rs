use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::{Permission, SideEffect};
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

/// Reads file contents from disk.
pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file.read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file at a given path"
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::FileRead]
    }

    async fn execute(
        &self,
        task_id: Uuid,
        parameters: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let path = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("path".into()))?;

        match tokio::fs::read_to_string(path).await {
            Ok(contents) => Ok(ToolResult::success(
                task_id,
                "file.read",
                serde_json::json!({ "contents": contents, "path": path }),
            )),
            Err(e) => Ok(ToolResult::failure(task_id, "file.read", e.to_string())),
        }
    }
}

/// Writes content to a file on disk.
pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file.write"
    }

    fn description(&self) -> &str {
        "Write content to a file at a given path"
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::FileWrite]
    }

    async fn execute(
        &self,
        task_id: Uuid,
        parameters: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let path = parameters
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("path".into()))?;

        let content = parameters
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("content".into()))?;

        match tokio::fs::write(path, content).await {
            Ok(()) => {
                let mut result =
                    ToolResult::success(task_id, "file.write", serde_json::json!({ "path": path }));
                result.side_effects.push(SideEffect {
                    kind: "file_written".into(),
                    description: format!("Wrote {} bytes to {path}", content.len()),
                    path: Some(path.to_string()),
                });
                Ok(result)
            }
            Err(e) => Ok(ToolResult::failure(task_id, "file.write", e.to_string())),
        }
    }
}
