use async_trait::async_trait;
use glorfindel_schemas::{AgentResponse, CapabilityManifest, MessageEnvelope, TaskRequest, ToolCall, ToolResult};

use crate::error::TransportError;

/// Abstraction over the DDS control plane.
///
/// The control plane handles structured, reliable message delivery for:
/// - Task routing (TaskRequest → agent assignment)
/// - Agent discovery (CapabilityManifest publication)
/// - Task responses (AgentResponse delivery)
///
/// Implementations must provide DDS-style topic-based pub/sub semantics.
#[async_trait]
pub trait ControlPlane: Send + Sync {
    /// Publish a task request for the orchestrator to route.
    async fn publish_task(&self, task: MessageEnvelope<TaskRequest>) -> Result<(), TransportError>;

    /// Subscribe to incoming task requests. Returns a receiver stream.
    async fn subscribe_tasks(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<MessageEnvelope<TaskRequest>>, TransportError>;

    /// Publish an agent's capability manifest for discovery.
    async fn publish_capability(
        &self,
        manifest: MessageEnvelope<CapabilityManifest>,
    ) -> Result<(), TransportError>;

    /// Subscribe to capability announcements from agents.
    async fn subscribe_capabilities(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<MessageEnvelope<CapabilityManifest>>, TransportError>;

    /// Publish a task response.
    async fn publish_response(
        &self,
        response: MessageEnvelope<AgentResponse>,
    ) -> Result<(), TransportError>;

    /// Subscribe to task responses.
    async fn subscribe_responses(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<MessageEnvelope<AgentResponse>>, TransportError>;
}

/// Abstraction over the ZMQ data plane.
///
/// The data plane handles high-throughput, potentially large message delivery for:
/// - Tool call dispatch (agent → tool executor)
/// - Tool result return (tool executor → agent)
/// - Streaming output (token-by-token LLM responses, large file contents)
///
/// Implementations should optimize for throughput over reliability — the control
/// plane handles coordination and acknowledgment.
#[async_trait]
pub trait DataPlane: Send + Sync {
    /// Send a tool call to the executor.
    async fn send_tool_call(
        &self,
        call: MessageEnvelope<ToolCall>,
    ) -> Result<(), TransportError>;

    /// Receive incoming tool calls (for the executor).
    async fn receive_tool_calls(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<MessageEnvelope<ToolCall>>, TransportError>;

    /// Send a tool result back to the agent.
    async fn send_tool_result(
        &self,
        result: MessageEnvelope<ToolResult>,
    ) -> Result<(), TransportError>;

    /// Receive tool results (for the agent).
    async fn receive_tool_results(
        &self,
    ) -> Result<tokio::sync::mpsc::Receiver<MessageEnvelope<ToolResult>>, TransportError>;
}
