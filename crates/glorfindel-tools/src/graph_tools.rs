use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use chrono::Utc;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::{Permission, SideEffect};
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Parse `---\nkey: value\n---\nrest` into (metadata, body).
/// Handles the flat YAML front-matter written by graph-stack's persistence layer.
fn parse_frontmatter(text: &str) -> (HashMap<String, String>, String) {
    let mut meta = HashMap::new();
    if !text.starts_with("---\n") {
        return (meta, text.to_string());
    }
    let rest = &text[4..];
    let end = match rest.find("\n---\n") {
        Some(i) => i,
        None => return (meta, text.to_string()),
    };
    let front = &rest[..end];
    let body = rest[end + 5..].trim_start_matches('\n').to_string();
    for line in front.lines() {
        if let Some((k, v)) = line.split_once(": ") {
            meta.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    (meta, body)
}

/// Serialize metadata + body back into the graph-stack markdown format.
fn format_frontmatter(meta: &HashMap<String, String>, body: &str) -> String {
    // Write keys in a stable order matching graph-stack's format.
    const KEY_ORDER: &[&str] = &["id", "type", "name", "created", "from", "to", "relationship"];
    let mut yaml = String::from("---\n");
    for key in KEY_ORDER {
        if let Some(val) = meta.get(*key) {
            yaml.push_str(&format!("{key}: {val}\n"));
        }
    }
    for (k, v) in meta {
        if !KEY_ORDER.contains(&k.as_str()) {
            yaml.push_str(&format!("{k}: {v}\n"));
        }
    }
    yaml.push_str("---\n");
    if !body.trim().is_empty() {
        yaml.push('\n');
        yaml.push_str(body.trim());
        yaml.push('\n');
    }
    yaml
}

fn keyword_score(query: &str, text: &str) -> f64 {
    let text_lower = text.to_lowercase();
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return 0.0;
    }
    let hits = terms
        .iter()
        .filter(|t| text_lower.contains(&t.to_lowercase()))
        .count();
    hits as f64 / terms.len() as f64
}

async fn list_md_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(r) => r,
        Err(_) => return files,
    };
    while let Ok(Some(entry)) = rd.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files.sort();
    files
}

// ── graph.query ───────────────────────────────────────────────────────────────

pub struct GraphQueryTool {
    data_dir: PathBuf,
}

impl GraphQueryTool {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into() }
    }
}

#[async_trait]
impl Tool for GraphQueryTool {
    fn name(&self) -> &str { "graph.query" }

    fn description(&self) -> &str {
        "Search graph nodes by keyword. \
         Parameters: query (string), top_k (int, default 5)."
    }

    fn required_permissions(&self) -> Vec<Permission> { vec![] }

    async fn execute(&self, task_id: Uuid, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let top_k = params.get("top_k").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let nodes_dir = self.data_dir.join("nodes");
        let files = list_md_files(&nodes_dir).await;

        let mut scored: Vec<(String, f64, serde_json::Value)> = Vec::new();
        for path in files {
            let text = match tokio::fs::read_to_string(&path).await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let (meta, body) = parse_frontmatter(&text);
            let id = meta.get("id").cloned().unwrap_or_default();
            let all_text = format!(
                "{} {}",
                meta.values().cloned().collect::<Vec<_>>().join(" "),
                body
            );
            let score = keyword_score(query, &all_text);
            if score > 0.0 {
                let mut node = serde_json::Map::new();
                for (k, v) in &meta {
                    node.insert(k.clone(), serde_json::Value::String(v.clone()));
                }
                node.insert("body".to_string(), serde_json::Value::String(body));
                scored.push((id, score, serde_json::Value::Object(node)));
            }
        }
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        let results: Vec<serde_json::Value> = scored
            .into_iter()
            .map(|(id, score, node)| serde_json::json!({ "id": id, "score": score, "node": node }))
            .collect();
        let count = results.len();
        Ok(ToolResult::success(task_id, "graph.query", serde_json::json!({ "results": results, "count": count })))
    }
}

// ── graph.node ────────────────────────────────────────────────────────────────

pub struct GraphNodeTool {
    data_dir: PathBuf,
}

impl GraphNodeTool {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into() }
    }
}

#[async_trait]
impl Tool for GraphNodeTool {
    fn name(&self) -> &str { "graph.node" }

    fn description(&self) -> &str {
        "Get a graph node by ID. Parameter: node_id (string)."
    }

    fn required_permissions(&self) -> Vec<Permission> { vec![] }

    async fn execute(&self, task_id: Uuid, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        let node_id = params
            .get("node_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("node_id".into()))?;

        let path = self.data_dir.join("nodes").join(format!("{node_id}.md"));
        match tokio::fs::read_to_string(&path).await {
            Ok(text) => {
                let (meta, body) = parse_frontmatter(&text);
                let mut node = serde_json::Map::new();
                for (k, v) in meta {
                    node.insert(k, serde_json::Value::String(v));
                }
                node.insert("body".to_string(), serde_json::Value::String(body));
                Ok(ToolResult::success(task_id, "graph.node", serde_json::Value::Object(node)))
            }
            Err(_) => Ok(ToolResult::failure(
                task_id,
                "graph.node",
                format!("node not found: {node_id}"),
            )),
        }
    }
}

// ── graph.neighbors ───────────────────────────────────────────────────────────

pub struct GraphNeighborsTool {
    data_dir: PathBuf,
}

impl GraphNeighborsTool {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into() }
    }
}

#[async_trait]
impl Tool for GraphNeighborsTool {
    fn name(&self) -> &str { "graph.neighbors" }

    fn description(&self) -> &str {
        "Get nodes within N hops of a given node. \
         Parameters: node_id (string), hops (int, default 1)."
    }

    fn required_permissions(&self) -> Vec<Permission> { vec![] }

    async fn execute(&self, task_id: Uuid, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        let node_id = params
            .get("node_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("node_id".into()))?;
        let hops = params.get("hops").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

        // Build undirected adjacency from edge files.
        let edges_dir = self.data_dir.join("edges");
        let files = list_md_files(&edges_dir).await;
        let mut adj: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for path in files {
            let text = match tokio::fs::read_to_string(&path).await {
                Ok(t) => t,
                Err(_) => continue,
            };
            let (meta, _) = parse_frontmatter(&text);
            if let (Some(from), Some(to), rel) = (
                meta.get("from").cloned(),
                meta.get("to").cloned(),
                meta.get("relationship").cloned().unwrap_or_default(),
            ) {
                adj.entry(from.clone()).or_default().push((to.clone(), rel.clone()));
                adj.entry(to.clone()).or_default().push((from.clone(), rel));
            }
        }

        // BFS outward from start node.
        let mut visited: HashMap<String, usize> = HashMap::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        queue.push_back((node_id.to_string(), 0));
        visited.insert(node_id.to_string(), 0);
        while let Some((current, depth)) = queue.pop_front() {
            if depth >= hops {
                continue;
            }
            if let Some(neighbors) = adj.get(&current) {
                for (neighbor, _) in neighbors {
                    if !visited.contains_key(neighbor) {
                        visited.insert(neighbor.clone(), depth + 1);
                        queue.push_back((neighbor.clone(), depth + 1));
                    }
                }
            }
        }

        let neighbors: Vec<serde_json::Value> = visited
            .into_iter()
            .filter(|(id, _)| id != node_id)
            .map(|(id, dist)| serde_json::json!({ "id": id, "distance": dist }))
            .collect();

        Ok(ToolResult::success(
            task_id,
            "graph.neighbors",
            serde_json::json!({ "node_id": node_id, "neighbors": neighbors }),
        ))
    }
}

// ── graph.add_node ────────────────────────────────────────────────────────────

pub struct GraphAddNodeTool {
    data_dir: PathBuf,
}

impl GraphAddNodeTool {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into() }
    }
}

#[async_trait]
impl Tool for GraphAddNodeTool {
    fn name(&self) -> &str { "graph.add_node" }

    fn description(&self) -> &str {
        "Add a node to the knowledge graph. \
         Parameters: node_id (string, slug form e.g. node-deny-airspace), \
         type (string: mission|capability|system|threat), \
         name (string), body (string, optional description)."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("graph.write".into())]
    }

    async fn execute(&self, task_id: Uuid, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        let node_id = params
            .get("node_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("node_id".into()))?;
        let node_type = params.get("type").and_then(|v| v.as_str()).unwrap_or("unknown");
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or(node_id);
        let body = params.get("body").and_then(|v| v.as_str()).unwrap_or("");

        let nodes_dir = self.data_dir.join("nodes");
        if let Err(e) = tokio::fs::create_dir_all(&nodes_dir).await {
            return Ok(ToolResult::failure(task_id, "graph.add_node", e.to_string()));
        }

        let mut meta = HashMap::new();
        meta.insert("id".to_string(), node_id.to_string());
        meta.insert("type".to_string(), node_type.to_string());
        meta.insert("name".to_string(), name.to_string());
        meta.insert("created".to_string(), Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string());

        let full_body = if body.is_empty() {
            format!("# {name}\n")
        } else {
            format!("# {name}\n\n{body}")
        };
        let content = format_frontmatter(&meta, &full_body);
        let path = nodes_dir.join(format!("{node_id}.md"));

        match tokio::fs::write(&path, content).await {
            Ok(()) => {
                let mut result = ToolResult::success(
                    task_id,
                    "graph.add_node",
                    serde_json::json!({ "node_id": node_id, "type": node_type, "name": name }),
                );
                result.side_effects.push(SideEffect {
                    kind: "node_created".into(),
                    description: format!("Created graph node {node_id} ({node_type}: {name})"),
                    path: Some(path.to_string_lossy().to_string()),
                });
                Ok(result)
            }
            Err(e) => Ok(ToolResult::failure(task_id, "graph.add_node", e.to_string())),
        }
    }
}

// ── graph.add_edge ────────────────────────────────────────────────────────────

pub struct GraphAddEdgeTool {
    data_dir: PathBuf,
}

impl GraphAddEdgeTool {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self { data_dir: data_dir.into() }
    }
}

#[async_trait]
impl Tool for GraphAddEdgeTool {
    fn name(&self) -> &str { "graph.add_edge" }

    fn description(&self) -> &str {
        "Add a directed edge between two graph nodes. \
         Parameters: edge_id (string), from_id (string), to_id (string), \
         relationship (string: requires|provides|counters|vulnerable-to|threatens)."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("graph.write".into())]
    }

    async fn execute(&self, task_id: Uuid, params: serde_json::Value) -> Result<ToolResult, ToolError> {
        let edge_id = params
            .get("edge_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("edge_id".into()))?;
        let from_id = params
            .get("from_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("from_id".into()))?;
        let to_id = params
            .get("to_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("to_id".into()))?;
        let relationship = params
            .get("relationship")
            .and_then(|v| v.as_str())
            .unwrap_or("related");

        let edges_dir = self.data_dir.join("edges");
        if let Err(e) = tokio::fs::create_dir_all(&edges_dir).await {
            return Ok(ToolResult::failure(task_id, "graph.add_edge", e.to_string()));
        }

        let mut meta = HashMap::new();
        meta.insert("id".to_string(), edge_id.to_string());
        meta.insert("from".to_string(), from_id.to_string());
        meta.insert("to".to_string(), to_id.to_string());
        meta.insert("relationship".to_string(), relationship.to_string());
        meta.insert("created".to_string(), Utc::now().format("%Y-%m-%dT%H:%M:%S").to_string());

        let content = format_frontmatter(&meta, "");
        let path = edges_dir.join(format!("{edge_id}.md"));

        match tokio::fs::write(&path, content).await {
            Ok(()) => {
                let mut result = ToolResult::success(
                    task_id,
                    "graph.add_edge",
                    serde_json::json!({
                        "edge_id": edge_id,
                        "from": from_id,
                        "to": to_id,
                        "relationship": relationship,
                    }),
                );
                result.side_effects.push(SideEffect {
                    kind: "edge_created".into(),
                    description: format!("Created edge {from_id} --[{relationship}]--> {to_id}"),
                    path: Some(path.to_string_lossy().to_string()),
                });
                Ok(result)
            }
            Err(e) => Ok(ToolResult::failure(task_id, "graph.add_edge", e.to_string())),
        }
    }
}
