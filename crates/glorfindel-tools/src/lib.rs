//! # Glorfindel Tools
//!
//! Tool interface and built-in tool implementations for the agentic framework.
//! Tools are the mechanism by which agents interact with the outside world.
//! All tool execution goes through the `ToolExecutor`, which enforces
//! deny-by-default permissions.

pub mod bash_tool;
pub mod error;
pub mod executor;
pub mod file_tools;
pub mod search_tool;
pub mod traits;

pub use bash_tool::BashTool;
pub use error::ToolError;
pub use executor::ToolExecutor;
pub use file_tools::{FileReadTool, FileWriteTool};
pub use search_tool::SearchTool;
pub use traits::Tool;
