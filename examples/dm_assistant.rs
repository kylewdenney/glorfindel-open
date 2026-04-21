//! DM Assistant — interactive REPL for helping a Dungeon Master run a campaign.
//!
//! Features:
//! - Persistent campaign notes in a local directory (world, players, NPCs, locations, sessions)
//! - RAG search over your rulebook files (.txt / .md) with inline citations
//! - Powered by a local Ollama model — nothing leaves your machine
//!
//! Usage:
//!   cargo run --example dm_assistant -- \
//!     --campaign-dir ~/campaigns/my-campaign \
//!     --rulebooks-dir ~/rulebooks \
//!     --model mistral \
//!     --embed-model nomic-embed-text
//!
//! On first run the campaign directory is populated with starter files.
//! Type 'quit' or 'exit' to stop.

use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use glorfindel_agent::{Agent, DmAssistantAgent};
use glorfindel_schemas::task::TaskRequest;
use glorfindel_schemas::types::Permission;
use glorfindel_tools::{CampaignListTool, CampaignReadTool, CampaignWriteTool, RulebookTool, ToolExecutor};

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "dm_assistant",
    about = "An AI co-DM that knows your campaign and cites the rulebooks"
)]
struct Args {
    /// Directory where campaign notes are stored (created if it doesn't exist)
    #[arg(long, env = "DM_CAMPAIGN_DIR")]
    campaign_dir: PathBuf,

    /// Directory containing rulebook .txt / .md files for RAG search (optional)
    #[arg(long, env = "DM_RULEBOOKS_DIR")]
    rulebooks_dir: Option<PathBuf>,

    /// Ollama model to use for the DM assistant
    #[arg(long, env = "GLORFINDEL_MODEL", default_value = "mistral")]
    model: String,

    /// Ollama embedding model for rulebook indexing
    #[arg(long, env = "DM_EMBED_MODEL", default_value = "nomic-embed-text")]
    embed_model: String,

    /// Ollama API host
    #[arg(long, env = "OLLAMA_HOST", default_value = "http://localhost:11434")]
    ollama_host: String,
}

// ---------------------------------------------------------------------------
// Starter campaign files
// ---------------------------------------------------------------------------

const STARTER_FILES: &[(&str, &str)] = &[
    (
        "world.md",
        "# World Overview\n\nSetting: [Describe your world here]\n\nTone: [e.g. High fantasy, grim dark, swashbuckling]\n\nCurrent date in-world: [Day/Month/Year]\n",
    ),
    (
        "players.md",
        "# Player Characters\n\n## [Character Name]\n- Player: [Player name]\n- Class/Level: [Class] [Level]\n- Race: [Race]\n- Key traits: [Personality, goals, backstory hooks]\n",
    ),
    (
        "npcs.md",
        "# Notable NPCs\n\n## [NPC Name]\n- Role: [e.g. Quest giver, villain, merchant]\n- Location: [Where they can be found]\n- Attitude toward party: [Friendly / Neutral / Hostile]\n- Notes: [Secrets, motivations, plot connections]\n",
    ),
    (
        "locations.md",
        "# Key Locations\n\n## [Location Name]\n- Type: [City / Dungeon / Wilderness / etc.]\n- Notable features: [What makes it interesting]\n- Connected NPCs: [Who lives or operates here]\n- Status: [Explored / Rumored / Active threat]\n",
    ),
    (
        "session_notes.md",
        "# Session Notes\n\n## Session 1 — [Date]\n[What happened, decisions made, loose threads]\n",
    ),
    (
        "rulings.md",
        "# House Rules & Rulings\n\n[Document any rulings you've made at the table so the assistant remembers them]\n",
    ),
];

async fn ensure_campaign_dir(dir: &PathBuf) -> Result<bool> {
    let is_new = !dir.exists();
    tokio::fs::create_dir_all(dir).await?;

    for (filename, content) in STARTER_FILES {
        let path = dir.join(filename);
        if !path.exists() {
            tokio::fs::write(&path, content).await?;
        }
    }

    Ok(is_new)
}

// ---------------------------------------------------------------------------
// Response formatting
// ---------------------------------------------------------------------------

fn print_response(result: &serde_json::Value, actions_taken: usize) {
    let text = result
        .as_str()
        .map(str::to_string)
        .or_else(|| result.get("result").and_then(|v| v.as_str()).map(str::to_string))
        .unwrap_or_else(|| serde_json::to_string_pretty(result).unwrap_or_default());

    println!("\n{text}");

    if actions_taken > 0 {
        println!("\n  [{actions_taken} tool call(s) made]");
    }
    println!();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,dm_assistant=info".into()),
        )
        .init();

    let args = Args::parse();

    // Ensure campaign directory exists with starter files
    let is_new = ensure_campaign_dir(&args.campaign_dir).await?;
    if is_new {
        println!(
            "Created new campaign directory at: {}",
            args.campaign_dir.display()
        );
        println!("Starter files written — fill them in to give your assistant context.\n");
    } else {
        println!(
            "Loaded campaign: {}",
            args.campaign_dir.display()
        );
    }

    // Build the tool executor
    let mut executor = ToolExecutor::new();
    executor.register(Box::new(CampaignReadTool::new(&args.campaign_dir)));
    executor.register(Box::new(CampaignWriteTool::new(&args.campaign_dir)));
    executor.register(Box::new(CampaignListTool::new(&args.campaign_dir)));

    // Optionally index rulebooks
    if let Some(rulebooks_dir) = &args.rulebooks_dir {
        println!("Indexing rulebooks in {}...", rulebooks_dir.display());
        println!("(This embeds each chunk via Ollama — may take a moment on first run)\n");

        match RulebookTool::build(rulebooks_dir, &args.ollama_host, &args.embed_model).await {
            Ok(tool) => {
                executor.register(Box::new(tool));
                println!("Rulebook index ready.\n");
            }
            Err(e) => {
                eprintln!("Warning: could not build rulebook index: {e}");
                eprintln!("Continuing without rulebook search.\n");
            }
        }
    } else {
        println!("No --rulebooks-dir specified. Rule citation disabled.\n");
        println!("Tip: point --rulebooks-dir at a folder of .txt/.md rulebook files to enable RAG search.\n");
    }

    // Build the DM assistant agent
    let agent = DmAssistantAgent::new(
        "dm-assistant",
        &args.model,
        &args.ollama_host,
        executor,
        &args.campaign_dir,
    );

    // Permissions granted to every task
    let permissions = vec![
        Permission::Custom("campaign.read".into()),
        Permission::Custom("campaign.write".into()),
        Permission::Custom("rulebook.search".into()),
    ];

    println!("===========================================");
    println!("  Glorfindel DM Assistant");
    println!("  Model: {} | Campaign: {}", args.model, args.campaign_dir.display());
    println!("  Type 'quit' or 'exit' to stop.");
    println!("===========================================\n");

    // REPL loop
    loop {
        print!("DM> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            // EOF
            break;
        }

        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        if matches!(input.to_lowercase().as_str(), "quit" | "exit" | "q") {
            println!("May your dice roll true. Farewell!");
            break;
        }

        let mut task = TaskRequest::new(&input);
        task.constraints.granted_permissions = permissions.clone();
        task.constraints.max_iterations = Some(15);

        print!("Thinking...");
        io::stdout().flush()?;

        match agent.handle_task(task).await {
            Ok(response) => {
                print!("\r             \r"); // clear "Thinking..."
                print_response(&response.result, response.actions_taken.len());
            }
            Err(e) => {
                print!("\r             \r");
                eprintln!("Error: {e}");
            }
        }
    }

    Ok(())
}
