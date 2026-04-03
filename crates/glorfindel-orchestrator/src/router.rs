use glorfindel_agent::AgentRegistry;
use glorfindel_schemas::agent::CapabilityManifest;
use glorfindel_schemas::task::TaskRequest;
use tracing::{info, warn};

/// Routes incoming tasks to the most suitable agent based on capabilities.
///
/// The router consults the AgentRegistry to find agents that match the
/// task's requirements (domains, tools) and selects the best candidate.
pub struct Router {
    registry: AgentRegistry,
}

impl Router {
    pub fn new(registry: AgentRegistry) -> Self {
        Self { registry }
    }

    /// Find the best agent for a given task.
    ///
    /// Selection criteria:
    /// 1. Agent must support required domains (if specified)
    /// 2. Agent must have required tools (if specified)
    /// 3. Among candidates, prefer agents with fewer domains (more specialized)
    pub async fn route(&self, task: &TaskRequest) -> Option<CapabilityManifest> {
        let required_tools = &task.constraints.allowed_tools;

        // For now, domain matching is implicit from the task intent.
        // Future: extract domains from intent via NLP.
        let required_domains: Vec<String> = Vec::new();

        let mut candidates = self
            .registry
            .find_capable_agents(&required_domains, required_tools)
            .await;

        if candidates.is_empty() {
            warn!(task_id = %task.task_id, "No capable agents found for task");
            return None;
        }

        // Prefer more specialized agents (fewer domains = more specialized)
        candidates.sort_by_key(|c| c.domains.len());

        let selected = candidates.into_iter().next().unwrap();
        info!(
            task_id = %task.task_id,
            agent_id = %selected.agent_id,
            agent_name = %selected.name,
            "Routed task to agent"
        );

        Some(selected)
    }

    /// Get a reference to the underlying registry.
    pub fn registry(&self) -> &AgentRegistry {
        &self.registry
    }
}
