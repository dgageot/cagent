//! A2A (Agent-to-Agent) toolset
//!
//! This module provides a toolset implementation for connecting to remote A2A agents.
//! A2A is a protocol for agent-to-agent communication that allows one agent to call
//! another agent's skills.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::{debug, info};

use crate::tools::{Tool, ToolAnnotations, ToolCall, ToolCallResult, ToolSet};

/// A2A Agent Card returned from the well-known endpoint
#[derive(Debug, Clone, Deserialize)]
pub struct AgentCard {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub skills: Vec<AgentSkill>,
    pub url: String,
    #[serde(default)]
    pub version: String,
    #[serde(rename = "preferredTransport")]
    pub preferred_transport: Option<String>,
    #[serde(default)]
    pub capabilities: AgentCapabilities,
}

/// An A2A agent skill
#[derive(Debug, Clone, Deserialize)]
pub struct AgentSkill {
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A2A agent capabilities
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AgentCapabilities {
    #[serde(default)]
    pub streaming: bool,
}

/// A2A JSON-RPC request
#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    method: &'static str,
    params: serde_json::Value,
    id: String,
}

/// A2A JSON-RPC response
#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    #[allow(dead_code)]
    id: Option<String>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
}

/// A2A JSON-RPC error
#[derive(Debug, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// A2A message
#[derive(Debug, Serialize, Deserialize)]
struct A2aMessage {
    role: String,
    parts: Vec<A2aPart>,
}

/// A2A message part
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum A2aPart {
    Text { text: String },
}

/// A2A task status
#[derive(Debug, Deserialize)]
struct A2aTask {
    #[allow(dead_code)]
    id: Option<String>,
    status: Option<A2aTaskStatus>,
}

/// A2A task status
#[derive(Debug, Deserialize)]
struct A2aTaskStatus {
    message: Option<A2aMessage>,
}

/// A2A Toolset for connecting to remote A2A agents
pub struct A2aToolset {
    name: String,
    url: String,
    headers: HashMap<String, String>,
    client: Client,
    card: Arc<RwLock<Option<AgentCard>>>,
}

impl A2aToolset {
    /// Create a new A2A toolset for the given URL
    ///
    /// # Arguments
    /// * `name` - The toolset name for prefixing tools
    /// * `url` - The URL of the A2A agent (or its well-known agent card endpoint)
    /// * `headers` - Optional HTTP headers to include in requests
    pub fn new(name: impl Into<String>, url: impl Into<String>, headers: HashMap<String, String>) -> Self {
        let name = name.into();
        let url = url.into();

        debug!(name = %name, url = %url, "Creating A2A toolset");

        Self {
            name,
            url,
            headers,
            client: Client::new(),
            card: Arc::new(RwLock::new(None)),
        }
    }

    /// Get the well-known agent card URL
    fn well_known_url(&self) -> String {
        // If URL already ends with well-known path, use it as-is
        if self.url.contains("/.well-known/agent") {
            return self.url.clone();
        }

        // Otherwise, append the well-known path
        let base = self.url.trim_end_matches('/');
        format!("{}/.well-known/agent.json", base)
    }

    /// Fetch the agent card from the remote agent
    async fn fetch_agent_card(&self) -> Result<AgentCard> {
        let url = self.well_known_url();
        debug!(url = %url, "Fetching A2A agent card");

        let mut req = self.client.get(&url);
        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        let resp = req.send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Failed to fetch A2A agent card: {}", resp.status());
        }

        let card: AgentCard = resp.json().await?;
        Ok(card)
    }

    /// Send a message to the A2A agent and get a response
    async fn send_message(&self, message: &str) -> Result<String> {
        let card = {
            let guard = self.card.read().await;
            guard.clone().context("A2A toolset not started")?
        };

        // Build the A2A message
        let a2a_message = A2aMessage {
            role: "user".to_string(),
            parts: vec![A2aPart::Text {
                text: message.to_string(),
            }],
        };

        // Build JSON-RPC request
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            method: "message/send",
            params: serde_json::json!({
                "message": a2a_message
            }),
            id: uuid::Uuid::new_v4().to_string(),
        };

        debug!(
            url = %card.url,
            method = request.method,
            "Sending A2A request"
        );

        let mut req = self.client.post(&card.url).json(&request);
        for (key, value) in &self.headers {
            req = req.header(key, value);
        }

        let resp = req.send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("A2A request failed: {}", resp.status());
        }

        let response: JsonRpcResponse = resp.json().await?;

        // Check for error
        if let Some(error) = response.error {
            anyhow::bail!("A2A error ({}): {}", error.code, error.message);
        }

        // Extract text from result
        let result = response.result.context("A2A response missing result")?;
        
        // Try to parse as task and extract message
        if let Ok(task) = serde_json::from_value::<A2aTask>(result.clone()) {
            if let Some(status) = task.status {
                if let Some(message) = status.message {
                    return Ok(Self::extract_text(&message));
                }
            }
        }

        // Try to parse as direct message
        if let Ok(message) = serde_json::from_value::<A2aMessage>(result.clone()) {
            return Ok(Self::extract_text(&message));
        }

        // Fall back to stringifying the result
        Ok(result.to_string())
    }

    /// Extract text from an A2A message
    fn extract_text(message: &A2aMessage) -> String {
        message
            .parts
            .iter()
            .filter_map(|part| match part {
                A2aPart::Text { text } => Some(text.clone()),
            })
            .collect::<Vec<_>>()
            .join("")
    }

    /// Sanitize a tool name to only contain allowed characters
    fn sanitize_tool_name(name: &str) -> String {
        let result: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect();

        // Remove leading/trailing underscores and collapse multiple underscores
        let result = result.trim_matches('_');
        let mut collapsed = String::new();
        let mut last_was_underscore = false;
        for c in result.chars() {
            if c == '_' {
                if !last_was_underscore {
                    collapsed.push(c);
                }
                last_was_underscore = true;
            } else {
                collapsed.push(c);
                last_was_underscore = false;
            }
        }
        collapsed
    }
}

#[async_trait]
impl ToolSet for A2aToolset {
    async fn start(&self) -> Result<()> {
        // Check if already started
        {
            let card = self.card.read().await;
            if card.is_some() {
                debug!(name = %self.name, "A2A toolset already started");
                return Ok(());
            }
        }

        info!(
            name = %self.name,
            url = %self.url,
            "Starting A2A toolset"
        );

        let card = self.fetch_agent_card().await?;

        info!(
            name = %self.name,
            agent_name = %card.name,
            skills = card.skills.len(),
            "A2A toolset started"
        );

        *self.card.write().await = Some(card);

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        debug!(name = %self.name, "Stopping A2A toolset");
        *self.card.write().await = None;
        Ok(())
    }

    async fn tools(&self) -> Result<Vec<Tool>> {
        let card = {
            let guard = self.card.read().await;
            guard.clone().context("A2A toolset not started - call start() first")?
        };

        // If skills are defined, create a tool for each skill; otherwise create one tool for the agent
        let skills = if card.skills.is_empty() {
            vec![AgentSkill {
                id: Some(card.name.clone()),
                name: card.name.clone(),
                description: card.description.clone(),
                tags: vec![],
            }]
        } else {
            card.skills.clone()
        };

        let tools: Vec<Tool> = skills
            .iter()
            .map(|skill| {
                let skill_name = skill.id.as_ref().unwrap_or(&skill.name);
                let name = if self.name.is_empty() {
                    Self::sanitize_tool_name(skill_name)
                } else {
                    format!(
                        "{}_{}",
                        Self::sanitize_tool_name(&self.name),
                        Self::sanitize_tool_name(skill_name)
                    )
                };

                Tool {
                    name: name.clone(),
                    category: Some("a2a".to_string()),
                    description: format!(
                        "Calls the '{}' skill of the {} agent. {}",
                        skill.name, card.name, skill.description
                    ),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "message": {
                                "type": "string",
                                "description": "The message or request to send to the agent"
                            }
                        },
                        "required": ["message"]
                    }),
                    annotations: ToolAnnotations {
                        read_only_hint: false,
                        title: Some(name),
                    },
                    output_schema: None,
                }
            })
            .collect();

        debug!(name = %self.name, count = tools.len(), "Listed A2A tools");
        Ok(tools)
    }

    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolCallResult> {
        debug!(
            name = %self.name,
            tool = %tool_call.function.name,
            "Executing A2A tool"
        );

        // Parse the message argument
        #[derive(Deserialize)]
        struct Args {
            message: String,
        }

        let args: Args = serde_json::from_str(&tool_call.function.arguments)
            .context("Failed to parse A2A tool arguments")?;

        // Send the message to the A2A agent
        let response = self.send_message(&args.message).await?;

        debug!(
            name = %self.name,
            tool = %tool_call.function.name,
            response_len = response.len(),
            "A2A tool call completed"
        );

        Ok(ToolCallResult::success(response))
    }

    fn instructions(&self) -> Option<String> {
        // Try to get instructions from the agent card
        let card = self.card.try_read().ok()?.clone()?;

        let mut sb = String::new();
        sb.push_str(&format!("## {}\n\n{}\n", card.name, card.description));

        for skill in &card.skills {
            sb.push_str(&format!("- **{}**: {}\n", skill.name, skill.description));
        }

        Some(sb)
    }
}

impl std::fmt::Debug for A2aToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A2aToolset")
            .field("name", &self.name)
            .field("url", &self.url)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a2a_toolset_creation() {
        let toolset = A2aToolset::new("test", "http://localhost:8080", HashMap::new());
        assert_eq!(toolset.name, "test");
        assert_eq!(toolset.url, "http://localhost:8080");
    }

    #[test]
    fn test_well_known_url() {
        let toolset = A2aToolset::new("test", "http://localhost:8080", HashMap::new());
        assert_eq!(
            toolset.well_known_url(),
            "http://localhost:8080/.well-known/agent.json"
        );

        let toolset2 = A2aToolset::new(
            "test",
            "http://localhost:8080/.well-known/agent.json",
            HashMap::new(),
        );
        assert_eq!(
            toolset2.well_known_url(),
            "http://localhost:8080/.well-known/agent.json"
        );
    }

    #[test]
    fn test_sanitize_tool_name() {
        assert_eq!(A2aToolset::sanitize_tool_name("hello-world"), "hello_world");
        assert_eq!(A2aToolset::sanitize_tool_name("Hello World"), "hello_world");
        assert_eq!(A2aToolset::sanitize_tool_name("test__name"), "test_name");
        assert_eq!(A2aToolset::sanitize_tool_name("__test__"), "test");
        assert_eq!(A2aToolset::sanitize_tool_name("CamelCase"), "camelcase");
    }

    #[test]
    fn test_extract_text() {
        let message = A2aMessage {
            role: "assistant".to_string(),
            parts: vec![
                A2aPart::Text {
                    text: "Hello ".to_string(),
                },
                A2aPart::Text {
                    text: "World".to_string(),
                },
            ],
        };
        assert_eq!(A2aToolset::extract_text(&message), "Hello World");
    }
}
