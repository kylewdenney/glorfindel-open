//! Example: Submit a simple task and observe the agentic loop.
//!
//! This demonstrates the end-to-end flow:
//! TaskRequest → Orchestrator routes → Agent handles → ToolCalls → AgentResponse

use glorfindel_agent::{Agent, OllamaAgent};
use glorfindel_schemas::task::{ContextEntry, ContextRole, TaskRequest};
use glorfindel_schemas::types::Permission;
use glorfindel_tools::{BashTool, FileReadTool, SearchTool, ToolExecutor};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    let ollama_host =
        std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://localhost:11434".into());
    let model = std::env::var("GLORFINDEL_MODEL").unwrap_or_else(|_| "mistral".into());

    // Set up tools
    let mut executor = ToolExecutor::new();
    executor.register(Box::new(FileReadTool));
    executor.register(Box::new(BashTool::default()));
    executor.register(Box::new(SearchTool));

    // Create an agent
    let agent = OllamaAgent::new(
        "example-agent",
        &model,
        &ollama_host,
        executor,
        vec!["general".into()],
    );

    // Build a task
    let mut task = TaskRequest::new("List all files in the current directory and tell me what you see.");
    task.constraints.granted_permissions = vec![
        Permission::FileRead,
        Permission::BashExec,
    ];
    task.context.push(ContextEntry {
        role: ContextRole::System,
        content: "You are running inside a Glorfindel agent container.".into(),
    });

    println!("Submitting task: {}", task.intent);
    println!("Task ID: {}", task.task_id);
    println!("---");

    // Run the agent
    let response = agent.handle_task(task).await?;

    println!("Status: {:?}", response.status);
    println!("Actions taken: {}", response.actions_taken.len());
    for (i, action) in response.actions_taken.iter().enumerate() {
        println!(
            "  Action {}: {} -> {:?}",
            i + 1,
            action.tool_call.tool_name,
            action.tool_result.status
        );
    }
    println!("Result: {}", serde_json::to_string_pretty(&response.result)?);

    Ok(())
}
