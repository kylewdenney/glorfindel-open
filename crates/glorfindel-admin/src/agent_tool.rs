use std::sync::Arc;

use async_trait::async_trait;
use glorfindel_agent::Agent;
use glorfindel_schemas::task::{TaskConstraints, TaskRequest};
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use glorfindel_tools::{Tool, ToolError};
use uuid::Uuid;

/// Wraps a sub-agent as a callable tool.
///
/// The orchestrator calls it like any other tool:
///   {"action":"agent.rule-consultant","query":"DC for Unfriendly NPC persuasion","justification":"..."}
///
/// The AgentTool submits that query as a full TaskRequest to the sub-agent and
/// returns its result as tool output.
pub struct AgentTool {
    name: String,
    description: String,
    agent: Arc<dyn Agent>,
    granted_permissions: Vec<Permission>,
}

impl AgentTool {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        agent: Arc<dyn Agent>,
        granted_permissions: Vec<Permission>,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            agent,
            granted_permissions,
        }
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn required_permissions(&self) -> Vec<Permission> {
        // AgentTools are gated by the orchestrator's own permissions —
        // no additional permissions required to *invoke* one.
        vec![]
    }

    async fn execute(&self, parent_task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        let query = parameters
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("query".into()))?;

        let sub_task_id = Uuid::new_v4();
        let task = TaskRequest {
            task_id: sub_task_id,
            parent_task_id: Some(parent_task_id),
            intent: query.to_string(),
            context: vec![],
            constraints: TaskConstraints {
                granted_permissions: self.granted_permissions.clone(),
                max_iterations: Some(6),
                ..Default::default()
            },
            reply_to: format!("agent-tool:{}", self.name),
        };

        match self.agent.handle_task(task).await {
            Ok(response) => {
                // Summarise what the sub-agent did for the orchestrator's context
                let result_text = match &response.result {
                    serde_json::Value::String(s) => s.clone(),
                    other => serde_json::to_string_pretty(other).unwrap_or_default(),
                };

                // Include raw tool outputs so citations / rolls bubble up
                let tool_outputs: Vec<serde_json::Value> = response
                    .actions_taken
                    .iter()
                    .map(|a| serde_json::json!({
                        "tool": a.tool_call.tool_name,
                        "output": a.tool_result.output,
                    }))
                    .collect();

                Ok(ToolResult::success(
                    parent_task_id,
                    &self.name,
                    serde_json::json!({
                        "agent": self.name,
                        "answer": result_text,
                        "tool_outputs": tool_outputs,
                    }),
                ))
            }
            Err(e) => Ok(ToolResult::failure(parent_task_id, &self.name, e.to_string())),
        }
    }
}
