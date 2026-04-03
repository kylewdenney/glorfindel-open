use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tool::{ToolCall, ToolResult};
use crate::types::{AgentType, ModelBackend, Status};

/// An agent's response to a task, published on the DDS control plane.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    /// The task this response is for.
    pub task_id: Uuid,
    /// Final status of the task.
    pub status: Status,
    /// The result produced by the agent.
    pub result: serde_json::Value,
    /// Log of tool calls and their results during execution.
    pub actions_taken: Vec<ActionRecord>,
    /// Sub-tasks delegated to other agents.
    pub delegated_to: Vec<Uuid>,
}

/// A record of one tool call and its result within an agent's execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    pub tool_call: ToolCall,
    pub tool_result: ToolResult,
}

/// Declares an agent's identity and capabilities, published on DDS for discovery.
///
/// When an agent starts, it publishes its CapabilityManifest so the orchestrator
/// knows what tasks it can handle. This is the OMS-style service registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Unique identifier for this agent instance.
    pub agent_id: String,
    /// Human-readable name.
    pub name: String,
    /// What role this agent plays.
    pub agent_type: AgentType,
    /// Tools this agent knows how to use.
    pub tools_available: Vec<String>,
    /// What model backs this agent's inference.
    pub model_backend: ModelBackend,
    /// Domains this agent specializes in.
    pub domains: Vec<String>,
    /// Resource requirements for scheduling.
    pub resource_requirements: ResourceRequirements,
}

/// Hardware/resource requirements for running an agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ResourceRequirements {
    /// Whether a GPU is required.
    pub gpu_required: bool,
    /// Minimum memory in MB.
    pub min_memory_mb: Option<u64>,
    /// Minimum VRAM in MB.
    pub min_vram_mb: Option<u64>,
}
