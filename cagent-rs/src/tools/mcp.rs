//! MCP (Model Context Protocol) support
//!
//! This module provides MCP client functionality for connecting to MCP servers
//! via different transports:
//! - **Stdio**: Local command execution via stdin/stdout
//! - **SSE**: Server-Sent Events over HTTP  
//! - **Streamable HTTP**: Bidirectional streaming over HTTP
//!
//! # Elicitation Support
//!
//! MCP servers may request additional information from the user via "elicitation".
//! This module provides types and handlers for elicitation requests.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_mcp_sdk::mcp_client::{client_runtime, ClientHandler, ClientRuntime, McpClientOptions};
use rust_mcp_sdk::schema::{
    CallToolRequestParams, ClientCapabilities, ClientElicitation, ContentBlock, 
    ElicitRequestParams, ElicitResult, ElicitResultAction, ElicitResultContent,
    ElicitResultContentPrimitive, GetPromptRequestParams, Implementation, 
    InitializeRequestParams, PaginatedRequestParams, PromptMessage, RpcError, 
    LATEST_PROTOCOL_VERSION,
};
use rust_mcp_sdk::{ClientSseTransport, ClientSseTransportOptions, ClientStreamableTransport, StreamableTransportOptions, McpClient, StdioTransport, ToMcpClientHandler, TransportOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, RwLock};
use tracing::{debug, error, info, warn};

use crate::tools::{Tool, ToolAnnotations, ToolCall, ToolCallResult, ToolSet};

// ============================================================================
// Prompt Support
// ============================================================================

/// Information about an available MCP prompt
/// 
/// Prompts are reusable templates that MCP servers can provide for common
/// interactions. Users can invoke these prompts with arguments to get
/// pre-formatted content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptInfo {
    /// Unique name of the prompt
    pub name: String,
    /// Human-readable description of what the prompt does
    pub description: String,
    /// List of arguments this prompt accepts
    pub arguments: Vec<PromptArgument>,
}

/// A single argument for an MCP prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// Name of the argument
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Whether this argument is required
    pub required: bool,
}

/// Result of getting a prompt with arguments
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptResult {
    /// The rendered prompt content
    pub content: String,
    /// Optional description of the prompt
    pub description: Option<String>,
}

/// Trait for toolsets that support MCP prompts
#[async_trait]
pub trait PromptProvider: Send + Sync {
    /// List available prompts from the MCP server
    async fn list_prompts(&self) -> Result<Vec<PromptInfo>>;
    
    /// Get a prompt with the provided arguments
    async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptResult>;
}

// ============================================================================
// Elicitation Support
// ============================================================================

/// A request for additional information from the user during MCP tool execution.
///
/// MCP servers may request interactive input from users when they need additional
/// information to complete a task. This is commonly used for:
/// - OAuth authorization flows
/// - API key entry
/// - Confirmation of sensitive operations
/// - Selection from options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    /// Unique request ID for matching responses
    pub request_id: String,
    /// Human-readable message explaining what information is needed
    pub message: String,
    /// The JSON schema describing the expected input format
    pub requested_schema: serde_json::Value,
    /// Name of the MCP server making the request
    pub server_name: String,
}

/// User's response to an elicitation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResponse {
    /// The request ID this response is for
    pub request_id: String,
    /// User's action (accept with data, decline, or cancel)
    pub action: ElicitationAction,
}

/// The action taken by the user in response to an elicitation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ElicitationAction {
    /// User provided the requested data
    Accept {
        /// The data provided by the user, matching the requested schema
        data: serde_json::Value,
    },
    /// User declined to provide the data
    Decline,
    /// User cancelled the operation
    Cancel,
}

impl ElicitationResponse {
    /// Create an accept response with the given data
    pub fn accept(request_id: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            request_id: request_id.into(),
            action: ElicitationAction::Accept { data },
        }
    }

    /// Create a decline response
    pub fn decline(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            action: ElicitationAction::Decline,
        }
    }

    /// Create a cancel response
    pub fn cancel(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            action: ElicitationAction::Cancel,
        }
    }

    /// Convert to MCP SDK's ElicitResult
    pub fn to_elicit_result(&self) -> ElicitResult {
        match &self.action {
            ElicitationAction::Accept { data } => {
                // Convert data to HashMap<String, ElicitResultContent>
                let content: HashMap<String, ElicitResultContent> = if let Some(obj) = data.as_object() {
                    obj.iter()
                        .map(|(k, v)| {
                            let content = match v {
                                Value::String(s) => ElicitResultContent::Primitive(
                                    ElicitResultContentPrimitive::String(s.clone())
                                ),
                                Value::Bool(b) => ElicitResultContent::Primitive(
                                    ElicitResultContentPrimitive::Boolean(*b)
                                ),
                                Value::Number(n) => {
                                    if let Some(i) = n.as_i64() {
                                        ElicitResultContent::Primitive(
                                            ElicitResultContentPrimitive::Integer(i)
                                        )
                                    } else {
                                        // Fallback to string for floats
                                        ElicitResultContent::Primitive(
                                            ElicitResultContentPrimitive::String(n.to_string())
                                        )
                                    }
                                }
                                Value::Array(arr) => {
                                    // Convert array to string array if possible
                                    let strings: Vec<String> = arr.iter()
                                        .map(|v| match v {
                                            Value::String(s) => s.clone(),
                                            _ => v.to_string(),
                                        })
                                        .collect();
                                    ElicitResultContent::StringArray(strings)
                                }
                                _ => ElicitResultContent::Primitive(
                                    ElicitResultContentPrimitive::String(v.to_string())
                                ),
                            };
                            (k.clone(), content)
                        })
                        .collect()
                } else {
                    // Wrap non-object data in a "value" key
                    let content = match data {
                        Value::String(s) => ElicitResultContent::Primitive(
                            ElicitResultContentPrimitive::String(s.clone())
                        ),
                        Value::Bool(b) => ElicitResultContent::Primitive(
                            ElicitResultContentPrimitive::Boolean(*b)
                        ),
                        Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                ElicitResultContent::Primitive(
                                    ElicitResultContentPrimitive::Integer(i)
                                )
                            } else {
                                ElicitResultContent::Primitive(
                                    ElicitResultContentPrimitive::String(n.to_string())
                                )
                            }
                        }
                        _ => ElicitResultContent::Primitive(
                            ElicitResultContentPrimitive::String(data.to_string())
                        ),
                    };
                    let mut map = HashMap::new();
                    map.insert("value".to_string(), content);
                    map
                };
                ElicitResult {
                    action: ElicitResultAction::Accept,
                    content: Some(content),
                    meta: None,
                }
            }
            ElicitationAction::Decline => ElicitResult {
                action: ElicitResultAction::Decline,
                content: None,
                meta: None,
            },
            ElicitationAction::Cancel => ElicitResult {
                action: ElicitResultAction::Cancel,
                content: None,
                meta: None,
            },
        }
    }
}

/// Channel type for sending elicitation requests to the runtime/UI
pub type ElicitationRequestSender = mpsc::Sender<(ElicitationRequest, oneshot::Sender<ElicitationResponse>)>;

/// Channel type for receiving elicitation requests in the runtime/UI
pub type ElicitationRequestReceiver = mpsc::Receiver<(ElicitationRequest, oneshot::Sender<ElicitationResponse>)>;

// ============================================================================
// Client Handler with Elicitation Support
// ============================================================================

/// Handler for MCP client with optional elicitation support
/// 
/// When an elicitation sender is provided, the handler will forward elicitation
/// requests from MCP servers to the runtime/UI for user interaction.
struct CagentClientHandler {
    server_name: String,
    elicitation_tx: Option<ElicitationRequestSender>,
}

impl CagentClientHandler {
    /// Create a new handler without elicitation support
    fn new(server_name: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            elicitation_tx: None,
        }
    }

    /// Create a new handler with elicitation support
    #[allow(dead_code)]
    fn with_elicitation(
        server_name: impl Into<String>,
        elicitation_tx: ElicitationRequestSender,
    ) -> Self {
        Self {
            server_name: server_name.into(),
            elicitation_tx: Some(elicitation_tx),
        }
    }
}

#[async_trait]
impl ClientHandler for CagentClientHandler {
    /// Handle elicitation requests from MCP servers
    async fn handle_elicit_request(
        &self,
        params: ElicitRequestParams,
        _runtime: &dyn McpClient,
    ) -> std::result::Result<ElicitResult, RpcError> {
        // Extract message based on params variant
        let (message, requested_schema) = match &params {
            ElicitRequestParams::FormParams(form) => {
                let schema = serde_json::to_value(&form.requested_schema)
                    .unwrap_or_else(|_| serde_json::json!({}));
                (form.message.clone(), schema)
            }
            ElicitRequestParams::UrlParams(url) => {
                // URL-based elicitation - schema represents the URL to navigate to
                let schema = serde_json::json!({ "url": url.url });
                (url.message.clone(), schema)
            }
        };

        debug!(
            server = %self.server_name,
            message = %message,
            "Received elicitation request from MCP server"
        );

        // If we have an elicitation channel, forward the request
        if let Some(ref tx) = self.elicitation_tx {
            let request_id = uuid::Uuid::new_v4().to_string();
            let request = ElicitationRequest {
                request_id: request_id.clone(),
                message: message.clone(),
                requested_schema,
                server_name: self.server_name.clone(),
            };

            // Create a oneshot channel for the response
            let (response_tx, response_rx) = oneshot::channel();

            // Send the request to the runtime/UI
            if let Err(e) = tx.send((request, response_tx)).await {
                warn!(
                    server = %self.server_name,
                    error = %e,
                    "Failed to send elicitation request to runtime"
                );
                return Ok(ElicitResult {
                    action: ElicitResultAction::Cancel,
                    content: None,
                    meta: None,
                });
            }

            // Wait for the response
            match response_rx.await {
                Ok(response) => {
                    debug!(
                        server = %self.server_name,
                        action = ?response.action,
                        "Received elicitation response from user"
                    );
                    Ok(response.to_elicit_result())
                }
                Err(e) => {
                    warn!(
                        server = %self.server_name,
                        error = %e,
                        "Elicitation response channel closed"
                    );
                    Ok(ElicitResult {
                        action: ElicitResultAction::Cancel,
                        content: None,
                        meta: None,
                    })
                }
            }
        } else {
            // No elicitation handler configured - auto-decline
            warn!(
                server = %self.server_name,
                message = %message,
                "Elicitation requested but no handler configured, declining"
            );
            Ok(ElicitResult {
                action: ElicitResultAction::Decline,
                content: None,
                meta: None,
            })
        }
    }
}

/// MCP Toolset that connects to an MCP server via stdio
pub struct McpToolset {
    name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    #[allow(dead_code)]
    cwd: Option<PathBuf>,
    client: Arc<RwLock<Option<Arc<ClientRuntime>>>>,
    server_instructions: Arc<RwLock<Option<String>>>,
}

impl McpToolset {
    /// Create a new MCP toolset from a command
    pub fn new(
        name: impl Into<String>,
        command: impl Into<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
        cwd: Option<PathBuf>,
    ) -> Self {
        let name = name.into();
        let command = command.into();
        debug!(
            name = %name,
            command = %command,
            args = ?args,
            "Creating MCP toolset"
        );

        Self {
            name,
            command,
            args,
            env,
            cwd,
            client: Arc::new(RwLock::new(None)),
            server_instructions: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait]
impl ToolSet for McpToolset {
    async fn start(&self) -> Result<()> {
        // Check if already started
        {
            let client = self.client.read().await;
            if client.is_some() {
                debug!(name = %self.name, "MCP toolset already started");
                return Ok(());
            }
        }

        info!(
            name = %self.name,
            command = %self.command,
            "Starting MCP toolset"
        );

        // Define client details and capabilities
        let client_details = InitializeRequestParams {
            capabilities: ClientCapabilities {
                elicitation: Some(ClientElicitation {
                    form: Some(serde_json::Map::new()),
                    url: Some(serde_json::Map::new()),
                }),
                ..Default::default()
            },
            client_info: Implementation {
                name: "cagent".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                title: Some("cagent MCP Client".into()),
                description: Some("cagent AI agent runner".into()),
                icons: vec![],
                website_url: Some("https://github.com/docker/cagent".into()),
            },
            protocol_version: LATEST_PROTOCOL_VERSION.into(),
            meta: None,
        };

        // Build environment - merge current env with custom env
        let mut env_map: HashMap<String, String> = std::env::vars().collect();
        for (k, v) in &self.env {
            env_map.insert(k.clone(), v.clone());
        }

        // Create transport options
        let transport_opts = TransportOptions::default();

        // Create the transport - pass env as HashMap
        let transport = StdioTransport::create_with_server_launch(
            &self.command,
            self.args.clone(),
            Some(env_map),
            transport_opts,
        )
        .map_err(|e| anyhow::anyhow!("Failed to create MCP transport: {}", e))?;

        // Create handler for notifications
        let handler = CagentClientHandler::new(&self.name);

        // Create the MCP client
        let client = client_runtime::create_client(McpClientOptions {
            client_details,
            transport,
            handler: handler.to_mcp_client_handler(),
            task_store: None,
            server_task_store: None,
        });

        // Start the client
        client
            .clone()
            .start()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start MCP client: {}", e))?;

        // Get server info
        if let Some(server_info) = client.server_info() {
            info!(
                name = %self.name,
                server_name = %server_info.server_info.name,
                server_version = %server_info.server_info.version,
                "MCP toolset initialized"
            );

            // Store instructions if provided
            if let Some(ref instructions) = server_info.instructions {
                let mut guard = self.server_instructions.write().await;
                *guard = Some(instructions.clone());
            }
        }

        *self.client.write().await = Some(client);

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut client_guard = self.client.write().await;
        if let Some(client) = client_guard.take() {
            debug!(name = %self.name, "Stopping MCP toolset");
            if let Err(e) = client.shut_down().await {
                error!(name = %self.name, error = %e, "Error shutting down MCP client");
            }
        }
        Ok(())
    }

    async fn tools(&self) -> Result<Vec<Tool>> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("MCP toolset not started - call start() first")?;

        debug!(name = %self.name, "Listing MCP tools");

        let params = PaginatedRequestParams {
            cursor: None,
            meta: None,
        };
        let result = client
            .request_tool_list(Some(params))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list MCP tools: {}", e))?;

        let tools: Vec<Tool> = result
            .tools
            .into_iter()
            .map(|mcp_tool| {
                // Prefix tool name with toolset name if set
                let name = if self.name.is_empty() {
                    mcp_tool.name.clone()
                } else {
                    format!("{}_{}", self.name, mcp_tool.name)
                };

                // Convert MCP tool schema to our format
                let parameters = serde_json::to_value(&mcp_tool.input_schema)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));

                Tool {
                    name,
                    category: Some("mcp".to_string()),
                    description: mcp_tool.description.unwrap_or_default(),
                    parameters,
                    annotations: ToolAnnotations {
                        read_only_hint: mcp_tool
                            .annotations
                            .as_ref()
                            .and_then(|a| a.read_only_hint)
                            .unwrap_or(false),
                        title: mcp_tool
                            .annotations
                            .as_ref()
                            .and_then(|a| a.title.clone()),
                    },
                    output_schema: mcp_tool
                        .output_schema
                        .and_then(|s| serde_json::to_value(s).ok()),
                }
            })
            .collect();

        debug!(name = %self.name, count = tools.len(), "Listed MCP tools");
        Ok(tools)
    }

    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolCallResult> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("MCP toolset not started - call start() first")?;

        // Strip the toolset prefix from the tool name
        let tool_name = if !self.name.is_empty() && tool_call.function.name.starts_with(&self.name)
        {
            tool_call
                .function
                .name
                .strip_prefix(&format!("{}_", self.name))
                .unwrap_or(&tool_call.function.name)
        } else {
            &tool_call.function.name
        };

        debug!(
            name = %self.name,
            tool = %tool_name,
            "Calling MCP tool"
        );

        // Parse arguments as serde_json::Map
        let arguments: Option<serde_json::Map<String, Value>> =
            if tool_call.function.arguments.is_empty() {
                None
            } else {
                serde_json::from_str(&tool_call.function.arguments).ok()
            };

        let params = CallToolRequestParams {
            name: tool_name.to_string(),
            arguments,
            meta: None,
            task: None,
        };

        let result = client
            .request_tool_call(params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call MCP tool: {}", e))?;

        // Process the result content
        let mut output = String::new();
        for content in &result.content {
            // Extract text from the content block
            if let ContentBlock::TextContent(text_content) = content {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&text_content.text);
            }
        }

        if output.is_empty() {
            output = "no output".to_string();
        }

        debug!(
            name = %self.name,
            tool = %tool_name,
            is_error = result.is_error.unwrap_or(false),
            output_len = output.len(),
            "MCP tool call completed"
        );

        if result.is_error.unwrap_or(false) {
            Ok(ToolCallResult::error(output))
        } else {
            Ok(ToolCallResult::success(output))
        }
    }

    fn instructions(&self) -> Option<String> {
        // We can't easily read from async RwLock in a sync context
        // For now, return None and rely on toolset instruction from config
        None
    }
}

impl std::fmt::Debug for McpToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpToolset")
            .field("name", &self.name)
            .field("command", &self.command)
            .field("args", &self.args)
            .finish()
    }
}

// Implement PromptProvider for McpToolset
#[async_trait]
impl PromptProvider for McpToolset {
    async fn list_prompts(&self) -> Result<Vec<PromptInfo>> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("MCP toolset not started - call start() first")?;

        debug!(name = %self.name, "Listing MCP prompts");

        let params = PaginatedRequestParams {
            cursor: None,
            meta: None,
        };
        
        let result = client
            .request_prompt_list(Some(params))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list MCP prompts: {}", e))?;

        let prompts: Vec<PromptInfo> = result
            .prompts
            .into_iter()
            .map(|p| {
                let arguments = p
                    .arguments
                    .into_iter()
                    .map(|arg| PromptArgument {
                        name: arg.name,
                        description: arg.description.unwrap_or_default(),
                        required: arg.required.unwrap_or(false),
                    })
                    .collect();

                PromptInfo {
                    name: p.name,
                    description: p.description.unwrap_or_default(),
                    arguments,
                }
            })
            .collect();

        debug!(name = %self.name, count = prompts.len(), "Listed MCP prompts");
        Ok(prompts)
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptResult> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("MCP toolset not started - call start() first")?;

        debug!(name = %self.name, prompt = %name, "Getting MCP prompt");

        let params = GetPromptRequestParams {
            name: name.to_string(),
            arguments: if arguments.is_empty() {
                None
            } else {
                Some(arguments)
            },
            meta: None,
        };

        let result = client
            .request_prompt(params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get MCP prompt: {}", e))?;

        // Convert the messages to content string
        let content = extract_prompt_content(&result.messages);

        debug!(
            name = %self.name,
            prompt = %name,
            content_len = content.len(),
            "Got MCP prompt"
        );

        Ok(PromptResult {
            content,
            description: result.description,
        })
    }
}

/// Extract text content from MCP prompt messages
fn extract_prompt_content(messages: &[PromptMessage]) -> String {
    let mut content = String::new();

    for message in messages {
        // Extract text content from the content block
        if let ContentBlock::TextContent(text) = &message.content {
            if !content.is_empty() {
                content.push_str("\n\n");
            }
            content.push_str(&text.text);
        }
    }

    content
}

// ============================================================================
// Remote MCP Toolset (SSE and Streamable HTTP transports)
// ============================================================================

/// Transport type for remote MCP connections
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportType {
    /// Server-Sent Events transport
    Sse,
    /// Streamable HTTP transport (bidirectional)
    StreamableHttp,
}

impl TransportType {
    /// Parse transport type from string
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "sse" => Some(TransportType::Sse),
            "streamable" | "streamable-http" | "streamable_http" => {
                Some(TransportType::StreamableHttp)
            }
            _ => None,
        }
    }
}

/// Remote MCP Toolset that connects to an MCP server via HTTP (SSE or Streamable HTTP)
pub struct RemoteMcpToolset {
    name: String,
    url: String,
    transport_type: TransportType,
    headers: HashMap<String, String>,
    client: Arc<RwLock<Option<Arc<ClientRuntime>>>>,
    server_instructions: Arc<RwLock<Option<String>>>,
}

impl RemoteMcpToolset {
    /// Create a new remote MCP toolset
    pub fn new(
        name: String,
        url: String,
        transport_type: TransportType,
        headers: HashMap<String, String>,
    ) -> Self {
        debug!(
            name = %name,
            url = %url,
            transport = ?transport_type,
            "Creating remote MCP toolset"
        );

        Self {
            name,
            url,
            transport_type,
            headers,
            client: Arc::new(RwLock::new(None)),
            server_instructions: Arc::new(RwLock::new(None)),
        }
    }

    /// Create client initialization parameters
    fn create_client_details() -> InitializeRequestParams {
        InitializeRequestParams {
            capabilities: ClientCapabilities {
                elicitation: Some(ClientElicitation {
                    form: Some(serde_json::Map::new()),
                    url: Some(serde_json::Map::new()),
                }),
                ..Default::default()
            },
            client_info: Implementation {
                name: "cagent".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                title: Some("cagent MCP Client".into()),
                description: Some("cagent AI agent runner".into()),
                icons: vec![],
                website_url: Some("https://github.com/docker/cagent".into()),
            },
            protocol_version: LATEST_PROTOCOL_VERSION.into(),
            meta: None,
        }
    }
}

#[async_trait]
impl ToolSet for RemoteMcpToolset {
    async fn start(&self) -> Result<()> {
        // Check if already started
        {
            let client = self.client.read().await;
            if client.is_some() {
                debug!(name = %self.name, "Remote MCP toolset already started");
                return Ok(());
            }
        }

        info!(
            name = %self.name,
            url = %self.url,
            transport = ?self.transport_type,
            "Starting remote MCP toolset"
        );

        let client_details = Self::create_client_details();

        // Create the appropriate transport
        let handler = CagentClientHandler::new(&self.name);

        let client: Arc<ClientRuntime> = match self.transport_type {
            TransportType::Sse => {
                // Create SSE transport options with custom headers
                let options = ClientSseTransportOptions {
                    custom_headers: if self.headers.is_empty() {
                        None
                    } else {
                        Some(self.headers.clone())
                    },
                    ..Default::default()
                };
                let transport = ClientSseTransport::new(&self.url, options)
                    .map_err(|e| anyhow::anyhow!("Failed to create SSE transport: {}", e))?;

                client_runtime::create_client(McpClientOptions {
                    client_details,
                    transport,
                    handler: handler.to_mcp_client_handler(),
                    task_store: None,
                    server_task_store: None,
                })
            }
            TransportType::StreamableHttp => {
                // Create Streamable HTTP transport options with custom headers
                let options = StreamableTransportOptions {
                    mcp_url: self.url.clone(),
                    request_options: rust_mcp_sdk::RequestOptions {
                        custom_headers: if self.headers.is_empty() {
                            None
                        } else {
                            Some(self.headers.clone())
                        },
                        ..Default::default()
                    },
                };
                let transport = ClientStreamableTransport::new(&options, None, false)
                    .map_err(|e| {
                        anyhow::anyhow!("Failed to create Streamable HTTP transport: {}", e)
                    })?;

                client_runtime::create_client(McpClientOptions {
                    client_details,
                    transport,
                    handler: handler.to_mcp_client_handler(),
                    task_store: None,
                    server_task_store: None,
                })
            }
        };

        // Start the client
        client
            .clone()
            .start()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to start remote MCP client: {}", e))?;

        // Get server info
        if let Some(server_info) = client.server_info() {
            info!(
                name = %self.name,
                server_name = %server_info.server_info.name,
                server_version = %server_info.server_info.version,
                "Remote MCP toolset initialized"
            );

            // Store instructions if provided
            if let Some(ref instructions) = server_info.instructions {
                let mut guard = self.server_instructions.write().await;
                *guard = Some(instructions.clone());
            }
        }

        *self.client.write().await = Some(client);

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut client_guard = self.client.write().await;
        if let Some(client) = client_guard.take() {
            debug!(name = %self.name, "Stopping remote MCP toolset");
            if let Err(e) = client.shut_down().await {
                error!(name = %self.name, error = %e, "Error shutting down remote MCP client");
            }
        }
        Ok(())
    }

    async fn tools(&self) -> Result<Vec<Tool>> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("Remote MCP toolset not started - call start() first")?;

        debug!(name = %self.name, "Listing remote MCP tools");

        let params = PaginatedRequestParams {
            cursor: None,
            meta: None,
        };
        let result = client
            .request_tool_list(Some(params))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list remote MCP tools: {}", e))?;

        let tools: Vec<Tool> = result
            .tools
            .into_iter()
            .map(|mcp_tool| {
                // Prefix tool name with toolset name if set
                let name = if self.name.is_empty() {
                    mcp_tool.name.clone()
                } else {
                    format!("{}_{}", self.name, mcp_tool.name)
                };

                // Convert MCP tool schema to our format
                let parameters = serde_json::to_value(&mcp_tool.input_schema)
                    .unwrap_or_else(|_| serde_json::json!({"type": "object", "properties": {}}));

                Tool {
                    name,
                    category: Some("mcp".to_string()),
                    description: mcp_tool.description.unwrap_or_default(),
                    parameters,
                    annotations: ToolAnnotations {
                        read_only_hint: mcp_tool
                            .annotations
                            .as_ref()
                            .and_then(|a| a.read_only_hint)
                            .unwrap_or(false),
                        title: mcp_tool
                            .annotations
                            .as_ref()
                            .and_then(|a| a.title.clone()),
                    },
                    output_schema: mcp_tool
                        .output_schema
                        .and_then(|s| serde_json::to_value(s).ok()),
                }
            })
            .collect();

        debug!(name = %self.name, count = tools.len(), "Listed remote MCP tools");
        Ok(tools)
    }

    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolCallResult> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("Remote MCP toolset not started - call start() first")?;

        // Strip the toolset prefix from the tool name
        let prefix = format!("{}_", self.name);
        let tool_name = if !self.name.is_empty() && tool_call.function.name.starts_with(&self.name)
        {
            tool_call
                .function
                .name
                .strip_prefix(&prefix)
                .unwrap_or(&tool_call.function.name)
        } else {
            &tool_call.function.name
        };

        debug!(
            name = %self.name,
            tool = %tool_name,
            "Calling remote MCP tool"
        );

        // Parse arguments as serde_json::Map
        let arguments: Option<serde_json::Map<String, Value>> =
            if tool_call.function.arguments.is_empty() {
                None
            } else {
                serde_json::from_str(&tool_call.function.arguments).ok()
            };

        let params = CallToolRequestParams {
            name: tool_name.to_string(),
            arguments,
            meta: None,
            task: None,
        };

        let result = client
            .request_tool_call(params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to call remote MCP tool: {}", e))?;

        // Process the result content
        let mut output = String::new();
        for content in &result.content {
            // Extract text from the content block
            if let ContentBlock::TextContent(text_content) = content {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&text_content.text);
            }
        }

        if output.is_empty() {
            output = "no output".to_string();
        }

        debug!(
            name = %self.name,
            tool = %tool_name,
            is_error = result.is_error.unwrap_or(false),
            output_len = output.len(),
            "Remote MCP tool call completed"
        );

        if result.is_error.unwrap_or(false) {
            Ok(ToolCallResult::error(output))
        } else {
            Ok(ToolCallResult::success(output))
        }
    }

    fn instructions(&self) -> Option<String> {
        // We can't easily read from async RwLock in a sync context
        // For now, return None and rely on toolset instruction from config
        None
    }
}

impl std::fmt::Debug for RemoteMcpToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RemoteMcpToolset")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("transport_type", &self.transport_type)
            .finish()
    }
}

// Implement PromptProvider for RemoteMcpToolset
#[async_trait]
impl PromptProvider for RemoteMcpToolset {
    async fn list_prompts(&self) -> Result<Vec<PromptInfo>> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("Remote MCP toolset not started - call start() first")?;

        debug!(name = %self.name, "Listing remote MCP prompts");

        let params = PaginatedRequestParams {
            cursor: None,
            meta: None,
        };
        
        let result = client
            .request_prompt_list(Some(params))
            .await
            .map_err(|e| anyhow::anyhow!("Failed to list remote MCP prompts: {}", e))?;

        let prompts: Vec<PromptInfo> = result
            .prompts
            .into_iter()
            .map(|p| {
                let arguments = p
                    .arguments
                    .into_iter()
                    .map(|arg| PromptArgument {
                        name: arg.name,
                        description: arg.description.unwrap_or_default(),
                        required: arg.required.unwrap_or(false),
                    })
                    .collect();

                PromptInfo {
                    name: p.name,
                    description: p.description.unwrap_or_default(),
                    arguments,
                }
            })
            .collect();

        debug!(name = %self.name, count = prompts.len(), "Listed remote MCP prompts");
        Ok(prompts)
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptResult> {
        let client_guard = self.client.read().await;
        let client = client_guard
            .as_ref()
            .context("Remote MCP toolset not started - call start() first")?;

        debug!(name = %self.name, prompt = %name, "Getting remote MCP prompt");

        let params = GetPromptRequestParams {
            name: name.to_string(),
            arguments: if arguments.is_empty() {
                None
            } else {
                Some(arguments)
            },
            meta: None,
        };

        let result = client
            .request_prompt(params)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get remote MCP prompt: {}", e))?;

        let content = extract_prompt_content(&result.messages);

        debug!(
            name = %self.name,
            prompt = %name,
            content_len = content.len(),
            "Got remote MCP prompt"
        );

        Ok(PromptResult {
            content,
            description: result.description,
        })
    }
}

// ============================================================================
// Gateway MCP Toolset (Docker MCP Gateway integration)
// ============================================================================

use crate::catalog::{self, DOCKER_CATALOG_URL};
use crate::environment::EnvProvider;

/// Gateway MCP Toolset that connects to MCP servers via Docker's MCP Gateway
///
/// This toolset wraps Docker's MCP Gateway CLI to connect to MCP servers from
/// the Docker catalog (e.g., `docker:github-official`).
pub struct GatewayMcpToolset {
    name: String,
    mcp_server_name: String,
    config: Option<serde_json::Value>,
    env_provider: Arc<dyn EnvProvider>,
    cwd: Option<PathBuf>,
    inner: Arc<RwLock<Option<McpToolset>>>,
    cleanup_files: Arc<RwLock<Vec<PathBuf>>>,
}

impl GatewayMcpToolset {
    /// Create a new Gateway MCP toolset
    ///
    /// # Arguments
    /// * `name` - The toolset name for prefixing tools
    /// * `mcp_server_name` - The MCP server name from the Docker catalog (e.g., "github-official")
    /// * `config` - Optional configuration to pass to the MCP server
    /// * `env_provider` - Environment provider for resolving secrets
    /// * `cwd` - Working directory for the MCP Gateway process
    pub fn new(
        name: impl Into<String>,
        mcp_server_name: impl Into<String>,
        config: Option<serde_json::Value>,
        env_provider: Arc<dyn EnvProvider>,
        cwd: Option<PathBuf>,
    ) -> Self {
        let name = name.into();
        let mcp_server_name = mcp_server_name.into();

        debug!(
            name = %name,
            mcp_server = %mcp_server_name,
            "Creating Gateway MCP toolset"
        );

        Self {
            name,
            mcp_server_name,
            config,
            env_provider,
            cwd,
            inner: Arc::new(RwLock::new(None)),
            cleanup_files: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Parse a server reference like "docker:github-official" -> "github-official"
    pub fn parse_server_ref(ref_str: &str) -> &str {
        ref_str.strip_prefix("docker:").unwrap_or(ref_str)
    }

    /// Write secrets to a temporary file for the MCP Gateway
    async fn write_secrets_file(&self, secrets: &[catalog::Secret]) -> Result<PathBuf> {
        let mut secret_lines = Vec::new();

        for secret in secrets {
            let (value, found) = self.env_provider.get(&secret.name);
            if !found {
                return Err(anyhow::anyhow!(
                    "Missing environment variable '{}' required by MCP server '{}'",
                    secret.name,
                    self.mcp_server_name
                ));
            }

            if value.is_empty() {
                return Err(anyhow::anyhow!(
                    "Environment variable '{}' is empty (required by MCP server '{}')",
                    secret.name,
                    self.mcp_server_name
                ));
            }

            secret_lines.push(format!("{}={}", secret.name, value));
        }

        let content = secret_lines.join("\n");
        let temp_file = std::env::temp_dir().join(format!("mcp-secrets-{}.txt", uuid::Uuid::new_v4()));

        std::fs::write(&temp_file, content)?;

        // Track file for cleanup
        self.cleanup_files.write().await.push(temp_file.clone());

        Ok(temp_file)
    }

    /// Write config to a temporary file for the MCP Gateway
    async fn write_config_file(&self) -> Result<PathBuf> {
        let config_map = serde_json::json!({
            &self.mcp_server_name: self.config.clone().unwrap_or(serde_json::json!({}))
        });

        let content = serde_yaml::to_string(&config_map)?;
        let temp_file = std::env::temp_dir().join(format!("mcp-config-{}.yaml", uuid::Uuid::new_v4()));

        std::fs::write(&temp_file, content)?;

        // Track file for cleanup
        self.cleanup_files.write().await.push(temp_file.clone());

        Ok(temp_file)
    }

    /// Clean up temporary files
    async fn cleanup(&self) {
        let files = self.cleanup_files.write().await;
        for file in files.iter() {
            if let Err(e) = std::fs::remove_file(file) {
                debug!(file = ?file, error = %e, "Failed to remove temp file");
            }
        }
    }
}

#[async_trait]
impl ToolSet for GatewayMcpToolset {
    async fn start(&self) -> Result<()> {
        // Check if already started
        {
            let inner = self.inner.read().await;
            if inner.is_some() {
                debug!(name = %self.name, "Gateway MCP toolset already started");
                return Ok(());
            }
        }

        info!(
            name = %self.name,
            mcp_server = %self.mcp_server_name,
            "Starting Gateway MCP toolset"
        );

        // Get required secrets from the catalog
        let secrets = catalog::required_env_vars(&self.mcp_server_name).await?;

        // Write secrets to a temp file
        let secrets_file = self.write_secrets_file(&secrets).await?;

        // Write config to a temp file
        let config_file = self.write_config_file().await?;

        // Build the docker mcp gateway command
        let args = vec![
            "mcp".to_string(),
            "gateway".to_string(),
            "run".to_string(),
            "--servers".to_string(),
            self.mcp_server_name.clone(),
            "--catalog".to_string(),
            DOCKER_CATALOG_URL.to_string(),
            "--secrets".to_string(),
            secrets_file.to_string_lossy().to_string(),
            "--config".to_string(),
            config_file.to_string_lossy().to_string(),
        ];

        // Create the inner MCP toolset
        let inner_toolset = McpToolset::new(
            &self.name,
            "docker",
            args,
            HashMap::new(),
            self.cwd.clone(),
        );

        // Start the inner toolset
        inner_toolset.start().await?;

        *self.inner.write().await = Some(inner_toolset);

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut inner_guard = self.inner.write().await;
        if let Some(inner) = inner_guard.take() {
            debug!(name = %self.name, "Stopping Gateway MCP toolset");
            inner.stop().await?;
        }

        // Clean up temp files
        drop(inner_guard);
        self.cleanup().await;

        Ok(())
    }

    async fn tools(&self) -> Result<Vec<Tool>> {
        let inner_guard = self.inner.read().await;
        let inner = inner_guard
            .as_ref()
            .context("Gateway MCP toolset not started - call start() first")?;

        inner.tools().await
    }

    async fn execute(&self, tool_call: &ToolCall) -> Result<ToolCallResult> {
        let inner_guard = self.inner.read().await;
        let inner = inner_guard
            .as_ref()
            .context("Gateway MCP toolset not started - call start() first")?;

        inner.execute(tool_call).await
    }

    fn instructions(&self) -> Option<String> {
        // Return instructions from the inner toolset if available
        None
    }
}

impl std::fmt::Debug for GatewayMcpToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayMcpToolset")
            .field("name", &self.name)
            .field("mcp_server_name", &self.mcp_server_name)
            .finish()
    }
}

// Implement PromptProvider for GatewayMcpToolset
#[async_trait]
impl PromptProvider for GatewayMcpToolset {
    async fn list_prompts(&self) -> Result<Vec<PromptInfo>> {
        let inner_guard = self.inner.read().await;
        let inner = inner_guard
            .as_ref()
            .context("Gateway MCP toolset not started - call start() first")?;

        inner.list_prompts().await
    }

    async fn get_prompt(
        &self,
        name: &str,
        arguments: HashMap<String, String>,
    ) -> Result<PromptResult> {
        let inner_guard = self.inner.read().await;
        let inner = inner_guard
            .as_ref()
            .context("Gateway MCP toolset not started - call start() first")?;

        inner.get_prompt(name, arguments).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mcp_toolset_creation() {
        let toolset = McpToolset::new(
            "test",
            "echo",
            vec!["hello".to_string()],
            HashMap::new(),
            None,
        );
        assert_eq!(toolset.name, "test");
        assert_eq!(toolset.command, "echo");
    }

    #[test]
    fn test_remote_mcp_toolset_creation_sse() {
        let toolset = RemoteMcpToolset::new(
            "test".to_string(),
            "http://localhost:8080/mcp".to_string(),
            TransportType::Sse,
            HashMap::new(),
        );
        assert_eq!(toolset.name, "test");
        assert_eq!(toolset.url, "http://localhost:8080/mcp");
    }

    #[test]
    fn test_remote_mcp_toolset_creation_streamable() {
        let toolset = RemoteMcpToolset::new(
            "test".to_string(),
            "http://localhost:8080/mcp".to_string(),
            TransportType::StreamableHttp,
            HashMap::new(),
        );
        assert_eq!(toolset.name, "test");
    }

    #[test]
    fn test_parse_server_ref() {
        assert_eq!(GatewayMcpToolset::parse_server_ref("docker:github-official"), "github-official");
        assert_eq!(GatewayMcpToolset::parse_server_ref("github-official"), "github-official");
        assert_eq!(GatewayMcpToolset::parse_server_ref("docker:slack"), "slack");
    }
}
