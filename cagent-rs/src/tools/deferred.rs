//! Deferred (lazy-loaded) tools implementation
//!
//! This module provides a toolset that wraps other toolsets and allows
//! tools to be discovered and activated on-demand, reducing the initial
//! tool count shown to the model.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::tools::{Tool, ToolCall, ToolCallResult, ToolSet, ToolSetBox};

/// Tool name for searching available deferred tools
pub const TOOL_NAME_SEARCH_TOOL: &str = "search_tool";
/// Tool name for activating a deferred tool
pub const TOOL_NAME_ADD_TOOL: &str = "add_tool";

/// Entry for a deferred tool
struct DeferredToolEntry {
    tool: Tool,
    #[allow(dead_code)]
    source_index: usize,
}

/// Source of deferred tools
struct DeferredSource {
    toolset: ToolSetBox,
    defer_all: bool,
    tool_names: Vec<String>,
}

/// A toolset that wraps other toolsets and provides lazy loading
///
/// This toolset exposes two tools:
/// - `search_tool`: Search for available deferred tools by name or description
/// - `add_tool`: Activate a deferred tool, making it available for use
pub struct DeferredToolset {
    /// Tools that are deferred (not yet activated)
    deferred_tools: Arc<RwLock<HashMap<String, DeferredToolEntry>>>,
    /// Tools that have been activated
    activated_tools: Arc<RwLock<HashMap<String, Tool>>>,
    /// Sources of deferred tools
    sources: Arc<RwLock<Vec<DeferredSource>>>,
    /// Whether the toolset has been started
    started: Arc<RwLock<bool>>,
}

impl Clone for DeferredToolset {
    fn clone(&self) -> Self {
        Self {
            deferred_tools: Arc::clone(&self.deferred_tools),
            activated_tools: Arc::clone(&self.activated_tools),
            sources: Arc::clone(&self.sources),
            started: Arc::clone(&self.started),
        }
    }
}

impl DeferredToolset {
    /// Create a new deferred toolset
    pub fn new() -> Self {
        Self {
            deferred_tools: Arc::new(RwLock::new(HashMap::new())),
            activated_tools: Arc::new(RwLock::new(HashMap::new())),
            sources: Arc::new(RwLock::new(Vec::new())),
            started: Arc::new(RwLock::new(false)),
        }
    }

    /// Add a source of deferred tools
    ///
    /// # Arguments
    /// * `toolset` - The toolset containing tools to defer
    /// * `defer_all` - If true, defer all tools from this source
    /// * `tool_names` - Specific tool names to defer (ignored if defer_all is true)
    pub async fn add_source(&self, toolset: ToolSetBox, defer_all: bool, tool_names: Vec<String>) {
        let mut sources = self.sources.write().await;
        sources.push(DeferredSource {
            toolset,
            defer_all,
            tool_names,
        });
    }

    /// Check if any sources have been added
    pub async fn has_sources(&self) -> bool {
        !self.sources.read().await.is_empty()
    }

    /// Initialize deferred tools from sources
    async fn start(&self) -> anyhow::Result<()> {
        {
            let started = self.started.read().await;
            if *started {
                return Ok(());
            }
        }

        let sources = self.sources.read().await;
        let mut deferred = self.deferred_tools.write().await;

        for (source_index, source) in sources.iter().enumerate() {
            let tools = source.toolset.tools().await?;

            for tool in tools {
                // Check if this tool should be deferred
                let should_defer = source.defer_all
                    || source.tool_names.iter().any(|n| n == &tool.name);

                if !should_defer {
                    continue;
                }

                // Don't override existing entries
                if !deferred.contains_key(&tool.name) {
                    deferred.insert(
                        tool.name.clone(),
                        DeferredToolEntry {
                            tool,
                            source_index,
                        },
                    );
                }
            }
        }

        *self.started.write().await = true;
        Ok(())
    }

    /// Handle search_tool call
    async fn handle_search_tool(&self, query: &str) -> ToolCallResult {
        let query_lower = query.to_lowercase();
        let deferred = self.deferred_tools.read().await;

        let results: Vec<SearchToolResult> = deferred
            .iter()
            .filter(|(name, entry): &(&String, &DeferredToolEntry)| {
                name.to_lowercase().contains(&query_lower)
                    || entry
                        .tool
                        .description
                        .to_lowercase()
                        .contains(&query_lower)
            })
            .map(|(name, entry): (&String, &DeferredToolEntry)| SearchToolResult {
                name: name.clone(),
                description: entry.tool.description.clone(),
            })
            .collect();

        if results.is_empty() {
            return ToolCallResult::error(format!(
                "No deferred tools found matching '{}'",
                query
            ));
        }

        let output = serde_json::to_string_pretty(&results)
            .unwrap_or_else(|_| format!("{:?}", results));

        ToolCallResult::success(format!(
            "Found {} deferred tool(s):\n{}",
            results.len(),
            output
        ))
    }

    /// Handle add_tool call
    async fn handle_add_tool(&self, name: &str) -> ToolCallResult {
        // Check if already activated
        {
            let activated = self.activated_tools.read().await;
            if activated.contains_key(name) {
                return ToolCallResult::success(format!("Tool '{}' is already active", name));
            }
        }

        // Try to activate the tool
        let mut deferred = self.deferred_tools.write().await;
        let entry = match deferred.remove(name) {
            Some(e) => e,
            None => {
                return ToolCallResult::error(format!("Tool '{}' not found.", name));
            }
        };

        let description = entry.tool.description.clone();
        self.activated_tools.write().await.insert(name.to_string(), entry.tool);

        ToolCallResult::success(format!(
            "Tool '{}' has been activated and is now available for use.\n\nDescription: {}",
            name, description
        ))
    }
}

impl Default for DeferredToolset {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SearchToolArgs {
    query: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AddToolArgs {
    name: String,
}

#[derive(Debug, Serialize)]
struct SearchToolResult {
    name: String,
    description: String,
}

#[async_trait]
impl ToolSet for DeferredToolset {
    fn instructions(&self) -> Option<String> {
        Some(
            r#"## Deferred Tool Loading

This agent has access to additional tools that can be discovered and loaded on-demand.

Use the search_tool to find available tools by name or description pattern.
When searching a tool, prefer to search by action keywords (e.g., "remote", "read", "write") rather than specific tool names.
Use single words to maximize matching results.

Use the add_tool to activate a discovered tool for use."#
                .to_string(),
        )
    }

    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        // Ensure we've started and loaded deferred tools
        self.start().await?;

        let mut result = vec![
            Tool {
                name: TOOL_NAME_SEARCH_TOOL.to_string(),
                description: "Search for available deferred tools by name or description. Use this to discover tools that can be activated.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Search query to find tools by name or description (case-insensitive)"
                        }
                    },
                    "required": ["query"]
                }),
                annotations: crate::tools::ToolAnnotations {
                    read_only_hint: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            Tool {
                name: TOOL_NAME_ADD_TOOL.to_string(),
                description: "Activate a deferred tool by name, making it available for use. Use search_tool first to find available tools.".to_string(),
                parameters: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "description": "The name of the tool to activate"
                        }
                    },
                    "required": ["name"]
                }),
                annotations: crate::tools::ToolAnnotations {
                    read_only_hint: true,
                    ..Default::default()
                },
                ..Default::default()
            },
        ];

        // Add activated tools
        let activated = self.activated_tools.read().await;
        for tool in activated.values() {
            result.push(tool.clone());
        }

        Ok(result)
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        match tool_call.function.name.as_str() {
            TOOL_NAME_SEARCH_TOOL => {
                let args: SearchToolArgs = serde_json::from_str(&tool_call.function.arguments)?;
                Ok(self.handle_search_tool(&args.query).await)
            }
            TOOL_NAME_ADD_TOOL => {
                let args: AddToolArgs = serde_json::from_str(&tool_call.function.arguments)?;
                Ok(self.handle_add_tool(&args.name).await)
            }
            _ => {
                // Check if it's an activated tool
                let activated = self.activated_tools.read().await;
                if activated.contains_key(&tool_call.function.name) {
                    drop(activated); // Release the read lock
                    // Find the source toolset that can execute this tool
                    let sources = self.sources.read().await;
                    for source in sources.iter() {
                        let tools: Vec<Tool> = source.toolset.tools().await?;
                        if tools.iter().any(|t| t.name == tool_call.function.name) {
                            return source.toolset.execute(tool_call).await;
                        }
                    }
                }
                Ok(ToolCallResult::error(format!(
                    "Tool '{}' not found",
                    tool_call.function.name
                )))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock toolset for testing
    struct MockToolset {
        tools: Vec<Tool>,
    }

    impl MockToolset {
        fn new(tools: Vec<Tool>) -> Self {
            Self { tools }
        }
    }

    #[async_trait]
    impl ToolSet for MockToolset {
        async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
            Ok(self.tools.clone())
        }

        async fn execute(&self, _tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
            Ok(ToolCallResult::success("executed"))
        }
    }

    #[tokio::test]
    async fn test_search_tool_no_results() {
        let deferred = DeferredToolset::new();
        let mock = MockToolset::new(vec![Tool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            ..Default::default()
        }]);
        deferred.add_source(Box::new(mock), true, vec![]).await;

        // Start to load tools
        deferred.start().await.unwrap();

        let result = deferred.handle_search_tool("nonexistent").await;
        assert!(result.is_error);
        assert!(result.output.contains("No deferred tools found"));
    }

    #[tokio::test]
    async fn test_search_tool_finds_by_name() {
        let deferred = DeferredToolset::new();
        let mock = MockToolset::new(vec![Tool {
            name: "file_reader".to_string(),
            description: "Reads files from disk".to_string(),
            ..Default::default()
        }]);
        deferred.add_source(Box::new(mock), true, vec![]).await;
        deferred.start().await.unwrap();

        let result = deferred.handle_search_tool("file").await;
        assert!(!result.is_error);
        assert!(result.output.contains("file_reader"));
    }

    #[tokio::test]
    async fn test_search_tool_finds_by_description() {
        let deferred = DeferredToolset::new();
        let mock = MockToolset::new(vec![Tool {
            name: "my_tool".to_string(),
            description: "Reads files from disk".to_string(),
            ..Default::default()
        }]);
        deferred.add_source(Box::new(mock), true, vec![]).await;
        deferred.start().await.unwrap();

        let result = deferred.handle_search_tool("disk").await;
        assert!(!result.is_error);
        assert!(result.output.contains("my_tool"));
    }

    #[tokio::test]
    async fn test_add_tool_not_found() {
        let deferred = DeferredToolset::new();
        deferred.start().await.unwrap();

        let result = deferred.handle_add_tool("nonexistent").await;
        assert!(result.is_error);
        assert!(result.output.contains("not found"));
    }

    #[tokio::test]
    async fn test_add_tool_success() {
        let deferred = DeferredToolset::new();
        let mock = MockToolset::new(vec![Tool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            ..Default::default()
        }]);
        deferred.add_source(Box::new(mock), true, vec![]).await;
        deferred.start().await.unwrap();

        let result = deferred.handle_add_tool("test_tool").await;
        assert!(!result.is_error);
        assert!(result.output.contains("has been activated"));

        // Tool should now be in activated list
        let tools = deferred.tools().await.unwrap();
        assert!(tools.iter().any(|t| t.name == "test_tool"));
    }

    #[tokio::test]
    async fn test_add_tool_already_active() {
        let deferred = DeferredToolset::new();
        let mock = MockToolset::new(vec![Tool {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            ..Default::default()
        }]);
        deferred.add_source(Box::new(mock), true, vec![]).await;
        deferred.start().await.unwrap();

        // Add once
        deferred.handle_add_tool("test_tool").await;
        // Add again
        let result = deferred.handle_add_tool("test_tool").await;
        assert!(!result.is_error);
        assert!(result.output.contains("already active"));
    }

    #[tokio::test]
    async fn test_specific_tool_deferral() {
        let deferred = DeferredToolset::new();
        let mock = MockToolset::new(vec![
            Tool {
                name: "tool_a".to_string(),
                description: "Tool A".to_string(),
                ..Default::default()
            },
            Tool {
                name: "tool_b".to_string(),
                description: "Tool B".to_string(),
                ..Default::default()
            },
        ]);
        // Only defer tool_a
        deferred.add_source(Box::new(mock), false, vec!["tool_a".to_string()]).await;
        deferred.start().await.unwrap();

        // Only tool_a should be in deferred
        let result_a = deferred.handle_search_tool("tool_a").await;
        assert!(!result_a.is_error);
        assert!(result_a.output.contains("tool_a"));

        let result_b = deferred.handle_search_tool("tool_b").await;
        assert!(result_b.output.contains("No deferred tools found"));
    }
}
