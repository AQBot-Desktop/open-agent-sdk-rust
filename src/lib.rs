//! # Open Agent SDK
//!
//! A Rust framework for building autonomous AI agents that run the full
//! agentic loop in-process. Supports 15+ built-in tools, MCP integration,
//! permission systems, cost tracking, and more.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use open_agent_sdk::{Agent, AgentOptions};
//!
//! #[tokio::main]
//! async fn main() {
//!     let mut agent = Agent::new(AgentOptions::default()).await.unwrap();
//!     let result = agent.prompt("What files are in the current directory?").await.unwrap();
//!     println!("{}", result.text);
//! }
//! ```

pub mod agent;
pub mod api;
pub mod context;
pub mod costtracker;
pub mod hooks;
pub mod mcp;
pub mod permissions;
pub mod tools;
pub mod types;
pub mod utils;

// Re-export commonly used types
pub use agent::{Agent, AgentOptions, SubagentDefinition};
pub use api::ApiClient;
pub use costtracker::CostTracker;
pub use hooks::{HookConfig, HookEvent, HookFn, HookInput, HookOutput, HookRule};
pub use mcp::McpClient;
pub use tools::ToolRegistry;
pub use types::{
    ApiToolParam, CanUseToolFn, ContentBlock, Message, MessageRole, PermissionDecision,
    PermissionMode, QueryResult, SDKMessage, ThinkingConfig, Tool, ToolError, ToolInputSchema,
    ToolResult, ToolResultContent, ToolUseContext, Usage,
};
pub use utils::tokens::{estimate_cost, estimate_tokens};
