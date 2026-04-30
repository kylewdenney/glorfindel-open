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

pub struct OllamaAgent {
    agent_id: String,
    name: String,
    agent_type: AgentType,
    model: String,
    ollama_host: String,
    client: Client,
    tool_executor: ToolExecutor,
    domains: Vec<String>,
    custom_system_prompt: Option<String>,
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

/// Structured action returned by the model on every turn.
enum AgentAction {
    ToolCall {
        tool: String,
        parameters: serde_json::Value,
        justification: Option<String>,
    },
    Result {
        result: serde_json::Value,
    },
}

impl OllamaAgent {
    pub fn new(
        agent_id: impl Into<String>,
        model: impl Into<String>,
        ollama_host: impl Into<String>,
        tool_executor: ToolExecutor,
        domains: Vec<String>,
    ) -> Self {
        let model = model.into();
        let name = format!("Ollama Agent ({})", model);
        Self {
            agent_id: agent_id.into(),
            name,
            agent_type: AgentType::Executor,
            model,
            ollama_host: ollama_host.into(),
            client: Client::new(),
            tool_executor,
            domains,
            custom_system_prompt: None,
        }
    }

    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    pub fn with_agent_type(mut self, agent_type: AgentType) -> Self {
        self.agent_type = agent_type;
        self
    }

    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.custom_system_prompt = Some(prompt.into());
        self
    }

    fn build_system_prompt(&self) -> String {
        let base = self
            .custom_system_prompt
            .clone()
            .unwrap_or_else(|| "You are an AI agent in the Glorfindel framework.".into());

        let tool_schemas: Vec<String> = self
            .tool_executor
            .available_tools()
            .iter()
            .map(|t| format!("  {}", Self::tool_schema_example(t)))
            .collect();

        let tools_section = if tool_schemas.is_empty() {
            "  (none available)".into()
        } else {
            tool_schemas.join("\n")
        };

        // Always append JSON dispatch contract. Use the flat schema Mistral
        // naturally produces in JSON mode: action = tool name, params inline.
        format!(
            r#"{base}

Respond ONLY with a raw JSON object each turn. Never include text outside the JSON.
Call tools one at a time and wait for the result before the next call.

To call a tool:
  {{"action":"<tool_name>","param1":"value1","justification":"<why>"}}

To return your final answer after all tool calls are complete:
  {{"action":"result","result":"<summary of what you did>"}}

Available tools (use exact parameter names shown):
{tools_section}"#
        )
    }

    fn tool_schema_example(tool_name: &str) -> &'static str {
        match tool_name {
            "campaign.write" => r#"campaign.write  → {"action":"campaign.write","filename":"name.md","content":"text","append":false,"justification":"why"}"#,
            "campaign.read"  => r#"campaign.read   → {"action":"campaign.read","filename":"name.md","justification":"why"}"#,
            "campaign.list"  => r#"campaign.list   → {"action":"campaign.list","justification":"why"}"#,
            "file.read"      => r#"file.read       → {"action":"file.read","path":"/path/to/file","justification":"why"}"#,
            "file.write"     => r#"file.write      → {"action":"file.write","path":"/path/to/file","content":"text","justification":"why"}"#,
            "bash.exec"      => r#"bash.exec       → {"action":"bash.exec","command":"shell command","justification":"why"}"#,
            "search.grep"    => r#"search.grep     → {"action":"search.grep","pattern":"regex","path":".","justification":"why"}"#,
            "rulebook.search"=> r#"rulebook.search → {"action":"rulebook.search","query":"rules question","justification":"why"}"#,
            "dice.roll"      => r#"dice.roll       → {"action":"dice.roll","notation":"2d6+3","justification":"why"}"#,
            "graph.query"    => r#"graph.query     → {"action":"graph.query","query":"search terms","justification":"why"}"#,
            "graph.node"     => r#"graph.node      → {"action":"graph.node","node_id":"mission-example","justification":"why"}"#,
            "graph.neighbors"=> r#"graph.neighbors → {"action":"graph.neighbors","node_id":"mission-example","hops":1,"justification":"why"}"#,
            "graph.add_node" => r#"graph.add_node  → {"action":"graph.add_node","node_id":"mission-deny-airspace","type":"mission","name":"Deny Airspace","body":"Description here.","justification":"why"}"#,
            "graph.add_edge" => r#"graph.add_edge  → {"action":"graph.add_edge","edge_id":"edge-mission-requires-cap","from_id":"mission-deny-airspace","to_id":"capability-electronic-warfare","relationship":"requires","justification":"why"}"#,
            n if n.starts_with("agent.") => r#"agent.<name>    → {"action":"agent.<name>","query":"question for sub-agent","justification":"why"}"#,
            _                => r#"unknown         → {"action":"<tool_name>","justification":"why"}"#,
        }
    }

    fn parse_action(&self, text: &str) -> Option<AgentAction> {
        let stripped = text
            .trim()
            .trim_start_matches("```json")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim();

        // Try full parse first; if that fails, scan for first '{' and try from there
        let mut value: serde_json::Value = serde_json::from_str(stripped).ok().or_else(|| {
            let start = stripped.find('{')?;
            // Try to find a matching closing brace by tracking depth
            let slice = &stripped[start..];
            let mut depth = 0i32;
            let mut end = 0;
            for (i, ch) in slice.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 { end = i + 1; break; }
                    }
                    _ => {}
                }
            }
            if end == 0 { return None; }
            serde_json::from_str(&slice[..end]).ok()
        })?;
        let action = value.get("action").and_then(|v| v.as_str())?.to_string();

        if action == "result" {
            let result = value
                .get("result")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            return Some(AgentAction::Result { result });
        }

        // Flat schema: action IS the tool name, remaining keys are parameters.
        let justification = value
            .as_object_mut()
            .and_then(|o| o.remove("justification"))
            .and_then(|v| v.as_str().map(String::from));
        if let Some(obj) = value.as_object_mut() {
            obj.remove("action");
        }

        Some(AgentAction::ToolCall {
            tool: action,
            parameters: value,
            justification,
        })
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
    /// If campaign tools are available and permitted, pre-read all campaign files
    /// and inject them as system context so the model has the data without
    /// needing to call campaign.read (which Mistral 7B reliably skips).
    async fn seed_campaign_context(
        &self,
        task: &TaskRequest,
        granted_permissions: &HashSet<glorfindel_schemas::types::Permission>,
    ) -> Option<String> {
        let tools = self.tool_executor.available_tools();
        let has_list = tools.iter().any(|t| t == "campaign.list");
        let has_read = tools.iter().any(|t| t == "campaign.read");
        if !has_list || !has_read {
            return None;
        }

        let list_result = self
            .tool_executor
            .execute(
                "campaign.list",
                task.task_id,
                serde_json::json!({}),
                granted_permissions,
            )
            .await
            .ok()?;

        let files: Vec<String> = list_result
            .output
            .get("files")
            .and_then(|f| f.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if files.is_empty() {
            return None;
        }

        // Prioritise session-ending/notes files first (most useful for continuity),
        // then fall back to recency order. Cap total injected chars at ~2000 to avoid
        // consuming the entire 4096-token context window of small models.
        const MAX_CONTEXT_CHARS: usize = 2000;

        let mut ordered = files.clone();
        ordered.sort_by(|a, b| {
            let a_score = if a.contains("ending") || a.contains("notes") { 0 } else { 1 };
            let b_score = if b.contains("ending") || b.contains("notes") { 0 } else { 1 };
            a_score.cmp(&b_score)
        });

        let mut context_parts = vec!["## Campaign Context (recent session files)\n".to_string()];
        let mut total_chars = 0usize;

        for filename in &ordered {
            if total_chars >= MAX_CONTEXT_CHARS {
                break;
            }
            let read_result = self
                .tool_executor
                .execute(
                    "campaign.read",
                    task.task_id,
                    serde_json::json!({ "filename": filename }),
                    granted_permissions,
                )
                .await
                .ok();

            if let Some(result) = read_result {
                if let Some(contents) = result.output.get("contents").and_then(|c| c.as_str()) {
                    let remaining = MAX_CONTEXT_CHARS.saturating_sub(total_chars);
                    let chunk = if contents.len() > remaining {
                        &contents[..remaining]
                    } else {
                        contents
                    };
                    let entry = format!("### {filename}\n{chunk}\n");
                    total_chars += entry.len();
                    context_parts.push(entry);
                }
            }
        }

        Some(context_parts.join("\n"))
    }
}

#[async_trait]
impl Agent for OllamaAgent {
    fn capability(&self) -> CapabilityManifest {
        CapabilityManifest {
            agent_id: self.agent_id.clone(),
            name: self.name.clone(),
            agent_type: self.agent_type.clone(),
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

        let mut messages = vec![OllamaMessage {
            role: "system".into(),
            content: self.build_system_prompt(),
        }];

        // Pre-seed campaign files as system context to avoid relying on the model
        // to call campaign.read (small models reliably skip reads, hallucinating instead).
        if let Some(campaign_ctx) = self.seed_campaign_context(&task, &granted_permissions).await {
            info!(task_id = %task.task_id, "Seeded campaign context from disk");
            messages.push(OllamaMessage {
                role: "system".into(),
                content: campaign_ctx,
            });
        }

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
        // Track filenames written so we can detect re-write loops.
        let mut written_files: HashSet<String> = HashSet::new();

        for iteration in 0..max_iterations {
            debug!(task_id = %task.task_id, iteration, "Agent loop iteration");

            // Keep context window bounded: always preserve system + initial user intent
            // so the agent never loses its task description. Append last 4 messages for
            // recent tool call context.
            let chat_messages = if messages.len() > 7 {
                let system = messages[0].clone();
                let intent = messages[1].clone(); // initial user intent — must never be dropped
                let tail: Vec<_> = messages[messages.len() - 4..].to_vec();
                std::iter::once(system)
                    .chain(std::iter::once(intent))
                    .chain(tail)
                    .collect()
            } else {
                messages.clone()
            };
            let response_text = self.chat(&chat_messages).await?;

            match self.parse_action(&response_text) {
                Some(AgentAction::Result { result }) => {
                    info!(task_id = %task.task_id, iterations = iteration + 1, "Task complete");
                    return Ok(AgentResponse {
                        task_id: task.task_id,
                        status: Status::Complete,
                        result,
                        actions_taken,
                        delegated_to: Vec::new(),
                    });
                }

                Some(AgentAction::ToolCall {
                    tool: tool_name,
                    parameters,
                    justification,
                }) => {
                    // Auto-complete: if the model tries to rewrite a file it already wrote,
                    // it has looped. Treat the previous write as the final output.
                    if tool_name == "campaign.write" {
                        if let Some(fname) = parameters.get("filename").and_then(|v| v.as_str()) {
                            if written_files.contains(fname) {
                                info!(
                                    task_id = %task.task_id,
                                    file = fname,
                                    "Detected re-write loop; auto-completing task"
                                );
                                return Ok(AgentResponse {
                                    task_id: task.task_id,
                                    status: Status::Complete,
                                    result: serde_json::json!({
                                        "written_files": written_files.iter().collect::<Vec<_>>()
                                    }),
                                    actions_taken,
                                    delegated_to: Vec::new(),
                                });
                            }
                            written_files.insert(fname.to_string());
                        }
                    }

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
                            "Tool result for {tool_name}: {}",
                            serde_json::to_string(&tool_result.output).unwrap_or_default()
                        ),
                    });
                }

                None => {
                    // JSON mode should prevent this, but handle gracefully if it slips through.
                    warn!(
                        task_id = %task.task_id,
                        response = %response_text,
                        "Model returned non-JSON; treating as final answer"
                    );
                    return Ok(AgentResponse {
                        task_id: task.task_id,
                        status: Status::Complete,
                        result: serde_json::json!({ "response": response_text }),
                        actions_taken,
                        delegated_to: Vec::new(),
                    });
                }
            }
        }

        warn!(task_id = %task.task_id, "Max iterations exceeded");
        Ok(AgentResponse {
            task_id: task.task_id,
            status: Status::Complete,
            result: serde_json::json!({"note": "max_iterations_reached", "actions": actions_taken.len()}),
            actions_taken,
            delegated_to: Vec::new(),
        })
    }
}
