use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use glorfindel_schemas::agent::CapabilityManifest;
use tracing::{info, warn};

/// Maintains a live roster of registered agents and their capabilities.
///
/// The registry listens for CapabilityManifest publications on the DDS
/// control plane and keeps an up-to-date view of what agents are available
/// and what they can do.
pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, CapabilityManifest>>>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register or update an agent's capabilities.
    pub async fn register(&self, manifest: CapabilityManifest) {
        info!(
            agent_id = %manifest.agent_id,
            name = %manifest.name,
            agent_type = ?manifest.agent_type,
            tools = ?manifest.tools_available,
            "Agent registered"
        );
        self.agents
            .write()
            .await
            .insert(manifest.agent_id.clone(), manifest);
    }

    /// Remove an agent from the registry.
    pub async fn deregister(&self, agent_id: &str) {
        if self.agents.write().await.remove(agent_id).is_some() {
            info!(agent_id, "Agent deregistered");
        } else {
            warn!(agent_id, "Attempted to deregister unknown agent");
        }
    }

    /// Find agents capable of handling a task based on domain and tool requirements.
    pub async fn find_capable_agents(
        &self,
        required_domains: &[String],
        required_tools: &[String],
    ) -> Vec<CapabilityManifest> {
        let agents = self.agents.read().await;
        agents
            .values()
            .filter(|manifest| {
                // If domains are specified, agent must cover at least one
                let domain_match = required_domains.is_empty()
                    || required_domains
                        .iter()
                        .any(|d| manifest.domains.contains(d));

                // If tools are specified, agent must have all of them
                let tool_match = required_tools.is_empty()
                    || required_tools
                        .iter()
                        .all(|t| manifest.tools_available.contains(t));

                domain_match && tool_match
            })
            .cloned()
            .collect()
    }

    /// Get all registered agents.
    pub async fn list_agents(&self) -> Vec<CapabilityManifest> {
        self.agents.read().await.values().cloned().collect()
    }

    /// Get a specific agent's manifest.
    pub async fn get_agent(&self, agent_id: &str) -> Option<CapabilityManifest> {
        self.agents.read().await.get(agent_id).cloned()
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
