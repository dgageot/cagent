use std::sync::Arc;

use async_trait::async_trait;

use crate::tools::{ToolCall, ToolCallResult, ToolSet, ToolSetWithInstruction};

#[derive(Debug)]
struct FakeToolSet;

#[async_trait]
impl ToolSet for FakeToolSet {
    async fn tools(&self) -> anyhow::Result<Vec<crate::tools::Tool>> {
        Ok(vec![])
    }

    async fn execute(&self, _tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult::success("ok"))
    }

    fn instructions(&self) -> Option<String> {
        Some("original".to_string())
    }
}

#[tokio::test]
async fn toolset_with_instruction_overrides_instructions() {
    let inner: Arc<dyn ToolSet> = Arc::new(FakeToolSet);
    let wrapped = ToolSetWithInstruction::new(inner, "override");

    assert_eq!(wrapped.instructions().as_deref(), Some("override"));
}
