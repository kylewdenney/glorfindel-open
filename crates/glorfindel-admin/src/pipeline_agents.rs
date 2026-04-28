/// Specialist pipeline agents for the DM scene turn pipeline.
///
/// Each agent handles exactly one step of the critic → rules → dice → writer → impact → summary
/// chain. They are dispatched as DDS sub-tasks by the DM Manager (`run_scene_pipeline`)
/// and can be independently configured with different models and Ollama hosts.
///
/// Domain registry (configure via agent definitions in the UI):
///   `campaign-fact-check`  → FactChecker
///   `ttrpg-rules`          → RulesAssessor
///   `dm-narrative`         → DmWriter
///   `char-impact`          → CharImpact
///   `dm-summary`           → DmSummarizer
use glorfindel_schemas::agent::AgentResponse;
use glorfindel_schemas::types::Status;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ─── Shared Ollama helper ─────────────────────────────────────────────────────

pub(crate) async fn ollama_chat(
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

fn ok_response(task_id: Uuid, output: String) -> AgentResponse {
    AgentResponse {
        task_id,
        status: Status::Complete,
        result: serde_json::json!({ "output": output }),
        actions_taken: vec![],
        delegated_to: vec![],
    }
}

fn err_response(task_id: Uuid, error: String) -> AgentResponse {
    AgentResponse {
        task_id,
        status: Status::Failed,
        result: serde_json::json!({ "error": error }),
        actions_taken: vec![],
        delegated_to: vec![],
    }
}

// ─── Fact Checker ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct FactCheckParams {
    pub task_type: String, // "fact_check"
    pub ollama_host: String,
    pub model: String,
    pub character: String,
    pub action: String,
    pub grounding_block: String,
}

/// Reads grounding files and produces a structured character/event summary.
///
/// Domain: `campaign-fact-check`
/// Output (`result.output`): CHARACTERS / RECENT EVENTS / ACTING CHARACTER block.
pub async fn fact_check(params: FactCheckParams, task_id: Uuid) -> AgentResponse {
    let system = format!(
        "You are a campaign fact-checker. Read the files below. \
         CHARACTERS: list every named character with their current situation (one line each). \
         RECENT EVENTS: 2 sentences on what happened most recently. \
         ACTING CHARACTER: one paragraph on who {} is, their stats, their Devotion, \
         and where they stand right now. \
         PEDANTIC RULE: only use names and facts that appear verbatim in the files provided.",
        params.character
    );
    let user = format!(
        "Acting character: {}\nPlayer action: {}\n\n{}",
        params.character, params.action, params.grounding_block
    );
    match ollama_chat(&params.ollama_host, &params.model, &system, &user, 512).await {
        Ok(output) => ok_response(task_id, output),
        Err(e) => err_response(task_id, e.to_string()),
    }
}

// ─── Rules Assessor ───────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct RulesAssessParams {
    pub task_type: String, // "rules_assess"
    pub ollama_host: String,
    pub model: String,
    pub character: String,
    pub action: String,
    pub party_state: String,  // compact current state table
    pub rules_context: String, // check types + tiers from rules.toml
}

/// Determines whether a dice check is required and outputs a structured ROLL line.
///
/// Domain: `ttrpg-rules`
/// Output (`result.output`): exactly one `ROLL|char|check_type|tier|reason` line or `NO_ROLL`.
/// Rust resolves all DCs and modifiers — this agent only categorises the check.
pub async fn rules_assess(params: RulesAssessParams, task_id: Uuid) -> AgentResponse {
    let system = "\
You are a TTRPG rules assessor. Decide if a dice check is needed and output ONE line.

OUTPUT FORMAT — choose exactly one:
  ROLL|CharacterName|check_type|tier|brief reason (what failure means)
  NO_ROLL

CHOOSING check_type:
  cosmic_dread — action involves the Thing from Below: impossible geometry, void, eldritch entities, forbidden knowledge
  fear         — witnessing supernatural horror: undead, possession, unnatural death
  <skill_name> — any other action: use the exact skill name from the rules (e.g. Investigation, Arcana, Medicine)

CHOOSING tier:
  Read the tiers listed for the check_type. Pick the lowest tier that fits the severity.
  For cosmic_dread: evidence | fragment | attention | true_form | comprehension
  For fear:         unsettling | disturbing | terrifying | soul_shaking
  For skills:       easy | moderate | hard | extreme

REASON:
  One short phrase — what specifically fails if the roll fails.
  BAD:  what it determines
  GOOD: resists the cosmic impression
  GOOD: the rune pattern stays opaque

Only call for a roll if failure has a meaningful consequence. Trivial actions: NO_ROLL.
Output ONLY the single ROLL| line or NO_ROLL. No prose. No explanation.";

    let user = format!(
        "CHARACTER: {character}\nACTION: {action}\n\nCURRENT STATE:\n{state}\n\nRULES:\n{rules}\n\nOutput now:",
        character = params.character,
        action    = params.action,
        state     = params.party_state,
        rules     = params.rules_context,
    );
    match ollama_chat(&params.ollama_host, &params.model, system, &user, 80).await {
        Ok(output) => {
            let clean = output
                .lines()
                .find(|l| l.starts_with("ROLL|") || l.trim() == "NO_ROLL")
                .unwrap_or("NO_ROLL")
                .trim()
                .to_string();
            ok_response(task_id, clean)
        }
        Err(e) => err_response(task_id, e.to_string()),
    }
}

// ─── DM Writer ────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct DmWriteParams {
    pub task_type: String, // "dm_write"
    pub ollama_host: String,
    pub model: String,
    pub campaign_name: String,
    pub session_dir: String,
    pub scene_dir: String,
    pub character: String,
    pub action: String,
    pub campaign_facts: String,
    pub critic_context: String,
    pub dice_context: String,
    pub system_prompt: String,
}

/// Produces immersive DM prose in response to the player action and dice outcomes.
///
/// Domain: `dm-narrative`
/// Output (`result.output`): 3-5 paragraphs of narrative prose.
pub async fn dm_write(params: DmWriteParams, task_id: Uuid) -> AgentResponse {
    let user = format!(
        "CAMPAIGN:\n{campaign_facts}\n\n\
         SCENE: {session_dir}/{scene_dir}\n\n\
         GROUNDED FACTS (use ONLY these names and events):\n{critic_context}\n\n\
         DICE OUTCOMES (already happened — honour them exactly):\n{dice_context}\n\n\
         PLAYER ACTION: {character} — {action}\n\n\
         CRITICAL RULES:\n\
         - The scene picks up from the grounded facts. Do NOT re-describe events that already happened.\n\
         - Narrate only what happens next. What does the character experience? What does the world do back?\n\
         - SUCCESS means the action worked. FAILURE means it did not — narrate the consequence, not the success.\n\
         - NEVER write dice notation (1d20, +N, DC, roll numbers). Translate outcomes into pure sensation and event.\n\
         - Write in second or third person as the scene demands. 3-5 paragraphs.\n\
         OUTPUT ONLY NARRATIVE PROSE. No JSON. No tool calls. No structured data. Just the scene.",
        campaign_facts = params.campaign_facts,
        session_dir = params.session_dir,
        scene_dir = params.scene_dir,
        critic_context = params.critic_context,
        dice_context = params.dice_context,
        character = params.character,
        action = params.action,
    );
    match ollama_chat(&params.ollama_host, &params.model, &params.system_prompt, &user, 1200).await {
        Ok(output) => ok_response(task_id, output),
        Err(e) => err_response(task_id, format!("*(DM response error: {e})*")),
    }
}

// ─── Character Impact ─────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct CharImpactParams {
    pub task_type: String, // "char_impact"
    pub ollama_host: String,
    pub model: String,
    pub character: String,
    pub roll_outcome: String, // "SUCCESS", "FAILURE", or "NO_ROLL" — unambiguous
    pub dice_context: String, // outcome + consequence (no notation)
    pub dm_prose: String,
    pub party_state: String,  // compact current state table
    pub cosmic_rules: String,
}

/// Reads the dice outcome and narrative and outputs structured stat changes.
///
/// Domain: `char-impact`
/// Output (`result.output`): one `FIELD|Name|Value` line per changed stat, or `NO_CHANGE`.
pub async fn char_impact(params: CharImpactParams, task_id: Uuid) -> AgentResponse {
    let system = "You are a TTRPG condition tracker. CDP and Dread are handled by the rules engine. \
Your only job: did the narrative result in a named lasting condition on this character?

OUTPUT FORMAT:
  CONDITION|Full Character Name|ConditionName
  NO_CHANGE

STRICT RULES:
1. Only output CONDITION if the narrative explicitly describes a lasting named status effect.
2. Valid conditions: Frightened, Shaken, Counted, Paralysed, Blinded, Cursed, Stunned. 1-2 words max.
3. Do NOT output CDP or DREAD lines — the rules engine handles those.
4. Do NOT infer conditions from dramatic tone. Only set what the narrative explicitly states.
5. ROLL_OUTCOME=SUCCESS means the check passed — do not apply failure conditions.
6. Use the character's EXACT full name.
7. If no condition changed: output exactly NO_CHANGE.
8. NO prose. NO explanation. ONLY the structured line or NO_CHANGE.";

    let user = format!(
        "CHARACTER: {character}\nROLL_OUTCOME: {roll_outcome}\n\n\
         DICE OUTCOME:\n{dice_context}\n\n\
         NARRATIVE:\n{dm_prose}\n\n\
         CURRENT PARTY STATS:\n{party_text}\n\n\
         COSMIC RULES:\n{cosmic_rules}\n\n\
         Output stat change lines now:",
        character    = params.character,
        roll_outcome = params.roll_outcome,
        dice_context = params.dice_context,
        dm_prose     = params.dm_prose,
        party_text   = params.party_state,
        cosmic_rules = params.cosmic_rules,
    );
    match ollama_chat(&params.ollama_host, &params.model, system, &user, 80).await {
        Ok(output) => {
            let clean: Vec<&str> = output
                .lines()
                .filter(|l| {
                    l.starts_with("CDP|")
                        || l.starts_with("DREAD|")
                        || l.starts_with("CONDITION|")
                        || l.trim() == "NO_CHANGE"
                })
                .collect();
            let clean = if clean.is_empty() {
                "NO_CHANGE".to_string()
            } else {
                clean.join("\n")
            };
            ok_response(task_id, clean)
        }
        Err(e) => err_response(task_id, e.to_string()),
    }
}

// ─── Summarizer ───────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct DmSummarizeParams {
    pub task_type: String, // "dm_summarize"
    pub ollama_host: String,
    pub model: String,
    pub character: String,
    pub prose: String,
}

/// Condenses a scene turn into a single archivable sentence.
///
/// Domain: `dm-summary`
/// Output (`result.output`): one sentence, past tense, full character name.
pub async fn dm_summarize(params: DmSummarizeParams, task_id: Uuid) -> AgentResponse {
    let system = "Write one sentence: who did what, and what was the outcome. \
                  Past tense. Use the character's full name exactly as given. \
                  If a roll shaped the scene, say so briefly. \
                  Do NOT start with 'In turn', 'In this scene', or any meta-framing. \
                  One sentence only.";
    let user = format!(
        "Character full name: {}\n\nScene:\n{}",
        params.character,
        params.prose.chars().take(2000).collect::<String>()
    );
    match ollama_chat(&params.ollama_host, &params.model, system, &user, 128).await {
        Ok(output) => ok_response(task_id, output),
        Err(e) => err_response(task_id, e.to_string()),
    }
}
