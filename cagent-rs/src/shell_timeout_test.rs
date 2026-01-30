use std::path::PathBuf;

use crate::tools::builtin::ShellToolset;
use crate::tools::{FunctionCall, ToolCall, ToolSet, ToolType};

#[tokio::test]
async fn shell_tool_times_out() {
    // A short timeout should cancel a long-running command.
    let toolset = ShellToolset::new(PathBuf::from("."));

    let call = ToolCall {
        id: "1".into(),
        call_type: ToolType::Function,
        function: FunctionCall {
            name: "shell".into(),
            arguments: serde_json::json!({
                "cmd": "sleep 2",
                "timeout": 1
            })
            .to_string(),
        },
    };

    let res = toolset.execute(&call).await.unwrap();
    assert!(res.is_error);
    assert!(
        res.output.contains("Timeout"),
        "unexpected output: {}",
        res.output
    );
}
