use std::collections::HashMap;
use std::sync::Arc;

use glorfindel_agent::OllamaAgent;
use glorfindel_schemas::types::Permission;
use glorfindel_tools::{
    BashTool, CampaignListTool, CampaignReadTool, CampaignWriteTool, DiceRollTool, FileReadTool,
    FileWriteTool, JellyfinClient, MediaItemTool, MediaLibraryTool, MediaRecentTool,
    MediaRefreshTool, MediaScanTool, MediaSearchTool, MediaSessionsTool, MediaUsersTool,
    RulebookTool, SearchTool, Tool, ToolExecutor,
};
use serde::Serialize;
use uuid::Uuid;

use crate::agent_tool::AgentTool;
use crate::state::AgentDefinition;

#[derive(Debug, Clone, Serialize)]
pub struct ToolCatalogEntry {
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub requires_campaign_dir: bool,
}

pub fn catalog() -> Vec<ToolCatalogEntry> {
    let standard: Vec<Box<dyn Tool>> = vec![
        Box::new(FileReadTool),
        Box::new(FileWriteTool),
        Box::new(BashTool::default()),
        Box::new(SearchTool),
    ];
    let mut entries: Vec<ToolCatalogEntry> = standard
        .iter()
        .map(|t| ToolCatalogEntry {
            name: t.name().to_string(),
            description: t.description().to_string(),
            permissions: t.required_permissions(),
            requires_campaign_dir: false,
        })
        .collect();

    let campaign: Vec<Box<dyn Tool>> = vec![
        Box::new(CampaignReadTool::new(".")),
        Box::new(CampaignWriteTool::new(".")),
        Box::new(CampaignListTool::new(".")),
    ];
    for t in &campaign {
        entries.push(ToolCatalogEntry {
            name: t.name().to_string(),
            description: t.description().to_string(),
            permissions: t.required_permissions(),
            requires_campaign_dir: true,
        });
    }

    // Static entries for tools that need runtime config
    for name in &["media.search", "media.library", "media.recent", "media.sessions",
                  "media.scan", "media.refresh", "media.item", "media.users"] {
        entries.push(ToolCatalogEntry {
            name: name.to_string(),
            description: format!("{name} — requires jellyfin_url/api_key/user_id in deployed_context"),
            permissions: vec![Permission::Custom("media.read".into())],
            requires_campaign_dir: false,
        });
    }
    entries.push(ToolCatalogEntry {
        name: "rulebook.search".to_string(),
        description: "Semantic search over indexed rulebook files. Requires rulebook_dir in deployed_context.".to_string(),
        permissions: vec![Permission::Custom("rulebook.search".into())],
        requires_campaign_dir: false,
    });
    entries.push(ToolCatalogEntry {
        name: "dice.roll".to_string(),
        description: "Roll dice using standard notation (d20, 2d6+3). No permissions needed.".to_string(),
        permissions: vec![],
        requires_campaign_dir: false,
    });

    entries
}

/// Build a `ToolExecutor` from a list of tool names and optional configuration.
///
/// - `campaign_dir`: legacy shorthand for campaign.* tools
/// - `deployed_context`: general config (jellyfin creds, rulebook_dir, sub_agents map, …)
/// - `definitions`: full definition map, required to build sub-agents from deployed_context.sub_agents
pub async fn build_executor(
    tool_names: &[String],
    campaign_dir: Option<&str>,
    deployed_context: Option<&serde_json::Value>,
    definitions: Option<&HashMap<Uuid, AgentDefinition>>,
) -> ToolExecutor {
    let mut executor = ToolExecutor::new();

    // Helper: read a string from deployed_context
    let ctx_str = |key: &str| -> Option<String> {
        deployed_context
            .and_then(|c| c.get(key))
            .and_then(|v| v.as_str())
            .map(String::from)
    };

    // Resolved campaign dir: prefer deployed_context["campaign_dir"], fall back to campaign_dir arg
    let effective_campaign_dir: Option<String> = ctx_str("campaign_dir")
        .or_else(|| campaign_dir.map(String::from));

    // Build JellyfinClient once if any media tool is requested
    let jellyfin: Option<JellyfinClient> = {
        let url = ctx_str("jellyfin_url");
        let key = ctx_str("jellyfin_api_key");
        let uid = ctx_str("jellyfin_user_id");
        match (url, key, uid) {
            (Some(u), Some(k), Some(i)) => Some(JellyfinClient::new(u, k, i)),
            _ => None,
        }
    };

    for name in tool_names {
        match name.as_str() {
            "file.read"   => executor.register(Box::new(FileReadTool)),
            "file.write"  => executor.register(Box::new(FileWriteTool)),
            "bash.exec"   => executor.register(Box::new(BashTool::default())),
            "search.grep" => executor.register(Box::new(SearchTool)),

            "campaign.read" | "campaign.write" | "campaign.list" => {
                match &effective_campaign_dir {
                    Some(dir) => match name.as_str() {
                        "campaign.read"  => executor.register(Box::new(CampaignReadTool::new(dir))),
                        "campaign.write" => executor.register(Box::new(CampaignWriteTool::new(dir))),
                        "campaign.list"  => executor.register(Box::new(CampaignListTool::new(dir))),
                        _ => {}
                    },
                    None => tracing::warn!(tool = %name, "campaign.* requires campaign_dir — skipping"),
                }
            }

            "media.search"   => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaSearchTool::new(j.clone()))),
                None    => tracing::warn!("media.search requires jellyfin config in deployed_context — skipping"),
            },
            "media.library"  => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaLibraryTool::new(j.clone()))),
                None    => tracing::warn!("media.library requires jellyfin config — skipping"),
            },
            "media.recent"   => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaRecentTool::new(j.clone()))),
                None    => tracing::warn!("media.recent requires jellyfin config — skipping"),
            },
            "media.sessions" => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaSessionsTool::new(j.clone()))),
                None    => tracing::warn!("media.sessions requires jellyfin config — skipping"),
            },
            "media.scan"     => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaScanTool::new(j.clone()))),
                None    => tracing::warn!("media.scan requires jellyfin config — skipping"),
            },
            "media.refresh"  => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaRefreshTool::new(j.clone()))),
                None    => tracing::warn!("media.refresh requires jellyfin config — skipping"),
            },
            "media.item"     => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaItemTool::new(j.clone()))),
                None    => tracing::warn!("media.item requires jellyfin config — skipping"),
            },
            "media.users"    => match &jellyfin {
                Some(j) => executor.register(Box::new(MediaUsersTool::new(j.clone()))),
                None    => tracing::warn!("media.users requires jellyfin config — skipping"),
            },

            "rulebook.search" => {
                let rulebook_dir = ctx_str("rulebook_dir");
                let ollama_host  = ctx_str("ollama_host").unwrap_or_else(|| "http://localhost:11434".into());
                let embed_model  = ctx_str("embed_model").unwrap_or_else(|| "nomic-embed-text".into());
                match rulebook_dir {
                    Some(dir) => {
                        match RulebookTool::build(std::path::Path::new(&dir), &ollama_host, &embed_model).await {
                            Ok(tool) => {
                                tracing::info!(dir = %dir, "Rulebook index built");
                                executor.register(Box::new(tool));
                            }
                            Err(e) => tracing::warn!(error = %e, "Failed to build rulebook index — skipping"),
                        }
                    }
                    None => tracing::warn!("rulebook.search requires rulebook_dir in deployed_context — skipping"),
                }
            }

            "dice.roll" => executor.register(Box::new(DiceRollTool)),

            name if name.starts_with("agent.") => {
                let sub_name = &name["agent.".len()..];
                if let Some(sub_id) = ctx_str(&format!("sub_agents.{sub_name}"))
                    .or_else(|| {
                        // Also check nested: deployed_context.sub_agents is an object
                        deployed_context
                            .and_then(|c| c.get("sub_agents"))
                            .and_then(|s| s.get(sub_name))
                            .and_then(|v| v.as_str())
                            .map(String::from)
                    })
                {
                    let def_id: Uuid = match sub_id.parse() {
                        Ok(id) => id,
                        Err(_) => {
                            tracing::warn!(sub_agent = sub_name, "Invalid UUID in sub_agents");
                            continue;
                        }
                    };
                    let def = definitions
                        .and_then(|defs| defs.get(&def_id))
                        .cloned();

                    match def {
                        Some(def) => {
                            // Build sub-agent executor (recursive, but no further sub-agents)
                            let sub_executor = Box::pin(build_executor(
                                &def.tools,
                                def.campaign_dir.as_deref(),
                                def.deployed_context.as_ref(),
                                definitions,
                            )).await;

                            let sub_agent = Arc::new(
                                OllamaAgent::new(
                                    Uuid::new_v4().to_string(),
                                    &def.model,
                                    &def.ollama_host,
                                    sub_executor,
                                    def.domains.clone(),
                                )
                                .with_name(&def.name)
                                .with_system_prompt(
                                    def.system_prompt
                                        .clone()
                                        .unwrap_or_else(|| "You are a helpful AI agent.".into()),
                                ),
                            );

                            let desc = format!(
                                "Sub-agent '{}': {}. Parameter: 'query' (string).",
                                def.name, def.description
                            );
                            let perms = def.default_permissions.clone();
                            executor.register(Box::new(AgentTool::new(name, desc, sub_agent, perms)));
                            tracing::info!(sub_agent = sub_name, "Registered sub-agent tool");
                        }
                        None => tracing::warn!(sub_agent = sub_name, def_id = %def_id, "Sub-agent definition not found"),
                    }
                } else {
                    tracing::warn!(tool = name, "agent.* tool has no matching entry in deployed_context.sub_agents");
                }
            }

            _ => tracing::warn!(tool = %name, "Unknown tool in definition, skipping"),
        }
    }

    executor
}
