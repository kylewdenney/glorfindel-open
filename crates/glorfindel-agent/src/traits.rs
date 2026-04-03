use async_trait::async_trait;
use glorfindel_schemas::agent::{AgentResponse, CapabilityManifest};
use glorfindel_schemas::task::TaskRequest;

use crate::error::AgentError;

/// An agent that can receive tasks and produce responses.
///
/// Agents implement the core agentic loop:
/// 1. Receive a TaskRequest
/// 2. Analyze the intent
/// 3. Optionally call tools (via the data plane)
/// 4. Observe tool results
/// 5. Repeat steps 3-4 or produce a final AgentResponse
///
/// Each agent declares its capabilities via a CapabilityManifest,
/// which the orchestrator uses for task routing.
#[async_trait]
pub trait Agent: Send + Sync {
    /// Return this agent's capability manifest for DDS registration.
    fn capability(&self) -> CapabilityManifest;

    /// Handle an incoming task and produce a response.
    async fn handle_task(&self, task: TaskRequest) -> Result<AgentResponse, AgentError>;
}
