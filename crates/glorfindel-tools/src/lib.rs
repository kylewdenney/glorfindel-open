//! # Glorfindel Tools
//!
//! Tool interface and built-in tool implementations for the agentic framework.
//! Tools are the mechanism by which agents interact with the outside world.
//! All tool execution goes through the `ToolExecutor`, which enforces
//! deny-by-default permissions.

pub mod bash_tool;
pub mod campaign_tool;
pub mod dice_tool;
pub mod error;
pub mod executor;
pub mod file_tools;
pub mod graph_tools;
pub mod rulebook_tool;
pub mod search_tool;
pub mod traits;

pub use bash_tool::BashTool;
pub use dice_tool::{DiceRollTool, parse_notation as parse_dice_notation};
pub use campaign_tool::{CampaignListTool, CampaignReadTool, CampaignWriteTool};
pub use error::ToolError;
pub use executor::ToolExecutor;
pub use file_tools::{FileReadTool, FileWriteTool};
pub use graph_tools::{GraphAddEdgeTool, GraphAddNodeTool, GraphNeighborsTool, GraphNodeTool, GraphQueryTool};
pub use rulebook_tool::RulebookTool;
pub use search_tool::SearchTool;
pub use traits::Tool;
