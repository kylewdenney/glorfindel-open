//! # Glorfindel Schemas
//!
//! Core message type definitions for the Glorfindel agentic AI framework.
//! These types define the protocol spoken across the DDS control plane
//! and ZMQ data plane, following OMS-derived standardized message formats.

pub mod agent;
pub mod envelope;
pub mod task;
pub mod tool;
pub mod types;

// Re-export primary types for convenience.
pub use agent::{AgentResponse, CapabilityManifest};
pub use envelope::MessageEnvelope;
pub use task::TaskRequest;
pub use tool::{ToolCall, ToolResult};
