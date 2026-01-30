//! cagent - AI Agent Runner (Rust Port)
//!
//! A multi-agent AI system with hierarchical agent structure and pluggable tool ecosystem.

pub mod a2a_server;
pub mod agent;
pub mod api;
pub mod catalog;
pub mod chat;
pub mod cli;
pub mod config;
pub mod desktop;
pub mod environment;
pub mod evaluation;
pub mod fake;
pub mod gateway;
pub mod hooks;
pub mod mcp_server;
pub mod model;
pub mod oci;
pub mod paths;
pub mod permissions;
pub mod runtime;
pub mod session;
pub mod telemetry;
pub mod tools;

pub mod logging;

pub mod user_config;

#[cfg(test)]
pub mod testing;

#[cfg(feature = "tui")]
pub mod tui;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod logging_test;

#[cfg(test)]
mod config_path_test;

#[cfg(test)]
mod runtime_max_iterations_test;

#[cfg(test)]
mod welcome_message_test;

#[cfg(test)]
mod custom_provider_test;

#[cfg(test)]
mod openai_sampling_test;

#[cfg(test)]
mod tool_filtering_test;

#[cfg(test)]
mod tool_instruction_test;

#[cfg(test)]
mod shell_timeout_test;

pub use agent::{Agent, Team};
pub use chat::Message;
pub use config::Config;
pub use model::Provider;
pub use runtime::{Event, LocalRuntime};
pub use session::Session;
pub use tools::{Tool, ToolCall, ToolCallResult, ToolSet};
