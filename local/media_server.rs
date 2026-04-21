//! Media Server Assistant — interactive REPL for querying a Jellyfin instance.
//!
//! This file lives in local/ and is gitignored. Put your Jellyfin URL, API key,
//! and user ID here (or pass them via environment variables / CLI flags).
//!
//! Usage:
//!   cargo run --example media_server -- \
//!     --jellyfin-url http://jellyfin.lan:8096 \
//!     --api-key YOUR_API_KEY \
//!     --user-id YOUR_USER_ID \
//!     --model mistral
//!
//! Get your API key:  Jellyfin Dashboard → API Keys → +
//! Get your user ID: Jellyfin Dashboard → Users → click your user → copy the ID from the URL
//!
//! Type 'quit' or 'exit' to stop.

use std::io::{self, Write};

use anyhow::Result;
use clap::Parser;
use glorfindel_agent::{Agent, MediaServerAgent};
use glorfindel_schemas::task::TaskRequest;
use glorfindel_schemas::types::Permission;
use glorfindel_tools::{
    JellyfinClient, MediaLibraryTool, MediaRecentTool, MediaSearchTool, MediaSessionsTool,
    ToolExecutor,
};

// ---------------------------------------------------------------------------
// CLI args
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    name = "media_server",
    about = "Ask your Jellyfin library questions in plain English"
)]
struct Args {
    /// Jellyfin base URL (e.g. http://jellyfin.lan:8096)
    #[arg(long, env = "JELLYFIN_URL")]
    jellyfin_url: String,

    /// Jellyfin API key
    #[arg(long, env = "JELLYFIN_API_KEY")]
    api_key: String,

    /// Jellyfin user ID (from the admin dashboard URL)
    #[arg(long, env = "JELLYFIN_USER_ID")]
    user_id: String,

    /// Friendly name shown in the system prompt
    #[arg(long, env = "JELLYFIN_SERVER_NAME", default_value = "My Media Server")]
    server_name: String,

    /// Ollama model to use
    #[arg(long, env = "GLORFINDEL_MODEL", default_value = "mistral")]
    model: String,

    /// Ollama API host
    #[arg(long, env = "OLLAMA_HOST", default_value = "http://localhost:11434")]
    ollama_host: String,
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
            std::env::var("RUST_LOG").unwrap_or_else(|_| "warn,media_server=info".into()),
        )
        .init();

    let args = Args::parse();

    let jellyfin = JellyfinClient::new(&args.jellyfin_url, &args.api_key, &args.user_id);

    let mut executor = ToolExecutor::new();
    executor.register(Box::new(MediaSearchTool::new(jellyfin.clone())));
    executor.register(Box::new(MediaLibraryTool::new(jellyfin.clone())));
    executor.register(Box::new(MediaRecentTool::new(jellyfin.clone())));
    executor.register(Box::new(MediaSessionsTool::new(jellyfin.clone())));

    let agent = MediaServerAgent::new(
        "media-server",
        &args.model,
        &args.ollama_host,
        executor,
        &args.server_name,
    );

    let permissions = vec![Permission::Custom("media.read".into())];

    println!("===========================================");
    println!("  Glorfindel Media Server Assistant");
    println!("  Model: {} | Server: {}", args.model, args.server_name);
    println!("  Type 'quit' or 'exit' to stop.");
    println!("===========================================\n");

    loop {
        print!("Media> ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }

        let input = input.trim().to_string();
        if input.is_empty() {
            continue;
        }
        if matches!(input.to_lowercase().as_str(), "quit" | "exit" | "q") {
            println!("Goodbye!");
            break;
        }

        let mut task = TaskRequest::new(&input);
        task.constraints.granted_permissions = permissions.clone();
        task.constraints.max_iterations = Some(10);

        print!("Thinking...");
        io::stdout().flush()?;

        match agent.handle_task(task).await {
            Ok(response) => {
                print!("\r             \r");
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
