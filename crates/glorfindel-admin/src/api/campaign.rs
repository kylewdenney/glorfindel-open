use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use glorfindel_agent::Agent;
use glorfindel_schemas::task::{TaskConstraints, TaskRequest};
use glorfindel_transport::ControlPlane;
use glorfindel_schemas::types::{Permission, Status};
use glorfindel_tools::parse_dice_notation;
use rand::Rng;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use tracing::info;

use crate::api::ApiError;
use crate::state::{AppState, TaskEventKind, TaskRecord};
use crate::tools::build_executor;

#[derive(Serialize)]
pub struct CampaignInfo {
    pub name: String,
    pub file_count: usize,
}

#[derive(Serialize)]
pub struct CampaignFile {
    pub filename: String,
    pub path: String,      // relative path from campaign root, e.g. "session1/opening.md"
    pub subdir: Option<String>, // e.g. "session1", None for root files
    pub size_bytes: u64,
}

#[derive(Serialize)]
pub struct FileContents {
    pub filename: String,
    pub contents: String,
}

#[derive(Deserialize)]
pub struct RunTaskBody {
    pub definition_id: Uuid,
    pub intent: String,
    pub permissions: Vec<Permission>,
    pub max_iterations: Option<u32>,
}

#[derive(Serialize)]
pub struct RunTaskResponse {
    pub task_id: Uuid,
    pub agent_instance_id: Uuid,
}

fn safe_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('\\') && !name.contains("..")
}

fn safe_path(path: &str) -> bool {
    !path.is_empty() && !path.contains('\\') && !path.contains("..")
        && path.chars().all(|c| c.is_alphanumeric() || "/._- ".contains(c))
}

/// Resolve a campaign directory. Checks `data_dir/local/campaigns/{name}` first
/// so local-only campaigns take priority without appearing in the repo.
pub(crate) fn campaign_root(data_dir: &std::path::Path, name: &str) -> std::path::PathBuf {
    let local = data_dir.join("local").join("campaigns").join(name);
    if local.exists() { local } else { data_dir.join("campaigns").join(name) }
}

pub async fn list_campaigns(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<CampaignInfo>>, ApiError> {
    let mut campaigns: Vec<CampaignInfo> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    let scan_dirs = [
        state.data_dir.join("campaigns"),
        state.data_dir.join("local").join("campaigns"),
    ];

    for dir in &scan_dirs {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            if let Ok(ft) = entry.file_type().await {
                if ft.is_dir() {
                    if let Some(name) = entry.file_name().to_str() {
                        if seen.insert(name.to_string()) {
                            let file_count = count_files(&entry.path()).await;
                            campaigns.push(CampaignInfo { name: name.to_string(), file_count });
                        }
                    }
                }
            }
        }
    }

    campaigns.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Json(campaigns))
}

async fn count_files(dir: &std::path::Path) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return 0;
    };
    let mut count = 0;
    while let Ok(Some(e)) = entries.next_entry().await {
        if e.file_type().await.map(|t| t.is_file()).unwrap_or(false) {
            count += 1;
        }
    }
    count
}

pub async fn list_files(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<Vec<CampaignFile>>, ApiError> {
    if !safe_name(&name) {
        return Err(ApiError::BadRequest("invalid campaign name".into()));
    }
    let campaign_dir = campaign_root(&state.data_dir, &name);
    if !campaign_dir.exists() {
        return Err(ApiError::NotFound(format!("campaign '{name}' not found")));
    }
    let mut files = Vec::new();
    collect_files(&campaign_dir, &campaign_dir, &mut files).await;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(files))
}

// Recursively collect files, up to two directory levels deep (session/ and session/scene/).
#[async_recursion::async_recursion]
async fn collect_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<CampaignFile>,
) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    let depth = dir.components().count().saturating_sub(root.components().count());
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(ft) = entry.file_type().await else { continue };
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if name_str.starts_with('.') { continue; }

        if ft.is_file() {
            let rel = entry.path()
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let subdir = dir.strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .ok()
                .filter(|s| !s.is_empty());
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            out.push(CampaignFile {
                filename: name_str.to_string(),
                path: rel.to_string(),
                subdir,
                size_bytes: size,
            });
        } else if ft.is_dir() && depth < 2 {
            collect_files(root, &entry.path(), out).await;
        }
    }
}

pub async fn read_file(
    State(state): State<Arc<AppState>>,
    Path((name, file_path)): Path<(String, String)>,
) -> Result<Json<FileContents>, ApiError> {
    if !safe_name(&name) || !safe_path(&file_path) {
        return Err(ApiError::BadRequest("invalid path".into()));
    }
    let path = campaign_root(&state.data_dir, &name).join(&file_path);

    let contents = tokio::fs::read_to_string(&path)
        .await
        .map_err(|_| ApiError::NotFound(format!("file '{file_path}' not found")))?;

    let filename = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&file_path)
        .to_string();

    Ok(Json(FileContents { filename, contents }))
}

#[derive(Deserialize)]
pub struct WriteFileBody {
    pub content: String,
}

pub async fn write_file(
    State(state): State<Arc<AppState>>,
    Path((name, file_path)): Path<(String, String)>,
    Json(body): Json<WriteFileBody>,
) -> Result<Json<FileContents>, ApiError> {
    if !safe_name(&name) || !safe_path(&file_path) {
        return Err(ApiError::BadRequest("invalid path".into()));
    }
    let path = campaign_root(&state.data_dir, &name).join(&file_path);

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| ApiError::Internal(e.to_string()))?;
    }

    tokio::fs::write(&path, &body.content)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let filename = std::path::Path::new(&file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&file_path)
        .to_string();

    Ok(Json(FileContents { filename, contents: body.content }))
}

pub async fn run_task(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<RunTaskBody>,
) -> Result<Json<RunTaskResponse>, ApiError> {
    if !safe_name(&campaign_name) {
        return Err(ApiError::BadRequest("invalid campaign name".into()));
    }

    let definition = {
        let defs = state.definitions.read().await;
        defs.get(&body.definition_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("definition {} not found", body.definition_id)))?
    };

    let campaign_dir = campaign_root(&state.data_dir, &campaign_name)
        .to_string_lossy()
        .into_owned();

    let instance_id = Uuid::new_v4();
    let defs_snapshot = state.definitions.read().await.clone();
    let tool_executor = build_executor(&definition.tools, Some(&campaign_dir), definition.deployed_context.as_ref(), Some(&defs_snapshot)).await;

    let agent: Arc<dyn Agent> = Arc::new(
        glorfindel_agent::OllamaAgent::new(
            instance_id.to_string(),
            &definition.model,
            &definition.ollama_host,
            tool_executor,
            definition.domains.clone(),
        )
        .with_name(&definition.name)
        .with_system_prompt(
            definition
                .system_prompt
                .clone()
                .unwrap_or_else(|| "You are a helpful AI agent.".into()),
        ),
    );

    {
        let mut instances = state.instances.write().await;
        instances.insert(instance_id, agent.clone());
    }

    let task_id = Uuid::new_v4();
    let task_request = TaskRequest {
        task_id,
        parent_task_id: None,
        intent: body.intent.clone(),
        context: vec![],
        constraints: TaskConstraints {
            granted_permissions: body.permissions,
            max_iterations: body.max_iterations.or(Some(12)),
            ..Default::default()
        },
        reply_to: "campaign-ui".into(),
    };

    let agent_name = definition.name.clone();
    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(
            task_id,
            TaskRecord {
                task_id,
                agent_instance_id: instance_id,
                agent_name: agent_name.clone(),
                intent: body.intent,
                status: Status::InProgress,
                submitted_at: Utc::now(),
                completed_at: None,
                response: None,
                error: None,
            },
        );
    }

    let state_clone = state.clone();
    tokio::spawn(async move {
        let result = agent.handle_task(task_request).await;
        let mut tasks = state_clone.tasks.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            match result {
                Ok(response) => {
                    record.status = Status::Complete;
                    record.completed_at = Some(Utc::now());
                    record.response = Some(response.clone());
                    let _ = state_clone.task_events.send(crate::state::TaskEvent {
                        task_id,
                        kind: crate::state::TaskEventKind::Complete { response },
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    record.status = Status::Failed;
                    record.completed_at = Some(Utc::now());
                    record.error = Some(msg.clone());
                    let _ = state_clone.task_events.send(crate::state::TaskEvent {
                        task_id,
                        kind: crate::state::TaskEventKind::Failed { message: msg },
                    });
                }
            }
        }
        // Clean up the ephemeral instance
        let mut instances = state_clone.instances.write().await;
        instances.remove(&instance_id);
    });

    Ok(Json(RunTaskResponse {
        task_id,
        agent_instance_id: instance_id,
    }))
}

#[derive(Deserialize)]
pub struct AppendNoteBody {
    pub text: String,
}

#[derive(Serialize)]
pub struct AppendNoteResponse {
    pub filename: String,
    pub appended: String,
}

pub async fn append_note(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<AppendNoteBody>,
) -> Result<Json<AppendNoteResponse>, ApiError> {
    if !safe_name(&campaign_name) {
        return Err(ApiError::BadRequest("invalid campaign name".into()));
    }
    let text = body.text.trim().to_string();
    if text.is_empty() {
        return Err(ApiError::BadRequest("note text is empty".into()));
    }

    let notes_path = campaign_root(&state.data_dir, &campaign_name).join("session_notes.md");

    let timestamp = Utc::now().format("%H:%M").to_string();
    let line = format!("**[{}]** {}\n\n", timestamp, text);

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&notes_path)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    file.write_all(line.as_bytes())
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(Json(AppendNoteResponse {
        filename: "session_notes.md".into(),
        appended: line,
    }))
}

// ─── Deep Think ───────────────────────────────────────────────────────────────

/// Append a timestamped line to session_notes.md directly (no HTTP round-trip).
async fn append_note_raw(notes_path: &std::path::Path, prefix: &str, text: &str) {
    let timestamp = Utc::now().format("%H:%M:%S").to_string();
    let line = format!("**[{}] {}** {}\n\n", timestamp, prefix, text);
    if let Ok(mut f) = tokio::fs::OpenOptions::new()
        .create(true).append(true).open(notes_path).await
    {
        let _ = f.write_all(line.as_bytes()).await;
    }
}

/// Single Ollama chat call — no tool scaffolding, just raw text completion.
async fn ollama_chat_once(
    host: &str,
    model: &str,
    system: &str,
    user: &str,
) -> anyhow::Result<String> {
    ollama_chat_once_with_tokens(host, model, system, user, 512).await
}

async fn ollama_chat_once_with_tokens(
    host: &str,
    model: &str,
    system: &str,
    user: &str,
    num_predict: u32,
) -> anyhow::Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let resp: serde_json::Value = client
        .post(format!("{host}/api/chat"))
        .json(&serde_json::json!({
            "model": model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user",   "content": user}
            ],
            "stream": false,
            "options": {"temperature": 0.7, "num_predict": num_predict}
        }))
        .send()
        .await?
        .json()
        .await?;
    Ok(resp["message"]["content"]
        .as_str()
        .unwrap_or("(no response)")
        .trim()
        .to_string())
}

/// Roll dice notation in pure Rust — no agent round-trip.
fn roll_inline(notation: &str) -> Option<(Vec<u32>, i32, i32)> {
    let (count, sides, modifier) = parse_dice_notation(notation)?;
    let mut rng = rand::thread_rng();
    let rolls: Vec<u32> = (0..count).map(|_| rng.gen_range(1..=sides)).collect();
    let total: i32 = rolls.iter().map(|&r| r as i32).sum::<i32>() + modifier;
    Some((rolls, modifier, total))
}

pub async fn think_and_run(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<RunTaskBody>,
) -> Result<Json<RunTaskResponse>, ApiError> {
    if !safe_name(&campaign_name) {
        return Err(ApiError::BadRequest("invalid campaign name".into()));
    }

    let definition = {
        let defs = state.definitions.read().await;
        defs.get(&body.definition_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("definition {} not found", body.definition_id)))?
    };

    let campaign_path = campaign_root(&state.data_dir, &campaign_name);
    let campaign_dir  = campaign_path.to_string_lossy().into_owned();
    let notes_path    = campaign_path.join("session_notes.md");
    let ollama_host   = definition.ollama_host.clone();
    let model         = definition.model.clone();
    let intent        = body.intent.clone();

    let instance_id = Uuid::new_v4();
    let task_id     = Uuid::new_v4();
    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(task_id, TaskRecord {
            task_id,
            agent_instance_id: instance_id,
            agent_name: format!("🧠 {}", definition.name),
            intent: format!("[think] {}", body.intent),
            status: Status::InProgress,
            submitted_at: Utc::now(),
            completed_at: None,
            response: None,
            error: None,
        });
    }

    let state_clone   = state.clone();
    let permissions   = body.permissions.clone();
    let max_iters     = body.max_iterations;

    tokio::spawn(async move {

        // ── 1. THINKER ───────────────────────────────────────────────────────
        append_note_raw(&notes_path, "🧠 THINKER", "Analyzing intent…").await;
        let campaign_facts = load_campaign_facts(&campaign_path, &campaign_name).await;
        let thought = ollama_chat_once(&ollama_host, &model,
            "You are a TTRPG DM Thinker. Using ONLY the campaign facts provided, \
             analyze the DM intent and produce a concrete plan in four labelled sections:\n\
             NARRATIVE: tone, atmosphere, and the story beat being played.\n\
             CHARACTERS: which player characters and NPCs are involved — use their EXACT names from the campaign facts.\n\
             RULES: which game mechanics apply — with rough DCs.\n\
             TOOLS NEEDED: list every tool call required: campaign files to read (names), \
             dice to roll (notations like 1d20, 2d6+2).\n\
             Use ONLY the characters listed in the campaign facts. Do not invent characters. Two sentences per section maximum.",
            &format!("{campaign_facts}\n\nDM Intent: {intent}"),
        ).await.unwrap_or_else(|e| format!("(thinker error: {e})"));
        append_note_raw(&notes_path, "🧠 THINKER", &thought).await;

        // ── 2. CRITIC ────────────────────────────────────────────────────────
        append_note_raw(&notes_path, "🔍 CRITIC", "Reviewing plan…").await;
        let critique = ollama_chat_once(&ollama_host, &model,
            "You are a DM Critic. You receive a DM intent and a thinker's plan. Your job:\n\
             1. Correct any wrong rule DCs or missing mechanics.\n\
             2. Add gothic horror flavour the thinker missed (dread, shadows, rain, isolation).\n\
             3. Confirm or revise the list of tools to call — be specific and minimal.\n\
             Output 3-5 sentences. Direct and actionable.",
            &format!("Intent: {intent}\n\nThinker:\n{thought}"),
        ).await.unwrap_or_else(|e| format!("(critic error: {e})"));
        append_note_raw(&notes_path, "🔍 CRITIC", &critique).await;

        // Shared context string built up across tool steps
        let mut tool_context = String::new();

        // ── 3a. RULES LAWYER ─────────────────────────────────────────────────
        // Only use a Rule Consultant whose campaign_dir matches this campaign.
        // Never fall back to a global consultant — it would search a different campaign's rulebook.
        let campaign_name_segment = campaign_path.file_name()
            .and_then(|n| n.to_str()).unwrap_or(&campaign_name);
        let rule_def = {
            let defs = state_clone.definitions.read().await;
            defs.values()
                .find(|d| d.name.contains("Rule Consultant")
                    && d.deployed_context.is_some()
                    && d.campaign_dir.as_deref()
                        .map(|cd| cd.ends_with(campaign_name_segment))
                        .unwrap_or(false))
                .cloned()
        };
        if let Some(rule_def) = rule_def {
            append_note_raw(&notes_path, "⚖ RULES LAWYER", "Extracting rule query…").await;
            let rule_query = ollama_chat_once(&ollama_host, &model,
                "Output ONE rulebook search query (under 12 words). Nothing else, no quotes.",
                &format!("Intent: {intent}\nPlan: {thought}\nCritique: {critique}"),
            ).await.unwrap_or_else(|_| intent.chars().take(60).collect());
            let rule_query = rule_query.trim().trim_matches('"').to_string();

            let defs_snapshot = state_clone.definitions.read().await.clone();
            let rule_executor = build_executor(
                &rule_def.tools, None,
                rule_def.deployed_context.as_ref(), Some(&defs_snapshot),
            ).await;
            let rule_agent: Arc<dyn Agent> = Arc::new(
                glorfindel_agent::OllamaAgent::new(
                    Uuid::new_v4().to_string(), &rule_def.model, &rule_def.ollama_host,
                    rule_executor, rule_def.domains.clone(),
                )
                .with_name("Rules Lawyer")
                .with_system_prompt(
                    rule_def.system_prompt.clone()
                        .unwrap_or_else(|| "Search the rulebook and cite rules with source and text.".into()),
                )
            );
            let rule_task = TaskRequest {
                task_id: Uuid::new_v4(), parent_task_id: Some(task_id),
                intent: rule_query.clone(), context: vec![],
                constraints: TaskConstraints {
                    granted_permissions: vec![Permission::Custom("rulebook.search".into())],
                    max_iterations: Some(4), ..Default::default()
                },
                reply_to: "think-pipeline".into(),
            };
            match rule_agent.handle_task(rule_task).await {
                Ok(resp) => {
                    let hits: Vec<String> = resp.actions_taken.iter()
                        .filter(|a| a.tool_call.tool_name == "rulebook.search")
                        .flat_map(|a| {
                            a.tool_result.output.get("results")
                                .and_then(|r| r.as_array()).into_iter().flatten()
                                .filter_map(|h| {
                                    let src  = h.get("source").and_then(|v| v.as_str()).unwrap_or("?");
                                    let text = h.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                    Some(format!("[{}] {}", src, &text[..text.len().min(150)]))
                                }).collect::<Vec<_>>()
                        }).collect();
                    let result_str = match &resp.result {
                        serde_json::Value::String(s) => s.clone(),
                        v => serde_json::to_string_pretty(v).unwrap_or_default(),
                    };
                    let note = format!("Query: \"{rule_query}\"\n{}\n\n{result_str}",
                        hits.join("\n"));
                    tool_context.push_str(&format!("\n\n=== RULES ===\n{note}"));
                    append_note_raw(&notes_path, "⚖ RULES LAWYER", &note).await;
                }
                Err(e) => {
                    append_note_raw(&notes_path, "⚖ RULES LAWYER", &format!("(failed: {e})")).await;
                }
            }
        }

        // ── 3b. CAMPAIGN REFERENCER ──────────────────────────────────────────
        append_note_raw(&notes_path, "📚 CAMPAIGN", "Selecting relevant files…").await;

        // List what's available first
        let available_files: Vec<String> = {
            let mut out = Vec::new();
            if let Ok(list) = collect_file_list(&campaign_path).await {
                out = list;
            }
            out
        };
        let files_list = available_files.join(", ");

        let file_selection = ollama_chat_once(&ollama_host, &model,
            "Output a JSON array of up to 3 filenames from the provided list that are most relevant. \
             No prose, only the JSON array, e.g.: [\"world.md\",\"party.md\"]",
            &format!("Intent: {intent}\nPlan: {thought}\nAvailable files: [{files_list}]"),
        ).await.unwrap_or_default();

        // Parse JSON array of filenames; fall back gracefully
        let selected: Vec<String> = serde_json::from_str(
            file_selection.trim().trim_start_matches("```json").trim_end_matches("```").trim()
        ).unwrap_or_else(|_| {
            // Try to extract from prose
            available_files.iter()
                .filter(|f| f.contains("world") || f.contains("party") || f.contains("npc"))
                .take(2).cloned().collect()
        });

        let mut file_context = String::new();
        for filename in &selected {
            let fpath = campaign_path.join(filename);
            if let Ok(contents) = tokio::fs::read_to_string(&fpath).await {
                let trimmed = if contents.len() > 2000 { &contents[..2000] } else { &contents };
                file_context.push_str(&format!("### {filename}\n{trimmed}\n\n"));
            }
        }
        if !file_context.is_empty() {
            tool_context.push_str(&format!("\n\n=== CAMPAIGN FILES ===\n{file_context}"));
            append_note_raw(&notes_path, "📚 CAMPAIGN",
                &format!("Read: {}\n\n{}", selected.join(", "), &file_context.chars().take(400).collect::<String>())).await;
        } else {
            append_note_raw(&notes_path, "📚 CAMPAIGN", "(no relevant files found)").await;
        }

        // ── 3c. DICE ROLLER ──────────────────────────────────────────────────
        append_note_raw(&notes_path, "🎲 DICE", "Identifying needed rolls…").await;
        let dice_extraction = ollama_chat_once(&ollama_host, &model,
            "List every dice roll needed for this turn as a JSON array of notation strings. \
             E.g. [\"1d20\",\"2d6+2\"]. If no rolls needed output []. Only JSON, no prose.",
            &format!("Intent: {intent}\nPlan: {thought}{tool_context}"),
        ).await.unwrap_or_else(|_| "[]".into());

        let notations: Vec<String> = serde_json::from_str(
            dice_extraction.trim().trim_start_matches("```json").trim_end_matches("```").trim()
        ).unwrap_or_default();

        let mut dice_lines = Vec::new();
        for notation in &notations {
            if let Some((rolls, modifier, total)) = roll_inline(notation) {
                let rolls_str = rolls.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ");
                let mod_str = if modifier > 0 { format!("+{modifier}") }
                              else if modifier < 0 { modifier.to_string() }
                              else { String::new() };
                let line = format!("{notation} → [{rolls_str}]{mod_str} = **{total}**");
                dice_lines.push(line);
            }
        }
        let dice_summary = if dice_lines.is_empty() {
            "No dice rolled.".into()
        } else {
            dice_lines.join("\n")
        };
        tool_context.push_str(&format!("\n\n=== DICE RESULTS ===\n{dice_summary}"));
        append_note_raw(&notes_path, "🎲 DICE", &dice_summary).await;

        // ── 4. ACTION SUMMARIZER ─────────────────────────────────────────────
        append_note_raw(&notes_path, "📋 ACTION SUMMARY", "Synthesizing turn…").await;
        let action_summary = ollama_chat_once(&ollama_host, &model,
            "You are a Turn Summarizer for a Gothic Horror TTRPG. \
             Given the DM intent, all rules looked up, campaign context read, and dice results, \
             write a crisp 2-3 sentence summary of WHAT HAPPENS this turn from the players' perspective. \
             Include specific dice outcomes and their narrative meaning (hit/miss/partial). \
             Gothic tone — dread, shadow, rain. No DM meta-commentary.",
            &format!("Intent: {intent}\nPlan: {thought}\nCritique: {critique}{tool_context}"),
        ).await.unwrap_or_else(|e| format!("(summarizer error: {e})"));
        append_note_raw(&notes_path, "📋 ACTION SUMMARY", &action_summary).await;

        // ── 5. DM WRITER — runs with full pre-computed context ───────────────
        let enriched = format!(
"CAMPAIGN CONTEXT:
{campaign_facts}

TASK: {intent}

=== PRE-COMPUTED CONTEXT ===
{tool_context}

ACTION SUMMARY:
{action_summary}

Write immersive prose for this scene using ONLY the characters and setting from the campaign context above. \
Use the dice results as-is. 3-5 paragraphs. DO NOT explain your reasoning. DO NOT call tools. Just write the scene."
        );

        append_note_raw(&notes_path, "⚡ DM WRITING", "Running DM agent…").await;

        let defs_snapshot = state_clone.definitions.read().await.clone();
        let tool_executor = build_executor(
            &definition.tools, Some(&campaign_dir),
            definition.deployed_context.as_ref(), Some(&defs_snapshot),
        ).await;
        let agent: Arc<dyn Agent> = Arc::new(
            glorfindel_agent::OllamaAgent::new(
                instance_id.to_string(), &definition.model, &definition.ollama_host,
                tool_executor, definition.domains.clone(),
            )
            .with_name(&definition.name)
            .with_system_prompt(definition.system_prompt.clone()
                .unwrap_or_else(|| format!("You are the DM for the {campaign_name} campaign. \
                    Write immersive prose true to the campaign's tone. Always save your output to a campaign file.")))
        );

        {
            let mut instances = state_clone.instances.write().await;
            instances.insert(instance_id, agent.clone());
        }

        let task_request = TaskRequest {
            task_id, parent_task_id: None, intent: enriched, context: vec![],
            constraints: TaskConstraints {
                granted_permissions: permissions,
                max_iterations: max_iters.or(Some(12)),
                ..Default::default()
            },
            reply_to: "campaign-ui".into(),
        };

        let result = agent.handle_task(task_request).await;
        let mut tasks = state_clone.tasks.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            match result {
                Ok(response) => {
                    record.status = Status::Complete;
                    record.completed_at = Some(Utc::now());
                    record.response = Some(response.clone());
                    let _ = state_clone.task_events.send(crate::state::TaskEvent {
                        task_id,
                        kind: crate::state::TaskEventKind::Complete { response },
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    record.status = Status::Failed;
                    record.completed_at = Some(Utc::now());
                    record.error = Some(msg.clone());
                    let _ = state_clone.task_events.send(crate::state::TaskEvent {
                        task_id,
                        kind: crate::state::TaskEventKind::Failed { message: msg },
                    });
                }
            }
        }
        let mut instances = state_clone.instances.write().await;
        instances.remove(&instance_id);
    });

    Ok(Json(RunTaskResponse { task_id, agent_instance_id: instance_id }))
}

// ─────────────────────────────────────────────────────────────────────────────
// SESSION TURN — structured turn API where the server owns all file writes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SessionTurnBody {
    pub definition_id: Uuid,
    pub session_dir: String,            // e.g. "session1"
    pub intent: String,                 // the player action / DM prompt
    pub output_file: Option<String>,    // e.g. "scene_01_arrival.md"; auto if omitted
    pub permissions: Vec<Permission>,
    pub max_iterations: Option<u32>,
}

#[derive(Serialize)]
pub struct SessionTurnResponse {
    pub task_id: Uuid,
    pub session_dir: String,
    pub output_file: String,
}

pub async fn session_turn(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<SessionTurnBody>,
) -> Result<Json<SessionTurnResponse>, ApiError> {
    if !safe_name(&campaign_name) {
        return Err(ApiError::BadRequest("invalid campaign name".into()));
    }
    if body.session_dir.contains("..") || body.session_dir.contains('\\') {
        return Err(ApiError::BadRequest("invalid session_dir".into()));
    }

    let definition = {
        let defs = state.definitions.read().await;
        defs.get(&body.definition_id)
            .cloned()
            .ok_or_else(|| ApiError::NotFound(format!("definition {} not found", body.definition_id)))?
    };

    let campaign_path = campaign_root(&state.data_dir, &campaign_name);
    let session_path  = campaign_path.join(&body.session_dir);
    tokio::fs::create_dir_all(&session_path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // Determine output filename — auto-increment turn_NNN.md if not provided
    let output_file = match &body.output_file {
        Some(f) if !f.is_empty() => {
            let f = f.trim_start_matches('/').replace(['\\', '/', '.', ' '], "_");
            if f.ends_with("_md") { f.trim_end_matches("_md").to_string() + ".md" }
            else if !f.ends_with(".md") { f + ".md" }
            else { f }
        }
        _ => {
            // Scan session dir for existing turn_NNN.md files
            let mut n = 1u32;
            if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if let Some(rest) = name.strip_prefix("turn_") {
                        if let Ok(num) = rest.trim_end_matches(".md").parse::<u32>() {
                            if num >= n { n = num + 1; }
                        }
                    }
                }
            }
            format!("turn_{n:03}.md")
        }
    };

    let task_id     = Uuid::new_v4();
    let instance_id = Uuid::new_v4();
    let ollama_host = definition.ollama_host.clone();
    let model       = definition.model.clone();
    let intent      = body.intent.clone();
    let permissions = body.permissions.clone();
    let session_dir = body.session_dir.clone();
    let out_file    = output_file.clone();
    let state_clone = state.clone();

    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(task_id, TaskRecord {
            task_id,
            agent_instance_id: instance_id,
            agent_name: format!("📖 {} / {}", campaign_name, session_dir),
            intent: format!("[turn] {}", &intent),
            status: Status::InProgress,
            submitted_at: Utc::now(),
            completed_at: None,
            response: None,
            error: None,
        });
    }

    tokio::spawn(async move {
        let meta_dir  = session_path.join(".meta");
        let _ = tokio::fs::create_dir_all(&meta_dir).await;
        let meta_path = meta_dir.join(out_file.replace(".md", ".log"));
        let prose_path = session_path.join(&out_file);

        // Helper: append a section to the meta log
        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new()
                    .create(true).append(true).open(&meta_path).await
                {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }

        // ── Bus emit helpers ─────────────────────────────────────────────
        macro_rules! emit_step {
            ($step:expr, $body:expr) => {{
                let step_str: String = $step.to_string();
                let body_str: String = $body.to_string();
                info!(step = step_str.as_str(), campaign = campaign_name.as_str(),
                      session = session_dir.as_str(), task = %task_id,
                      "pipeline: {}", &body_str.chars().take(120).collect::<String>());
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep { step: step_str, body: body_str },
                });
            }};
        }
        macro_rules! emit_agent {
            ($name:expr, $model:expr, $ctx:expr) => {{
                info!(name = $name, model = $model, "control-plane: agent spawned");
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::AgentSpawned {
                        name: $name.to_string(),
                        model: $model.to_string(),
                        context: $ctx.to_string(),
                    },
                });
            }};
        }
        macro_rules! emit_tool {
            ($tool:expr, $input:expr, $output:expr) => {{
                info!(tool = $tool, "data-plane: tool call");
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::ToolCall {
                        tool: $tool.to_string(),
                        input: $input.to_string(),
                        output: $output.to_string(),
                    },
                });
            }};
        }
        macro_rules! emit_file {
            ($path:expr, $bytes:expr) => {{
                info!(path = $path, bytes = $bytes, "data-plane: file write");
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::FileWrite {
                        path: $path.to_string(),
                        bytes: $bytes,
                    },
                });
            }};
        }

        // Signal task started on the bus
        let _ = state_clone.task_events.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::Started,
        });

        // Write header
        {
            let header = format!(
                "# Turn: {}\n**Campaign:** {}  **Session:** {}  **File:** {}  **Time:** {}\n",
                intent, campaign_name, session_dir, out_file,
                Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            );
            let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;
        }

        let campaign_facts = load_campaign_facts(&campaign_path, &campaign_name).await;

        // ── 1. THINKER ───────────────────────────────────────────────────────
        let thought = ollama_chat_once(&ollama_host, &model,
            "You are a TTRPG DM Thinker. Using ONLY the campaign facts provided, \
             analyze the DM intent and produce a concrete plan:\n\
             NARRATIVE: tone and story beat (2 sentences).\n\
             CHARACTERS: which PCs/NPCs are involved — use their EXACT names from the campaign facts (2 sentences).\n\
             RULES: which mechanics apply — with DCs (2 sentences).\n\
             TOOLS: campaign files to read (filenames), dice notations.\n\
             Do not invent characters. Use only names from the campaign facts.",
            &format!("{campaign_facts}\n\nDM Intent: {intent}"),
        ).await.unwrap_or_else(|e| format!("(thinker error: {e})"));
        meta!("🧠 Thinker", &thought);
        emit_step!("🧠 Thinker", &thought);

        let mut tool_context = String::new(); // kept for macro compat; no longer used in DM Writer

        // ── 2. WHAT HAPPENED CRITIC — deterministic file reader ──────────────
        // Reads actual recent turn files + world/npcs — no LLM file selection, no hallucination.
        let mut grounding_files: Vec<(String, String)> = Vec::new();

        // Last 2 turns from this session (excluding the output file we're about to write)
        {
            let mut entries: Vec<String> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") && name != out_file && name != "TURNS.md" {
                        entries.push(name);
                    }
                }
            }
            entries.sort();
            for name in entries.iter().rev().take(2) {
                if let Ok(text) = tokio::fs::read_to_string(session_path.join(name)).await {
                    let snippet = text.chars().take(1500).collect::<String>();
                    emit_tool!("campaign.read", &format!("{session_dir}/{name}"), "grounding read".to_string());
                    grounding_files.push((format!("{session_dir}/{name}"), snippet));
                }
            }
        }

        // Last turn from the previous session (session N-1)
        if let Some(num) = session_dir.trim_start_matches("session").parse::<u32>().ok().filter(|&n| n > 1) {
            let prev = format!("session{}", num - 1);
            let prev_path = campaign_path.join(&prev);
            let mut prev_entries: Vec<String> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&prev_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") && name != "TURNS.md" {
                        prev_entries.push(name);
                    }
                }
            }
            prev_entries.sort();
            if let Some(last) = prev_entries.last() {
                if let Ok(text) = tokio::fs::read_to_string(prev_path.join(last)).await {
                    let snippet = text.chars().take(1500).collect::<String>();
                    emit_tool!("campaign.read", &format!("{prev}/{last}"), "grounding read (prev session)".to_string());
                    grounding_files.push((format!("{prev}/{last}"), snippet));
                }
            }
        }

        // world/npcs.md always
        if let Ok(text) = tokio::fs::read_to_string(campaign_path.join("world/npcs.md")).await {
            let snippet = text.chars().take(1500).collect::<String>();
            emit_tool!("campaign.read", "world/npcs.md", "grounding read".to_string());
            grounding_files.push(("world/npcs.md".into(), snippet));
        }

        let grounding_block = grounding_files.iter()
            .map(|(name, text)| format!("### {name}\n{text}\n"))
            .collect::<Vec<_>>().join("\n");

        let critic_context = ollama_chat_once(&ollama_host, &model,
            "You are a campaign fact-checker. Read the files below. \
             CHARACTERS: list every named character with their current situation (one line each). \
             RECENT EVENTS: 2 sentences on what happened most recently relevant to the intent. \
             PEDANTIC RULE: only use names, places, and facts that appear verbatim in the files. \
             Do NOT invent or embellish anything not present in the files.",
            &format!("Intent: {intent}\n\n{grounding_block}"),
        ).await.unwrap_or_default();
        meta!("🔍 What Happened", &critic_context);
        emit_step!("🔍 What Happened", &critic_context);

        // ── 3. RULES LAWYER — structured ROLL| output ────────────────────────
        // Read all rules/ files directly — no LLM selection
        let mut rules_text = String::new();
        if let Ok(mut rd) = tokio::fs::read_dir(campaign_path.join("rules")).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let p = e.path();
                if p.extension().map_or(false, |x| x == "md") {
                    if let Ok(content) = tokio::fs::read_to_string(&p).await {
                        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                        emit_tool!("campaign.read", &format!("rules/{name}"), "rules read".to_string());
                        rules_text.push_str(&format!("### rules/{name}\n{content}\n\n"));
                    }
                }
            }
        }

        let rules_output = ollama_chat_once(&ollama_host, &model,
            "You are a TTRPG rules lawyer. The intent specifies which characters make which checks — use those EXACT character names.\n\
             For each ability check in the intent, output ONE line:\n\
             ROLL|CharacterName|ABILITY|1d20+N|DC|what the roll determines\n\
             Example: ROLL|Kay|ENDURE|1d20+3|16|whether he holds the line or says too much\n\
             STRICT RULES:\n\
             - notation MUST be exactly 1d20+N or 1d20 (no spaces, no 'keep highest', no advantage)\n\
             - use the modifier from the rules file for that character+ability (e.g. Gareth PRESENCE+2 → 1d20+2)\n\
             - if a character has no modifier for that ability, use 1d20\n\
             - the character name must match the intent exactly\n\
             - DC must be a number (10, 14, 18, 22) — never output the literal word 'DC'\n\
             - if unsure of DC, default to 14\n\
             Output ONLY ROLL| lines. No prose. No JSON. No explanation.",
            &format!("Intent: {intent}\nContext:\n{critic_context}\n\nRules:\n{rules_text}"),
        ).await.unwrap_or_default();
        meta!("⚖ Rules Lawyer", &rules_output);
        emit_step!("⚖ Rules Lawyer", &rules_output);

        // ── 4. TURN EXECUTOR (Rust) — parse ROLL| lines, roll, compute pass/fail
        let mut dice_lines: Vec<String> = Vec::new();   // notation strings for TURNS.md
        let mut dice_results: Vec<String> = Vec::new(); // full outcome strings for DM Writer

        for line in rules_output.lines() {
            let line = line.trim();
            if !line.starts_with("ROLL|") { continue; }
            let parts: Vec<&str> = line.splitn(6, '|').collect();
            if parts.len() < 5 { continue; }
            let (char_name, ability, notation, dc_str, reason) =
                (parts[1].trim(), parts[2].trim(), parts[3].trim(), parts[4].trim(),
                 parts.get(5).map(|s| s.trim()).unwrap_or(""));
            // Handle |DC|22 (model uses "DC" as label) by falling back to reason field
            let dc: i32 = dc_str.parse()
                .or_else(|_| reason.trim().parse())
                .unwrap_or(14);

            if let Some((rolls, modifier, total)) = roll_inline(notation) {
                let rolls_str = rolls.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ");
                let mod_str = if modifier > 0 { format!("+{modifier}") }
                              else if modifier < 0 { modifier.to_string() }
                              else { String::new() };
                let outcome = if total >= dc { "SUCCESS" } else { "FAILURE" };
                let result_line = format!(
                    "{char_name} {ability}: {notation} → [{rolls_str}]{mod_str} = **{total}** vs DC {dc} → **{outcome}** ({reason})"
                );
                emit_tool!("dice.roll", notation, format!("total={total} dc={dc} {outcome}"));
                dice_lines.push(notation.to_string());
                dice_results.push(result_line);
            }
        }

        let dice_context = if dice_results.is_empty() {
            format!("No roll required. Focus the scene on: {intent}")
        } else {
            dice_results.join("\n")
        };
        meta!("🎲 Dice", &dice_context);
        emit_step!("🎲 Dice", &dice_context);

        // ── 5. DM WRITER — prose written knowing actual dice outcomes ─────────
        let system_prompt = definition.system_prompt.clone()
            .unwrap_or_else(|| format!(
                "You are the DM for the {campaign_name} campaign. \
                 Write immersive prose true to the campaign's tone and characters."
            ));

        let dm_prompt = format!(
            "CAMPAIGN:\n{campaign_facts}\n\n\
             GROUNDED FACTS (from actual files — use ONLY these names and events):\n\
             {critic_context}\n\n\
             DICE OUTCOMES (these already happened — let them shape the scene):\n\
             {dice_context}\n\n\
             TASK: {intent}\n\n\
             CRITICAL: The grounded facts above are the current state of the world. \
             The scene picks up from there. Do NOT re-describe or re-resolve events that already happened. \
             Write immersive prose. Let the dice outcomes determine what succeeds and what fails. \
             3-5 paragraphs. \
             OUTPUT ONLY NARRATIVE PROSE. No JSON. No tool calls. No structured data. No explanations. Just the scene."
        );

        let dm_prose = ollama_chat_once(&ollama_host, &model, &system_prompt, &dm_prompt)
            .await.unwrap_or_else(|e| format!("*(DM writer error: {e})*"));
        meta!("✍ DM Prose", &dm_prose);
        emit_step!("✍ DM Writer", &dm_prose);

        // ── Write prose to the output file ───────────────────────────────────
        let prose_bytes = dm_prose.len();
        let _ = tokio::fs::write(&prose_path, dm_prose.as_bytes()).await;
        emit_file!(
            format!("{campaign_name}/{session_dir}/{out_file}"),
            prose_bytes
        );

        // ── 6. SUMMARIZER — reads the actual written file ────────────────────
        let written_text = tokio::fs::read_to_string(&prose_path).await.unwrap_or_default();
        let text_for_summary = written_text.chars().take(2000).collect::<String>();
        let action_summary = ollama_chat_once(&ollama_host, &model,
            "Write one sentence: who did what, and what was the outcome. \
             Past tense. Name the characters. If a roll shaped the scene, say so briefly. \
             Do NOT start with 'In turn', 'In this scene', or any meta-framing. \
             One sentence only.",
            &text_for_summary,
        ).await.unwrap_or_else(|e| format!("(summarizer error: {e})"));
        meta!("📋 Summary", &action_summary);
        emit_step!("📋 Summary", &action_summary);

        // ── Append to session TURNS.md index ─────────────────────────────────
        let index_path = session_path.join("TURNS.md");
        let meta_stem  = out_file.replace(".md", ".log");
        let dice_inline = if dice_lines.is_empty() {
            "—".to_string()
        } else {
            dice_lines.join(", ")
        };
        let summary_short = action_summary
            .lines().next().unwrap_or("").chars().take(120).collect::<String>();

        // Write header row on first turn
        let needs_header = !index_path.exists();
        let index_row = if needs_header {
            format!(
                "# {campaign_name} / {session_dir} — Turn Index\n\n\
                 | # | Time (UTC) | Data plane | Control plane | Dice | Summary |\n\
                 |---|-----------|-----------|--------------|------|---------|\n\
                 | 1 | {} | [{out_file}]({out_file}) | [.meta/{meta_stem}](.meta/{meta_stem}) | {dice_inline} | {summary_short} |\n",
                Utc::now().format("%H:%M:%S")
            )
        } else {
            // Count existing data rows to get turn number
            let existing = tokio::fs::read_to_string(&index_path).await.unwrap_or_default();
            let turn_num = existing.lines()
                .filter(|l| l.starts_with("| ") && !l.starts_with("| #") && !l.starts_with("|---"))
                .count() + 1;
            format!(
                "| {turn_num} | {} | [{out_file}]({out_file}) | [.meta/{meta_stem}](.meta/{meta_stem}) | {dice_inline} | {summary_short} |\n",
                Utc::now().format("%H:%M:%S")
            )
        };
        if let Ok(mut f) = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&index_path).await
        {
            let row_len = index_row.len();
            let _ = f.write_all(index_row.as_bytes()).await;
            emit_file!(
                format!("{campaign_name}/{session_dir}/TURNS.md"),
                row_len
            );
        }

        // ── Finish task ───────────────────────────────────────────────────────
        use glorfindel_schemas::agent::AgentResponse;
        let response = AgentResponse {
            task_id,
            status: Status::Complete,
            result: serde_json::json!({
                "output_file": format!("{}/{}", session_dir, out_file),
                "meta_log":    format!("{}/{}", session_dir, format!(".meta/{}", out_file.replace(".md", ".log"))),
                "session_dir": session_dir,
                "action_summary": action_summary,
            }),
            actions_taken: vec![],
            delegated_to: vec![],
        };

        let mut tasks = state_clone.tasks.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            record.status = Status::Complete;
            record.completed_at = Some(Utc::now());
            record.response = Some(response.clone());
            let _ = state_clone.task_events.send(crate::state::TaskEvent {
                task_id,
                kind: crate::state::TaskEventKind::Complete { response },
            });
        }
    });

    Ok(Json(SessionTurnResponse {
        task_id,
        session_dir: body.session_dir,
        output_file,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// PLAYER TURN — player submits an action; DM responds
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct PlayerTurnBody {
    pub definition_id: Option<Uuid>,
    pub session_dir: String,
    pub character: String,       // which PC is acting
    pub action: String,          // what they do
    pub output_file: Option<String>,
}

pub async fn player_turn(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<PlayerTurnBody>,
) -> Result<Json<SessionTurnResponse>, ApiError> {
    if !safe_name(&campaign_name) {
        return Err(ApiError::BadRequest("invalid campaign name".into()));
    }
    if body.session_dir.contains("..") || body.session_dir.contains('\\') {
        return Err(ApiError::BadRequest("invalid session_dir".into()));
    }
    let character = body.character.trim().to_string();
    let action    = body.action.trim().to_string();
    if character.is_empty() || action.is_empty() {
        return Err(ApiError::BadRequest("character and action required".into()));
    }

    // Resolve definition — prefer explicit, then campaign DM def
    let definition = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned()
                .ok_or_else(|| ApiError::NotFound(format!("definition {id} not found")))?
        } else {
            let campaign_path_str = campaign_root(&state.data_dir, &campaign_name)
                .to_string_lossy().to_string();
            defs.values()
                .find(|d| d.campaign_dir.as_deref()
                    .map_or(false, |cd| campaign_path_str.ends_with(cd) || cd.ends_with(&campaign_name)))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no definition found".into()))?
        }
    };

    let campaign_path = campaign_root(&state.data_dir, &campaign_name);
    let session_path  = campaign_path.join(&body.session_dir);
    tokio::fs::create_dir_all(&session_path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let output_file = match &body.output_file {
        Some(f) if !f.is_empty() => {
            let f = f.trim_start_matches('/').replace(['\\', '/', '.', ' '], "_");
            if f.ends_with("_md") { f.trim_end_matches("_md").to_string() + ".md" }
            else if !f.ends_with(".md") { f + ".md" }
            else { f }
        }
        _ => {
            let mut n = 1u32;
            if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if let Some(rest) = name.strip_prefix("turn_") {
                        if let Ok(num) = rest.trim_end_matches(".md").parse::<u32>() {
                            if num >= n { n = num + 1; }
                        }
                    }
                }
            }
            format!("turn_{n:03}.md")
        }
    };

    let task_id     = Uuid::new_v4();
    let instance_id = Uuid::new_v4();
    let ollama_host = definition.ollama_host.clone();
    let model       = definition.model.clone();
    let session_dir = body.session_dir.clone();
    let out_file    = output_file.clone();
    let state_clone = state.clone();

    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(task_id, TaskRecord {
            task_id,
            agent_instance_id: instance_id,
            agent_name: format!("🎭 {} / {} / {}", campaign_name, session_dir, character),
            intent: format!("[player] {character}: {action}"),
            status: Status::InProgress,
            submitted_at: Utc::now(),
            completed_at: None,
            response: None,
            error: None,
        });
    }

    tokio::spawn(async move {
        let meta_dir  = session_path.join(".meta");
        let _ = tokio::fs::create_dir_all(&meta_dir).await;
        let meta_path = meta_dir.join(out_file.replace(".md", ".log"));
        let prose_path = session_path.join(&out_file);

        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new()
                    .create(true).append(true).open(&meta_path).await
                {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }
        macro_rules! emit_step {
            ($step:expr, $body:expr) => {{
                let step_str: String = $step.to_string();
                let body_str: String = $body.to_string();
                info!(step = step_str.as_str(), campaign = campaign_name.as_str(),
                      session = session_dir.as_str(), task = %task_id,
                      "pipeline: {}", &body_str.chars().take(120).collect::<String>());
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep { step: step_str, body: body_str },
                });
            }};
        }
        macro_rules! emit_tool {
            ($tool:expr, $input:expr, $output:expr) => {{
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::ToolCall {
                        tool: $tool.to_string(),
                        input: $input.to_string(),
                        output: $output.to_string(),
                    },
                });
            }};
        }
        macro_rules! emit_file {
            ($path:expr, $bytes:expr) => {{
                let _ = state_clone.task_events.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::FileWrite {
                        path: $path.to_string(),
                        bytes: $bytes,
                    },
                });
            }};
        }

        let _ = state_clone.task_events.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::Started,
        });

        {
            let header = format!(
                "# Player Turn: {character} — {action}\n\
                 **Campaign:** {campaign_name}  **Session:** {session_dir}  **File:** {out_file}  **Time:** {}\n",
                Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
            );
            let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;
        }

        let campaign_facts = load_campaign_facts(&campaign_path, &campaign_name).await;

        // ── 1. WHAT HAPPENED CRITIC — deterministic reads ─────────────────────
        let mut grounding_files: Vec<(String, String)> = Vec::new();

        // Last 2 turns from this session
        {
            let mut entries: Vec<String> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") && name != out_file && name != "TURNS.md" {
                        entries.push(name);
                    }
                }
            }
            entries.sort();
            for name in entries.iter().rev().take(2) {
                if let Ok(text) = tokio::fs::read_to_string(session_path.join(name)).await {
                    let snippet = text.chars().take(1500).collect::<String>();
                    emit_tool!("campaign.read", &format!("{session_dir}/{name}"), "grounding read");
                    grounding_files.push((format!("{session_dir}/{name}"), snippet));
                }
            }
        }

        // Last turn from previous session
        if let Some(num) = session_dir.trim_start_matches("session").parse::<u32>().ok().filter(|&n| n > 1) {
            let prev = format!("session{}", num - 1);
            let prev_path = campaign_path.join(&prev);
            let mut prev_entries: Vec<String> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&prev_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.ends_with(".md") && name != "TURNS.md" {
                        prev_entries.push(name);
                    }
                }
            }
            prev_entries.sort();
            if let Some(last) = prev_entries.last() {
                if let Ok(text) = tokio::fs::read_to_string(prev_path.join(last)).await {
                    let snippet = text.chars().take(1500).collect::<String>();
                    emit_tool!("campaign.read", &format!("{prev}/{last}"), "grounding read (prev session)");
                    grounding_files.push((format!("{prev}/{last}"), snippet));
                }
            }
        }

        // world/npcs.md
        if let Ok(text) = tokio::fs::read_to_string(campaign_path.join("world/npcs.md")).await {
            let snippet = text.chars().take(1500).collect::<String>();
            emit_tool!("campaign.read", "world/npcs.md", "grounding read");
            grounding_files.push(("world/npcs.md".into(), snippet));
        }

        // world/party.md — read in full (no truncation: bounded reference doc, not AI prose)
        let party_text = tokio::fs::read_to_string(campaign_path.join("world/party.md")).await.unwrap_or_default();
        if !party_text.is_empty() {
            emit_tool!("campaign.read", "world/party.md", "character sheet read");
            grounding_files.push(("world/party.md".into(), party_text.clone()));
        }

        let grounding_block = grounding_files.iter()
            .map(|(name, text)| format!("### {name}\n{text}\n"))
            .collect::<Vec<_>>().join("\n");

        let critic_context = ollama_chat_once(&ollama_host, &model,
            "You are a campaign fact-checker. Read the files below. \
             CHARACTERS: list every named character with their current situation (one line each). \
             RECENT EVENTS: 2 sentences on what happened most recently. \
             ACTING CHARACTER: one paragraph on who {character} is, their stats, their Devotion, and where they stand. \
             PEDANTIC RULE: only use names and facts that appear verbatim in the files.",
            &format!("Acting character: {character}\nPlayer action: {action}\n\n{grounding_block}"),
        ).await.unwrap_or_default();
        meta!("🔍 What Happened", &critic_context);
        emit_step!("🔍 What Happened", &critic_context);

        // ── 2. RULES LAWYER — dice_and_abilities.md only (keep it focused) ──────
        let dice_rules = tokio::fs::read_to_string(campaign_path.join("rules/dice_and_abilities.md"))
            .await.unwrap_or_default();
        if !dice_rules.is_empty() {
            emit_tool!("campaign.read", "rules/dice_and_abilities.md", "rules read");
        }

        let rules_output = ollama_chat_once(&ollama_host, &model,
            "You are a TTRPG rules assessor. Output ONLY a ROLL| line or NO_ROLL. No prose.",
            &format!(
                "Character: {character}\nAction: {action}\n\n\
                 Party Stats:\n{party_text}\n\n\
                 Rules:\n{dice_rules}\n\n\
                 Does this action need a check? If yes, output exactly:\n\
                 ROLL|{character}|STAT_NAME|1d20+N|22|what it determines\n\
                 Replace STAT_NAME with the ability used (e.g. OFFERING, PRESENCE, BLADE).\n\
                 Replace N with the integer from the stat line. Examples:\n\
                 OFFERING +3 → 1d20+3\n\
                 PRESENCE +3 with Devotion rating 1 → 1d20+4\n\
                 BLADE +4 → 1d20+4\n\
                 Replace 22 with the appropriate difficulty: 10, 14, 18, 22, or 26\n\
                 If no check needed: NO_ROLL\n\
                 RESPOND WITH ONLY THE ROLL| LINE OR NO_ROLL:"
            ),
        ).await.unwrap_or_default();
        meta!("⚖ Rules Assessor", &rules_output);
        emit_step!("⚖ Rules Assessor", &rules_output);

        // ── 3. TURN EXECUTOR (Rust) ───────────────────────────────────────────
        let mut dice_lines: Vec<String>   = Vec::new();
        let mut dice_results: Vec<String> = Vec::new();

        for line in rules_output.lines() {
            let line = line.trim();
            if !line.starts_with("ROLL|") { continue; }
            let parts: Vec<&str> = line.splitn(6, '|').collect();
            if parts.len() < 5 { continue; }
            let (char_name, ability, notation, dc_str, reason) =
                (parts[1].trim(), parts[2].trim(), parts[3].trim(), parts[4].trim(),
                 parts.get(5).map(|s| s.trim()).unwrap_or(""));
            // Handle |DC|22 (model uses "DC" as label) by falling back to reason field
            let dc: i32 = dc_str.parse()
                .or_else(|_| reason.trim().parse())
                .unwrap_or(14);

            if let Some((rolls, modifier, total)) = roll_inline(notation) {
                let rolls_str = rolls.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ");
                let mod_str = if modifier > 0 { format!("+{modifier}") }
                              else if modifier < 0 { modifier.to_string() }
                              else { String::new() };
                let outcome = if total >= dc { "SUCCESS" } else { "FAILURE" };
                let result_line = format!(
                    "{char_name} {ability}: {notation} → [{rolls_str}]{mod_str} = **{total}** vs DC {dc} → **{outcome}** ({reason})"
                );
                emit_tool!("dice.roll", notation, format!("total={total} dc={dc} {outcome}"));
                dice_lines.push(notation.to_string());
                dice_results.push(result_line);
            }
        }

        let dice_context = if dice_results.is_empty() {
            format!("No roll required. Focus the scene on: {character} — {action}")
        } else {
            dice_results.join("\n")
        };
        meta!("🎲 Dice", &dice_context);
        emit_step!("🎲 Dice", &dice_context);

        // ── 4. DM RESPONSE WRITER ─────────────────────────────────────────────
        let system_prompt = definition.system_prompt.clone()
            .unwrap_or_else(|| format!(
                "You are the DM for the {campaign_name} campaign. \
                 Respond to player actions with immersive prose true to the campaign's tone."
            ));

        let dm_prompt = format!(
            "CAMPAIGN:\n{campaign_facts}\n\n\
             GROUNDED FACTS (use ONLY these names and events):\n\
             {critic_context}\n\n\
             DICE OUTCOMES (already happened — let them shape what follows):\n\
             {dice_context}\n\n\
             PLAYER ACTION: {character} — {action}\n\n\
             CRITICAL: The grounded facts above are the current state of the world. \
             The scene picks up from there. Do NOT re-describe or re-resolve events that already happened. \
             Narrate only what happens next in response to this action. \
             What does the character experience? What changes? What does the world do back? \
             Let dice outcomes determine success and failure. \
             Write in second person (\"you\") if the scene benefits from it, or third if more fitting. \
             3-5 paragraphs. \
             OUTPUT ONLY NARRATIVE PROSE. No JSON. No tool calls. No structured data. No explanations. Just the scene."
        );

        let dm_prose = ollama_chat_once(&ollama_host, &model, &system_prompt, &dm_prompt)
            .await.unwrap_or_else(|e| format!("*(DM response error: {e})*"));
        meta!("✍ DM Response", &dm_prose);
        emit_step!("✍ DM Response", &dm_prose);

        let prose_bytes = dm_prose.len();
        let _ = tokio::fs::write(&prose_path, dm_prose.as_bytes()).await;
        emit_file!(
            format!("{campaign_name}/{session_dir}/{out_file}"),
            prose_bytes
        );

        // ── 5. SUMMARIZER ─────────────────────────────────────────────────────
        let written_text = tokio::fs::read_to_string(&prose_path).await.unwrap_or_default();
        let text_for_summary = written_text.chars().take(2000).collect::<String>();
        let action_summary = ollama_chat_once(&ollama_host, &model,
            "Write one sentence: who did what, and what was the outcome. \
             Past tense. Use the character's full name exactly as given. \
             If a roll shaped the scene, say so briefly. \
             Do NOT start with 'In turn', 'In this scene', or any meta-framing. \
             One sentence only.",
            &format!("Character full name: {character}\n\nScene:\n{text_for_summary}"),
        ).await.unwrap_or_else(|e| format!("(summarizer error: {e})"));
        meta!("📋 Summary", &action_summary);
        emit_step!("📋 Summary", &action_summary);

        // ── Append to TURNS.md ────────────────────────────────────────────────
        let index_path = session_path.join("TURNS.md");
        let meta_stem  = out_file.replace(".md", ".log");
        let dice_inline = if dice_lines.is_empty() { "—".to_string() } else { dice_lines.join(", ") };
        let summary_short = action_summary.lines().next().unwrap_or("").chars().take(120).collect::<String>();

        let needs_header = !index_path.exists();
        let index_row = if needs_header {
            format!(
                "# {campaign_name} / {session_dir} — Turn Index\n\n\
                 | # | Time (UTC) | Data plane | Control plane | Dice | Summary |\n\
                 |---|-----------|-----------|--------------|------|---------|\n\
                 | 1 | {} | [{out_file}]({out_file}) | [.meta/{meta_stem}](.meta/{meta_stem}) | {dice_inline} | {summary_short} |\n",
                Utc::now().format("%H:%M:%S")
            )
        } else {
            let existing = tokio::fs::read_to_string(&index_path).await.unwrap_or_default();
            let turn_num = existing.lines()
                .filter(|l| l.starts_with("| ") && !l.starts_with("| #") && !l.starts_with("|---"))
                .count() + 1;
            format!(
                "| {turn_num} | {} | [{out_file}]({out_file}) | [.meta/{meta_stem}](.meta/{meta_stem}) | {dice_inline} | {summary_short} |\n",
                Utc::now().format("%H:%M:%S")
            )
        };
        if let Ok(mut f) = tokio::fs::OpenOptions::new()
            .create(true).append(true).open(&index_path).await
        {
            let row_len = index_row.len();
            let _ = f.write_all(index_row.as_bytes()).await;
            emit_file!(format!("{campaign_name}/{session_dir}/TURNS.md"), row_len);
        }

        use glorfindel_schemas::agent::AgentResponse;
        let response = AgentResponse {
            task_id,
            status: Status::Complete,
            result: serde_json::json!({
                "output_file":    format!("{}/{}", session_dir, out_file),
                "meta_log":       format!("{}/{}", session_dir, format!(".meta/{}", out_file.replace(".md", ".log"))),
                "session_dir":    session_dir,
                "character":      character,
                "action_summary": action_summary,
            }),
            actions_taken: vec![],
            delegated_to: vec![],
        };

        let mut tasks = state_clone.tasks.write().await;
        if let Some(record) = tasks.get_mut(&task_id) {
            record.status = Status::Complete;
            record.completed_at = Some(Utc::now());
            record.response = Some(response.clone());
            let _ = state_clone.task_events.send(crate::state::TaskEvent {
                task_id,
                kind: crate::state::TaskEventKind::Complete { response },
            });
        }
    });

    Ok(Json(SessionTurnResponse {
        task_id,
        session_dir: body.session_dir,
        output_file,
    }))
}

/// Flat list of relative file paths in a campaign directory (recursive).
// SESSION SUMMARY — reads every *.md in the session dir, synthesizes a recap
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SessionSummaryBody {
    pub definition_id: Option<Uuid>,
    #[serde(default)]
    pub session_dir: String,
    pub output_file: Option<String>,
}

#[derive(Serialize)]
pub struct SessionSummaryResponse {
    pub task_id: Uuid,
    pub output_file: String,
}

pub async fn session_summary(
    State(state): State<Arc<AppState>>,
    Path((campaign_name, session_dir_param)): Path<(String, String)>,
    Json(body): Json<SessionSummaryBody>,
) -> Result<Json<SessionSummaryResponse>, ApiError> {
    // Prefer path param over body field so the route is self-contained
    let body = SessionSummaryBody {
        session_dir: if body.session_dir.is_empty() { session_dir_param } else { body.session_dir },
        ..body
    };
    if !safe_name(&campaign_name) || !safe_name(&body.session_dir) {
        return Err(ApiError::BadRequest("invalid name".into()));
    }

    let def = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned()
                .ok_or_else(|| ApiError::NotFound("definition not found".into()))?
        } else {
            // Pick any DM-flavoured def, otherwise the first available
            defs.values()
                .find(|d| d.name.to_lowercase().contains("dm") || d.name.to_lowercase().contains("dungeon"))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no agent definitions found".into()))?
        }
    };

    let data_dir      = state.data_dir.clone();
    let task_id       = Uuid::new_v4();
    let session_dir   = body.session_dir.clone();
    let out_filename  = body.output_file.clone()
        .unwrap_or_else(|| "session_summary.md".to_string());
    let tx            = state.task_events.clone();

    let out_filename_clone = out_filename.clone();
    tokio::spawn(async move {
        let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::Started });

        let campaign_path = campaign_root(&data_dir, &campaign_name);
        let session_path  = campaign_path.join(&session_dir);

        // Set up meta log immediately so we can stream entries into it
        let meta_dir      = session_path.join(".meta");
        let _ = tokio::fs::create_dir_all(&meta_dir).await;
        let meta_log_name = out_filename_clone.replace(".md", ".log");
        let meta_path     = meta_dir.join(&meta_log_name);
        let header = format!(
            "# Session Summary: {session_dir}\n**Campaign:** {campaign_name}  **Session:** {session_dir}  **File:** {out_filename_clone}  **Time:** {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;

        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new().append(true).open(&meta_path).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }

        // Check if session uses scene subdirectories; if so aggregate their scene_summary.md files
        let mut scene_summaries: Vec<(String, String)> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                if !e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
                let sname = e.file_name().to_string_lossy().to_string();
                if !sname.starts_with("scene") { continue; }
                let summary_p = e.path().join("scene_summary.md");
                if let Ok(t) = tokio::fs::read_to_string(&summary_p).await {
                    scene_summaries.push((sname, t));
                }
            }
        }
        scene_summaries.sort_by(|(a, _), (b, _)| a.cmp(b));

        // Collect turn files — skip TURNS.md, summaries, and anything that isn't a turn prose file
        let mut turn_files: Vec<std::path::PathBuf> = Vec::new();
        if scene_summaries.is_empty() {
            if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) != Some("md") { continue; }
                    let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                    if name.starts_with("turn") {
                        turn_files.push(p);
                    }
                }
            }
            turn_files.sort();
        }

        if turn_files.is_empty() && scene_summaries.is_empty() {
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::Failed { message: "No turn files or scene summaries found in session dir".into() },
            });
            return;
        }

        // ── MAP: summarize each turn individually (or use scene summaries) ───
        meta!("📚 Turn Files", if !scene_summaries.is_empty() {
            scene_summaries.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>().join("\n")
        } else {
            turn_files.iter()
                .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(String::from))
                .collect::<Vec<_>>().join("\n")
        });

        let mut turn_summaries: Vec<(String, String)> = Vec::new(); // (label, mini-summary)

        if !scene_summaries.is_empty() {
            // Scene-based session: use each scene's pre-computed summary directly
            for (sname, text) in &scene_summaries {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep {
                        step: "Scene Aggregator".into(),
                        body: format!("Using {sname}/scene_summary.md ({} chars)", text.len()),
                    },
                });
                meta!(format!("📖 {sname}"), text.chars().take(400).collect::<String>());
                turn_summaries.push((sname.clone(), text.clone()));
            }
        } else {
            for path in &turn_files {
                let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let text = match tokio::fs::read_to_string(path).await {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::ToolCall {
                        tool: "campaign.read".into(),
                        input: format!("{session_dir}/{fname}"),
                        output: format!("{} chars", text.len()),
                    },
                });
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep {
                        step: "Turn Summarizer".into(),
                        body: format!("Condensing {fname}…"),
                    },
                });

                let mini = ollama_chat_once_with_tokens(
                    &def.ollama_host,
                    &def.model,
                    "You are condensing one scene from a TTRPG session. \
                     Write 2-3 sentences in past tense describing only what happened: \
                     who acted, what they found or did, and what changed. \
                     No commentary. No new events. Only what the text describes.",
                    &format!("Scene: {fname}\n\n{text}"),
                    256,
                ).await.unwrap_or_else(|_| format!("(failed to summarize {fname})"));

                meta!(format!("✍ {fname}"), &mini);
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep {
                        step: format!("✍ {fname}"),
                        body: mini.chars().take(160).collect::<String>(),
                    },
                });

                turn_summaries.push((fname, mini));
            }
        }

        // ── REDUCE: synthesize all mini-summaries into the final recap ───────
        let condensed = turn_summaries.iter()
            .map(|(f, s)| format!("**{f}**: {s}"))
            .collect::<Vec<_>>()
            .join("\n\n");

        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Session Writer".into(),
                body: format!("Synthesizing {} turn summaries into final recap…", turn_summaries.len()),
            },
        });

        let summary = match ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are writing a session record for a TTRPG campaign log. \
             You have been given factual scene summaries for each scene in the session. \
             Write 3-4 sentences of past-tense prose covering what happened across the session. \
             RULES: \
             (1) Use character names exactly as given — no added titles, roles, or descriptors. \
             (2) Do not add events, locations, or characters not present in the input. \
             (3) Cover each scene in order. Note significant dice outcomes if mentioned. \
             (4) End with the situation the party is left in. \
             Output only the prose. Nothing else.",
            &format!("Campaign: {campaign_name}\nSession: {session_dir}\n\nScene summaries:\n{condensed}"),
            600,
        ).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed { message: e.to_string() },
                });
                return;
            }
        };

        meta!("🧠 Session Writer", &summary);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Session Writer".into(),
                body: summary.chars().take(200).collect::<String>(),
            },
        });

        // Write meta log file-write event
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::FileWrite {
                path: format!("{session_dir}/.meta/{meta_log_name}"),
                bytes: summary.len(),
            },
        });

        // Write prose output
        let out_path = session_path.join(&out_filename_clone);
        let bytes = summary.len();
        if let Ok(mut f) = tokio::fs::File::create(&out_path).await {
            let _ = f.write_all(summary.as_bytes()).await;
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::FileWrite {
                    path: format!("{session_dir}/{out_filename_clone}"),
                    bytes,
                },
            });
        }

        // ── Upsert into campaign SESSIONS.md ─────────────────────────────────
        let sessions_index = campaign_path.join("SESSIONS.md");
        let scene_count = turn_summaries.len();
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S UTC").to_string();

        let one_liner = summary.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .chars().take(120)
            .collect::<String>();

        let existing = tokio::fs::read_to_string(&sessions_index).await.unwrap_or_default();

        let new_content = if existing.is_empty() {
            format!(
                "# {campaign_name} — Session Index\n\n\
                 | # | Session | Time (UTC) | Scenes | Summary |\n\
                 |---|---------|-----------|--------|---------|\n\
                 | 1 | {session_dir} | {timestamp} | {scene_count} | [{out_filename_clone}]({session_dir}/{out_filename_clone}) — {one_liner} |\n"
            )
        } else {
            let mut header_lines: Vec<&str> = Vec::new();
            let mut data_rows: Vec<String>  = Vec::new();
            let mut in_data = false;

            for line in existing.lines() {
                if line.starts_with("|---") {
                    in_data = true;
                    header_lines.push(line);
                } else if in_data && line.starts_with("| ") {
                    data_rows.push(line.to_string());
                } else {
                    header_lines.push(line);
                }
            }

            let existing_row = data_rows.iter().position(|r| {
                let cols: Vec<&str> = r.splitn(6, '|').collect();
                cols.get(2).map(|c| c.trim()) == Some(session_dir.as_str())
            });

            let new_row_num = existing_row
                .map(|i| {
                    data_rows[i].splitn(6, '|')
                        .nth(1).and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(i + 1)
                })
                .unwrap_or(data_rows.len() + 1);

            let new_row = format!(
                "| {new_row_num} | {session_dir} | {timestamp} | {scene_count} | [{out_filename_clone}]({session_dir}/{out_filename_clone}) — {one_liner} |"
            );

            if let Some(idx) = existing_row {
                data_rows[idx] = new_row;
            } else {
                data_rows.push(new_row);
            }

            format!("{}\n{}\n", header_lines.join("\n"), data_rows.join("\n"))
        };

        let _ = tokio::fs::write(&sessions_index, new_content.as_bytes()).await;
    });

    Ok(Json(SessionSummaryResponse { task_id, output_file: out_filename }))
}

// GRAND OPENER — reads the hook session's summary, twists it dark, opens next session
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct GrandOpenerBody {
    pub definition_id: Option<Uuid>,
    pub hook_session: String,   // e.g. "session5"
}

#[derive(Serialize)]
pub struct GrandOpenerResponse {
    pub task_id: Uuid,
    pub next_session: String,
    pub output_path: String,
}

pub async fn grand_opener(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<GrandOpenerBody>,
) -> Result<Json<GrandOpenerResponse>, ApiError> {
    if !safe_name(&campaign_name) || !safe_name(&body.hook_session) {
        return Err(ApiError::BadRequest("invalid name".into()));
    }

    let def = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned().ok_or_else(|| ApiError::NotFound("definition not found".into()))?
        } else {
            defs.values()
                .find(|d| d.name.to_lowercase().contains("dm") || d.name.to_lowercase().contains("dungeon"))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no agent definitions found".into()))?
        }
    };

    // Derive next session name: session5 → session6, sessionX → sessionX+1, else append _next
    let next_session = {
        let prefix = "session";
        if let Some(num_str) = body.hook_session.strip_prefix(prefix) {
            if let Ok(n) = num_str.parse::<u32>() {
                format!("{prefix}{}", n + 1)
            } else {
                format!("{}_next", body.hook_session)
            }
        } else {
            format!("{}_next", body.hook_session)
        }
    };

    let task_id        = Uuid::new_v4();
    let hook_session   = body.hook_session.clone();
    let next_sess      = next_session.clone();
    let data_dir       = state.data_dir.clone();
    let tx             = state.task_events.clone();
    let out_filename   = "turn01_opening.md".to_string();
    let output_path    = format!("{next_session}/{out_filename}");

    tokio::spawn(async move {
        let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::Started });

        let campaign_path  = campaign_root(&data_dir, &campaign_name);
        let hook_path      = campaign_path.join(&hook_session);
        let next_sess_path = campaign_path.join(&next_sess);
        let meta_dir       = next_sess_path.join(".meta");
        let _ = tokio::fs::create_dir_all(&meta_dir).await;

        let meta_path = meta_dir.join("turn01_opening.log");
        let header = format!(
            "# Grand Opener: {next_sess}\n**Campaign:** {campaign_name}  **Hook:** {hook_session}  **Time:** {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;

        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new().append(true).open(&meta_path).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }

        // Build rich context from the hook session:
        // session_summary.md + each scene's scene_summary.md + last .meta turn log per scene
        let mut context_parts: Vec<String> = Vec::new();

        // Session summary (primary)
        let session_summary_text = tokio::fs::read_to_string(hook_path.join("session_summary.md"))
            .await.unwrap_or_default();
        if !session_summary_text.is_empty() {
            let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::ToolCall {
                tool: "campaign.read".into(),
                input: format!("{hook_session}/session_summary.md"),
                output: format!("{} chars", session_summary_text.len()),
            }});
            context_parts.push(format!("### {hook_session}/session_summary.md\n{session_summary_text}"));
        }

        // Scene summaries, sorted
        let mut scene_dirs: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&hook_path).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with("scene") { scene_dirs.push(e.path()); }
                }
            }
        }
        scene_dirs.sort();

        for scene_path in &scene_dirs {
            let sname = scene_path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();

            // Scene summary
            if let Ok(t) = tokio::fs::read_to_string(scene_path.join("scene_summary.md")).await {
                let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::ToolCall {
                    tool: "campaign.read".into(),
                    input: format!("{hook_session}/{sname}/scene_summary.md"),
                    output: format!("{} chars", t.len()),
                }});
                context_parts.push(format!("### {hook_session}/{sname}/scene_summary.md\n{}", t.chars().take(600).collect::<String>()));
            }

            // Last .meta turn log — most recent grounded facts from this scene
            let meta_dir = scene_path.join(".meta");
            let mut log_files: Vec<std::path::PathBuf> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&meta_dir).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with("turn") && n.ends_with(".log") { log_files.push(e.path()); }
                }
            }
            log_files.sort();
            if let Some(last_log) = log_files.last() {
                let fname = last_log.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if let Ok(t) = tokio::fs::read_to_string(last_log).await {
                    // Extract just the 📋 Summary line — the grounded one-liner
                    let summary_line = t.lines()
                        .skip_while(|l| !l.contains("📋 Summary"))
                        .skip(1)
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("")
                        .to_string();
                    if !summary_line.is_empty() {
                        let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::ToolCall {
                            tool: "campaign.read".into(),
                            input: format!("{hook_session}/{sname}/.meta/{fname}"),
                            output: format!("summary: {}", &summary_line.chars().take(80).collect::<String>()),
                        }});
                        context_parts.push(format!("### Last turn ({sname})\n{summary_line}"));
                    }
                }
            }
        }

        // Fall back to last flat turn file if no scene structure found and no session summary
        if context_parts.is_empty() {
            let mut turn_files: Vec<std::path::PathBuf> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&hook_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let p = e.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("md") {
                        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if n.starts_with("turn") { turn_files.push(p); }
                    }
                }
            }
            turn_files.sort();
            if let Some(last) = turn_files.last() {
                let fname = last.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let t = tokio::fs::read_to_string(last).await.unwrap_or_default();
                context_parts.push(format!("### {hook_session}/{fname} (fallback)\n{}", t.chars().take(1200).collect::<String>()));
                let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::ToolCall {
                    tool: "campaign.read".into(),
                    input: format!("{hook_session}/{fname}"),
                    output: format!("{} chars (fallback)", t.len()),
                }});
            }
        }

        let hook_text = context_parts.join("\n\n");

        if hook_text.is_empty() {
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::Failed { message: "No hook material found in previous session".into() },
            });
            return;
        }

        meta!("📚 Hook Source", format!("{} chars from {} sources", hook_text.len(), context_parts.len()));

        // Step 1: identify unresolved tension and dark complication
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Twist Finder".into(),
                body: "Identifying unresolved threads and dark complication…".into(),
            },
        });

        let twist = ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are a TTRPG DM reading the previous session's record. \
             Based only on what is described in the session material: \
             (1) TENSION: the single most unresolved problem or question the party is carrying into the next session. \
             (2) TWIST: one dark complication grounded in the session's events — something that makes the situation worse, costs more, or reveals something the party didn't know. Do not invent new factions or characters. \
             (3) OPENING IMAGE: one specific sentence describing the first thing the players perceive — a concrete detail, not a mood. \
             Reply in exactly three labelled lines: TENSION: / TWIST: / OPENING IMAGE:",
            &format!("Campaign: {campaign_name}\nPrevious session: {hook_session}\n\n{hook_text}"),
            300,
        ).await.unwrap_or_else(|_| "TENSION: unknown  TWIST: unknown  OPENING IMAGE: the road ahead".into());

        meta!("🔍 Twist Finder", &twist);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Twist Finder".into(),
                body: twist.chars().take(200).collect::<String>(),
            },
        });

        // Step 2: write the opener prose using the campaign's own system prompt
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "DM Writer".into(),
                body: format!("Writing opening scene for {next_sess}…"),
            },
        });

        let dm_system = def.system_prompt.clone()
            .unwrap_or_else(|| format!("You are the DM for the {campaign_name} campaign."));

        let prose = match ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            &dm_system,
            &format!(
                "You are opening {next_sess} of the {campaign_name} campaign. \
                 Write 3-4 paragraphs of past-tense prose that: \
                 opens in the middle of a moment of tension — no waking up, no arriving, no recap; \
                 names the characters using only names from the session record; \
                 introduces the dark complication naturally without announcing it; \
                 ends on a concrete unanswered question. \
                 No headers. No DM commentary. Only narrative prose.\n\n\
                 Session record:\n{hook_text}\n\n\
                 Twist analysis:\n{twist}"
            ),
            900,
        ).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed { message: e.to_string() },
                });
                return;
            }
        };

        meta!("✍ DM Writer", &prose);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "DM Writer".into(),
                body: prose.chars().take(200).collect::<String>(),
            },
        });

        // Write prose file
        let out_path = next_sess_path.join("turn01_opening.md");
        let bytes = prose.len();
        if let Ok(mut f) = tokio::fs::File::create(&out_path).await {
            let _ = f.write_all(prose.as_bytes()).await;
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::FileWrite {
                    path: format!("{next_sess}/turn01_opening.md"),
                    bytes,
                },
            });
        }

        // Write TURNS.md stub for new session
        let turns_path = next_sess_path.join("TURNS.md");
        let turns_content = format!(
            "# {campaign_name} / {next_sess} — Turn Index\n\n\
             | # | Time (UTC) | Data plane | Control plane | Summary |\n\
             |---|-----------|-----------|--------------|--------|\n\
             | 1 | {} | [turn01_opening.md](turn01_opening.md) | [.meta/turn01_opening.log](.meta/turn01_opening.log) | Grand Opener |\n",
            Utc::now().format("%H:%M:%S")
        );
        let _ = tokio::fs::write(&turns_path, turns_content.as_bytes()).await;
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::FileWrite {
                path: format!("{next_sess}/TURNS.md"),
                bytes: turns_content.len(),
            },
        });
    });

    Ok(Json(GrandOpenerResponse { task_id, next_session, output_path }))
}

// ── Eucatastrophe ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct EucatastropheBody {
    pub definition_id: Option<Uuid>,
}

pub async fn eucatastrophe(
    State(state): State<Arc<AppState>>,
    Path(campaign_name): Path<String>,
    Json(body): Json<EucatastropheBody>,
) -> Result<Json<GrandOpenerResponse>, ApiError> {
    if !safe_name(&campaign_name) {
        return Err(ApiError::BadRequest("invalid name".into()));
    }

    let def = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned().ok_or_else(|| ApiError::NotFound("definition not found".into()))?
        } else {
            defs.values()
                .find(|d| d.name.to_lowercase().contains("dm") || d.name.to_lowercase().contains("dungeon"))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no agent definitions found".into()))?
        }
    };

    let campaign_path = campaign_root(&state.data_dir, &campaign_name);

    // Collect and sort session directories
    let mut session_dirs: Vec<String> = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&campaign_path).await {
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                if let Some(name) = e.file_name().to_str() {
                    if name.starts_with("session") { session_dirs.push(name.to_string()); }
                }
            }
        }
    }
    session_dirs.sort();

    if session_dirs.len() < 5 {
        return Err(ApiError::BadRequest(
            format!("Eucatastrophe requires 5+ completed sessions — found {}", session_dirs.len())
        ));
    }

    let last_session = session_dirs.last().unwrap().clone();
    let next_session = {
        if let Some(num_str) = last_session.strip_prefix("session") {
            if let Ok(n) = num_str.parse::<u32>() { format!("session{}", n + 1) }
            else { format!("{last_session}_euca") }
        } else { format!("{last_session}_euca") }
    };

    let task_id      = Uuid::new_v4();
    let next_sess    = next_session.clone();
    let out_filename = "turn01_eucatastrophe.md".to_string();
    let output_path  = format!("{next_session}/{out_filename}");
    let data_dir     = state.data_dir.clone();
    let tx           = state.task_events.clone();

    tokio::spawn(async move {
        let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::Started });

        let next_sess_path = campaign_root(&data_dir, &campaign_name).join(&next_sess);
        let meta_dir       = next_sess_path.join(".meta");
        let _ = tokio::fs::create_dir_all(&meta_dir).await;

        let meta_path = meta_dir.join("turn01_eucatastrophe.log");
        let header = format!(
            "# Eucatastrophe: {next_sess}\n**Campaign:** {campaign_name}  **Sessions:** {}  **Time:** {}\n",
            session_dirs.join(", "),
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;

        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new().append(true).open(&meta_path).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }

        // ── SESSION ARCHIVE — scene-aware: session_summary + 📋 lines, TURNS.md fallback ─
        let mut session_materials: Vec<(String, String)> = Vec::new();
        let mut read_log: Vec<String> = Vec::new();

        for sdir in &session_dirs {
            let sdir_path = campaign_root(&data_dir, &campaign_name).join(sdir);
            let mut parts: Vec<String> = Vec::new();
            let mut sources: Vec<String> = Vec::new();

            // 1. Session-level summary (most grounded single source)
            if let Ok(t) = tokio::fs::read_to_string(sdir_path.join("session_summary.md")).await {
                sources.push(format!("{sdir}/session_summary.md"));
                parts.push(format!("## Session Summary\n{}", t.chars().take(600).collect::<String>()));
            }

            // 2. Scene subdirectories — extract 📋 Summary from each turn log
            let mut scene_dirs: Vec<String> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&sdir_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    if !e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
                    let n = e.file_name().to_string_lossy().to_string();
                    if n.starts_with("scene") { scene_dirs.push(n); }
                }
            }
            scene_dirs.sort();

            for scene in &scene_dirs {
                let meta_dir = sdir_path.join(scene).join(".meta");
                let mut log_files: Vec<std::path::PathBuf> = Vec::new();
                if let Ok(mut rd) = tokio::fs::read_dir(&meta_dir).await {
                    while let Ok(Some(e)) = rd.next_entry().await {
                        let p = e.path();
                        let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                        if n.starts_with("turn") && p.extension().and_then(|s| s.to_str()) == Some("log") {
                            log_files.push(p);
                        }
                    }
                }
                log_files.sort();
                let mut scene_summaries: Vec<String> = Vec::new();
                for log in &log_files {
                    if let Ok(t) = tokio::fs::read_to_string(log).await {
                        let summary = t.lines()
                            .skip_while(|l| !l.contains("📋 Summary"))
                            .skip(1)
                            .find(|l| !l.trim().is_empty())
                            .map(|l| l.trim().to_string());
                        if let Some(s) = summary {
                            let fname = log.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                            scene_summaries.push(format!("- {fname}: {s}"));
                        }
                    }
                }
                if !scene_summaries.is_empty() {
                    sources.push(format!("{sdir}/{scene}/.meta/*.log"));
                    parts.push(format!("## {scene}\n{}", scene_summaries.join("\n")));
                }
            }

            // 3. Fallback — no scene structure; try TURNS.md then last flat turn file
            if parts.is_empty() {
                let (source, text) = if let Ok(t) = tokio::fs::read_to_string(sdir_path.join("TURNS.md")).await {
                    (format!("{sdir}/TURNS.md"), t)
                } else {
                    let mut turn_files: Vec<std::path::PathBuf> = Vec::new();
                    if let Ok(mut rd) = tokio::fs::read_dir(&sdir_path).await {
                        while let Ok(Some(e)) = rd.next_entry().await {
                            let p = e.path();
                            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
                            if n.starts_with("turn") && p.extension().and_then(|s| s.to_str()) == Some("md") {
                                turn_files.push(p);
                            }
                        }
                    }
                    turn_files.sort();
                    if let Some(last) = turn_files.last() {
                        let fname = last.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                        let t = tokio::fs::read_to_string(last).await.unwrap_or_default();
                        (format!("{sdir}/{fname} (fallback)"), t)
                    } else { continue; }
                };
                sources.push(source);
                parts.push(text);
            }

            let text = parts.join("\n\n");
            let source_label = sources.join(", ");
            read_log.push(format!("- {source_label}: {} chars", text.len()));
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::ToolCall {
                    tool: "campaign.read".into(),
                    input: source_label.clone(),
                    output: format!("{} chars", text.len()),
                },
            });
            session_materials.push((sdir.clone(), text));
        }

        // world/npcs.md
        let campaign_path_arc = campaign_root(&data_dir, &campaign_name);
        let npcs_text = tokio::fs::read_to_string(campaign_path_arc.join("world/npcs.md"))
            .await.unwrap_or_default();
        if !npcs_text.is_empty() {
            read_log.push(format!("- world/npcs.md: {} chars", npcs_text.len()));
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::ToolCall {
                    tool: "campaign.read".into(),
                    input: "world/npcs.md".into(),
                    output: format!("{} chars", npcs_text.len()),
                },
            });
        }

        meta!("📖 File Reads", &read_log.join("\n"));

        let combined = session_materials.iter()
            .map(|(s, t)| format!("=== {s} ===\n{}", t.chars().take(1200).collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n\n");

        meta!("📚 Session Archive", &combined.chars().take(2000).collect::<String>());

        // ── WHAT HAPPENED CRITIC — grounds character names/arcs from real files ─
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "What Happened".into(),
                body: "Grounding character arcs from campaign files…".into(),
            },
        });

        let critic_context = ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are a campaign fact-checker. Read all session records provided. \
             CHARACTERS: list every named character and one line on what they specifically did or endured — \
             only facts from the files, no invention. \
             ARC: 3 sentences on what has concretely happened across the sessions — events, decisions, losses. \
             PEDANTIC RULE: use only names and events that appear verbatim in the files.",
            &format!("Campaign: {campaign_name}\n\nNPCs:\n{npcs_text}\n\nSession records:\n{combined}"),
            800,
        ).await.unwrap_or_default();

        meta!("🔍 What Happened", &critic_context);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "What Happened".into(),
                body: critic_context.chars().take(300).collect::<String>(),
            },
        });

        // Step 1: Grace Finder — identify the earned grace
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Grace Finder".into(),
                body: "Searching for the seed of earned grace across all sessions…".into(),
            },
        });

        let grace = ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are finding the eucatastrophe — the sudden unlooked-for grace (Tolkien's term) earned \
             by long endurance. It is not a victory. It is a breath. A mercy. A door opening. \
             Read the character arcs and session records. Identify:\n\
             WEIGHT: what specific things have these specific characters lost, endured, or sacrificed.\n\
             SEED: a small loyalty, act of honesty, or kept faith that has been present but unnoticed — \
             something one of these characters actually did that contained grace.\n\
             MOMENT: one sentence — a concrete image involving these specific characters where something \
             unexpectedly good arrives that the long struggle earned. Name the character. Name the act.\n\
             Reply in three labelled lines: WEIGHT: / SEED: / MOMENT:",
            &format!("Campaign: {campaign_name}\n\nCharacter arcs:\n{critic_context}\n\nFull arc:\n{combined}"),
            400,
        ).await.unwrap_or_else(|_| "WEIGHT: much endured SEED: a small loyalty MOMENT: the light returns".into());

        meta!("🌅 Grace Finder", &grace);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Grace Finder".into(),
                body: grace.chars().take(200).collect::<String>(),
            },
        });

        // Step 2: Scene Architect — block the scene before writing it
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Scene Architect".into(),
                body: "Blocking scene structure from grace moment…".into(),
            },
        });

        // Extract MOMENT content — case-insensitive, handles "MOMENT:", "- Moment:", etc.
        // May be a bare label with content on the next line, or inline after the colon.
        let moment_line = {
            let mut lines = grace.lines().peekable();
            let mut found = String::new();
            while let Some(l) = lines.next() {
                let stripped = l.trim().trim_start_matches('-').trim();
                let upper = stripped.to_uppercase();
                if upper.starts_with("MOMENT:") {
                    let inline = stripped["MOMENT:".len()..].trim().to_string();
                    if !inline.is_empty() {
                        found = inline;
                    } else {
                        found = lines
                            .find(|nl| !nl.trim().is_empty())
                            .map(|nl| nl.trim().trim_start_matches('-').trim().to_string())
                            .unwrap_or_default();
                    }
                    break;
                }
            }
            if found.is_empty() { "an unexpected arrival".to_string() } else { found }
        };

        let skeleton = ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are blocking a single scene beat. A MOMENT has been identified for you. \
             Your only job is to make that MOMENT concrete and playable. \
             Write four numbered lines and nothing else:\n\
             1. WHERE: one sentence — the specific location and who is physically present.\n\
             2. ACTION: one sentence — the single thing that happens. Specific verb. No vagueness.\n\
             3. WORDS: the exact words spoken, if any. Write NONE if silence is right.\n\
             4. LAST IMAGE: one specific object or gesture that ends the beat. Not a feeling. Not a meaning. A thing.\n\
             Do not summarize the campaign. Do not add context. Work only from the MOMENT provided.",
            &format!("MOMENT: {moment_line}\n\nCharacter names available: {critic_context}"),
            150,
        ).await.unwrap_or_else(|_| moment_line.clone());

        meta!("🎬 Scene Architect", &skeleton);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Scene Architect".into(),
                body: skeleton.chars().take(200).collect::<String>(),
            },
        });

        // Step 3: Write the eucatastrophe prose from the blocked scene
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "DM Writer".into(),
                body: format!("Writing eucatastrophe scene for {next_sess}…"),
            },
        });

        let prose = match ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are writing a eucatastrophe scene — the sudden unlooked-for grace after long hardship. \
             You have been given a scene skeleton. Render it in 2-3 paragraphs. \
             RULES: \
             (1) Render only what can be seen or heard. \
             (2) Do not name what any character feels. \
             (3) Do not use the words: hope, burden, glimmer, weight, tear, heart, soul, light, darkness, journey, struggle. \
             (4) Do not explain what the moment means. \
             (5) The last sentence must be a physical image — a specific object, gesture, or sound already present in the scene skeleton. Not a statement. Not a summary. \
             (6) Do not introduce any object, person, or location not already named in the scene skeleton. \
             Past tense. No headers. No game mechanics.",
            &format!("Campaign: {campaign_name}\n\nScene skeleton:\n{skeleton}\n\nCharacter facts:\n{critic_context}"),
            800,
        ).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed { message: e.to_string() },
                });
                return;
            }
        };

        meta!("✍ DM Writer", &prose);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "DM Writer".into(),
                body: prose.chars().take(200).collect::<String>(),
            },
        });

        // Write prose file
        let out_path = next_sess_path.join("turn01_eucatastrophe.md");
        let bytes = prose.len();
        if let Ok(mut f) = tokio::fs::File::create(&out_path).await {
            let _ = f.write_all(prose.as_bytes()).await;
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::FileWrite {
                    path: format!("{next_sess}/turn01_eucatastrophe.md"),
                    bytes,
                },
            });
        }

        // ── SUMMARIZER — reads the actual written file ────────────────────────
        let written = tokio::fs::read_to_string(&out_path).await.unwrap_or_default();
        let summary_sentence = ollama_chat_once_with_tokens(
            &def.ollama_host, &def.model,
            "Write one sentence describing what happened. Past tense. Name the characters. \
             Capture the unexpected grace. Do NOT start with 'In turn' or 'In this scene'. \
             One sentence only.",
            &written.chars().take(2000).collect::<String>(),
            80,
        ).await.unwrap_or_else(|_| "The eucatastrophe arrived.".into());
        meta!("📋 Summary", &summary_sentence);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Summary".into(),
                body: summary_sentence.clone(),
            },
        });

        // ── TURNS.md stub ─────────────────────────────────────────────────────
        let turns_path = next_sess_path.join("TURNS.md");
        let turns_content = format!(
            "# {campaign_name} / {next_sess} — Turn Index\n\n\
             | # | Time (UTC) | Data plane | Control plane | Dice | Summary |\n\
             |---|-----------|-----------|--------------|------|---------|\n\
             | 1 | {} | [turn01_eucatastrophe.md](turn01_eucatastrophe.md) | [.meta/turn01_eucatastrophe.log](.meta/turn01_eucatastrophe.log) | — | {} |\n",
            Utc::now().format("%H:%M:%S"),
            summary_sentence.chars().take(120).collect::<String>()
        );
        let _ = tokio::fs::write(&turns_path, turns_content.as_bytes()).await;
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::FileWrite {
                path: format!("{next_sess}/TURNS.md"),
                bytes: turns_content.len(),
            },
        });
    });

    Ok(Json(GrandOpenerResponse { task_id, next_session, output_path }))
}

/// Build a concise campaign context string for the Thinker/Critic prompts.
/// Reads world/party.md, world/setting.md, and world/npcs.md (whichever exist)
/// so the Thinker knows the actual characters and setting, not a hardcoded one.
async fn load_campaign_facts(campaign_path: &std::path::Path, campaign_name: &str) -> String {
    let world = campaign_path.join("world");
    let candidates = ["party.md", "setting.md", "npcs.md"];
    let mut parts: Vec<String> = vec![format!("CAMPAIGN: {campaign_name}.")];
    for fname in &candidates {
        let p = world.join(fname);
        if let Ok(text) = tokio::fs::read_to_string(&p).await {
            // First 600 chars of each file is enough context for the thinker
            let snippet = text.chars().take(600).collect::<String>();
            parts.push(format!("--- {} ---\n{snippet}", fname.trim_end_matches(".md").to_uppercase()));
        }
    }
    if parts.len() == 1 {
        // No world files found — minimal fallback
        parts.push("(No world files found. Use only information from the DM intent.)".into());
    }
    parts.join("\n\n")
}

async fn collect_file_list(root: &std::path::Path) -> std::io::Result<Vec<String>> {
    fn walk<'a>(
        base: &'a std::path::Path,
        dir: &'a std::path::Path,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::io::Result<Vec<String>>> + Send + 'a>> {
        Box::pin(async move {
            let mut out = Vec::new();
            let mut entries = tokio::fs::read_dir(dir).await?;
            while let Ok(Some(e)) = entries.next_entry().await {
                let ft = match e.file_type().await { Ok(ft) => ft, Err(_) => continue };
                let path = e.path();
                if ft.is_file() {
                    if let Ok(rel) = path.strip_prefix(base) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                } else if ft.is_dir() {
                    out.extend(walk(base, &path).await?);
                }
            }
            Ok(out)
        })
    }
    let mut files = walk(root, root).await?;
    files.sort();
    Ok(files)
}

// SCENE OPENER — generates scene_opener.md for a new scene within a session
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SceneOpenerBody {
    pub definition_id: Option<Uuid>,
    pub scene_name: Option<String>,  // e.g. "scene1"; auto if omitted
}

#[derive(Serialize)]
pub struct SceneOpenerResponse {
    pub task_id: Uuid,
    pub scene_name: String,
    pub output_path: String,
}

pub async fn create_scene_opener(
    State(state): State<Arc<AppState>>,
    Path((campaign_name, session_dir)): Path<(String, String)>,
    Json(body): Json<SceneOpenerBody>,
) -> Result<Json<SceneOpenerResponse>, ApiError> {
    if !safe_name(&campaign_name) || !safe_name(&session_dir) {
        return Err(ApiError::BadRequest("invalid name".into()));
    }

    let def = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned().ok_or_else(|| ApiError::NotFound("definition not found".into()))?
        } else {
            let campaign_path_str = campaign_root(&state.data_dir, &campaign_name)
                .to_string_lossy().to_string();
            defs.values()
                .find(|d| d.campaign_dir.as_deref()
                    .map_or(false, |cd| campaign_path_str.ends_with(cd) || cd.ends_with(&campaign_name)))
                .or_else(|| defs.values()
                    .find(|d| d.name.to_lowercase().contains("dm")))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no agent definitions found".into()))?
        }
    };

    let campaign_path = campaign_root(&state.data_dir, &campaign_name);
    let session_path  = campaign_path.join(&session_dir);

    // Auto-derive scene name if not provided
    let scene_name = if let Some(n) = body.scene_name.filter(|s| !s.is_empty()) {
        n
    } else {
        let mut max_n = 0u32;
        if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                if e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                    let n = e.file_name().to_string_lossy().to_string();
                    if let Some(rest) = n.strip_prefix("scene") {
                        if let Ok(num) = rest.parse::<u32>() {
                            if num > max_n { max_n = num; }
                        }
                    }
                }
            }
        }
        format!("scene{}", max_n + 1)
    };

    let scene_path   = session_path.join(&scene_name);
    tokio::fs::create_dir_all(&scene_path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let meta_dir = scene_path.join(".meta");
    tokio::fs::create_dir_all(&meta_dir).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let task_id      = Uuid::new_v4();
    let out_path     = format!("{session_dir}/{scene_name}/scene_opener.md");
    let out_path_c   = out_path.clone();
    let scene_name_c = scene_name.clone();
    let tx           = state.task_events.clone();
    let data_dir     = state.data_dir.clone();

    tokio::spawn(async move {
        let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::Started });

        let meta_path = meta_dir.join("scene_opener.log");
        let header = format!(
            "# Scene Opener: {session_dir}/{scene_name_c}\n**Campaign:** {campaign_name}  **Session:** {session_dir}  **Scene:** {scene_name_c}  **Time:** {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;

        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new().append(true).open(&meta_path).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }
        macro_rules! emit_step {
            ($step:expr, $body:expr) => {{
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep { step: $step.to_string(), body: $body.to_string() },
                });
            }};
        }

        // ── Read prior context ─────────────────────────────────────────────────
        let mut context_parts: Vec<String> = Vec::new();

        // Party roster
        if let Ok(t) = tokio::fs::read_to_string(campaign_path.join("world/party.md")).await {
            context_parts.push(format!("### world/party.md\n{}\n", t.chars().take(2000).collect::<String>()));
        }

        // World lore
        for fname in &["world.md", "setting.md", "lore.md", "factions.md"] {
            if let Ok(t) = tokio::fs::read_to_string(campaign_path.join("world").join(fname)).await {
                context_parts.push(format!("### world/{fname}\n{}\n", t.chars().take(1500).collect::<String>()));
            }
        }

        // Prior session summary — read the most recent session that has a session_summary.md
        {
            let mut prior_sessions: Vec<(String, String)> = Vec::new();
            if let Ok(mut rd) = tokio::fs::read_dir(&campaign_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    if !e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
                    let sname = e.file_name().to_string_lossy().to_string();
                    if sname == session_dir { continue; }
                    if !sname.starts_with("session") { continue; }
                    if let Ok(t) = tokio::fs::read_to_string(e.path().join("session_summary.md")).await {
                        prior_sessions.push((sname, t));
                    }
                }
            }
            prior_sessions.sort_by(|(a, _), (b, _)| a.cmp(b));
            if let Some((sname, text)) = prior_sessions.last() {
                context_parts.push(format!("### Previous session: {sname}/session_summary.md\n{}\n",
                    text.chars().take(2000).collect::<String>()));
            }
        }

        // Prior scene summary (most recent scene_summary.md in this session)
        let mut prior_scene_summaries: Vec<(String, String)> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                if !e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) { continue; }
                let sname = e.file_name().to_string_lossy().to_string();
                if sname == scene_name_c { continue; }
                let summary_path = e.path().join("scene_summary.md");
                if let Ok(t) = tokio::fs::read_to_string(&summary_path).await {
                    prior_scene_summaries.push((sname, t));
                }
            }
        }
        prior_scene_summaries.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (sname, text) in &prior_scene_summaries {
            context_parts.push(format!("### Prior scene: {sname}/scene_summary.md\n{}\n",
                text.chars().take(1500).collect::<String>()));
        }

        // Session summary (if this isn't the first scene)
        if let Ok(t) = tokio::fs::read_to_string(session_path.join("session_summary.md")).await {
            context_parts.push(format!("### {session_dir}/session_summary.md\n{}\n",
                t.chars().take(1500).collect::<String>()));
        }

        let context_block = context_parts.join("\n");
        meta!("📚 Context", format!("{} chars from {} sources", context_block.len(), context_parts.len()));
        emit_step!("📚 Context", format!("Loaded {} context sources for scene opener", context_parts.len()));

        // ── Generate scene opener ──────────────────────────────────────────────
        let opener_prompt = format!(
            "You are the GM for the {campaign_name} campaign. You are opening a new scene: {session_dir}/{scene_name_c}.\n\n\
             Context from prior play:\n{context_block}\n\n\
             Write a scene opener markdown document with EXACTLY this structure (no deviations):\n\n\
             First: A one-paragraph recap of what has just happened (2-4 sentences, past tense).\n\n\
             Second: Two paragraphs of opening atmosphere and situation — where the characters find themselves, \
             what is at stake, what the world looks like right now. Vivid, sensory, present tense.\n\n\
             Third: Exactly this line (fill in the character names from party.md, only those who are active or present):\n\
             **Characters:** Name1, Name2, Name3\n\n\
             Fourth: A section headed exactly:\n\
             ## Recommended Paths\n\
             One bullet per character listed above. Format: `- **CharacterName**: [one specific action or investigation the scene naturally invites for them, rooted in their Devotion and the current situation]`\n\n\
             RULES: Use only names and facts from the context. Do not invent new NPCs. \
             Do not add headers beyond ## Recommended Paths. Do not add commentary. \
             Do NOT wrap the output in a code block or backticks.\n\n\
             OUTPUT THE DOCUMENT ONLY. No preamble. No backticks."
        );

        let opener = match ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            &format!("You are a skilled GM for the {campaign_name} campaign. \
                      Write focused, atmospheric scene openers that ground players in the current situation."),
            &opener_prompt,
            1200,
        ).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed { message: e.to_string() },
                });
                return;
            }
        };

        meta!("✍ Scene Opener", &opener);
        emit_step!("✍ Scene Opener", opener.chars().take(200).collect::<String>());

        let out_file_path = scene_path.join("scene_opener.md");
        let bytes = opener.len();
        if let Ok(mut f) = tokio::fs::File::create(&out_file_path).await {
            let _ = f.write_all(opener.as_bytes()).await;
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::FileWrite { path: out_path_c, bytes },
            });
        }

        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::Complete {
                response: glorfindel_schemas::agent::AgentResponse {
                    task_id,
                    status: glorfindel_schemas::types::Status::Complete,
                    result: serde_json::json!({ "scene": scene_name_c }),
                    actions_taken: vec![],
                    delegated_to: vec![],
                },
            },
        });
    });

    Ok(Json(SceneOpenerResponse { task_id, scene_name, output_path: out_path }))
}

// SCENE PLAYER TURN — player turn scoped to a scene subdirectory
// ─────────────────────────────────────────────────────────────────────────────

pub async fn scene_player_turn(
    State(state): State<Arc<AppState>>,
    Path((campaign_name, session_dir, scene_dir)): Path<(String, String, String)>,
    Json(body): Json<PlayerTurnBody>,
) -> Result<Json<SessionTurnResponse>, ApiError> {
    if !safe_name(&campaign_name) || !safe_name(&session_dir) || !safe_name(&scene_dir) {
        return Err(ApiError::BadRequest("invalid name".into()));
    }
    let character = body.character.trim().to_string();
    let action    = body.action.trim().to_string();
    if character.is_empty() || action.is_empty() {
        return Err(ApiError::BadRequest("character and action required".into()));
    }

    let definition = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned()
                .ok_or_else(|| ApiError::NotFound(format!("definition {id} not found")))?
        } else {
            let campaign_path_str = campaign_root(&state.data_dir, &campaign_name)
                .to_string_lossy().to_string();
            defs.values()
                .find(|d| d.campaign_dir.as_deref()
                    .map_or(false, |cd| campaign_path_str.ends_with(cd) || cd.ends_with(&campaign_name)))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no definition found".into()))?
        }
    };

    let campaign_path = campaign_root(&state.data_dir, &campaign_name);
    let scene_path    = campaign_path.join(&session_dir).join(&scene_dir);
    tokio::fs::create_dir_all(&scene_path).await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    // The session_dir for downstream use — scene-relative
    let effective_session = format!("{session_dir}/{scene_dir}");
    let effective_session_ret = effective_session.clone();

    let output_file = match &body.output_file {
        Some(f) if !f.is_empty() => {
            let f = f.trim_start_matches('/').replace(['\\', '/', '.', ' '], "_");
            if f.ends_with("_md") { f.trim_end_matches("_md").to_string() + ".md" }
            else if !f.ends_with(".md") { f + ".md" }
            else { f }
        }
        _ => {
            let mut n = 1u32;
            if let Ok(mut rd) = tokio::fs::read_dir(&scene_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if let Some(rest) = name.strip_prefix("turn_") {
                        if let Ok(num) = rest.trim_end_matches(".md").parse::<u32>() {
                            if num >= n { n = num + 1; }
                        }
                    }
                }
            }
            format!("turn_{n:03}.md")
        }
    };

    let task_id     = Uuid::new_v4();
    let instance_id = Uuid::new_v4();
    let ollama_host = definition.ollama_host.clone();
    let model       = definition.model.clone();
    let out_file    = output_file.clone();

    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(task_id, TaskRecord {
            task_id,
            agent_instance_id: instance_id,
            agent_name: format!("🎭 {} / {} / {} / {}", campaign_name, session_dir, scene_dir, character),
            intent: format!("[scene] {character}: {action}"),
            status: glorfindel_schemas::types::Status::InProgress,
            submitted_at: Utc::now(),
            completed_at: None,
            response: None,
            error: None,
        });
    }

    // Emit Started before handing off to the DDS pipeline
    let _ = state.task_events.send(crate::state::TaskEvent {
        task_id,
        kind: TaskEventKind::Started,
    });

    // Publish TaskRequest via DDS — picked up by agent_worker::run_task_worker
    {
        let params = crate::agent_worker::SceneTurnParams {
            task_type: "scene_player_turn".into(),
            task_id,
            campaign_name: campaign_name.clone(),
            session_dir: session_dir.clone(),
            scene_dir: scene_dir.clone(),
            character: character.clone(),
            action: action.clone(),
            out_file: out_file.clone(),
            ollama_host,
            model,
            system_prompt: definition.system_prompt.clone(),
        };
        let intent = serde_json::to_string(&params)
            .map_err(|e| ApiError::Internal(e.to_string()))?;
        let task_request = glorfindel_schemas::task::TaskRequest {
            task_id,
            parent_task_id: None,
            intent,
            context: vec![],
            constraints: glorfindel_schemas::task::TaskConstraints::default(),
            reply_to: "glorfindel/tasks/response".into(),
        };
        let envelope = glorfindel_schemas::MessageEnvelope::new("campaign-handler", task_request);
        if let Err(e) = state.control_plane.publish_task(envelope).await {
            return Err(ApiError::Internal(format!("DDS publish failed: {e}")));
        }
    }


    Ok(Json(SessionTurnResponse {
        task_id,
        session_dir: effective_session_ret,
        output_file,
    }))
}

// SCENE SUMMARY — summarizes all turns in a scene subdirectory
// ─────────────────────────────────────────────────────────────────────────────

pub async fn scene_summary_handler(
    State(state): State<Arc<AppState>>,
    Path((campaign_name, session_dir, scene_dir)): Path<(String, String, String)>,
    Json(body): Json<SessionSummaryBody>,
) -> Result<Json<SessionSummaryResponse>, ApiError> {
    if !safe_name(&campaign_name) || !safe_name(&session_dir) || !safe_name(&scene_dir) {
        return Err(ApiError::BadRequest("invalid name".into()));
    }

    let def = {
        let defs = state.definitions.read().await;
        if let Some(id) = body.definition_id {
            defs.get(&id).cloned()
                .ok_or_else(|| ApiError::NotFound("definition not found".into()))?
        } else {
            defs.values()
                .find(|d| d.name.to_lowercase().contains("dm") || d.name.to_lowercase().contains("dungeon"))
                .or_else(|| defs.values().next())
                .cloned()
                .ok_or_else(|| ApiError::NotFound("no agent definitions found".into()))?
        }
    };

    let task_id     = Uuid::new_v4();
    let out_filename = body.output_file.clone().unwrap_or_else(|| "scene_summary.md".to_string());

    {
        let mut tasks = state.tasks.write().await;
        tasks.insert(task_id, TaskRecord {
            task_id,
            agent_instance_id: Uuid::new_v4(),
            agent_name: format!("📜 {} / {}/{}", campaign_name, session_dir, scene_dir),
            intent: format!("[scene summary] {session_dir}/{scene_dir}"),
            status: glorfindel_schemas::types::Status::InProgress,
            submitted_at: Utc::now(),
            completed_at: None,
            response: None,
            error: None,
        });
    }

    let _ = state.task_events.send(crate::state::TaskEvent {
        task_id,
        kind: TaskEventKind::Started,
    });

    let params = crate::agent_worker::SceneSummaryParams {
        task_type: "scene_summary".into(),
        task_id,
        campaign_name,
        session_dir,
        scene_dir,
        out_file: out_filename.clone(),
        ollama_host: def.ollama_host,
        model: def.model,
    };
    let intent = serde_json::to_string(&params)
        .map_err(|e| ApiError::Internal(e.to_string()))?;
    let task_request = glorfindel_schemas::task::TaskRequest {
        task_id,
        parent_task_id: None,
        intent,
        context: vec![],
        constraints: glorfindel_schemas::task::TaskConstraints::default(),
        reply_to: "glorfindel/tasks/response".into(),
    };
    if let Err(e) = state.control_plane
        .publish_task(glorfindel_schemas::MessageEnvelope::new("campaign-handler", task_request))
        .await
    {
        return Err(ApiError::Internal(format!("DDS publish failed: {e}")));
    }

    Ok(Json(SessionSummaryResponse { task_id, output_file: out_filename }))
}

#[allow(dead_code)]
async fn scene_summary_handler_old(
    State(state): State<Arc<AppState>>,
    Path((campaign_name, session_dir, scene_dir)): Path<(String, String, String)>,
    Json(body): Json<SessionSummaryBody>,
) -> Result<Json<SessionSummaryResponse>, ApiError> {
    let def = {
        let defs = state.definitions.read().await;
        defs.values().next().cloned().ok_or_else(|| ApiError::NotFound("no definitions".into()))?
    };
    let data_dir    = state.data_dir.clone();
    let task_id     = Uuid::new_v4();
    let out_filename = body.output_file.clone().unwrap_or_else(|| "scene_summary.md".to_string());
    let tx          = state.task_events.clone();
    let out_fn_c    = out_filename.clone();

    tokio::spawn(async move {
        let _ = tx.send(crate::state::TaskEvent { task_id, kind: TaskEventKind::Started });

        let campaign_path = campaign_root(&data_dir, &campaign_name);
        let scene_path    = campaign_path.join(&session_dir).join(&scene_dir);
        let meta_dir      = scene_path.join(".meta");
        let _ = tokio::fs::create_dir_all(&meta_dir).await;

        let meta_path = meta_dir.join(out_fn_c.replace(".md", ".log"));
        let header = format!(
            "# Scene Summary: {session_dir}/{scene_dir}\n**Campaign:** {campaign_name}  **Session:** {session_dir}  **Scene:** {scene_dir}  **Time:** {}\n",
            Utc::now().format("%Y-%m-%d %H:%M:%S UTC")
        );
        let _ = tokio::fs::write(&meta_path, header.as_bytes()).await;

        macro_rules! meta {
            ($label:expr, $body:expr) => {{
                let line = format!("\n## {}\n{}\n", $label, $body);
                if let Ok(mut f) = tokio::fs::OpenOptions::new().append(true).open(&meta_path).await {
                    let _ = f.write_all(line.as_bytes()).await;
                }
            }};
        }

        // Collect .meta/turn_*.log files — these contain grounded facts (What Happened,
        // Dice results, per-turn Summary) rather than the creative DM prose, which the
        // scene writer cannot reliably extract character names and mechanics from.
        let meta_dir_path = scene_path.join(".meta");
        let mut turn_files: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&meta_dir_path).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("log") { continue; }
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                if name.starts_with("turn") {
                    turn_files.push(p);
                }
            }
        }
        turn_files.sort();

        if turn_files.is_empty() {
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::Failed { message: "No turn log files found in .meta dir".into() },
            });
            return;
        }

        // MAP: extract the pre-written 📋 Summary line from each log, falling back to
        // asking the model only if the section is absent.
        let mut turn_summaries: Vec<(String, String)> = Vec::new();
        for path in &turn_files {
            let fname = path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
            let Ok(text) = tokio::fs::read_to_string(path).await else { continue };

            // Extract the existing 📋 Summary section written by the pipeline summarizer
            let existing_summary = text.lines()
                .skip_while(|l| !l.contains("📋 Summary"))
                .skip(1)  // skip the "## 📋 Summary" header
                .find(|l| !l.trim().is_empty())
                .map(|l| l.trim().to_string());

            let mini = if let Some(s) = existing_summary {
                s
            } else {
                // Fallback: ask the model, but feed it the structured log not the prose
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::PipelineStep {
                        step: "Turn Summarizer".into(),
                        body: format!("Condensing {fname}…"),
                    },
                });
                ollama_chat_once_with_tokens(
                    &def.ollama_host,
                    &def.model,
                    "You are condensing one turn from a TTRPG campaign. \
                     The log below has structured sections: What Happened (grounded facts), \
                     Rules Assessor (ability used), Dice (roll result), and DM Response. \
                     Write exactly 1-2 sentences in past tense: who acted, what ability was \
                     rolled and whether it succeeded or failed, and what changed. \
                     Use only names that appear in the What Happened section.",
                    &format!("Turn log: {fname}\n\n{}", text.chars().take(2000).collect::<String>()),
                    150,
                ).await.unwrap_or_else(|_| format!("(failed to summarize {fname})"))
            };

            meta!(format!("✍ {fname}"), &mini);
            turn_summaries.push((fname.replace(".log", ".md"), mini));
        }

        // REDUCE: synthesize into scene summary
        let condensed = turn_summaries.iter()
            .map(|(f, s)| format!("**{f}**: {s}"))
            .collect::<Vec<_>>()
            .join("\n\n");

        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Scene Writer".into(),
                body: format!("Synthesizing {} turn summaries…", turn_summaries.len()),
            },
        });

        let summary = match ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are writing a scene record for a TTRPG campaign log. \
             You will be given one factual sentence per turn. \
             Write 2-4 sentences of past-tense prose that covers only what those sentences describe. \
             RULES: \
             (1) Use character names exactly as given — no added titles, roles, or descriptors. \
             (2) Do not add events, locations, or characters not present in the input. \
             (3) If a dice roll is mentioned, name the ability and whether it succeeded or failed. \
             (4) Do not invent atmosphere, motivation, or consequences beyond what is stated. \
             Output only the prose. Nothing else.",
            &format!("Campaign: {campaign_name}\nScene: {session_dir}/{scene_dir}\n\nTurn facts:\n{condensed}"),
            300,
        ).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::Failed { message: e.to_string() },
                });
                return;
            }
        };

        meta!("🧠 Scene Writer", &summary);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Scene Writer".into(),
                body: summary.chars().take(200).collect::<String>(),
            },
        });

        let out_path = scene_path.join(&out_fn_c);
        let bytes = summary.len();
        if let Ok(mut f) = tokio::fs::File::create(&out_path).await {
            let _ = f.write_all(summary.as_bytes()).await;
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::FileWrite {
                    path: format!("{session_dir}/{scene_dir}/{out_fn_c}"),
                    bytes,
                },
            });
        }

        // ── Upsert into session SCENES.md ────────────────────────────────────
        // Re-running a scene summary replaces the existing row for that scene
        // rather than appending a duplicate.
        let session_path = campaign_path.join(&session_dir);
        let scenes_index = session_path.join("SCENES.md");

        // Count turn files in this scene for the index row
        let turn_count = {
            let mut n = 0usize;
            if let Ok(mut rd) = tokio::fs::read_dir(&scene_path).await {
                while let Ok(Some(e)) = rd.next_entry().await {
                    let name = e.file_name().to_string_lossy().to_string();
                    if name.starts_with("turn_") && name.ends_with(".md") { n += 1; }
                }
            }
            n
        };

        // One-line summary: first non-empty line from the scene summary
        let one_liner = summary.lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .chars().take(120)
            .collect::<String>();

        let timestamp = Utc::now().format("%H:%M:%S").to_string();

        let existing = tokio::fs::read_to_string(&scenes_index).await.unwrap_or_default();

        let new_content = if existing.is_empty() {
            // First scene ever — write header + row 1
            format!(
                "# {campaign_name} / {session_dir} — Scene Index\n\n\
                 | # | Scene | Time (UTC) | Turns | Summary |\n\
                 |---|-------|-----------|-------|---------|\n\
                 | 1 | {scene_dir} | {timestamp} | {turn_count} | [{out_fn_c}]({scene_dir}/{out_fn_c}) — {one_liner} |\n"
            )
        } else {
            // Split into header lines and data rows
            let mut header_lines: Vec<&str> = Vec::new();
            let mut data_rows: Vec<String>  = Vec::new();
            let mut in_data = false;

            for line in existing.lines() {
                if line.starts_with("|---") {
                    in_data = true;
                    header_lines.push(line);
                } else if in_data && line.starts_with("| ") {
                    data_rows.push(line.to_string());
                } else {
                    header_lines.push(line);
                }
            }

            // Check if a row for this scene already exists
            let existing_row = data_rows.iter().position(|r| {
                // Each row is: | N | scene_dir | ... |
                let cols: Vec<&str> = r.splitn(6, '|').collect();
                cols.get(2).map(|c| c.trim()) == Some(scene_dir.as_str())
            });

            let new_row_num = existing_row
                .map(|i| {
                    data_rows[i].splitn(6, '|')
                        .nth(1).and_then(|s| s.trim().parse::<usize>().ok())
                        .unwrap_or(i + 1)
                })
                .unwrap_or(data_rows.len() + 1);

            let new_row = format!(
                "| {new_row_num} | {scene_dir} | {timestamp} | {turn_count} | [{out_fn_c}]({scene_dir}/{out_fn_c}) — {one_liner} |"
            );

            if let Some(idx) = existing_row {
                data_rows[idx] = new_row;
            } else {
                data_rows.push(new_row);
            }

            format!("{}\n{}\n", header_lines.join("\n"), data_rows.join("\n"))
        };

        let _ = tokio::fs::write(&scenes_index, new_content.as_bytes()).await;

        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::Complete {
                response: glorfindel_schemas::agent::AgentResponse {
                    task_id,
                    status: glorfindel_schemas::types::Status::Complete,
                    result: serde_json::json!({ "output": format!("{session_dir}/{scene_dir}/{out_fn_c}") }),
                    actions_taken: vec![],
                    delegated_to: vec![],
                },
            },
        });
    });

    Ok(Json(SessionSummaryResponse { task_id, output_file: out_filename }))
}
