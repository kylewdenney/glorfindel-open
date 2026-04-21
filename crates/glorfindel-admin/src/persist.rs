use std::collections::HashMap;
use std::path::Path;

use uuid::Uuid;

use crate::state::AgentDefinition;

pub async fn load_definitions(data_dir: &Path) -> HashMap<Uuid, AgentDefinition> {
    let dir = data_dir.join("definitions");
    let mut map = HashMap::new();

    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(e) => e,
        Err(_) => return map,
    };

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().map_or(false, |e| e == "json") {
            match tokio::fs::read_to_string(&path).await {
                Ok(data) => match serde_json::from_str::<AgentDefinition>(&data) {
                    Ok(def) => {
                        map.insert(def.id, def);
                    }
                    Err(e) => tracing::warn!(path = ?path, error = %e, "Failed to parse definition"),
                },
                Err(e) => tracing::warn!(path = ?path, error = %e, "Failed to read definition"),
            }
        }
    }

    tracing::info!(count = map.len(), "Loaded definitions from disk");
    map
}

pub async fn save_definition(data_dir: &Path, def: &AgentDefinition) -> anyhow::Result<()> {
    let dir = data_dir.join("definitions");
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(format!("{}.json", def.id));
    tokio::fs::write(path, serde_json::to_string_pretty(def)?).await?;
    Ok(())
}

pub async fn delete_definition(data_dir: &Path, id: Uuid) -> anyhow::Result<()> {
    let path = data_dir.join("definitions").join(format!("{id}.json"));
    let _ = tokio::fs::remove_file(path).await;
    Ok(())
}
