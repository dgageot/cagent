use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;

use crate::tools::{FilteredToolSet, Tool, ToolAnnotations, ToolCall, ToolCallResult, ToolSet};

#[derive(Debug)]
struct FakeToolSet;

#[async_trait]
impl ToolSet for FakeToolSet {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![
            Tool {
                name: "allowed".into(),
                category: None,
                description: String::new(),
                parameters: json!({}),
                annotations: ToolAnnotations::default(),
                output_schema: None,
            },
            Tool {
                name: "blocked".into(),
                category: None,
                description: String::new(),
                parameters: json!({}),
                annotations: ToolAnnotations::default(),
                output_schema: None,
            },
        ])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult::success(format!(
            "executed {}",
            tool_call.function.name
        )))
    }
}

#[tokio::test]
async fn filtered_toolset_filters_tools_list() {
    let inner: Arc<dyn ToolSet> = Arc::new(FakeToolSet);
    let filtered = FilteredToolSet::new(inner, vec!["allowed".to_string()]);

    let tools = filtered.tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "allowed");
}

#[tokio::test]
async fn filtered_toolset_blocks_execute() {
    let inner: Arc<dyn ToolSet> = Arc::new(FakeToolSet);
    let filtered = FilteredToolSet::new(inner, vec!["allowed".to_string()]);

    let blocked_call = ToolCall {
        id: "1".into(),
        call_type: crate::tools::ToolType::Function,
        function: crate::tools::FunctionCall {
            name: "blocked".into(),
            arguments: "{}".into(),
        },
    };

    let res = filtered.execute(&blocked_call).await.unwrap();
    assert!(res.is_error);
    assert!(res.output.contains("not allowed"));

    let allowed_call = ToolCall {
        id: "2".into(),
        call_type: crate::tools::ToolType::Function,
        function: crate::tools::FunctionCall {
            name: "allowed".into(),
            arguments: "{}".into(),
        },
    };

    let res = filtered.execute(&allowed_call).await.unwrap();
    assert!(!res.is_error);
    assert_eq!(res.output, "executed allowed");
}
