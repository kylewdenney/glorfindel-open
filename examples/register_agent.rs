//! Example: Register an Ollama agent and publish its capabilities.
//!
//! This demonstrates the agent registration flow:
//! 1. Create an agent with tools
//! 2. Generate its CapabilityManifest
//! 3. Publish it (locally here, via DDS in production)
//! 4. Wait for tasks

use glorfindel_agent::{Agent, AgentRegistry, ModelManager, OllamaAgent};
use glorfindel_tools::{BashTool, FileReadTool, FileWriteTool, SearchTool, ToolExecutor};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let ollama_host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("GLORFINDEL_MODEL").unwrap_or_else(|_| "mistral".into());

    // Ensure model is available
    let model_manager = ModelManager::new(&ollama_host);
    println!("Checking model availability...");
    model_manager.ensure_models(&model).await?;

    // Set up tools
    let mut executor = ToolExecutor::new();
    executor.register(Box::new(FileReadTool));
    executor.register(Box::new(FileWriteTool));
    executor.register(Box::new(BashTool::default()));
    executor.register(Box::new(SearchTool));

    // Create agent
    let agent = OllamaAgent::new(
        "my-agent",
        &model,
        &ollama_host,
        executor,
        vec!["general".into(), "code".into(), "devops".into()],
    );

    // Get capability manifest
    let manifest = agent.capability();
    println!("\nAgent Capability Manifest:");
    println!("{}", serde_json::to_string_pretty(&manifest)?);

    // Register with the registry
    let registry = AgentRegistry::new();
    registry.register(manifest).await;

    println!("\nAgent registered. In production, this manifest would be");
    println!("published via DDS for the orchestrator to discover.");
    println!("\nRegistered agents:");
    for agent in registry.list_agents().await {
        println!("  - {} ({:?})", agent.name, agent.agent_type);
    }

    Ok(())
}
