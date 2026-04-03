use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};

use crate::error::AgentError;

/// Manages Ollama model lifecycle — pulling, checking, and listing models.
///
/// On startup, the ModelManager checks which models are needed (from config
/// or environment variables) and ensures they're available in Ollama.
/// This enables portable deployment: deploy the container anywhere and
/// models self-provision on first boot.
pub struct ModelManager {
    client: Client,
    ollama_host: String,
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    models: Option<Vec<OllamaModel>>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
}

#[derive(Debug, Serialize)]
struct PullRequest {
    name: String,
    stream: bool,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct PullResponse {
    status: Option<String>,
    total: Option<u64>,
    completed: Option<u64>,
}

impl ModelManager {
    pub fn new(ollama_host: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            ollama_host: ollama_host.into(),
        }
    }

    /// List all models currently available in Ollama.
    pub async fn list_models(&self) -> Result<Vec<String>, AgentError> {
        let url = format!("{}/api/tags", self.ollama_host);
        let resp: OllamaTagsResponse = self
            .client
            .get(&url)
            .send()
            .await?
            .json()
            .await?;

        Ok(resp
            .models
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.name)
            .collect())
    }

    /// Check if a specific model is available.
    pub async fn has_model(&self, model_name: &str) -> Result<bool, AgentError> {
        let models = self.list_models().await?;
        Ok(models.iter().any(|m| m.starts_with(model_name)))
    }

    /// Pull a model from the Ollama registry. Blocks until complete.
    pub async fn pull_model(&self, model_name: &str) -> Result<(), AgentError> {
        info!(model = model_name, "Pulling model from Ollama registry");

        let url = format!("{}/api/pull", self.ollama_host);
        let resp = self
            .client
            .post(&url)
            .json(&PullRequest {
                name: model_name.to_string(),
                stream: false,
            })
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(AgentError::ModelNotAvailable(format!(
                "pull failed ({status}): {body}"
            )));
        }

        info!(model = model_name, "Model pull complete");
        Ok(())
    }

    /// Ensure all specified models are available, pulling any that are missing.
    ///
    /// Models can be specified as a comma-separated string (e.g., from the
    /// `GLORFINDEL_MODELS` environment variable).
    pub async fn ensure_models(&self, models: &str) -> Result<(), AgentError> {
        let model_list: Vec<&str> = models
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();

        if model_list.is_empty() {
            warn!("No models specified to ensure");
            return Ok(());
        }

        info!(models = ?model_list, "Ensuring models are available");

        for model in model_list {
            match self.has_model(model).await {
                Ok(true) => {
                    info!(model, "Model already available");
                }
                Ok(false) => {
                    info!(model, "Model not found, pulling...");
                    self.pull_model(model).await?;
                }
                Err(e) => {
                    error!(model, error = %e, "Failed to check model availability");
                    // Try to pull anyway
                    self.pull_model(model).await?;
                }
            }
        }

        Ok(())
    }
}
