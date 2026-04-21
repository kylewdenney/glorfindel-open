use std::path::Path;

use async_trait::async_trait;
use glorfindel_schemas::tool::ToolResult;
use glorfindel_schemas::types::Permission;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::error::ToolError;
use crate::traits::Tool;

// ---------------------------------------------------------------------------
// Chunking
// ---------------------------------------------------------------------------

const MAX_CHUNK_CHARS: usize = 600;

/// A single chunk of text from a rulebook file.
#[derive(Debug, Clone)]
struct Chunk {
    text: String,
    source: String,
    chunk_index: usize,
    embedding: Vec<f32>,
}

/// Split text into paragraph-sized chunks, capping at MAX_CHUNK_CHARS.
/// Each chunk keeps its source filename and a sequential index.
fn chunk_text(text: &str, source: &str) -> Vec<(String, String, usize)> {
    let mut results = Vec::new();
    let mut idx = 0;

    for paragraph in text.split("\n\n") {
        let paragraph = paragraph.trim();
        if paragraph.is_empty() {
            continue;
        }

        if paragraph.len() <= MAX_CHUNK_CHARS {
            results.push((paragraph.to_string(), source.to_string(), idx));
            idx += 1;
        } else {
            // Split long paragraphs at sentence boundaries
            let mut current = String::new();
            for sentence in paragraph.split(". ") {
                let candidate = if current.is_empty() {
                    sentence.to_string()
                } else {
                    format!("{current}. {sentence}")
                };

                if candidate.len() > MAX_CHUNK_CHARS && !current.is_empty() {
                    results.push((current.trim().to_string(), source.to_string(), idx));
                    idx += 1;
                    current = sentence.to_string();
                } else {
                    current = candidate;
                }
            }
            if !current.trim().is_empty() {
                results.push((current.trim().to_string(), source.to_string(), idx));
                idx += 1;
            }
        }
    }

    results
}

// ---------------------------------------------------------------------------
// Ollama embedding client
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct EmbedResponse {
    embedding: Vec<f32>,
}

async fn embed(client: &Client, ollama_host: &str, model: &str, text: &str) -> Result<Vec<f32>, String> {
    let url = format!("{ollama_host}/api/embeddings");
    let resp = client
        .post(&url)
        .json(&EmbedRequest { model, prompt: text })
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json::<EmbedResponse>()
        .await
        .map_err(|e| e.to_string())?;
    Ok(resp.embedding)
}

// ---------------------------------------------------------------------------
// Cosine similarity
// ---------------------------------------------------------------------------

fn cosine_sim(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

// ---------------------------------------------------------------------------
// RulebookTool
// ---------------------------------------------------------------------------

/// RAG search over indexed rulebook files.
///
/// On construction, reads all `.txt` and `.md` files from the rulebooks
/// directory, chunks them, and embeds each chunk via Ollama. At query time,
/// embeds the query and returns the top-k most similar chunks with citations.
pub struct RulebookTool {
    chunks: Vec<Chunk>,
    ollama_host: String,
    embed_model: String,
    client: Client,
}

impl RulebookTool {
    /// Build the index by reading and embedding all rulebook files.
    /// Returns an error string if the directory can't be read or embedding fails.
    pub async fn build(
        rulebooks_dir: &Path,
        ollama_host: &str,
        embed_model: &str,
    ) -> Result<Self, String> {
        let client = Client::new();
        let mut chunks = Vec::new();

        let mut entries = tokio::fs::read_dir(rulebooks_dir)
            .await
            .map_err(|e| format!("failed to read rulebooks dir: {e}"))?;

        let mut files: Vec<(String, String)> = Vec::new(); // (path, filename)
        while let Ok(Some(entry)) = entries.next_entry().await {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "txt" | "md") {
                if let (Some(path_str), Some(name)) = (
                    path.to_str().map(str::to_string),
                    path.file_name().and_then(|n| n.to_str()).map(str::to_string),
                ) {
                    files.push((path_str, name));
                }
            }
        }

        files.sort_by(|a, b| a.1.cmp(&b.1));
        info!(count = files.len(), "Indexing rulebook files");

        for (path, filename) in &files {
            let text = match tokio::fs::read_to_string(path).await {
                Ok(t) => t,
                Err(e) => {
                    warn!(file = %filename, error = %e, "Skipping unreadable rulebook file");
                    continue;
                }
            };

            let raw_chunks = chunk_text(&text, filename);
            info!(file = %filename, chunks = raw_chunks.len(), "Embedding rulebook file");

            for (chunk_text, source, chunk_index) in raw_chunks {
                match embed(&client, ollama_host, embed_model, &chunk_text).await {
                    Ok(embedding) => chunks.push(Chunk {
                        text: chunk_text,
                        source,
                        chunk_index,
                        embedding,
                    }),
                    Err(e) => {
                        warn!(file = %filename, chunk = chunk_index, error = %e, "Failed to embed chunk, skipping");
                    }
                }
            }
        }

        info!(total_chunks = chunks.len(), "Rulebook index built");

        Ok(Self {
            chunks,
            ollama_host: ollama_host.to_string(),
            embed_model: embed_model.to_string(),
            client,
        })
    }

    fn top_k(&self, query_embedding: &[f32], k: usize) -> Vec<&Chunk> {
        let mut scored: Vec<(f32, &Chunk)> = self
            .chunks
            .iter()
            .map(|c| (cosine_sim(query_embedding, &c.embedding), c))
            .collect();

        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().take(k).map(|(_, c)| c).collect()
    }
}

#[async_trait]
impl Tool for RulebookTool {
    fn name(&self) -> &str {
        "rulebook.search"
    }

    fn description(&self) -> &str {
        "Search indexed rulebooks using semantic similarity. Returns relevant rule text \
         with citations (source file and section number). Use this when a player disputes \
         a ruling or you need to cite a specific rule."
    }

    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::Custom("rulebook.search".into())]
    }

    async fn execute(&self, task_id: Uuid, parameters: serde_json::Value) -> Result<ToolResult, ToolError> {
        if self.chunks.is_empty() {
            return Ok(ToolResult::success(
                task_id,
                "rulebook.search",
                serde_json::json!({ "results": [], "note": "No rulebooks indexed." }),
            ));
        }

        let query = parameters
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::MissingParameter("query".into()))?;

        let k = parameters
            .get("k")
            .and_then(|v| v.as_u64())
            .unwrap_or(5) as usize;

        let query_embedding = match embed(&self.client, &self.ollama_host, &self.embed_model, query).await {
            Ok(e) => e,
            Err(e) => {
                return Ok(ToolResult::failure(task_id, "rulebook.search", format!("embedding failed: {e}")));
            }
        };

        let hits = self.top_k(&query_embedding, k);

        let results: Vec<serde_json::Value> = hits
            .iter()
            .map(|chunk| {
                serde_json::json!({
                    "source": chunk.source,
                    "section": chunk.chunk_index + 1,
                    "citation": format!("[{}, section {}]", chunk.source, chunk.chunk_index + 1),
                    "text": chunk.text,
                })
            })
            .collect();

        Ok(ToolResult::success(
            task_id,
            "rulebook.search",
            serde_json::json!({ "query": query, "results": results }),
        ))
    }
}
