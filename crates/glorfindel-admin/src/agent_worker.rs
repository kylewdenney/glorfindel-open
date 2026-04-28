use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use glorfindel_schemas::agent::AgentResponse;
use glorfindel_schemas::task::TaskRequest;
use glorfindel_schemas::tool::{ToolCall, ToolResult};
use glorfindel_schemas::types::Status;
use glorfindel_schemas::MessageEnvelope;
use glorfindel_tools::parse_dice_notation;
use glorfindel_transport::channel::ChannelDataPlane;
use glorfindel_transport::{ControlPlane, DataPlane};
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::pipeline_agents as agents;
use crate::state::{AppState, TaskEvent, TaskEventKind};

/// Params for a scene summary task, serialized as `TaskRequest.intent`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SceneSummaryParams {
    pub task_type: String, // "scene_summary"
    pub task_id: Uuid,
    pub campaign_name: String,
    pub session_dir: String,
    pub scene_dir: String,
    pub out_file: String,
    pub ollama_host: String,
    pub model: String,
}

/// Params for a top-level scene turn task, serialized as `TaskRequest.intent`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SceneTurnParams {
    pub task_type: String, // "scene_player_turn"
    pub task_id: Uuid,
    pub campaign_name: String,
    pub session_dir: String,
    pub scene_dir: String,
    pub character: String,
    pub action: String,
    pub out_file: String,
    pub ollama_host: String,
    pub model: String,
    pub system_prompt: Option<String>,
}

// ─── Structured rules lookup ─────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
pub struct RulesDb {
    pub check_types: Vec<CheckType>,
    pub characters: Vec<CharacterRules>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CheckType {
    pub id: String,
    pub ability: String,
    pub description: String,
    pub prose: String,
    pub tiers: Vec<CheckTier>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CheckTier {
    pub id: String,
    pub dc: i32,
    pub triggers: Vec<String>,
    pub on_fail: String,
    pub on_success: String,
    #[serde(default)]
    pub fail_effects: Vec<String>,
    #[serde(default)]
    pub succ_effects: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct CharacterRules {
    pub name: String,
    pub abilities: Vec<AbilityEntry>,
    pub special_rules: Vec<SpecialRule>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AbilityEntry {
    pub name: String,
    pub modifier: i32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SpecialRule {
    pub id: String,
    pub description: String,
}

impl RulesDb {
    /// Resolve modifier for a character + ability/save name. Returns 0 if not found.
    pub fn modifier(&self, character: &str, ability: &str) -> i32 {
        self.characters
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(character))
            .and_then(|c| {
                c.abilities
                    .iter()
                    .find(|a| a.name.eq_ignore_ascii_case(ability))
                    .map(|a| a.modifier)
            })
            .unwrap_or(0)
    }

    /// Resolve DC for a check_type + tier. Returns 12 as a safe default.
    pub fn dc(&self, check_type: &str, tier: &str) -> i32 {
        self.check_types
            .iter()
            .find(|ct| ct.id == check_type)
            .and_then(|ct| ct.tiers.iter().find(|t| t.id == tier))
            .map(|t| t.dc)
            .unwrap_or(12)
    }

    /// Return on_fail / on_success prose for a check_type + tier.
    pub fn consequences(&self, check_type: &str, tier: &str) -> (&str, &str) {
        self.check_types
            .iter()
            .find(|ct| ct.id == check_type)
            .and_then(|ct| ct.tiers.iter().find(|t| t.id == tier))
            .map(|t| (t.on_fail.as_str(), t.on_success.as_str()))
            .unwrap_or(("Failure consequence.", "Success."))
    }

    /// Return the ability name for a check_type (Intelligence for cosmic_dread, etc.).
    pub fn ability_for_check(&self, check_type: &str) -> &str {
        self.check_types
            .iter()
            .find(|ct| ct.id == check_type)
            .map(|ct| ct.ability.as_str())
            .unwrap_or("Unknown")
    }

    /// Special rules for a character as a single formatted string.
    pub fn special_rules_text(&self, character: &str) -> String {
        self.characters
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(character))
            .map(|c| {
                c.special_rules
                    .iter()
                    .map(|r| format!("- {}", r.description))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    /// Format rules context for RulesAssessor — check type list + character special rules.
    pub fn assessor_context(&self, character: &str) -> String {
        let check_types: String = self
            .check_types
            .iter()
            .map(|ct| {
                let tiers: String = ct
                    .tiers
                    .iter()
                    .map(|t| format!("    {} (DC {}): {}", t.id, t.dc, t.triggers.join(", ")))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{} [{}]\n{}\n", ct.id, ct.ability, tiers)
            })
            .collect::<Vec<_>>()
            .join("\n");

        let special = self.special_rules_text(character);
        format!(
            "CHECK TYPES AND TIERS:\n{check_types}\nSPECIAL RULES FOR {character}:\n{special}"
        )
    }
}

/// Load rules.toml for a campaign. Returns None if file is missing or unparseable.
async fn load_rules_db(campaign_path: &std::path::Path) -> Option<RulesDb> {
    let path = campaign_path.join("rules/rules.toml");
    let text = tokio::fs::read_to_string(&path).await.ok()?;
    toml::from_str(&text).ok()
}

// ─── Party state (JSON) ───────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CharState {
    pub dread: u8,
    pub cdp: u8,
    pub conditions: Vec<String>,
}

pub type PartyState = std::collections::HashMap<String, CharState>;

async fn load_party_state(campaign_path: &std::path::Path) -> PartyState {
    let path = campaign_path.join("world/party_state.json");
    tokio::fs::read_to_string(&path)
        .await
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

async fn save_party_state(campaign_path: &std::path::Path, state: &PartyState) {
    let path = campaign_path.join("world/party_state.json");
    if let Ok(json) = serde_json::to_string_pretty(state) {
        let _ = tokio::fs::write(&path, json).await;
    }
}

/// Render a compact markdown table from party_state.json for LLM context.
fn render_party_state_md(state: &PartyState) -> String {
    let mut rows = vec![
        "| Character | Dread | CDP | Conditions |".to_string(),
        "|-----------|-------|-----|------------|".to_string(),
    ];
    let mut names: Vec<&String> = state.keys().collect();
    names.sort();
    for name in names {
        let s = &state[name];
        let cond = if s.conditions.is_empty() {
            "—".to_string()
        } else {
            s.conditions.join(", ")
        };
        rows.push(format!("| {} | {} | {} | {} |", name, s.dread, s.cdp, cond));
    }
    rows.join("\n")
}

/// Apply CharacterImpact structured output lines to a PartyState.
/// Apply a slice of effect strings from rules.toml to a single character's state.
///
/// Effect grammar:
///   "cdp+N"           — increment CDP by N
///   "dread+N"         — increment Dread by N
///   "condition:Name"  — add named condition
///   "condition:clear" — clear all conditions
fn apply_effects(entry: &mut CharState, effects: &[String]) {
    for effect in effects {
        if let Some(rest) = effect.strip_prefix("cdp+") {
            if let Ok(n) = rest.parse::<u8>() { entry.cdp = entry.cdp.saturating_add(n); }
        } else if let Some(rest) = effect.strip_prefix("dread+") {
            if let Ok(n) = rest.parse::<u8>() { entry.dread = entry.dread.saturating_add(n); }
        } else if let Some(cond) = effect.strip_prefix("condition:") {
            if cond == "clear" {
                entry.conditions.clear();
            } else if !entry.conditions.iter().any(|c| c == cond) {
                entry.conditions.push(cond.to_string());
            }
        }
    }
}

fn apply_impact_to_state(state: &mut PartyState, impact_output: &str) {
    for line in impact_output.lines() {
        let line = line.trim();
        if line.is_empty() || line == "NO_CHANGE" { continue; }
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        if parts.len() < 3 { continue; }
        let (field, name, value) = (parts[0].trim(), parts[1].trim(), parts[2].trim());
        let entry = state.entry(name.to_string()).or_insert(CharState {
            dread: 0, cdp: 0, conditions: vec![],
        });
        match field {
            "DREAD" => { if let Ok(v) = value.parse::<u8>() { entry.dread = v; } }
            "CDP"   => { if let Ok(v) = value.parse::<u8>() { entry.cdp = v; } }
            "CONDITION" => {
                if value == "—" || value.is_empty() {
                    entry.conditions.clear();
                } else {
                    if !entry.conditions.contains(&value.to_string()) {
                        entry.conditions.push(value.to_string());
                    }
                }
            }
            _ => {}
        }
    }
}

// ─── Domain → model lookup ────────────────────────────────────────────────────

/// Look up the Ollama host + model for a specialist agent domain.
/// Falls back to the parent task's host/model if no matching definition is registered.
async fn find_agent_model(
    state: &AppState,
    domain: &str,
    fallback_host: &str,
    fallback_model: &str,
) -> (String, String) {
    let defs = state.definitions.read().await;
    defs.values()
        .find(|d| d.domains.iter().any(|dn| dn == domain))
        .map(|d| (d.ollama_host.clone(), d.model.clone()))
        .unwrap_or_else(|| (fallback_host.to_string(), fallback_model.to_string()))
}

// ─── DDS sub-task dispatch ────────────────────────────────────────────────────

/// Publish a sub-task via DDS and synchronously await the AgentResponse.
///
/// Registers a oneshot in `state.pending_sub_tasks` before publishing;
/// `run_response_dispatcher` routes the response back when it arrives.
async fn dispatch_sub_task(
    intent_value: &serde_json::Value,
    parent_task_id: Uuid,
    state: &AppState,
) -> Result<AgentResponse, String> {
    let sub_task_id = Uuid::new_v4();
    let intent = serde_json::to_string(intent_value).map_err(|e| e.to_string())?;

    let (tx, rx) = tokio::sync::oneshot::channel();
    state
        .pending_sub_tasks
        .write()
        .await
        .insert(sub_task_id, tx);

    let task_request = glorfindel_schemas::task::TaskRequest {
        task_id: sub_task_id,
        parent_task_id: Some(parent_task_id),
        intent,
        context: vec![],
        constraints: glorfindel_schemas::task::TaskConstraints::default(),
        reply_to: "glorfindel/tasks/response".into(),
    };

    if let Err(e) = state
        .control_plane
        .publish_task(MessageEnvelope::new("dm-manager", task_request))
        .await
    {
        state.pending_sub_tasks.write().await.remove(&sub_task_id);
        return Err(format!("DDS publish failed: {e}"));
    }

    tokio::time::timeout(std::time::Duration::from_secs(300), rx)
        .await
        .map_err(|_| "sub-task timeout (300 s)".to_string())?
        .map_err(|_| "sub-task oneshot closed".to_string())
}

// ─── DDS workers ─────────────────────────────────────────────────────────────

/// Subscribes to all DDS TaskRequests and dispatches by `task_type`.
///
/// Top-level (`scene_player_turn`): spawns the DM Manager pipeline.
/// Sub-task types: run the appropriate specialist agent and publish the response.
pub async fn run_task_worker(
    mut task_rx: mpsc::Receiver<MessageEnvelope<TaskRequest>>,
    state: Arc<AppState>,
) {
    info!("DDS task worker started");
    // Dedup set — transient-local QoS re-delivers historical messages to new
    // DDS participants created on each publish. Without this, every publish
    // causes all prior task requests to be re-processed.
    let mut seen: std::collections::HashSet<Uuid> = std::collections::HashSet::new();

    while let Some(envelope) = task_rx.recv().await {
        let task = envelope.payload;

        if !seen.insert(task.task_id) {
            warn!(task_id = %task.task_id, "Duplicate DDS task delivery — skipping");
            continue;
        }

        let task_type = serde_json::from_str::<serde_json::Value>(&task.intent)
            .ok()
            .and_then(|v| v["task_type"].as_str().map(str::to_string))
            .unwrap_or_default();

        info!(task_id = %task.task_id, task_type = %task_type, "DDS task received");

        let state = state.clone();

        match task_type.as_str() {
            // ── Top-level: DM Manager orchestrates the full pipeline ──────────
            "scene_player_turn" => {
                let params: SceneTurnParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => {
                        error!(error = %e, "Failed to parse SceneTurnParams");
                        continue;
                    }
                };
                tokio::spawn(async move {
                    let data_plane = Arc::new(ChannelDataPlane::new());
                    let tool_call_rx = match data_plane.receive_tool_calls().await {
                        Ok(rx) => rx,
                        Err(e) => { error!(error = %e, "tool call rx failed"); return; }
                    };
                    let exec_plane = data_plane.clone();
                    let exec_state = state.clone();
                    tokio::spawn(run_tool_executor(
                        task.task_id,
                        tool_call_rx,
                        exec_plane,
                        exec_state,
                    ));

                    let response =
                        run_scene_pipeline(task.task_id, params, data_plane, state.clone()).await;

                    if let Err(e) = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("dm-manager", response))
                        .await
                    {
                        error!(error = %e, "Failed to publish AgentResponse via DDS");
                    }
                });
            }

            // ── Sub-task: Fact Checker ────────────────────────────────────────
            "fact_check" => {
                let params: agents::FactCheckParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => { error!(error = %e, "FactCheckParams parse failed"); continue; }
                };
                tokio::spawn(async move {
                    let response = agents::fact_check(params, task.task_id).await;
                    let _ = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("fact-checker", response))
                        .await;
                });
            }

            // ── Sub-task: Rules Assessor ──────────────────────────────────────
            "rules_assess" => {
                let params: agents::RulesAssessParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => { error!(error = %e, "RulesAssessParams parse failed"); continue; }
                };
                tokio::spawn(async move {
                    let response = agents::rules_assess(params, task.task_id).await;
                    let _ = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("rules-assessor", response))
                        .await;
                });
            }

            // ── Sub-task: DM Writer ───────────────────────────────────────────
            "dm_write" => {
                let params: agents::DmWriteParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => { error!(error = %e, "DmWriteParams parse failed"); continue; }
                };
                tokio::spawn(async move {
                    let response = agents::dm_write(params, task.task_id).await;
                    let _ = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("dm-writer", response))
                        .await;
                });
            }

            // ── Sub-task: Summarizer ──────────────────────────────────────────
            "dm_summarize" => {
                let params: agents::DmSummarizeParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => { error!(error = %e, "DmSummarizeParams parse failed"); continue; }
                };
                tokio::spawn(async move {
                    let response = agents::dm_summarize(params, task.task_id).await;
                    let _ = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("dm-summarizer", response))
                        .await;
                });
            }

            // ── Sub-task: Character Impact ────────────────────────────────────
            "char_impact" => {
                let params: agents::CharImpactParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => { error!(error = %e, "CharImpactParams parse failed"); continue; }
                };
                tokio::spawn(async move {
                    let response = agents::char_impact(params, task.task_id).await;
                    let _ = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("char-impact", response))
                        .await;
                });
            }

            // ── Top-level: Scene summary ──────────────────────────────────────
            "scene_summary" => {
                let params: SceneSummaryParams = match serde_json::from_str(&task.intent) {
                    Ok(p) => p,
                    Err(e) => { error!(error = %e, "SceneSummaryParams parse failed"); continue; }
                };
                tokio::spawn(async move {
                    let data_plane = Arc::new(ChannelDataPlane::new());
                    let tool_call_rx = match data_plane.receive_tool_calls().await {
                        Ok(rx) => rx,
                        Err(e) => { error!(error = %e, "tool call rx failed"); return; }
                    };
                    tokio::spawn(run_tool_executor(
                        task.task_id,
                        tool_call_rx,
                        data_plane.clone(),
                        state.clone(),
                    ));
                    let response = run_scene_summary(task.task_id, params, data_plane, state.clone()).await;
                    if let Err(e) = state
                        .control_plane
                        .publish_response(MessageEnvelope::new("scene-summarizer", response))
                        .await
                    {
                        error!(error = %e, "Failed to publish scene summary response");
                    }
                });
            }

            other => {
                warn!(task_type = %other, task_id = %task.task_id, "Unknown task type — ignoring");
            }
        }
    }
    info!("DDS task worker terminated");
}

/// Routes DDS AgentResponses to their destinations.
///
/// Sub-tasks (registered in `pending_sub_tasks`): resolved via oneshot, unblocking the
/// DM Manager pipeline step that dispatched them.
/// Top-level tasks: update the task record and broadcast `TaskEventKind::Complete`.
pub async fn run_response_dispatcher(
    mut response_rx: mpsc::Receiver<MessageEnvelope<AgentResponse>>,
    state: Arc<AppState>,
) {
    info!("DDS response dispatcher started");
    while let Some(envelope) = response_rx.recv().await {
        let response = envelope.payload;
        let task_id = response.task_id;

        // Sub-task: route to the awaiting pipeline step
        {
            let mut pending = state.pending_sub_tasks.write().await;
            if let Some(tx) = pending.remove(&task_id) {
                let _ = tx.send(response);
                continue;
            }
        }

        // Top-level task: update record and broadcast completion
        info!(task_id = %task_id, "DDS AgentResponse received — completing task");
        let mut tasks = state.tasks.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            record.status = Status::Complete;
            record.completed_at = Some(Utc::now());
            record.response = Some(response.clone());
            let _ = state.task_events.send(TaskEvent {
                task_id,
                kind: TaskEventKind::Complete { response },
            });
        } else {
            warn!(task_id = %task_id, "Response for unknown task — already resolved?");
        }
    }
    info!("DDS response dispatcher terminated");
}

// ─── Tool executor (data plane) ───────────────────────────────────────────────

async fn run_tool_executor(
    task_id: Uuid,
    mut tool_call_rx: mpsc::Receiver<MessageEnvelope<ToolCall>>,
    data_plane: Arc<ChannelDataPlane>,
    state: Arc<AppState>,
) {
    while let Some(envelope) = tool_call_rx.recv().await {
        let call = envelope.payload.clone();
        let tool_name = call.tool_name.clone();
        let params_str = call.parameters.to_string();

        let result = execute_tool_call(&call).await;
        let output_str = result.output.to_string();

        let _ = state.task_events.send(TaskEvent {
            task_id,
            kind: TaskEventKind::ToolCall {
                tool: tool_name.clone(),
                input: params_str,
                output: output_str,
            },
        });

        if tool_name == "file.write" || tool_name == "file.append" {
            if let Some(path) = call.parameters.get("path").and_then(|v| v.as_str()) {
                let bytes = call
                    .parameters
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0);
                let _ = state.task_events.send(TaskEvent {
                    task_id,
                    kind: TaskEventKind::FileWrite {
                        path: path.to_string(),
                        bytes,
                    },
                });
            }
        }

        let result_envelope = MessageEnvelope::new("tool-executor", result);
        if data_plane.send_tool_result(result_envelope).await.is_err() {
            break;
        }
    }
}

async fn execute_tool_call(call: &ToolCall) -> ToolResult {
    match call.tool_name.as_str() {
        "file.read" => {
            let path = call.parameters["path"].as_str().unwrap_or("");
            match tokio::fs::read_to_string(path).await {
                Ok(content) => ToolResult::success(call.task_id, "file.read", serde_json::json!(content)),
                Err(_) => ToolResult::success(call.task_id, "file.read", serde_json::json!("")),
            }
        }
        "file.write" => {
            let path = call.parameters["path"].as_str().unwrap_or("");
            let content = call.parameters["content"].as_str().unwrap_or("");
            if let Some(parent) = std::path::Path::new(path).parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            match tokio::fs::write(path, content.as_bytes()).await {
                Ok(_) => ToolResult::success(
                    call.task_id,
                    "file.write",
                    serde_json::json!({"bytes": content.len()}),
                ),
                Err(e) => ToolResult::failure(call.task_id, "file.write", e.to_string()),
            }
        }
        "file.append" => {
            let path = call.parameters["path"].as_str().unwrap_or("");
            let content = call.parameters["content"].as_str().unwrap_or("");
            if let Some(parent) = std::path::Path::new(path).parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let mut f = match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
            {
                Ok(f) => f,
                Err(e) => return ToolResult::failure(call.task_id, "file.append", e.to_string()),
            };
            match f.write_all(content.as_bytes()).await {
                Ok(_) => ToolResult::success(
                    call.task_id,
                    "file.append",
                    serde_json::json!({"bytes": content.len()}),
                ),
                Err(e) => ToolResult::failure(call.task_id, "file.append", e.to_string()),
            }
        }
        other => ToolResult::failure(call.task_id, other, format!("unknown tool: {other}")),
    }
}

// ─── Data plane helpers (agent side) ─────────────────────────────────────────

async fn dp_read(
    task_id: Uuid,
    path: &std::path::Path,
    dp: &ChannelDataPlane,
    rx: &mut mpsc::Receiver<MessageEnvelope<ToolResult>>,
) -> String {
    let call = ToolCall {
        task_id,
        agent_id: "dm-manager".into(),
        tool_name: "file.read".into(),
        parameters: serde_json::json!({"path": path.to_str().unwrap_or("")}),
        justification: None,
    };
    if dp.send_tool_call(MessageEnvelope::new("dm-manager", call)).await.is_err() {
        return String::new();
    }
    match rx.recv().await {
        Some(env) => env.payload.output.as_str().unwrap_or("").to_string(),
        None => String::new(),
    }
}

async fn dp_write(
    task_id: Uuid,
    path: &std::path::Path,
    content: &str,
    dp: &ChannelDataPlane,
    rx: &mut mpsc::Receiver<MessageEnvelope<ToolResult>>,
) {
    let call = ToolCall {
        task_id,
        agent_id: "dm-manager".into(),
        tool_name: "file.write".into(),
        parameters: serde_json::json!({
            "path": path.to_str().unwrap_or(""),
            "content": content
        }),
        justification: None,
    };
    let _ = dp.send_tool_call(MessageEnvelope::new("dm-manager", call)).await;
    let _ = rx.recv().await;
}

async fn dp_append(
    task_id: Uuid,
    path: &std::path::Path,
    content: &str,
    dp: &ChannelDataPlane,
    rx: &mut mpsc::Receiver<MessageEnvelope<ToolResult>>,
) {
    let call = ToolCall {
        task_id,
        agent_id: "dm-manager".into(),
        tool_name: "file.append".into(),
        parameters: serde_json::json!({
            "path": path.to_str().unwrap_or(""),
            "content": content
        }),
        justification: None,
    };
    let _ = dp.send_tool_call(MessageEnvelope::new("dm-manager", call)).await;
    let _ = rx.recv().await;
}

// ─── Dice ─────────────────────────────────────────────────────────────────────

fn roll_inline(notation: &str) -> Option<(Vec<u32>, i32, i32)> {
    let (count, sides, modifier) = parse_dice_notation(notation)?;
    let mut rng = rand::thread_rng();
    let rolls: Vec<u32> = (0..count).map(|_| rng.gen_range(1..=sides)).collect();
    let total: i32 = rolls.iter().map(|&r| r as i32).sum::<i32>() + modifier;
    Some((rolls, modifier, total))
}

// ─── DM Manager pipeline ──────────────────────────────────────────────────────

/// Orchestrates the full scene turn pipeline via DDS sub-tasks and the data plane.
///
/// Each reasoning step (fact-check, rules-assess, dm-write, summarize) is a separate
/// DDS sub-task dispatched to the appropriate specialist agent. The DM Manager handles
/// file I/O (grounding reads, meta log writes, prose output) via the ChannelDataPlane.
///
/// Sub-agent model selection: looks up `state.definitions` by domain; falls back to
/// the parent task's `ollama_host` / `model` if no specialist definition is registered.
async fn run_scene_pipeline(
    task_id: Uuid,
    params: SceneTurnParams,
    data_plane: Arc<ChannelDataPlane>,
    state: Arc<AppState>,
) -> AgentResponse {
    let campaign_path = crate::api::campaign::campaign_root(&state.data_dir, &params.campaign_name);
    let scene_path = campaign_path.join(&params.session_dir).join(&params.scene_dir);
    let meta_dir = scene_path.join(".meta");
    let meta_path = meta_dir.join(params.out_file.replace(".md", ".log"));
    let prose_path = scene_path.join(&params.out_file);
    let character = &params.character;
    let action = &params.action;

    macro_rules! emit {
        ($step:expr, $body:expr) => {
            let _ = state.task_events.send(TaskEvent {
                task_id,
                kind: TaskEventKind::PipelineStep {
                    step: $step.to_string(),
                    body: $body.to_string(),
                },
            });
        };
    }

    macro_rules! spawn_event {
        ($name:expr, $model:expr, $host:expr) => {
            let _ = state.task_events.send(TaskEvent {
                task_id,
                kind: TaskEventKind::AgentSpawned {
                    name: $name.to_string(),
                    model: $model.to_string(),
                    context: $host.to_string(),
                },
            });
        };
    }

    // Take tool result receiver for file I/O throughout the pipeline
    let mut rx = match data_plane.receive_tool_results().await {
        Ok(r) => r,
        Err(e) => {
            return AgentResponse {
                task_id,
                status: Status::Failed,
                result: serde_json::json!({"error": e.to_string()}),
                actions_taken: vec![],
                delegated_to: vec![],
            };
        }
    };
    let dp = data_plane.as_ref();

    // Write meta header (data plane)
    let header = format!(
        "# Player Turn: {character} — {action}\n\
         **Campaign:** {}  **Session:** {}  **Scene:** {}  **File:** {}  **Time:** {}\n\
         **DDS task_id:** {task_id}\n",
        params.campaign_name,
        params.session_dir,
        params.scene_dir,
        params.out_file,
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
    );
    dp_write(task_id, &meta_path, &header, dp, &mut rx).await;

    // ── Load structured rules + party state ──────────────────────────────────
    let rules_db = load_rules_db(&campaign_path).await;
    let mut party_state = load_party_state(&campaign_path).await;
    let party_state_md = render_party_state_md(&party_state);

    // ── Load grounding files (data plane) ────────────────────────────────────
    let scene_opener = dp_read(task_id, &scene_path.join("scene_opener.md"), dp, &mut rx).await;
    let party_text = dp_read(task_id, &campaign_path.join("world/party.md"), dp, &mut rx).await;
    let setting = dp_read(task_id, &campaign_path.join("world/setting.md"), dp, &mut rx).await;
    let npcs = dp_read(task_id, &campaign_path.join("world/npcs.md"), dp, &mut rx).await;

    let mut prev_turns_text = String::new();
    for prev_name in infer_prev_turns(&params.out_file, 2) {
        let text = dp_read(task_id, &scene_path.join(&prev_name), dp, &mut rx).await;
        if !text.is_empty() {
            prev_turns_text.push_str(&format!(
                "### {prev_name}\n{}\n\n",
                text.chars().take(1500).collect::<String>()
            ));
        }
    }

    let grounding_block = {
        let mut parts = Vec::new();
        if !scene_opener.is_empty() {
            parts.push(format!(
                "### scene_opener.md\n{}\n",
                scene_opener.chars().take(2000).collect::<String>()
            ));
        }
        if !prev_turns_text.is_empty() {
            parts.push(prev_turns_text.clone());
        }
        if !party_text.is_empty() {
            parts.push(format!("### world/party.md\n{party_text}\n"));
        }
        if !party_state_md.is_empty() {
            parts.push(format!("### Current State\n{party_state_md}\n"));
        }
        parts.join("\n")
    };

    let campaign_facts = build_campaign_facts(&params.campaign_name, &party_text, &setting, &npcs);

    // ── Sub-task 1: Fact Checker (DDS) ────────────────────────────────────────
    let (fact_host, fact_model) = find_agent_model(
        &state, "campaign-fact-check", &params.ollama_host, &params.model,
    ).await;
    spawn_event!("FactCheckerAgent", &fact_model, &fact_host);

    let fact_intent = serde_json::to_value(agents::FactCheckParams {
        task_type: "fact_check".into(),
        ollama_host: fact_host,
        model: fact_model,
        character: character.clone(),
        action: action.clone(),
        grounding_block: trunc(&grounding_block, 3000),
    })
    .unwrap_or_default();

    let critic_context = match dispatch_sub_task(&fact_intent, task_id, &state).await {
        Ok(r) => r.result["output"].as_str().unwrap_or("").to_string(),
        Err(e) => {
            error!(error = %e, "FactChecker sub-task failed");
            String::new()
        }
    };
    dp_append(
        task_id,
        &meta_path,
        &format!("\n## 🔍 What Happened\n{critic_context}\n"),
        dp,
        &mut rx,
    )
    .await;
    emit!("🔍 What Happened", &critic_context);

    // ── Sub-task 2: Rules Assessor (DDS) ─────────────────────────────────────
    let (rules_host, rules_model) = find_agent_model(
        &state, "ttrpg-rules", &params.ollama_host, &params.model,
    ).await;
    spawn_event!("RulesAssessorAgent", &rules_model, &rules_host);

    let rules_context = rules_db
        .as_ref()
        .map(|db| db.assessor_context(character))
        .unwrap_or_default();

    let rules_intent = serde_json::to_value(agents::RulesAssessParams {
        task_type: "rules_assess".into(),
        ollama_host: rules_host,
        model: rules_model,
        character: character.clone(),
        action: action.clone(),
        party_state: party_state_md.clone(),
        rules_context: trunc(&rules_context, 2000),
    })
    .unwrap_or_default();

    let rules_output = match dispatch_sub_task(&rules_intent, task_id, &state).await {
        Ok(r) => r.result["output"].as_str().unwrap_or("NO_ROLL").to_string(),
        Err(e) => {
            error!(error = %e, "RulesAssessor sub-task failed");
            "NO_ROLL".to_string()
        }
    };
    dp_append(
        task_id,
        &meta_path,
        &format!("\n## ⚖ Rules Assessor\n{rules_output}\n"),
        dp,
        &mut rx,
    )
    .await;
    emit!("⚖ Rules Assessor", &rules_output);

    // ── Dice (inline — Rust owns all numbers from rules.toml) ────────────────
    // New ROLL format: ROLL|CharName|check_type|tier|reason
    //   check_type: cosmic_dread | fear | <skill_name>
    //   tier:       tier id within that check_type (e.g. evidence, moderate)
    // Rust looks up DC and modifier from RulesDb — LLM never guesses numbers.
    let mut dice_lines: Vec<String> = Vec::new();
    let mut dice_results: Vec<String> = Vec::new();
    let mut dice_prose_results: Vec<String> = Vec::new();
    let mut roll_outcomes: Vec<String> = Vec::new();

    for line in rules_output.lines() {
        let line = line.trim();
        if !line.starts_with("ROLL|") { continue; }
        let parts: Vec<&str> = line.splitn(5, '|').collect();
        if parts.len() < 4 { continue; }
        let char_name  = parts[1].trim();
        let check_type = parts[2].trim();
        let tier       = parts[3].trim();
        let reason     = parts.get(4).map(|s| s.trim()).unwrap_or("");

        let (dc, modifier, ability_label, on_fail, on_success) = if let Some(db) = &rules_db {
            let dc = db.dc(check_type, tier);
            let ability_name = if matches!(check_type, "cosmic_dread" | "fear") {
                // Save uses INT_SAVE or WIS_SAVE
                let save_key = if check_type == "cosmic_dread" { "INT_SAVE" } else { "WIS_SAVE" };
                save_key.to_string()
            } else {
                check_type.to_string()
            };
            let modifier = db.modifier(char_name, &ability_name);
            let (on_fail, on_success) = db.consequences(check_type, tier);
            let label = if matches!(check_type, "cosmic_dread" | "fear") {
                format!("{} ({})", db.ability_for_check(check_type), check_type)
            } else {
                check_type.to_string()
            };
            (dc, modifier, label, on_fail.to_string(), on_success.to_string())
        } else {
            // Fallback if no rules.toml
            (12, 0, check_type.to_string(), "Failure consequence.".to_string(), "Success.".to_string())
        };

        let notation = if modifier >= 0 {
            format!("1d20+{modifier}")
        } else {
            format!("1d20{modifier}")
        };

        if let Some((rolls, _parsed_mod, total)) = roll_inline(&notation) {
            let rolls_str = rolls.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ");
            let mod_str = if modifier > 0 { format!("+{modifier}") }
                          else if modifier < 0 { modifier.to_string() }
                          else { String::new() };
            let outcome = if total >= dc { "SUCCESS" } else { "FAILURE" };
            let consequence = if outcome == "SUCCESS" { &on_success } else { &on_fail };
            dice_lines.push(notation.clone());
            // Full entry for meta log
            dice_results.push(format!(
                "{char_name} {ability_label}: {notation} → [{rolls_str}]{mod_str} = **{total}** vs DC {dc} → **{outcome}**\n  ↳ {consequence}\n  ↳ {reason}"
            ));
            // Prose-safe entry for DmWriter — no notation, just outcome + consequence
            dice_prose_results.push(format!(
                "{char_name} {ability_label}: {outcome}\n  ↳ {consequence}\n  ↳ {reason}"
            ));
            roll_outcomes.push(outcome.to_string());

            // Apply deterministic effects from rules.toml immediately
            if let Some(db) = &rules_db {
                let effects = db.check_types
                    .iter()
                    .find(|ct| ct.id == check_type)
                    .and_then(|ct| ct.tiers.iter().find(|t| t.id == tier))
                    .map(|t| if outcome == "SUCCESS" { &t.succ_effects } else { &t.fail_effects });

                if let Some(effects) = effects {
                    let entry = party_state.entry(char_name.to_string()).or_insert(CharState {
                        dread: 0, cdp: 0, conditions: vec![],
                    });
                    apply_effects(entry, effects);
                }
            }
        }
    }

    let dice_context = if dice_results.is_empty() {
        format!("No roll required. Focus the scene on: {character} — {action}")
    } else {
        dice_results.join("\n\n")
    };
    // Prose version passed to DmWriter — notation stripped so it never bleeds into narrative
    let dice_context_prose = if dice_prose_results.is_empty() {
        format!("No roll required. Narrate: {character} — {action}")
    } else {
        dice_prose_results.join("\n\n")
    };
    // Explicit outcome for CharacterImpact — unambiguous SUCCESS/FAILURE/NO_ROLL
    let roll_outcome_summary = if roll_outcomes.is_empty() {
        "NO_ROLL".to_string()
    } else {
        roll_outcomes.join(", ")
    };
    // Persist deterministic effects immediately — before LLM steps that may fail
    if !roll_outcomes.is_empty() {
        save_party_state(&campaign_path, &party_state).await;
    }

    dp_append(
        task_id,
        &meta_path,
        &format!("\n## 🎲 Dice\n{dice_context}\n"),
        dp,
        &mut rx,
    )
    .await;
    emit!("🎲 Dice", &dice_context);

    // ── Sub-task 3: DM Writer (DDS) ───────────────────────────────────────────
    let (writer_host, writer_model) = find_agent_model(
        &state, "dm-narrative", &params.ollama_host, &params.model,
    ).await;
    spawn_event!("DmWriterAgent", &writer_model, &writer_host);

    let system_prompt = params.system_prompt.clone().unwrap_or_else(|| {
        format!(
            "You are the DM for the {} campaign. \
             Respond to player actions with immersive prose true to the campaign's tone.",
            params.campaign_name
        )
    });

    let write_intent = serde_json::to_value(agents::DmWriteParams {
        task_type: "dm_write".into(),
        ollama_host: writer_host,
        model: writer_model,
        campaign_name: params.campaign_name.clone(),
        session_dir: params.session_dir.clone(),
        scene_dir: params.scene_dir.clone(),
        character: character.clone(),
        action: action.clone(),
        campaign_facts: trunc(&campaign_facts, 1500),
        critic_context: trunc(&critic_context, 1500),
        dice_context: trunc(&dice_context_prose, 500),
        system_prompt: trunc(&system_prompt, 800),
    })
    .unwrap_or_default();

    let dm_prose = match dispatch_sub_task(&write_intent, task_id, &state).await {
        Ok(r) => r.result["output"].as_str().unwrap_or("").to_string(),
        Err(e) => {
            error!(error = %e, "DmWriter sub-task failed");
            format!("*(DM response error: {e})*")
        }
    };
    dp_append(
        task_id,
        &meta_path,
        &format!("\n## ✍ DM Response\n{dm_prose}\n"),
        dp,
        &mut rx,
    )
    .await;
    emit!("✍ DM Response", &dm_prose);
    dp_write(task_id, &prose_path, &dm_prose, dp, &mut rx).await;

    // ── Sub-task 4: Character Impact (DDS) ───────────────────────────────────
    let (impact_host, impact_model) = find_agent_model(
        &state, "char-impact", &params.ollama_host, &params.model,
    ).await;
    spawn_event!("CharImpactAgent", &impact_model, &impact_host);

    // Build cosmic rules context from rules.toml (no extra dp_read needed)
    let cosmic_rules_context = rules_db.as_ref().map(|db| {
        db.check_types
            .iter()
            .filter(|ct| ct.id == "cosmic_dread")
            .map(|ct| {
                let tiers: String = ct.tiers.iter()
                    .map(|t| format!("  {} (DC {}): on_fail={}", t.id, t.dc, t.on_fail))
                    .collect::<Vec<_>>().join("\n");
                format!("{}: {}\n{}", ct.id, ct.description, tiers)
            })
            .collect::<Vec<_>>().join("\n")
    }).unwrap_or_default();

    let impact_intent = serde_json::to_value(agents::CharImpactParams {
        task_type: "char_impact".into(),
        ollama_host: impact_host,
        model: impact_model,
        character: character.clone(),
        roll_outcome: roll_outcome_summary.clone(),
        dice_context: trunc(&dice_context_prose, 400),
        dm_prose: trunc(&dm_prose, 1200),
        party_state: party_state_md.clone(),
        cosmic_rules: trunc(&cosmic_rules_context, 800),
    })
    .unwrap_or_default();

    let impact_output = match dispatch_sub_task(&impact_intent, task_id, &state).await {
        Ok(r) => r.result["output"].as_str().unwrap_or("NO_CHANGE").to_string(),
        Err(e) => {
            error!(error = %e, "CharImpact sub-task failed");
            "NO_CHANGE".to_string()
        }
    };
    dp_append(
        task_id,
        &meta_path,
        &format!("\n## 🎭 Character Impact\n{impact_output}\n"),
        dp,
        &mut rx,
    )
    .await;
    emit!("🎭 Character Impact", &impact_output);

    // Merge narrative conditions into state (CDP/Dread already applied after dice)
    let party_path = campaign_path.join("world/party.md");
    let has_condition = impact_output.lines().any(|l| l.starts_with("CONDITION|"));
    if has_condition {
        apply_impact_to_state(&mut party_state, &impact_output);
        save_party_state(&campaign_path, &party_state).await;
    }

    // ── Sub-task 5: Summarizer (DDS) ──────────────────────────────────────────
    let (summary_host, summary_model) = find_agent_model(
        &state, "dm-summary", &params.ollama_host, &params.model,
    ).await;
    spawn_event!("DmSummarizerAgent", &summary_model, &summary_host);

    let summarize_intent = serde_json::to_value(agents::DmSummarizeParams {
        task_type: "dm_summarize".into(),
        ollama_host: summary_host,
        model: summary_model,
        character: character.clone(),
        prose: trunc(&dm_prose, 2000),
    })
    .unwrap_or_default();

    let action_summary = match dispatch_sub_task(&summarize_intent, task_id, &state).await {
        Ok(r) => r.result["output"].as_str().unwrap_or("").to_string(),
        Err(e) => {
            error!(error = %e, "Summarizer sub-task failed");
            format!("(summarizer error: {e})")
        }
    };
    dp_append(
        task_id,
        &meta_path,
        &format!("\n## 📋 Summary\n{action_summary}\n"),
        dp,
        &mut rx,
    )
    .await;
    emit!("📋 Summary", &action_summary);

    // ── Update TURNS.md (data plane) ──────────────────────────────────────────
    let index_path = scene_path.join("TURNS.md");
    let meta_stem = params.out_file.replace(".md", ".log");
    let dice_inline = if dice_lines.is_empty() { "—".to_string() } else { dice_lines.join(", ") };
    let summary_short: String = action_summary.lines().next().unwrap_or("").chars().take(120).collect();
    let effective_session = format!("{}/{}", params.session_dir, params.scene_dir);
    let out_file = &params.out_file;

    let existing_index = dp_read(task_id, &index_path, dp, &mut rx).await;
    let needs_header = existing_index.is_empty();
    let turn_num = if needs_header {
        1usize
    } else {
        existing_index
            .lines()
            .filter(|l| l.starts_with("| ") && !l.starts_with("| #") && !l.starts_with("|---"))
            .count()
            + 1
    };

    let index_row = if needs_header {
        format!(
            "# {} / {} — Turn Index\n\n\
             | # | Time (UTC) | Data plane | Control plane | Dice | Summary |\n\
             |---|-----------|-----------|--------------|------|---------|\n\
             | 1 | {} | [{out_file}]({out_file}) | [.meta/{meta_stem}](.meta/{meta_stem}) | {dice_inline} | {summary_short} |\n",
            params.campaign_name,
            effective_session,
            Utc::now().format("%H:%M:%S"),
        )
    } else {
        format!(
            "| {turn_num} | {} | [{out_file}]({out_file}) | [.meta/{meta_stem}](.meta/{meta_stem}) | {dice_inline} | {summary_short} |\n",
            Utc::now().format("%H:%M:%S"),
        )
    };

    if needs_header {
        dp_write(task_id, &index_path, &index_row, dp, &mut rx).await;
    } else {
        dp_append(task_id, &index_path, &index_row, dp, &mut rx).await;
    }

    // ── Append turn breadcrumb to party.md (data plane) ──────────────────────
    let party_entry = format!(
        "<!-- Turn {turn_num} | {effective_session}/{out_file} | {} UTC | stats: .meta/{meta_stem} -->\n",
        Utc::now().format("%Y-%m-%d %H:%M"),
    );
    dp_append(task_id, &party_path, &party_entry, dp, &mut rx).await;

    AgentResponse {
        task_id,
        status: Status::Complete,
        result: serde_json::json!({
            "output_file": format!("{effective_session}/{out_file}"),
            "scene_dir": params.scene_dir,
            "session_dir": params.session_dir,
            "character": character,
            "action_summary": action_summary,
        }),
        actions_taken: vec![],
        delegated_to: vec![],
    }
}

// ─── Scene Summary pipeline ───────────────────────────────────────────────────

/// Map-reduce over turn meta logs to produce a scene summary.
///
/// Map: extract the pre-written 📋 Summary from each `turn_*.log`, falling back
///      to a model call if missing.
/// Reduce: synthesize all turn summaries into 2-4 sentences of scene prose.
/// Writes: `scene_summary.md` (via data plane) + upserts `SCENES.md` at session level.
async fn run_scene_summary(
    task_id: Uuid,
    params: SceneSummaryParams,
    data_plane: Arc<ChannelDataPlane>,
    state: Arc<AppState>,
) -> AgentResponse {
    let campaign_path = crate::api::campaign::campaign_root(&state.data_dir, &params.campaign_name);
    let scene_path    = campaign_path.join(&params.session_dir).join(&params.scene_dir);
    let meta_dir      = scene_path.join(".meta");
    let meta_log_path = meta_dir.join(params.out_file.replace(".md", ".log"));
    let out_path      = scene_path.join(&params.out_file);

    macro_rules! emit {
        ($step:expr, $body:expr) => {
            let _ = state.task_events.send(TaskEvent {
                task_id,
                kind: TaskEventKind::PipelineStep {
                    step: $step.to_string(),
                    body: $body.to_string(),
                },
            });
        };
    }

    let mut rx = match data_plane.receive_tool_results().await {
        Ok(r) => r,
        Err(e) => return AgentResponse {
            task_id,
            status: Status::Failed,
            result: serde_json::json!({"error": e.to_string()}),
            actions_taken: vec![],
            delegated_to: vec![],
        },
    };
    let dp = data_plane.as_ref();

    // Write meta header
    let header = format!(
        "# Scene Summary: {}/{}\n**Campaign:** {}  **Time:** {}\n",
        params.session_dir,
        params.scene_dir,
        params.campaign_name,
        Utc::now().format("%Y-%m-%d %H:%M:%S UTC"),
    );
    dp_write(task_id, &meta_log_path, &header, dp, &mut rx).await;

    // Collect turn_*.log files from .meta — direct fs scan, data plane reads
    let mut turn_files: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&meta_dir).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) != Some("log") { continue; }
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            if name.starts_with("turn") { turn_files.push(p); }
        }
    }
    turn_files.sort();

    if turn_files.is_empty() {
        let _ = state.task_events.send(TaskEvent {
            task_id,
            kind: TaskEventKind::Failed { message: "No turn log files found".into() },
        });
        return AgentResponse {
            task_id,
            status: Status::Failed,
            result: serde_json::json!({"error": "no turn logs"}),
            actions_taken: vec![],
            delegated_to: vec![],
        };
    }

    // Map: extract per-turn summary from the 📋 Summary section
    let mut turn_summaries: Vec<(String, String)> = Vec::new();
    for path in &turn_files {
        let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
        let text = dp_read(task_id, path, dp, &mut rx).await;
        if text.is_empty() { continue; }

        let existing = text.lines()
            .skip_while(|l| !l.contains("📋 Summary"))
            .skip(1)
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().to_string());

        let mini = if let Some(s) = existing {
            s
        } else {
            emit!("Turn Summarizer", format!("Condensing {fname}…"));
            agents::ollama_chat(
                &params.ollama_host,
                &params.model,
                "Condense this TTRPG turn log into 1-2 past-tense sentences: who acted, \
                 what ability was rolled, whether it succeeded or failed, and what changed. \
                 Use only names from the What Happened section. No prose flourishes.",
                &format!("Turn log: {fname}\n\n{}", text.chars().take(2000).collect::<String>()),
                150,
            ).await.unwrap_or_else(|_| format!("(failed to summarize {fname})"))
        };

        dp_append(
            task_id,
            &meta_log_path,
            &format!("\n## ✍ {fname}\n{mini}\n"),
            dp,
            &mut rx,
        ).await;
        turn_summaries.push((fname.replace(".log", ".md"), mini));
    }

    // Reduce: synthesize into scene summary prose
    let condensed = turn_summaries.iter()
        .map(|(f, s)| format!("**{f}**: {s}"))
        .collect::<Vec<_>>()
        .join("\n\n");

    emit!("Scene Writer", format!("Synthesizing {} turns…", turn_summaries.len()));

    let summary = match agents::ollama_chat(
        &params.ollama_host,
        &params.model,
        "Write 2-4 sentences of past-tense prose summarising this TTRPG scene from the turn facts given. \
         Rules: use character names exactly as given; do not add events not in the input; \
         if a dice roll is mentioned name the ability and outcome; no invented atmosphere. \
         Output only the prose.",
        &format!(
            "Campaign: {}\nScene: {}/{}\n\nTurn facts:\n{condensed}",
            params.campaign_name, params.session_dir, params.scene_dir,
        ),
        400,
    ).await {
        Ok(s) => s,
        Err(e) => {
            let _ = state.task_events.send(TaskEvent {
                task_id,
                kind: TaskEventKind::Failed { message: e.to_string() },
            });
            return AgentResponse {
                task_id,
                status: Status::Failed,
                result: serde_json::json!({"error": e.to_string()}),
                actions_taken: vec![],
                delegated_to: vec![],
            };
        }
    };

    emit!("Scene Writer", &summary);
    dp_append(task_id, &meta_log_path, &format!("\n## 🧠 Scene Writer\n{summary}\n"), dp, &mut rx).await;

    // Write scene_summary.md
    dp_write(task_id, &out_path, &summary, dp, &mut rx).await;
    let _ = state.task_events.send(TaskEvent {
        task_id,
        kind: TaskEventKind::FileWrite {
            path: format!("{}/{}/{}", params.session_dir, params.scene_dir, params.out_file),
            bytes: summary.len(),
        },
    });

    // Upsert SCENES.md at session level (direct fs — index file, not agent data)
    let session_path = campaign_path.join(&params.session_dir);
    let scenes_index = session_path.join("SCENES.md");
    let turn_count = turn_files.len();
    let one_liner: String = summary.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars().take(120).collect();
    let timestamp = Utc::now().format("%H:%M:%S").to_string();
    let out_fn = &params.out_file;
    let scene_dir = &params.scene_dir;

    let existing = tokio::fs::read_to_string(&scenes_index).await.unwrap_or_default();
    let new_content = if existing.is_empty() {
        format!(
            "# {} / {} — Scene Index\n\n\
             | # | Scene | Time (UTC) | Turns | Summary |\n\
             |---|-------|-----------|-------|---------|\n\
             | 1 | {scene_dir} | {timestamp} | {turn_count} | [{out_fn}]({scene_dir}/{out_fn}) — {one_liner} |\n",
            params.campaign_name, params.session_dir,
        )
    } else {
        let mut header_lines: Vec<&str> = Vec::new();
        let mut data_rows: Vec<String>  = Vec::new();
        let mut in_data = false;
        for line in existing.lines() {
            if line.starts_with("|---") { in_data = true; header_lines.push(line); }
            else if in_data && line.starts_with("| ") { data_rows.push(line.to_string()); }
            else { header_lines.push(line); }
        }
        let existing_idx = data_rows.iter().position(|r| {
            r.splitn(6, '|').nth(2).map(|c| c.trim()) == Some(scene_dir.as_str())
        });
        let row_num = existing_idx
            .and_then(|i| data_rows[i].splitn(6, '|').nth(1).and_then(|s| s.trim().parse::<usize>().ok()))
            .unwrap_or(data_rows.len() + 1);
        let new_row = format!(
            "| {row_num} | {scene_dir} | {timestamp} | {turn_count} | [{out_fn}]({scene_dir}/{out_fn}) — {one_liner} |"
        );
        if let Some(idx) = existing_idx { data_rows[idx] = new_row; } else { data_rows.push(new_row); }
        format!("{}\n{}\n", header_lines.join("\n"), data_rows.join("\n"))
    };
    let _ = tokio::fs::write(&scenes_index, new_content.as_bytes()).await;

    AgentResponse {
        task_id,
        status: Status::Complete,
        result: serde_json::json!({
            "output": format!("{}/{}/{}", params.session_dir, params.scene_dir, params.out_file),
        }),
        actions_taken: vec![],
        delegated_to: vec![],
    }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Truncate a string to `max` chars for safe embedding in a DDS intent payload.
/// DDS has a practical string-length limit; large grounding blocks must be clipped
/// before serialization to avoid "Invalid string" publish errors.
#[inline]
fn trunc(s: &str, max: usize) -> String {
    if s.len() <= max { s.to_string() } else { s.chars().take(max).collect() }
}

/// Applies structured stat-change lines (from CharImpact) to party.md text.
///
/// Each change line: `FIELD|Full Character Name|Value`
/// Fields: CDP, DREAD, CONDITION
fn infer_prev_turns(out_file: &str, count: usize) -> Vec<String> {
    if let Some(num_str) = out_file
        .strip_prefix("turn_")
        .and_then(|s| s.strip_suffix(".md"))
    {
        if let Ok(num) = num_str.parse::<u32>() {
            return (1..num).rev().take(count).map(|n| format!("turn_{n:03}.md")).collect();
        }
    }
    vec![]
}

fn build_campaign_facts(campaign_name: &str, party: &str, setting: &str, npcs: &str) -> String {
    let mut parts = vec![format!("CAMPAIGN: {campaign_name}.")];
    if !party.is_empty() {
        parts.push(format!("--- PARTY ---\n{}", party.chars().take(600).collect::<String>()));
    }
    if !setting.is_empty() {
        parts.push(format!("--- SETTING ---\n{}", setting.chars().take(600).collect::<String>()));
    }
    if !npcs.is_empty() {
        parts.push(format!("--- NPCS ---\n{}", npcs.chars().take(600).collect::<String>()));
    }
    parts.join("\n\n")
}
