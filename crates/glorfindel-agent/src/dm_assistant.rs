use std::path::PathBuf;

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

/// Files automatically loaded as campaign context at the start of each task.
const CONTEXT_FILES: &[&str] = &["world.md", "players.md", "npcs.md", "locations.md"];

/// An Ollama-backed agent specialized for helping a human DM run a campaign.
///
/// On each task it:
/// 1. Reads CONTEXT_FILES from the campaign directory to ground its knowledge
/// 2. Injects them into the system prompt
/// 3. Runs the standard tool-call agentic loop
///
/// It has access to campaign.* tools for persistent note-taking and
/// rulebook.search for RAG-based rule citations.
pub struct DmAssistantAgent {
    agent_id: String,
    model: String,
    ollama_host: String,
    client: Client,
    tool_executor: ToolExecutor,
    campaign_dir: PathBuf,
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

impl DmAssistantAgent {
    pub fn new(
        agent_id: impl Into<String>,
        model: impl Into<String>,
        ollama_host: impl Into<String>,
        tool_executor: ToolExecutor,
        campaign_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            agent_id: agent_id.into(),
            model: model.into(),
            ollama_host: ollama_host.into(),
            client: Client::new(),
            tool_executor,
            campaign_dir: campaign_dir.into(),
        }
    }

    /// Load key campaign files and return them as a formatted string.
    async fn load_campaign_context(&self) -> String {
        let mut context = String::new();
        for filename in CONTEXT_FILES {
            let path = self.campaign_dir.join(filename);
            if let Ok(contents) = tokio::fs::read_to_string(&path).await {
                if !contents.trim().is_empty() {
                    context.push_str(&format!("=== {filename} ===\n{contents}\n\n"));
                }
            }
        }
        if context.is_empty() {
            "No campaign files found yet. Use campaign.write to start building them.".to_string()
        } else {
            context
        }
    }

    fn build_system_prompt(&self, campaign_context: &str) -> String {
        let tool_list = self
            .tool_executor
            .available_tools()
            .iter()
            .map(|t| format!("- {t}"))
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            r#"You are an expert TTRPG Dungeon Master assistant helping a human DM run their campaign.

Your responsibilities:
- Answer rules questions accurately, using rulebook.search to find and cite the specific rule text
- Help the DM track campaign details by reading and updating campaign files
- Suggest narrative developments, NPC motivations, and encounter ideas
- Help the DM improvise responses to unexpected player actions
- When a player is being snooty about a ruling, find the exact rule text and cite it precisely

Guidelines:
- When answering a rules question, always use rulebook.search first — quote the returned text directly in your response with its citation (e.g. "[PHB.txt, section 12]")
- For campaign updates (NPCs met, locations discovered, session events), use campaign.write with append:true on session_notes.md or the relevant file
- For recalling campaign details, use campaign.read on the relevant file
- Keep responses practical and DM-focused — the human is at the table and needs quick, actionable answers

Current campaign context (preloaded):
{campaign_context}

Available tools:
{tool_list}

To use a tool, respond with a JSON block:
```tool
{{"tool": "<tool_name>", "parameters": {{...}}, "justification": "why"}}
```

When you have finished, respond with:
```result
{{"result": "your full response to the DM"}}
```

Think step by step. Cite rulebook sections directly. Be a great co-DM."#,
            campaign_context = campaign_context,
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
impl Agent for DmAssistantAgent {
    fn capability(&self) -> CapabilityManifest {
        CapabilityManifest {
            agent_id: self.agent_id.clone(),
            name: format!("DM Assistant ({})", self.model),
            agent_type: AgentType::Specialist,
            tools_available: self.tool_executor.available_tools(),
            model_backend: ModelBackend::Ollama {
                model: self.model.clone(),
                host: self.ollama_host.clone(),
            },
            domains: vec!["dm-assistant".into(), "ttrpg".into()],
            resource_requirements: ResourceRequirements {
                gpu_required: true,
                min_memory_mb: Some(4096),
                min_vram_mb: Some(4096),
            },
        }
    }

    async fn handle_task(&self, task: TaskRequest) -> Result<AgentResponse, AgentError> {
        info!(task_id = %task.task_id, intent = %task.intent, "DM assistant handling task");

        let max_iterations = task.constraints.max_iterations.unwrap_or(20) as usize;
        let granted_permissions: HashSet<_> =
            task.constraints.granted_permissions.iter().cloned().collect();

        // Load campaign context fresh for each task
        let campaign_context = self.load_campaign_context().await;
        let system_prompt = self.build_system_prompt(&campaign_context);

        let mut messages = vec![OllamaMessage {
            role: "system".into(),
            content: system_prompt,
        }];

        // Inject any extra context the caller provided
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
            debug!(task_id = %task.task_id, iteration, "DM assistant loop iteration");

            let response_text = self.chat(&messages).await?;

            if let Some(result) = self.parse_result(&response_text) {
                info!(task_id = %task.task_id, iterations = iteration + 1, "DM task complete");
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
                // No structured output — treat raw response as the answer
                info!(task_id = %task.task_id, iterations = iteration + 1, "DM task complete (freeform response)");
                return Ok(AgentResponse {
                    task_id: task.task_id,
                    status: Status::Complete,
                    result: serde_json::json!({ "result": response_text }),
                    actions_taken,
                    delegated_to: Vec::new(),
                });
            }
        }

        warn!(task_id = %task.task_id, "DM assistant max iterations exceeded");
        Err(AgentError::MaxIterationsExceeded)
    }
}
