use std::path::{Path, PathBuf};

use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::{Permission, SideEffect};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

/// Allow relative sub-paths like "session4/scene.md" while blocking traversal.
fn safe_path(base: &Path, filename: &str) -> Option<PathBuf> {
    if filename.contains("..") || filename.contains('\\') || filename.starts_with('/') {
        return None;
    }
    let candidate = base.join(filename);
    // Verify the resolved path is still under base (symlink-safe check)
    if !candidate.starts_with(base) {
        return None;
    }
    Some(candidate)
}

// ---------------------------------------------------------------------------
// campaign.read
// ---------------------------------------------------------------------------

/// Reads a file from the campaign directory by bare filename.
pub struct CampaignReadTool {
    campaign_dir: PathBuf,
}

impl CampaignReadTool {
    pub fn new(campaign_dir: impl Into<PathBuf>) -> Self {
        Self {
            campaign_dir: campaign_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for CampaignReadTool {
    fn name(&self) -> &str {
        "campaign.read"
    }

    fn description(&self) -> &str {
        "Read a campaign file by filename (e.g. 'npcs.md', 'session_notes.md'). \
         Returns the full contents."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("campaign.read".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let filename = parameters
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("filename".into()))?;

        let path = safe_path(&self.campaign_dir, filename)
            .ok_or_else(|| ToolError::InvalidParameter(format!("invalid filename: {filename}")))?;

        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => Ok(ToolResult::success(
                task_id,
                "campaign.read",
                serde_json::json!({ "filename": filename, "contents": contents }),
            )),
            Err(e) => Ok(ToolResult::failure(task_id, "campaign.read", e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// campaign.write
// ---------------------------------------------------------------------------

/// Writes (or appends to) a file in the campaign directory.
pub struct CampaignWriteTool {
    campaign_dir: PathBuf,
}

impl CampaignWriteTool {
    pub fn new(campaign_dir: impl Into<PathBuf>) -> Self {
        Self {
            campaign_dir: campaign_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for CampaignWriteTool {
    fn name(&self) -> &str {
        "campaign.write"
    }

    fn description(&self) -> &str {
        "Write content to a campaign file. Set 'append' to true to add to the end \
         (useful for session notes). Omit or set false to overwrite."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("campaign.write".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let filename = parameters
            .get("filename")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("filename".into()))?;

        let content = parameters
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("content".into()))?;

        let append = parameters
            .get("append")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let path = safe_path(&self.campaign_dir, filename)
            .ok_or_else(|| ToolError::InvalidParameter(format!("invalid filename: {filename}")))?;

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let write_result: Result<(), std::io::Error> = if append {
            let mut file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await?;
            file.write_all(content.as_bytes()).await?;
            Ok(())
        } else {
            tokio::fs::write(&path, content).await
        };

        match write_result {
            Ok(()) => {
                let mut result = ToolResult::success(
                    task_id,
                    "campaign.write",
                    serde_json::json!({ "filename": filename, "append": append }),
                );
                result.side_effects.push(SideEffect {
                    kind: "campaign_file_written".into(),
                    description: format!(
                        "{} {} bytes to campaign file {filename}",
                        if append { "Appended" } else { "Wrote" },
                        content.len()
                    ),
                    path: Some(path.to_string_lossy().into_owned()),
                });
                Ok(result)
            }
            Err(e) => Ok(ToolResult::failure(task_id, "campaign.write", e.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// campaign.list
// ---------------------------------------------------------------------------

fn collect_files_recursive<'a>(
    base: &'a Path,
    dir: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<Vec<String>>> + Send + 'a>> {
    Box::pin(async move {
        let mut files = Vec::new();
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Ok(Some(entry)) = entries.next_entry().await {
            let ft = match entry.file_type().await {
                Ok(ft) => ft,
                Err(_) => continue,
            };
            let path = entry.path();
            if ft.is_file() {
                if let Ok(rel) = path.strip_prefix(base) {
                    if let Some(s) = rel.to_str() {
                        files.push(s.to_string());
                    }
                }
            } else if ft.is_dir() {
                let mut sub = collect_files_recursive(base, &path).await?;
                files.append(&mut sub);
            }
        }
        files.sort();
        Ok(files)
    })
}

/// Lists all files in the campaign directory (recursive).
pub struct CampaignListTool {
    campaign_dir: PathBuf,
}

impl CampaignListTool {
    pub fn new(campaign_dir: impl Into<PathBuf>) -> Self {
        Self {
            campaign_dir: campaign_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for CampaignListTool {
    fn name(&self) -> &str {
        "campaign.list"
    }

    fn description(&self) -> &str {
        "List all files in the campaign directory."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("campaign.read".into())]
    }

    async fn execute(&self, task_id: Uuid, _parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let files = collect_files_recursive(&self.campaign_dir, &self.campaign_dir)
            .await
            .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

        Ok(ToolResult::success(
            task_id,
            "campaign.list",
            serde_json::json!({ "files": files }),
        ))
    }
}
