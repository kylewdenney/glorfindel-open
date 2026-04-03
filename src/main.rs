use clap::Parser;
use glorfindel_agent::{Agent, AgentRegistry, ModelManager, OllamaAgent};
use glorfindel_orchestrator::{Router, TaskManager};
use glorfindel_tools::{BashTool, FileReadTool, FileWriteTool, SearchTool, ToolExecutor};
use tracing::{error, info};

#[derive(Parser)]
#[command(name = "glorfindel")]
#[command(about = "Agentic AI framework with OMS-derived pub/sub architecture")]
struct Cli {
    /// Ollama host URL
    #[arg(long, env = "OLLAMA_HOST", default_value = "http://localhost:11434")]
    ollama_host: String,

    /// Comma-separated list of models to ensure are available
    #[arg(long, env = "GLORFINDEL_MODELS", default_value = "mistral")]
    models: String,

    /// DDS domain ID
    #[arg(long, env = "DDS_DOMAIN_ID", default_value_t = 0)]
    dds_domain_id: i32,

    /// ZMQ endpoint for tool calls
    #[arg(
        long,
        env = "ZMQ_TOOL_CALL_ENDPOINT",
        default_value = "tcp://127.0.0.1:5555"
    )]
    zmq_tool_call_endpoint: String,

    /// ZMQ endpoint for tool results
    #[arg(
        long,
        env = "ZMQ_TOOL_RESULT_ENDPOINT",
        default_value = "tcp://127.0.0.1:5556"
    )]
    zmq_tool_result_endpoint: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cli = Cli::parse();

    info!("Starting Glorfindel orchestrator");
    info!(ollama_host = %cli.ollama_host, models = %cli.models);

    // --- Model provisioning ---
    let model_manager = ModelManager::new(&cli.ollama_host);
    info!("Ensuring models are available...");
    if let Err(e) = model_manager.ensure_models(&cli.models).await {
        error!(error = %e, "Failed to provision models — Ollama may not be running yet");
        info!("Will retry model provisioning when Ollama becomes available");
    }

    // --- Tool executor ---
    let mut tool_executor = ToolExecutor::new();
    tool_executor.register(Box::new(FileReadTool));
    tool_executor.register(Box::new(FileWriteTool));
    tool_executor.register(Box::new(BashTool::default()));
    tool_executor.register(Box::new(SearchTool));
    info!(tools = ?tool_executor.available_tools(), "Tools registered");

    // --- Agent setup ---
    let primary_model = cli.models.split(',').next().unwrap_or("mistral").trim();
    let ollama_agent = OllamaAgent::new(
        "ollama-primary",
        primary_model,
        &cli.ollama_host,
        tool_executor,
        vec!["general".into(), "code".into()],
    );

    // --- Registry ---
    let registry = AgentRegistry::new();
    registry.register(ollama_agent.capability()).await;

    // --- Orchestrator ---
    let router = Router::new(registry);
    let _task_manager = TaskManager::new();

    info!("Glorfindel orchestrator ready");
    info!(
        agents = ?router.registry().list_agents().await.iter().map(|a| &a.name).collect::<Vec<_>>(),
        "Registered agents"
    );

    // --- Main loop: listen for tasks via DDS ---
    // In production, this would subscribe to the DDS control plane.
    // For now, we keep the process alive and ready.
    info!("Listening for tasks... (submit via examples/simple_task.rs)");

    // Keep the orchestrator running
    tokio::signal::ctrl_c().await?;
    info!("Shutting down Glorfindel orchestrator");

    Ok(())
}
