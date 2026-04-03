//! # Glorfindel Orchestrator
//!
//! Task routing and lifecycle management. The orchestrator is the "mission
//! manager" of the Glorfindel framework — it receives tasks, routes them
//! to capable agents, and tracks their execution through completion.

pub mod manager;
pub mod router;

pub use manager::TaskManager;
pub use router::Router;
