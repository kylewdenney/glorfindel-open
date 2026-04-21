use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Utc;
use glorfindel_agent::Agent;
use glorfindel_schemas::task::{TaskConstraints, TaskRequest};
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

pub async fn list_campaigns(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<CampaignInfo>>, ApiError> {
    let campaigns_dir = state.data_dir.join("campaigns");
    let mut entries = tokio::fs::read_dir(&campaigns_dir)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let mut campaigns = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        if let Ok(ft) = entry.file_type().await {
            if ft.is_dir() {
                if let Some(name) = entry.file_name().to_str() {
                    let campaign_dir = campaigns_dir.join(name);
                    let file_count = count_files(&campaign_dir).await;
                    campaigns.push(CampaignInfo {
                        name: name.to_string(),
                        file_count,
                    });
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
    let campaign_dir = state.data_dir.join("campaigns").join(&name);
    if !campaign_dir.exists() {
        return Err(ApiError::NotFound(format!("campaign '{name}' not found")));
    }
    let mut files = Vec::new();
    collect_files(&campaign_dir, &campaign_dir, &mut files).await;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Json(files))
}

// Recursively collect files, one level of subdirectories deep.
#[async_recursion::async_recursion]
async fn collect_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    out: &mut Vec<CampaignFile>,
) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(ft) = entry.file_type().await else { continue };
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };

        if ft.is_file() {
            let rel = entry.path()
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            let subdir = if dir == root {
                None
            } else {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .map(String::from)
            };
            let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
            out.push(CampaignFile {
                filename: name_str.to_string(),
                path: rel.to_string(),
                subdir,
                size_bytes: size,
            });
        } else if ft.is_dir() && dir == root {
            // One level of subdirectory only
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
    let path = state
        .data_dir
        .join("campaigns")
        .join(&name)
        .join(&file_path);

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
    let path = state
        .data_dir
        .join("campaigns")
        .join(&name)
        .join(&file_path);

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

    let campaign_dir = state
        .data_dir
        .join("campaigns")
        .join(&campaign_name)
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

    let notes_path = state
        .data_dir
        .join("campaigns")
        .join(&campaign_name)
        .join("session_notes.md");

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

    let campaign_path = state.data_dir.join("campaigns").join(&campaign_name);
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
        let campaign_facts = format!(
            "CAMPAIGN: {campaign_name}. PARTY: Sister Isolde Carrow (fallen cleric), \
             Dorian Ashgrove (disgraced investigator), Emmett Grave (graverobber/archaeologist), \
             Vera Nighthollow (witch). VILLAGE: Ravenmoor (in the Ashfen Marshes)."
        );
        let thought = ollama_chat_once(&ollama_host, &model,
            "You are a Gothic Horror TTRPG DM Thinker. \
             Analyze the DM intent and produce a concrete plan in four labelled sections:\n\
             NARRATIVE: tone, atmosphere, and the story beat being played.\n\
             CHARACTERS: which player characters and NPCs are involved and what matters about them.\n\
             RULES: which game mechanics apply — fear saves, investigation, combat, social — with rough DCs.\n\
             TOOLS NEEDED: list every tool call required: rules check (query), campaign files to read (names), \
             dice to roll (notations like 1d20, 2d6+2).\n\
             Be specific. Use the exact character names given. Two sentences per section maximum.",
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
        // Prefer a consultant whose campaign_dir ends with the same campaign name segment.
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
                .or_else(|| defs.values().find(|d| d.name == "Rule Consultant"))
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
"CAMPAIGN: {campaign_name}
PARTY: Sister Isolde Carrow (fallen cleric), Dorian Ashgrove (investigator), Emmett Grave (graverobber), Vera Nighthollow (witch)
VILLAGE: Ravenmoor (NOT Ebonhollow, NOT Hollowbrook — always call it RAVENMOOR)

TASK: {intent}

=== PRE-COMPUTED CONTEXT ===
{tool_context}

ACTION SUMMARY:
{action_summary}

Write vivid gothic horror prose for this scene. \
Use EXACTLY the names above for the party. The village is RAVENMOOR. \
Use the dice results as-is. Save the scene to the campaign/{campaign_name}/session1/ directory."
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
                .unwrap_or_else(|| "You are a gothic horror DM. Write atmospheric, \
                    dread-filled prose. Always save your output to a campaign file.".into()))
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

    let campaign_path = state.data_dir.join("campaigns").join(&campaign_name);
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

        let campaign_facts = format!(
            "CAMPAIGN: {campaign_name}. PARTY: Sister Isolde Carrow (fallen cleric), \
             Dorian Ashgrove (disgraced investigator), Emmett Grave (graverobber/archaeologist), \
             Vera Nighthollow (witch). VILLAGE: Ravenmoor (Ashfen Marshes)."
        );

        // ── 1. THINKER ───────────────────────────────────────────────────────
        let thought = ollama_chat_once(&ollama_host, &model,
            "You are a Gothic Horror TTRPG DM Thinker. \
             Analyze the DM intent and produce a concrete plan:\n\
             NARRATIVE: tone and story beat (2 sentences).\n\
             CHARACTERS: which PCs/NPCs are involved and what matters about them (2 sentences).\n\
             RULES: which mechanics apply — fear saves, investigation, social — with DCs (2 sentences).\n\
             TOOLS: rules query needed (1 line), campaign files to read (filenames), dice notations.",
            &format!("{campaign_facts}\n\nDM Intent: {intent}"),
        ).await.unwrap_or_else(|e| format!("(thinker error: {e})"));
        meta!("🧠 Thinker", &thought);
        emit_step!("🧠 Thinker", &thought);

        // ── 2. CRITIC ────────────────────────────────────────────────────────
        let critique = ollama_chat_once(&ollama_host, &model,
            "You are a DM Critic for a gothic horror campaign. Review the plan:\n\
             1. Correct wrong DCs or missing mechanics.\n\
             2. Add gothic atmosphere the thinker missed.\n\
             3. Confirm/revise the tool list — be specific and minimal.\n\
             Output 3-5 sentences.",
            &format!("{campaign_facts}\n\nIntent: {intent}\n\nThinker:\n{thought}"),
        ).await.unwrap_or_else(|e| format!("(critic error: {e})"));
        meta!("🔍 Critic", &critique);
        emit_step!("🔍 Critic", &critique);

        let mut tool_context = String::new();

        // ── 3a. RULES LAWYER ─────────────────────────────────────────────────
        let campaign_name_seg = campaign_path.file_name()
            .and_then(|n| n.to_str()).unwrap_or(&campaign_name);
        let rule_def = {
            let defs = state_clone.definitions.read().await;
            defs.values()
                .find(|d| d.name.contains("Rule Consultant")
                    && d.deployed_context.is_some()
                    && d.campaign_dir.as_deref()
                        .map(|cd| cd.ends_with(campaign_name_seg))
                        .unwrap_or(false))
                .or_else(|| defs.values().find(|d| d.name == "Rule Consultant"))
                .cloned()
        };
        if let Some(rule_def) = rule_def {
            let rule_query = ollama_chat_once(&ollama_host, &model,
                "Output ONE rulebook search query under 12 words. Nothing else.",
                &format!("Intent: {intent}\nPlan: {thought}\nCritique: {critique}"),
            ).await.unwrap_or_else(|_| intent.chars().take(60).collect());
            let rule_query = rule_query.trim().trim_matches('"').to_string();

            // Control plane: agent spawned
            emit_agent!("Rules Lawyer", &rule_def.model,
                format!("rulebook.search → \"{rule_query}\""));

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
                .with_system_prompt(rule_def.system_prompt.clone()
                    .unwrap_or_else(|| "Search the rulebook and cite rules verbatim.".into()))
            );
            let rule_task = TaskRequest {
                task_id: Uuid::new_v4(), parent_task_id: Some(task_id),
                intent: rule_query.clone(), context: vec![],
                constraints: TaskConstraints {
                    granted_permissions: vec![Permission::Custom("rulebook.search".into())],
                    max_iterations: Some(4), ..Default::default()
                },
                reply_to: "session-turn".into(),
            };
            let rule_note = match rule_agent.handle_task(rule_task).await {
                Ok(resp) => {
                    // Data plane: emit each tool call the agent made
                    for action in &resp.actions_taken {
                        if action.tool_call.tool_name == "rulebook.search" {
                            let hits_count = action.tool_result.output
                                .get("results").and_then(|r| r.as_array())
                                .map(|a| a.len()).unwrap_or(0);
                            emit_tool!(
                                "rulebook.search",
                                &action.tool_call.parameters.get("query")
                                    .and_then(|v| v.as_str()).unwrap_or(&rule_query),
                                format!("{hits_count} results returned")
                            );
                        }
                    }
                    let hits: Vec<String> = resp.actions_taken.iter()
                        .filter(|a| a.tool_call.tool_name == "rulebook.search")
                        .flat_map(|a| {
                            a.tool_result.output.get("results")
                                .and_then(|r| r.as_array()).into_iter().flatten()
                                .filter_map(|h| {
                                    let src  = h.get("source").and_then(|v| v.as_str()).unwrap_or("?");
                                    let text = h.get("text").and_then(|v| v.as_str()).unwrap_or("");
                                    Some(format!("[{}] {}", src, &text[..text.len().min(200)]))
                                }).collect::<Vec<_>>()
                        }).collect();
                    let result_str = match &resp.result {
                        serde_json::Value::String(s) => s.clone(),
                        v => serde_json::to_string_pretty(v).unwrap_or_default(),
                    };
                    format!("Query: \"{rule_query}\"\n{}\n\n{result_str}", hits.join("\n"))
                }
                Err(e) => format!("(rules lookup failed: {e})"),
            };
            tool_context.push_str(&format!("\n\n### RULES\n{rule_note}"));
            meta!("⚖ Rules Lawyer", &rule_note);
            emit_step!("⚖ Rules Lawyer", &rule_note);
        }

        // ── 3b. CAMPAIGN REFERENCER ──────────────────────────────────────────
        let available_files = collect_file_list(&campaign_path).await
            .unwrap_or_default()
            .into_iter()
            .filter(|f| !f.starts_with(".meta"))
            .collect::<Vec<_>>();
        let files_list = available_files.join(", ");

        let file_selection = ollama_chat_once(&ollama_host, &model,
            "Output a JSON array of up to 3 filenames from the list that are most relevant. \
             Prefer world/, rules/, and the current session dir. \
             No prose, only JSON array, e.g.: [\"world/party.md\",\"rules/fear_and_dread.md\"]",
            &format!("Intent: {intent}\nPlan: {thought}\nAvailable: [{files_list}]"),
        ).await.unwrap_or_default();

        let selected: Vec<String> = serde_json::from_str(
            file_selection.trim().trim_start_matches("```json").trim_end_matches("```").trim()
        ).unwrap_or_else(|_| {
            available_files.iter()
                .filter(|f| f.contains("party") || f.contains("npc") || f.contains("hook"))
                .take(2).cloned().collect()
        });

        let mut file_context = String::new();
        for filename in &selected {
            if filename.starts_with(".meta") { continue; }
            let fpath = campaign_path.join(filename);
            if let Ok(contents) = tokio::fs::read_to_string(&fpath).await {
                let trimmed = if contents.len() > 2000 { &contents[..2000] } else { &contents };
                // Data plane: campaign file read
                emit_tool!("campaign.read", filename,
                    format!("{} chars", contents.len()));
                file_context.push_str(&format!("### {filename}\n{trimmed}\n\n"));
            }
        }
        if !file_context.is_empty() {
            let files_note = format!("Read: {}\n\n{}", selected.join(", "), &file_context.chars().take(600).collect::<String>());
            tool_context.push_str(&format!("\n\n### CAMPAIGN FILES\n{file_context}"));
            meta!("📚 Campaign Files", &files_note);
            emit_step!("📚 Campaign Files", &files_note);
        }

        // ── 3c. DICE ROLLER ──────────────────────────────────────────────────
        let dice_extraction = ollama_chat_once(&ollama_host, &model,
            "List every dice roll needed as a JSON array of notation strings, e.g. [\"1d20\",\"2d6+2\"]. \
             If no rolls, output []. Only JSON.",
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
                // Data plane: each individual roll
                emit_tool!("dice.roll", notation, format!("total={total}"));
                dice_lines.push(line);
            }
        }
        let dice_summary = if dice_lines.is_empty() { "No dice rolled.".into() }
                           else { dice_lines.join("\n") };
        tool_context.push_str(&format!("\n\n### DICE\n{dice_summary}"));
        meta!("🎲 Dice", &dice_summary);
        emit_step!("🎲 Dice", &dice_summary);

        // ── 4. ACTION SUMMARIZER ─────────────────────────────────────────────
        let action_summary = ollama_chat_once(&ollama_host, &model,
            "You are a Turn Summarizer for a Gothic Horror TTRPG. \
             Given the intent, rules, campaign context, and dice results, \
             write 2-3 sentences summarising WHAT HAPPENS this turn from the players' perspective. \
             Include specific dice outcomes and their narrative meaning. Gothic tone. No DM meta-commentary.",
            &format!("Intent: {intent}\nPlan: {thought}\nCritique: {critique}{tool_context}"),
        ).await.unwrap_or_else(|e| format!("(summarizer error: {e})"));
        tool_context.push_str(&format!("\n\n### ACTION SUMMARY\n{action_summary}"));
        meta!("📋 Action Summary", &action_summary);
        emit_step!("📋 Action Summary", &action_summary);

        // ── 5. DM WRITER — prose only, server writes the file ────────────────
        let system_prompt = definition.system_prompt.clone()
            .unwrap_or_else(|| format!(
                "You are the DM for the {campaign_name} gothic horror campaign. \
                 Write atmospheric, dread-filled prose. Use second person for the party."
            ));

        let dm_prompt = format!(
            "CAMPAIGN: {campaign_name}\n\
             PARTY: Sister Isolde Carrow (fallen cleric), Dorian Ashgrove (investigator), \
             Emmett Grave (graverobber), Vera Nighthollow (witch)\n\
             VILLAGE: Ravenmoor\n\n\
             TASK: {intent}\n\n\
             === CONTEXT (use ONLY these facts, names, and dice results) ===\
             {tool_context}\n\n\
             Write immersive gothic horror prose for this scene. \
             Use EXACTLY the names above — Isolde, Dorian, Emmett, Vera — and the village RAVENMOOR. \
             3-5 paragraphs. DO NOT explain your reasoning. DO NOT call tools. Just write the scene."
        );

        let dm_prose = ollama_chat_once(&ollama_host, &model, &system_prompt, &dm_prompt)
            .await.unwrap_or_else(|e| format!("*(DM writer error: {e})*"));
        meta!("✍ DM Prose", &dm_prose);
        emit_step!("✍ DM Writer", &dm_prose);

        // ── Write prose to the output file (server controls the path) ────────
        let prose_bytes = dm_prose.len();
        let _ = tokio::fs::write(&prose_path, dm_prose.as_bytes()).await;
        emit_file!(
            format!("{campaign_name}/{session_dir}/{out_file}"),
            prose_bytes
        );

        // ── Append to session TURNS.md index (data + control plane quick ref) ─
        let index_path = session_path.join("TURNS.md");
        let meta_stem  = out_file.replace(".md", ".log");
        let dice_inline = if dice_lines.is_empty() {
            "—".to_string()
        } else {
            dice_lines.iter()
                .map(|l| l.split('→').next().unwrap_or("").trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
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

        let campaign_path = data_dir.join("campaigns").join(&campaign_name);
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

        // Collect turn files — skip TURNS.md, summaries, and anything that isn't a turn prose file
        let mut turn_files: Vec<std::path::PathBuf> = Vec::new();
        if let Ok(mut rd) = tokio::fs::read_dir(&session_path).await {
            while let Ok(Some(e)) = rd.next_entry().await {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("md") { continue; }
                let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                // Only process turn files — skip indices, summaries, interruptions
                if name.starts_with("turn") {
                    turn_files.push(p);
                }
            }
        }
        turn_files.sort();

        if turn_files.is_empty() {
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::Failed { message: "No turn files found in session dir".into() },
            });
            return;
        }

        // ── MAP: summarize each turn individually ────────────────────────────
        meta!("📚 Turn Files", turn_files.iter()
            .filter_map(|p| p.file_name().and_then(|s| s.to_str()).map(String::from))
            .collect::<Vec<_>>().join("\n"));

        let mut turn_summaries: Vec<(String, String)> = Vec::new(); // (filename, mini-summary)

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
                "You are condensing one scene from a gothic horror TTRPG session. \
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
            "You are the official chronicler of a gothic horror tabletop RPG campaign. \
             You have been given one-sentence scene summaries for each turn of a session. \
             Write a 5-paragraph session recap in vivid past-tense prose. \
             Do NOT use headers, bullets, or labels. Only flowing paragraphs. \
             Paragraph 1: the opening situation and atmosphere. \
             Paragraph 2: the first discovery or confrontation. \
             Paragraph 3: the central revelation or crisis. \
             Paragraph 4: escalation and climax. \
             Paragraph 5: how the session ended and the hook into the next session. \
             Use the characters' names. Do not invent events not in the summaries.",
            &format!("Campaign: {campaign_name}\nSession: {session_dir}\n\nScene summaries:\n{condensed}"),
            1600,
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

        let campaign_path  = data_dir.join("campaigns").join(&campaign_name);
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

        // Read the hook session's summary — fall back to last turn file if no summary
        let hook_text = {
            let summary_path = hook_path.join("session_summary.md");
            if let Ok(t) = tokio::fs::read_to_string(&summary_path).await {
                let _ = tx.send(crate::state::TaskEvent {
                    task_id,
                    kind: TaskEventKind::ToolCall {
                        tool: "campaign.read".into(),
                        input: format!("{hook_session}/session_summary.md"),
                        output: format!("{} chars", t.len()),
                    },
                });
                t
            } else {
                // Fall back: find last turn file
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
                    let _ = tx.send(crate::state::TaskEvent {
                        task_id,
                        kind: TaskEventKind::ToolCall {
                            tool: "campaign.read".into(),
                            input: format!("{hook_session}/{fname}"),
                            output: format!("{} chars (fallback)", t.len()),
                        },
                    });
                    t
                } else {
                    String::new()
                }
            }
        };

        if hook_text.is_empty() {
            let _ = tx.send(crate::state::TaskEvent {
                task_id,
                kind: TaskEventKind::Failed { message: "No hook material found in previous session".into() },
            });
            return;
        }

        meta!("📚 Hook Source", &hook_text.chars().take(600).collect::<String>());

        // Step 1: extract the hook and identify the dark twist angle
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Twist Finder".into(),
                body: "Identifying the darkest thread to pull…".into(),
            },
        });

        let twist = ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are a gothic horror DM identifying the darkest possible twist for the next session. \
             Read the session hook provided. Identify: \
             (1) The unresolved tension or question the party faces. \
             (2) One dark complication that makes everything worse — a betrayal, a revelation, a cost. \
             (3) The opening image: one sentence describing exactly what the players see in the first moment of the new session. \
             Reply in three short labelled lines: TENSION: / TWIST: / OPENING IMAGE:",
            &format!("Previous session hook:\n\n{hook_text}"),
            300,
        ).await.unwrap_or_else(|_| "TENSION: unknown  TWIST: unknown  OPENING IMAGE: darkness".into());

        meta!("🔍 Twist Finder", &twist);
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "Twist Finder".into(),
                body: twist.chars().take(180).collect::<String>(),
            },
        });

        // Step 2: write the opener prose
        let _ = tx.send(crate::state::TaskEvent {
            task_id,
            kind: TaskEventKind::PipelineStep {
                step: "DM Writer".into(),
                body: format!("Writing opening scene for {next_sess}…"),
            },
        });

        let prose = match ollama_chat_once_with_tokens(
            &def.ollama_host,
            &def.model,
            "You are the Dungeon Master writing the opening scene of a new session for a gothic horror TTRPG. \
             You have the previous session's hook and a dark twist to introduce. \
             Write 3-4 paragraphs of immersive past-tense prose that: \
             opens IN MEDIA RES — the party already in a moment of tension, not waking up or arriving; \
             lands the dark twist naturally within the scene; \
             ends with a clear dramatic question the party must answer. \
             No headers. No DM commentary. Pure narrative prose. Bleak, atmospheric, specific.",
            &format!("Campaign: {campaign_name}\nPrevious session: {hook_session}\n\nHook:\n{hook_text}\n\nTwist analysis:\n{twist}"),
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
