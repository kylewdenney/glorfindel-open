//! # Glorfindel Agent
//!
//! Agent abstraction layer for the Glorfindel agentic AI framework.
//! Provides the `Agent` trait defining the agentic loop, a registry
//! for capability-based discovery, and an Ollama-backed implementation.

pub mod dm_assistant;
pub mod error;
pub mod media_server_agent;
pub mod model_manager;
pub mod ollama;
pub mod registry;
pub mod traits;

pub use dm_assistant::DmAssistantAgent;
pub use error::AgentError;
pub use media_server_agent::MediaServerAgent;
pub use model_manager::ModelManager;
pub use ollama::OllamaAgent;
pub use registry::AgentRegistry;
pub use traits::Agent;
