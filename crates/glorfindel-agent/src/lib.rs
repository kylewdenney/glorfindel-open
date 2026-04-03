//! # Glorfindel Agent
//!
//! Agent abstraction layer for the Glorfindel agentic AI framework.
//! Provides the `Agent` trait defining the agentic loop, a registry
//! for capability-based discovery, and an Ollama-backed implementation.

pub mod error;
pub mod model_manager;
pub mod ollama;
pub mod registry;
pub mod traits;

pub use error::AgentError;
pub use model_manager::ModelManager;
pub use ollama::OllamaAgent;
pub use registry::AgentRegistry;
pub use traits::Agent;
