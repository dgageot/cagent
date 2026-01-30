//! Tool definitions and handlers

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub mod a2a;
pub mod builtin;
pub mod deferred;
pub mod mcp;
pub mod tokenstore;

// Re-export commonly used items
pub use a2a::A2aToolset;
pub use mcp::{GatewayMcpToolset, McpToolset, RemoteMcpToolset, TransportType};
pub use tokenstore::{FileTokenStore, InMemoryTokenStore, OAuthToken, TokenStore};

/// Tool call from the model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type", default)]
    pub call_type: ToolType,
    pub function: FunctionCall,
}

/// Tool type
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    #[default]
    Function,
}

/// Function call details
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FunctionCall {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// Result of a tool execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallResult {
    pub output: String,
    #[serde(default)]
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl ToolCallResult {
    pub fn success(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: false,
            meta: None,
        }
    }

    pub fn error(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            is_error: true,
            meta: None,
        }
    }

    pub fn with_meta(mut self, meta: Value) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// Tool annotations for hints
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolAnnotations {
    #[serde(default)]
    pub read_only_hint: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// A tool definition
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tool {
    #[serde(default)]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Value,
    #[serde(default)]
    pub annotations: ToolAnnotations,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
}

/// Type alias for tool handler function
pub type ToolHandlerFn = Arc<
    dyn Fn(ToolCall) -> Pin<Box<dyn Future<Output = anyhow::Result<ToolCallResult>> + Send>>
        + Send
        + Sync,
>;

/// A tool with its handler
pub struct ToolWithHandler {
    pub tool: Tool,
    pub handler: ToolHandlerFn,
}

impl std::fmt::Debug for ToolWithHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolWithHandler")
            .field("tool", &self.tool)
            .finish()
    }
}

/// Trait for tool sets
#[async_trait::async_trait]
pub trait ToolSet: Send + Sync {
    /// Get all tools in this set
    async fn tools(&self) -> anyhow::Result<Vec<Tool>>;

    /// Execute a tool call
    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult>;

    /// Get optional instructions for this toolset
    fn instructions(&self) -> Option<String> {
        None
    }

    /// Start the toolset (for MCP servers, etc.)
    async fn start(&self) -> anyhow::Result<()> {
        Ok(())
    }

    /// Stop the toolset
    async fn stop(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Wrap a toolset and allow only a subset of tool names.
///
/// This implements the config field `toolsets[].tools: ["tool1", ...]`.
#[derive(Clone)]
pub struct FilteredToolSet {
    inner: Arc<dyn ToolSet>,
    allowed: std::collections::HashSet<String>,
}

impl std::fmt::Debug for FilteredToolSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilteredToolSet")
            .field("allowed", &self.allowed)
            .finish()
    }
}

impl FilteredToolSet {
    pub fn new(inner: Arc<dyn ToolSet>, allowed: impl IntoIterator<Item = String>) -> Self {
        Self {
            inner,
            allowed: allowed.into_iter().collect(),
        }
    }
}

#[async_trait::async_trait]
impl ToolSet for FilteredToolSet {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        let tools = self.inner.tools().await?;
        Ok(tools
            .into_iter()
            .filter(|t| self.allowed.contains(&t.name))
            .collect())
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let name = &tool_call.function.name;
        if !self.allowed.contains(name) {
            return Ok(ToolCallResult::error(format!(
                "Tool '{}' is not allowed by toolset filter",
                name
            )));
        }
        self.inner.execute(tool_call).await
    }

    fn instructions(&self) -> Option<String> {
        self.inner.instructions()
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start().await
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop().await
    }
}

/// Wrap a toolset and override its instruction message.
///
/// This implements the config field `toolsets[].instruction`.
#[derive(Clone)]
pub struct ToolSetWithInstruction {
    inner: Arc<dyn ToolSet>,
    instruction: String,
}

impl std::fmt::Debug for ToolSetWithInstruction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolSetWithInstruction").finish()
    }
}

impl ToolSetWithInstruction {
    pub fn new(inner: Arc<dyn ToolSet>, instruction: impl Into<String>) -> Self {
        Self {
            inner,
            instruction: instruction.into(),
        }
    }
}

#[async_trait::async_trait]
impl ToolSet for ToolSetWithInstruction {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        self.inner.tools().await
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        self.inner.execute(tool_call).await
    }

    fn instructions(&self) -> Option<String> {
        Some(self.instruction.clone())
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start().await
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop().await
    }
}

/// Create a JSON schema for a struct using schemars
#[macro_export]
macro_rules! tool_schema {
    ($t:ty) => {{
        let schema = schemars::schema_for!($t);
        serde_json::to_value(&schema).expect("Failed to serialize schema")
    }};
}

/// Type alias for a boxed ToolSet
pub type ToolSetBox = Box<dyn ToolSet>;

/// Constant for the description parameter name
pub const DESCRIPTION_PARAM: &str = "description";

/// Wrap a toolset to add a "description" parameter to all tools.
///
/// This allows the LLM to provide context about what it's doing with each tool call.
#[derive(Clone)]
pub struct DescriptionToolSet {
    inner: Arc<dyn ToolSet>,
}

impl std::fmt::Debug for DescriptionToolSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DescriptionToolSet").finish()
    }
}

impl DescriptionToolSet {
    pub fn new(inner: Arc<dyn ToolSet>) -> Self {
        Self { inner }
    }

    /// Add the description parameter to a tool's schema
    fn add_description_param(tool: Tool) -> Tool {
        let mut schema = match tool.parameters.as_object() {
            Some(obj) => obj.clone(),
            None => return tool,
        };

        let properties = schema
            .entry("properties")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut();

        if let Some(props) = properties {
            props.insert(
                DESCRIPTION_PARAM.to_string(),
                serde_json::json!({
                    "type": "string",
                    "description": "A brief, human-readable description of what this tool call is doing"
                }),
            );
        }

        Tool {
            parameters: serde_json::Value::Object(schema),
            ..tool
        }
    }
}

#[async_trait::async_trait]
impl ToolSet for DescriptionToolSet {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        let tools = self.inner.tools().await?;
        Ok(tools.into_iter().map(Self::add_description_param).collect())
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        self.inner.execute(tool_call).await
    }

    fn instructions(&self) -> Option<String> {
        self.inner.instructions()
    }

    async fn start(&self) -> anyhow::Result<()> {
        self.inner.start().await
    }

    async fn stop(&self) -> anyhow::Result<()> {
        self.inner.stop().await
    }
}

/// Extract the description from tool call arguments.
pub fn extract_description(arguments: &str) -> Option<String> {
    let args: std::collections::HashMap<String, serde_json::Value> =
        serde_json::from_str(arguments).ok()?;
    args.get(DESCRIPTION_PARAM)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}
