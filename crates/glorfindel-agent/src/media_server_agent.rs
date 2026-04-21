use async_trait::async_trait;
use glorfindel_schemas::agent::{ActionRecord, AgentResponse, CapabilityManifest, ResourceRequirements};
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

/// An Ollama-backed agent that answers questions about a Jellyfin media server.
///
/// On each task it runs the standard tool-call agentic loop with access to
/// `media.*` tools for library queries, search, recent items, and sessions.
pub struct MediaServerAgent {
    agent_id: String,
    model: String,
    ollama_host: String,
    client: Client,
    tool_executor: ToolExecutor,
    server_name: String,
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

impl MediaServerAgent {
    pub fn new(
        agent_id: impl Into<String>,
        model: impl Into<String>,
        ollama_host: impl Into<String>,
        tool_executor: ToolExecutor,
        server_name: impl Into<String>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            model: model.into(),
            ollama_host: ollama_host.into(),
            client: Client::new(),
            tool_executor,
            server_name: server_name.into(),
        }
    }

    fn build_system_prompt(&self) -> String {
        let tool_list = self
            .tool_executor
            .available_tools()
            .iter()
            .map(|t| format!("- {t}"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"You are a helpful media server assistant for a Jellyfin instance called "{server_name}".

You help users:
- Find movies, TV shows, and other content in the library
- Discover what's been recently added
- Check what is currently being watched
- Get library statistics and an overview of what's available
- Make recommendations based on what's in the library

Guidelines:
- For search questions ("do you have X?", "find me X"), always use media.search first
- For "what's new?" or "recently added?" questions, use media.recent
- For "what's playing?" or "who's watching?", use media.sessions
- For library overviews or counts, use media.library
- Be concise and conversational — the user is likely on a couch
- When listing results, keep it scannable (name, year, one-line overview)

Available tools:
{tool_list}

To use a tool, respond with a JSON block:
```tool
{{"tool": "<tool_name>", "parameters": {{...}}, "justification": "why"}}
```

When you have finished, respond with:
```result
{{"result": "your response to the user"}}
```

Think step by step. Use tools to look up real data before answering."#,
            server_name = self.server_name,
            tool_list = tool_list,
        )
    }

    fn parse_tool_call(&self, text: &str) -> Option<(String, serde_json::Value, Option<String>)> {
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

    async fn chat(&self, messages: &[OllamaMessage]) -> Result<String, AgentError> {
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
impl Agent for MediaServerAgent {
    fn capability(&self) -> CapabilityManifest {
        CapabilityManifest {
            agent_id: self.agent_id.clone(),
            name: format!("Media Server Assistant ({})", self.model),
            agent_type: AgentType::Specialist,
            tools_available: self.tool_executor.available_tools(),
            model_backend: ModelBackend::Ollama {
                model: self.model.clone(),
                host: self.ollama_host.clone(),
            },
            domains: vec!["media".into(), "jellyfin".into()],
            resource_requirements: ResourceRequirements {
                gpu_required: true,
                min_memory_mb: Some(4096),
                min_vram_mb: Some(4096),
            },
        }
    }

    async fn handle_task(&self, task: TaskRequest) -> Result<AgentResponse, AgentError> {
        info!(task_id = %task.task_id, intent = %task.intent, "Media server agent handling task");

        let max_iterations = task.constraints.max_iterations.unwrap_or(20) as usize;
        let granted_permissions: HashSet<_> =
            task.constraints.granted_permissions.iter().cloned().collect();

        let mut messages = vec![OllamaMessage {
            role: "system".into(),
            content: self.build_system_prompt(),
        }];

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

        messages.push(OllamaMessage {
            role: "user".into(),
            content: task.intent.clone(),
        });

        let mut actions_taken = Vec::new();

        for iteration in 0..max_iterations {
            debug!(task_id = %task.task_id, iteration, "Media agent loop iteration");

            let response_text = self.chat(&messages).await?;

            if let Some(result) = self.parse_result(&response_text) {
                info!(task_id = %task.task_id, iterations = iteration + 1, "Media task complete");
                return Ok(AgentResponse {
                    task_id: task.task_id,
                    status: Status::Complete,
                    result,
                    actions_taken,
                    delegated_to: Vec::new(),
                });
            }

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

                let tool_result = self
                    .tool_executor
                    .execute(&tool_name, task.task_id, parameters, &granted_permissions)
                    .await
                    .map_err(|e| AgentError::ToolFailed(e.to_string()))?;

                actions_taken.push(ActionRecord {
                    tool_call,
                    tool_result: tool_result.clone(),
                });

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
                info!(task_id = %task.task_id, iterations = iteration + 1, "Media task complete (freeform)");
                return Ok(AgentResponse {
                    task_id: task.task_id,
                    status: Status::Complete,
                    result: serde_json::json!({ "result": response_text }),
                    actions_taken,
                    delegated_to: Vec::new(),
                });
            }
        }

        warn!(task_id = %task.task_id, "Media agent max iterations exceeded");
        Err(AgentError::MaxIterationsExceeded)
    }
}
