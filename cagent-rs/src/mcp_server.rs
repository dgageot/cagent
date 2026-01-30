//! MCP Server module
//!
//! This module provides MCP server functionality to expose cagent agents as MCP tools.
//! It supports multiple transports:
//! - **Stdio**: For integration with MCP clients like Claude Desktop
//! - **HTTP**: Streamable HTTP transport for web-based MCP clients

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_mcp_sdk::error::SdkResult;
use rust_mcp_sdk::mcp_server::{server_runtime, McpServerOptions, ServerHandler};
use rust_mcp_sdk::schema::{
    CallToolError, CallToolRequestParams, CallToolResult, Implementation, InitializeResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, RpcError, ServerCapabilities,
    ServerCapabilitiesTools, TextContent, Tool as McpTool, ToolAnnotations as McpToolAnnotations,
};
use rust_mcp_sdk::mcp_server::{hyper_server, HyperServerOptions};
use rust_mcp_sdk::{McpServer, StdioTransport, ToMcpServerHandler, TransportOptions};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::agent::{Agent, Team};
use crate::runtime::{Event, LocalRuntime, RuntimeConfig};
use crate::session::Session;

/// Input schema for agent tools exposed via MCP
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolInput {
    /// The message to send to the agent
    pub message: String,
}

/// Output schema for agent tools exposed via MCP
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolOutput {
    /// The response from the agent
    pub response: String,
}

/// MCP Server handler that exposes agents as tools
pub struct AgentMcpHandler {
    team: Arc<Team>,
    agent_name: Option<String>,
    tools: Arc<RwLock<Vec<McpTool>>>,
}

impl AgentMcpHandler {
    /// Create a new MCP handler for the given team
    ///
    /// # Arguments
    /// * `team` - The team of agents to expose
    /// * `agent_name` - Optional specific agent to expose (all agents if None)
    pub fn new(team: Arc<Team>, agent_name: Option<String>) -> Self {
        Self {
            team,
            agent_name,
            tools: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Get the list of agents to expose as tools
    fn get_agent_names(&self) -> Vec<String> {
        if let Some(ref name) = self.agent_name {
            vec![name.clone()]
        } else {
            self.team.agent_names()
        }
    }

    /// Check if an agent is read-only (all its tools have read_only_hint=true)
    async fn is_read_only_agent(&self, agent: &Agent) -> bool {
        match agent.tools().await {
            Ok(tools) => tools.iter().all(|t| t.annotations.read_only_hint),
            Err(_) => false,
        }
    }

    /// Build the list of MCP tools from the configured agents
    async fn build_tools(&self) -> Vec<McpTool> {
        let mut tools = Vec::new();

        for agent_name in self.get_agent_names() {
            if let Some(agent) = self.team.agent(&agent_name) {
                let description = agent
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("Run the {} agent", agent_name));

                let read_only = self.is_read_only_agent(agent).await;

                let input_schema = serde_json::from_value(json!({
                    "type": "object",
                    "properties": {
                        "message": {
                            "type": "string",
                            "description": "The message to send to the agent"
                        }
                    },
                    "required": ["message"]
                }))
                .unwrap();

                tools.push(McpTool {
                    name: agent_name.clone(),
                    title: Some(agent_name.clone()),
                    description: Some(description),
                    input_schema,
                    output_schema: None,
                    annotations: Some(McpToolAnnotations {
                        title: Some(agent_name),
                        read_only_hint: Some(read_only),
                        destructive_hint: None,
                        idempotent_hint: None,
                        open_world_hint: None,
                    }),
                    icons: vec![],
                    execution: None,
                    meta: None,
                });
            }
        }

        tools
    }

    /// Execute an agent with the given message
    async fn execute_agent(&self, agent_name: &str, message: &str) -> Result<String> {
        debug!(agent = %agent_name, message = %message, "Executing agent via MCP");

        let _agent = self
            .team
            .agent(agent_name)
            .context(format!("Agent '{}' not found", agent_name))?;

        // Create a session for this request
        let mut session = Session::new()
            .with_tools_approved(true)
            .with_user_message(message);

        // Create runtime and set current agent
        let rt = LocalRuntime::new((*self.team).clone(), RuntimeConfig::default())?;
        rt.set_current_agent(agent_name).await?;

        // Run the agent (non-streaming)
        let mut events = rt.run_stream(&mut session).await;

        // Collect events and extract final response
        let mut response = String::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::AgentChoice { content, .. } => {
                    response.push_str(&content);
                }
                Event::Error { message } => {
                    error!(agent = %agent_name, error = %message, "Agent execution error");
                    return Err(anyhow::anyhow!("Agent execution error: {}", message));
                }
                _ => {}
            }
        }

        if response.is_empty() {
            response = "No response from agent".to_string();
        }

        debug!(
            agent = %agent_name,
            response_len = response.len(),
            "Agent execution completed"
        );

        Ok(response)
    }
}

#[async_trait]
impl ServerHandler for AgentMcpHandler {
    async fn handle_list_tools_request(
        &self,
        _request: Option<PaginatedRequestParams>,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<ListToolsResult, RpcError> {
        // Build tools on first request if not already built
        let tools = {
            let guard = self.tools.read().await;
            if guard.is_empty() {
                drop(guard);
                let new_tools = self.build_tools().await;
                let mut guard = self.tools.write().await;
                *guard = new_tools.clone();
                new_tools
            } else {
                guard.clone()
            }
        };

        Ok(ListToolsResult {
            tools,
            meta: None,
            next_cursor: None,
        })
    }

    async fn handle_call_tool_request(
        &self,
        params: CallToolRequestParams,
        _runtime: Arc<dyn McpServer>,
    ) -> std::result::Result<CallToolResult, CallToolError> {
        let agent_name = &params.name;

        // Validate tool exists
        let agents = self.get_agent_names();
        if !agents.contains(&agent_name.to_string()) {
            return Err(CallToolError::unknown_tool(agent_name.clone()));
        }

        // Parse the input
        let message = params
            .arguments
            .as_ref()
            .and_then(|args| args.get("message"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CallToolError::invalid_arguments(agent_name, Some("Missing 'message' parameter".to_string()))
            })?;

        // Execute the agent
        match self.execute_agent(agent_name, message).await {
            Ok(response) => {
                let text_content = TextContent::new(response, None, None);
                Ok(CallToolResult::text_content(vec![text_content]))
            }
            Err(e) => {
                // Return error as text content with is_error flag
                let text_content = TextContent::new(format!("Error: {}", e), None, None);
                let mut result = CallToolResult::text_content(vec![text_content]);
                result.is_error = Some(true);
                Ok(result)
            }
        }
    }
}

/// Start an MCP server exposing agents with stdio transport
///
/// # Arguments
/// * `team` - The team of agents to expose
/// * `agent_name` - Optional specific agent to expose (all agents if None)
pub async fn start_mcp_server_stdio(team: Arc<Team>, agent_name: Option<String>) -> SdkResult<()> {
    info!("Starting MCP server with stdio transport");

    let handler = AgentMcpHandler::new(team, agent_name);
    let agent_names = handler.get_agent_names();
    debug!(agents = ?agent_names, "Exposing agents as MCP tools");

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "cagent".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("cagent MCP Server".into()),
            description: Some("cagent AI agent runner".into()),
            icons: vec![],
            website_url: Some("https://github.com/docker/cagent".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: None,
        meta: None,
    };

    let transport = StdioTransport::new(TransportOptions::default())?;

    let server = server_runtime::create_server(McpServerOptions {
        transport,
        handler: handler.to_mcp_server_handler(),
        server_details,
        task_store: None,
        client_task_store: None,
    });

    server.start().await
}

/// Start an MCP server exposing agents with HTTP (Streamable HTTP) transport
///
/// # Arguments
/// * `team` - The team of agents to expose
/// * `agent_name` - Optional specific agent to expose (all agents if None)
/// * `host` - The host to bind to (e.g., "127.0.0.1")
/// * `port` - The port to bind to (e.g., 8080)
pub async fn start_mcp_server_http(
    team: Arc<Team>,
    agent_name: Option<String>,
    host: &str,
    port: u16,
) -> SdkResult<()> {
    info!(host = %host, port = %port, "Starting MCP server with HTTP transport");

    let handler = AgentMcpHandler::new(team, agent_name);
    let agent_names = handler.get_agent_names();
    debug!(agents = ?agent_names, "Exposing agents as MCP tools via HTTP");

    let server_details = InitializeResult {
        server_info: Implementation {
            name: "cagent".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            title: Some("cagent MCP Server".into()),
            description: Some("cagent AI agent runner - HTTP transport".into()),
            icons: vec![],
            website_url: Some("https://github.com/docker/cagent".into()),
        },
        capabilities: ServerCapabilities {
            tools: Some(ServerCapabilitiesTools { list_changed: None }),
            ..Default::default()
        },
        protocol_version: ProtocolVersion::V2025_11_25.into(),
        instructions: None,
        meta: None,
    };

    let server = hyper_server::create_server(
        server_details,
        handler.to_mcp_server_handler(),
        HyperServerOptions {
            host: host.to_string(),
            port: port,
            ..Default::default()
        },
    );

    server.start().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_tool_input_schema() {
        let input = AgentToolInput {
            message: "Hello".to_string(),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_agent_tool_output_schema() {
        let output = AgentToolOutput {
            response: "World".to_string(),
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("World"));
    }
}
