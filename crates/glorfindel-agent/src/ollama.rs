use async_trait::async_trait;
use glorfindel_schemas::agent::{
    ActionRecord, AgentResponse, CapabilityManifest, ResourceRequirements,
};
use glorfindel_schemas::task::{ContextRole, TaskRequest};
use glorfindel_schemas::tool::ToolCall;
use glorfindel_schemas::types::{AgentType, ModelBackend, Status};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tracing::{debug, info, warn};

use crate::error::AgentError;
use crate::traits::Agent;
use glorfindel_tools::ToolExecutor;

/// An agent backed by Ollama for local LLM inference.
///
/// Implements the full agentic loop: receives a task, calls Ollama for
/// inference, parses tool calls from the response, executes them via
/// the ToolExecutor, feeds results back, and iterates until done.
pub struct OllamaAgent {
    agent_id: String,
    model: String,
    ollama_host: String,
    client: Client,
    tool_executor: ToolExecutor,
    domains: Vec<String>,
}

#[derive(Debug, Serialize)]
struct OllamaChatRequest {
    model: String,
    messages: Vec<OllamaMessage>,
    stream: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct OllamaMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OllamaChatResponse {
    message: Option<OllamaMessage>,
}

impl OllamaAgent {
    pub fn new(
        agent_id: impl Into<String>,
        model: impl Into<String>,
        ollama_host: impl Into<String>,
        tool_executor: ToolExecutor,
        domains: Vec<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            model: model.into(),
            ollama_host: ollama_host.into(),
            client: Client::new(),
            tool_executor,
            domains,
        }
    }

    /// Build the system prompt that instructs the model how to use tools.
    fn build_system_prompt(&self) -> String {
        let tool_list: Vec<String> = self
            .tool_executor
            .available_tools()
            .iter()
            .map(|t| format!("- {t}"))
            .collect();

        format!(
            r#"You are an AI agent in the Glorfindel framework. You can use tools to accomplish tasks.

Available tools:
{}

To use a tool, respond with a JSON block:
```tool
{{"tool": "<tool_name>", "parameters": {{...}}, "justification": "why"}}
```

When you have completed the task, respond with:
```result
{{"result": "your final answer or output"}}
```

Always think step by step. Use tools when you need to interact with the system."#,
            tool_list.join("\n")
        )
    }

    /// Parse tool calls from the model's response text.
    fn parse_tool_call(&self, text: &str) -> Option<(String, serde_json::Value, Option<String>)> {
        // Look for ```tool ... ``` blocks
        if let Some(start) = text.find("```tool") {
            let after_marker = &text[start + 7..];
            if let Some(end) = after_marker.find("```") {
                let json_str = after_marker[..end].trim();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    let tool_name = parsed.get("tool")?.as_str()?.to_string();
                    let parameters = parsed.get("parameters")?.clone();
                    let justification = parsed
                        .get("justification")
                        .and_then(|j| j.as_str())
                        .map(String::from);
                    return Some((tool_name, parameters, justification));
                }
            }
        }
        None
    }

    /// Parse the final result from the model's response.
    fn parse_result(&self, text: &str) -> Option<serde_json::Value> {
        if let Some(start) = text.find("```result") {
            let after_marker = &text[start + 9..];
            if let Some(end) = after_marker.find("```") {
                let json_str = after_marker[..end].trim();
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json_str) {
                    return parsed.get("result").cloned();
                }
            }
        }
        None
    }

    /// Call Ollama's chat API.
    async fn chat(
        &self,
        messages: &[OllamaMessage],
    ) -> Result<String, AgentError> {
        let url = format!("{}/api/chat", self.ollama_host);
        let request = OllamaChatRequest {
            model: self.model.clone(),
            messages: messages.to_vec(),
            stream: false,
        };

        let resp: OllamaChatResponse = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await?
            .json()
            .await?;

        resp.message
            .map(|m| m.content)
            .ok_or_else(|| AgentError::InferenceFailed("empty response from Ollama".into()))
    }
}

#[async_trait]
impl Agent for OllamaAgent {
    fn capability(&self) -> CapabilityManifest {
        CapabilityManifest {
            agent_id: self.agent_id.clone(),
            name: format!("Ollama Agent ({})", self.model),
            agent_type: AgentType::Executor,
            tools_available: self.tool_executor.available_tools(),
            model_backend: ModelBackend::Ollama {
                model: self.model.clone(),
                host: self.ollama_host.clone(),
            },
            domains: self.domains.clone(),
            resource_requirements: ResourceRequirements {
                gpu_required: true,
                min_memory_mb: Some(4096),
                min_vram_mb: Some(4096),
            },
        }
    }

    async fn handle_task(&self, task: TaskRequest) -> Result<AgentResponse, AgentError> {
        info!(task_id = %task.task_id, intent = %task.intent, "Handling task");

        let max_iterations = task.constraints.max_iterations.unwrap_or(20) as usize;
        let granted_permissions: HashSet<_> =
            task.constraints.granted_permissions.iter().cloned().collect();

        // Build initial message history
        let mut messages = vec![OllamaMessage {
            role: "system".into(),
            content: self.build_system_prompt(),
        }];

        // Add task context
        for entry in &task.context {
            let role = match entry.role {
                ContextRole::System => "system",
                ContextRole::User => "user",
                ContextRole::Assistant => "assistant",
                ContextRole::ToolOutput => "user",
            };
            messages.push(OllamaMessage {
                role: role.into(),
                content: entry.content.clone(),
            });
        }

        // Add the task intent
        messages.push(OllamaMessage {
            role: "user".into(),
            content: task.intent.clone(),
        });

        let mut actions_taken = Vec::new();

        // Agentic loop
        for iteration in 0..max_iterations {
            debug!(task_id = %task.task_id, iteration, "Agent loop iteration");

            let response_text = self.chat(&messages).await?;

            // Check for final result
            if let Some(result) = self.parse_result(&response_text) {
                info!(task_id = %task.task_id, iterations = iteration + 1, "Task complete");
                return Ok(AgentResponse {
                    task_id: task.task_id,
                    status: Status::Complete,
                    result,
                    actions_taken,
                    delegated_to: Vec::new(),
                });
            }

            // Check for tool call
            if let Some((tool_name, parameters, justification)) =
                self.parse_tool_call(&response_text)
            {
                let tool_call = ToolCall {
                    task_id: task.task_id,
                    agent_id: self.agent_id.clone(),
                    tool_name: tool_name.clone(),
                    parameters: parameters.clone(),
                    justification,
                };

                // Execute the tool
                let tool_result = self
                    .tool_executor
                    .execute(&tool_name, task.task_id, parameters, &granted_permissions)
                    .await
                    .map_err(|e| AgentError::ToolFailed(e.to_string()))?;

                // Record the action
                actions_taken.push(ActionRecord {
                    tool_call,
                    tool_result: tool_result.clone(),
                });

                // Add assistant response and tool result to messages
                messages.push(OllamaMessage {
                    role: "assistant".into(),
                    content: response_text,
                });

                messages.push(OllamaMessage {
                    role: "user".into(),
                    content: format!(
                        "Tool result for {tool_name}:\n{}",
                        serde_json::to_string_pretty(&tool_result.output).unwrap_or_default()
                    ),
                });
            } else {
                // No tool call and no result — treat the response as the final answer
                info!(task_id = %task.task_id, iterations = iteration + 1, "Task complete (no structured output)");
                return Ok(AgentResponse {
                    task_id: task.task_id,
                    status: Status::Complete,
                    result: serde_json::json!({ "response": response_text }),
                    actions_taken,
                    delegated_to: Vec::new(),
                });
            }
        }

        warn!(task_id = %task.task_id, "Max iterations exceeded");
        Err(AgentError::MaxIterationsExceeded)
    }
}
