use crate::chat::Message;
use crate::config::Config;
use crate::tools::builtin::FilesystemToolset;
use crate::tools::ToolSet;

#[test]
fn test_config_parsing() {
    let yaml = r#"
version: "3"
agents:
  root:
    model: openai/gpt-4o
    description: Test agent
    toolsets:
      - type: filesystem
"#;
    let config = Config::from_yaml(yaml).unwrap();
    assert!(config.agents.contains_key("root"));
    assert_eq!(
        config.agents["root"].model,
        Some("openai/gpt-4o".to_string())
    );
}

#[test]
fn test_message_creation() {
    let msg = Message::system("You are helpful.");
    assert_eq!(msg.role, crate::chat::MessageRole::System);
    assert_eq!(msg.content, "You are helpful.");
}

#[tokio::test]
async fn test_filesystem_tools() {
    let toolset = FilesystemToolset::new("/tmp");
    let tools = toolset.tools().await.unwrap();

    assert!(tools.iter().any(|t| t.name == "read_file"));
    assert!(tools.iter().any(|t| t.name == "write_file"));
    assert!(tools.iter().any(|t| t.name == "directory_tree"));
}

#[test]
fn test_model_ref_parsing() {
    use crate::config::parse_model_ref;

    assert_eq!(parse_model_ref("openai/gpt-4o"), Some(("openai", "gpt-4o")));
    assert_eq!(
        parse_model_ref("anthropic/claude-3-opus"),
        Some(("anthropic", "claude-3-opus"))
    );
    assert_eq!(parse_model_ref("invalid"), None);
}

// Script toolset tests
#[cfg(test)]
mod script_toolset_test {
    use crate::config::ScriptShellToolConfig;
    use crate::tools::builtin::ScriptToolset;
    use crate::tools::{FunctionCall, ToolCall, ToolSet, ToolType};
    use std::collections::HashMap;

    #[tokio::test]
    async fn script_toolset_creates_tools_from_config() {
        let mut shell = HashMap::new();
        shell.insert(
            "greet".to_string(),
            ScriptShellToolConfig {
                cmd: "echo Hello $name".to_string(),
                description: Some("Greet someone".to_string()),
                args: {
                    let mut args = HashMap::new();
                    args.insert(
                        "name".to_string(),
                        serde_json::json!({"type": "string", "description": "Name to greet"}),
                    );
                    args
                },
                required: vec!["name".to_string()],
                env: HashMap::new(),
                working_dir: None,
            },
        );

        let toolset = ScriptToolset::new("/tmp", shell, HashMap::new()).unwrap();
        let tools = toolset.tools().await.unwrap();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "greet");
        assert_eq!(tools[0].description, "Greet someone");
    }

    #[tokio::test]
    async fn script_toolset_executes_command_with_args() {
        let mut shell = HashMap::new();
        shell.insert(
            "echo_name".to_string(),
            ScriptShellToolConfig {
                cmd: "echo $name".to_string(),
                description: None,
                args: {
                    let mut args = HashMap::new();
                    args.insert(
                        "name".to_string(),
                        serde_json::json!({"type": "string", "description": "The name"}),
                    );
                    args
                },
                required: vec!["name".to_string()],
                env: HashMap::new(),
                working_dir: None,
            },
        );

        let toolset = ScriptToolset::new("/tmp", shell, HashMap::new()).unwrap();
        let tool_call = ToolCall {
            id: "test".to_string(),
            call_type: ToolType::Function,
            function: FunctionCall {
                name: "echo_name".to_string(),
                arguments: r#"{"name": "World"}"#.to_string(),
            },
        };

        let result = toolset.execute(&tool_call).await.unwrap();
        assert!(!result.is_error);
        assert!(result.output.contains("World"));
    }

    #[test]
    fn script_toolset_validates_undefined_args() {
        let mut shell = HashMap::new();
        shell.insert(
            "bad_tool".to_string(),
            ScriptShellToolConfig {
                cmd: "echo $undefined_arg".to_string(),
                description: None,
                args: HashMap::new(),
                required: vec![],
                env: HashMap::new(),
                working_dir: None,
            },
        );

        let result = ScriptToolset::new("/tmp", shell, HashMap::new());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("undefined args"));
    }

    #[test]
    fn script_toolset_validates_required_args_defined() {
        let mut shell = HashMap::new();
        shell.insert(
            "bad_required".to_string(),
            ScriptShellToolConfig {
                cmd: "echo hello".to_string(),
                description: None,
                args: HashMap::new(),
                required: vec!["missing".to_string()],
                env: HashMap::new(),
                working_dir: None,
            },
        );

        let result = ScriptToolset::new("/tmp", shell, HashMap::new());
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("required arg 'missing'"));
    }
}
