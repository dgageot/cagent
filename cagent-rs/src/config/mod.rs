//! Configuration loading and parsing

pub mod source_loader;

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("Validation: {0}")]
    Validation(String),
    #[error("Agent '{0}' references unknown sub-agent '{1}'")]
    InvalidSubAgent(String, String),
    #[error("Model '{0}' not found")]
    ModelNotFound(String),
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,
    #[serde(default)]
    pub models: HashMap<String, ModelConfig>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    /// Metadata about the configuration
    #[serde(default)]
    pub metadata: Option<Metadata>,
}

/// Metadata about the configuration (author, license, etc.)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    /// Author name or organization
    #[serde(default)]
    pub author: Option<String>,
    /// License for the agent configuration
    #[serde(default)]
    pub license: Option<String>,
    /// Description of what this agent configuration does
    #[serde(default)]
    pub description: Option<String>,
    /// Path to README file
    #[serde(default)]
    pub readme: Option<String>,
    /// Version of this agent configuration
    #[serde(default, rename = "agent_version")]
    pub agent_version: Option<String>,
}

fn default_version() -> String {
    "3".to_string()
}

/// Structured output configuration for JSON schema responses
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StructuredOutput {
    /// Name of the response format
    pub name: String,
    /// Optional description of the response format
    #[serde(default)]
    pub description: Option<String>,
    /// JSON schema object defining the structure
    #[serde(default)]
    pub schema: serde_json::Value,
    /// Enable strict schema adherence (OpenAI only)
    #[serde(default)]
    pub strict: bool,
}

/// Hook configuration for pre/post tool execution and session lifecycle
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// Hooks run before tool execution
    #[serde(default)]
    pub pre_tool_use: Vec<HookMatcherConfig>,
    /// Hooks run after tool execution
    #[serde(default)]
    pub post_tool_use: Vec<HookMatcherConfig>,
    /// Hooks run when a session begins
    #[serde(default)]
    pub session_start: Vec<HookDefinition>,
    /// Hooks run when a session ends
    #[serde(default)]
    pub session_end: Vec<HookDefinition>,
}

impl HooksConfig {
    /// Returns true if no hooks are configured
    pub fn is_empty(&self) -> bool {
        self.pre_tool_use.is_empty()
            && self.post_tool_use.is_empty()
            && self.session_start.is_empty()
            && self.session_end.is_empty()
    }

    /// Returns true if session start hooks are configured
    pub fn has_session_start_hooks(&self) -> bool {
        !self.session_start.is_empty()
    }

    /// Returns true if session end hooks are configured
    pub fn has_session_end_hooks(&self) -> bool {
        !self.session_end.is_empty()
    }
}

/// Hook matcher for tool-related hooks
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookMatcherConfig {
    /// Regex pattern to match tool names (e.g., "shell|edit_file")
    /// Use "*" to match all tools. Case-sensitive.
    #[serde(default)]
    pub matcher: String,
    /// Hooks to execute when the matcher matches
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
}

/// Single hook definition
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HookDefinition {
    /// Hook type (currently only "command" is supported)
    #[serde(rename = "type")]
    pub hook_type: String,
    /// Shell command to execute
    #[serde(default)]
    pub command: Option<String>,
    /// Execution timeout in seconds (default: 60)
    #[serde(default)]
    pub timeout: Option<u64>,
}

/// Docker sandbox configuration for shell tools
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConfig {
    /// Docker image to use for the sandbox container
    /// Defaults to "alpine:latest" if not specified
    #[serde(default)]
    pub image: Option<String>,
    /// Paths to bind-mount into the container
    /// Each path can optionally have a ":ro" suffix for read-only access
    /// Default is read-write (:rw) if no suffix is specified
    /// Example: [".", "/tmp", "/config:ro"]
    #[serde(default)]
    pub paths: Vec<String>,
}

/// RAG (Retrieval-Augmented Generation) configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagConfig {
    /// Tool configuration for the RAG tool
    #[serde(default)]
    pub tool: Option<RagToolConfig>,
    /// Shared documents across all strategies
    #[serde(default)]
    pub docs: Vec<String>,
    /// Whether to respect VCS ignore files like .gitignore (default: true)
    #[serde(default)]
    pub respect_vcs: Option<bool>,
    /// Array of strategy configurations
    #[serde(default)]
    pub strategies: Vec<RagStrategyConfig>,
    /// Results configuration
    #[serde(default)]
    pub results: Option<RagResultsConfig>,
}

impl RagConfig {
    /// Returns whether VCS ignore files should be respected, defaulting to true
    pub fn get_respect_vcs(&self) -> bool {
        self.respect_vcs.unwrap_or(true)
    }
}

/// RAG tool configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagToolConfig {
    /// Custom name for the tool (defaults to RAG source name if empty)
    #[serde(default)]
    pub name: Option<String>,
    /// Tool description (what the tool does)
    #[serde(default)]
    pub description: Option<String>,
    /// Tool instruction (how to use the tool effectively)
    #[serde(default)]
    pub instruction: Option<String>,
}

/// RAG strategy configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagStrategyConfig {
    /// Strategy type: "chunked-embeddings", "bm25", etc.
    #[serde(rename = "type")]
    pub strategy_type: String,
    /// Strategy-specific documents (augments shared docs)
    #[serde(default)]
    pub docs: Vec<String>,
    /// Database configuration
    #[serde(default)]
    pub database: Option<RagDatabaseConfig>,
    /// Chunking configuration
    #[serde(default)]
    pub chunking: Option<RagChunkingConfig>,
    /// Max results from this strategy (for fusion input)
    #[serde(default)]
    pub limit: Option<usize>,
    /// Strategy-specific parameters (embedding_model, k1, b, threshold, etc.)
    #[serde(flatten)]
    pub params: HashMap<String, serde_json::Value>,
}

/// RAG database configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagDatabaseConfig {
    /// Database path
    #[serde(default)]
    pub path: Option<String>,
    /// Collection/table name
    #[serde(default)]
    pub collection: Option<String>,
}

/// RAG chunking configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagChunkingConfig {
    /// Chunk size in tokens
    #[serde(default)]
    pub chunk_size: Option<usize>,
    /// Overlap between chunks in tokens
    #[serde(default)]
    pub chunk_overlap: Option<usize>,
}

/// RAG results configuration
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RagResultsConfig {
    /// Max total results to return
    #[serde(default)]
    pub limit: Option<usize>,
    /// Fusion strategy: "rrf" (Reciprocal Rank Fusion), "simple", etc.
    #[serde(default)]
    pub fusion: Option<String>,
    /// Reranking model for result reranking
    #[serde(default)]
    pub reranker: Option<String>,
}

/// Routing rule for rule-based model selection
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoutingRule {
    /// Reference to another model in the models section or an inline model spec (e.g., "openai/gpt-4o")
    pub model: String,
    /// Example phrases that should trigger routing to this model
    #[serde(default)]
    pub examples: Vec<String>,
}

/// A named command (shortcut) for quick prompts
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Command {
    /// Description shown in completion dialogs
    #[serde(default)]
    pub description: Option<String>,
    /// The prompt instruction sent to the agent
    #[serde(default)]
    pub instruction: Option<String>,
}

impl Command {
    /// Get the display text for this command
    pub fn display_text(&self) -> &str {
        self.description
            .as_deref()
            .or(self.instruction.as_deref())
            .unwrap_or("")
    }

    /// Get the instruction to send to the agent
    pub fn get_instruction(&self) -> &str {
        self.instruction
            .as_deref()
            .or(self.description.as_deref())
            .unwrap_or("")
    }
}

/// Custom deserializer for commands that supports multiple formats:
/// - Map of simple strings: `{"cmd": "instruction"}`
/// - Map of objects: `{"cmd": {"description": "...", "instruction": "..."}}`
fn deserialize_commands<'de, D>(deserializer: D) -> Result<HashMap<String, Command>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{MapAccess, Visitor};
    use std::fmt;

    struct CommandsVisitor;

    impl<'de> Visitor<'de> for CommandsVisitor {
        type Value = HashMap<String, Command>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a map of command names to strings or objects")
        }

        fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut commands = HashMap::new();

            while let Some(key) = map.next_key::<String>()? {
                // Try to deserialize the value as either a string or a Command object
                let value: serde_yaml::Value = map.next_value()?;

                let command = match &value {
                    serde_yaml::Value::String(s) => Command {
                        description: None,
                        instruction: Some(s.clone()),
                    },
                    serde_yaml::Value::Mapping(_) => {
                        // Parse as a Command object
                        serde_yaml::from_value(value).map_err(serde::de::Error::custom)?
                    }
                    _ => {
                        return Err(serde::de::Error::custom(
                            "command value must be a string or object",
                        ));
                    }
                };

                commands.insert(key, command);
            }

            Ok(commands)
        }
    }

    deserializer.deserialize_map(CommandsVisitor)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(skip)]
    pub name: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub welcome_message: Option<String>,
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub sub_agents: Vec<String>,
    #[serde(default)]
    pub handoffs: Vec<String>,
    #[serde(default)]
    pub toolsets: Vec<ToolsetConfig>,
    #[serde(default)]
    pub add_date: bool,
    #[serde(default)]
    pub add_environment_info: bool,
    #[serde(default)]
    pub max_iterations: usize,
    #[serde(default)]
    pub num_history_items: usize,
    #[serde(default)]
    pub skills: bool,
    /// Additional prompt files to include as system messages
    #[serde(default)]
    pub add_prompt_files: Vec<String>,
    /// Add a "description" parameter to all tools so the LLM explains what it's doing
    #[serde(default)]
    pub add_description_parameter: bool,
    /// Enable code mode tools (code execution in sandboxed environment)
    #[serde(default)]
    pub code_mode_tools: bool,
    /// Named commands (shortcuts) for quick prompts
    /// Supports simple strings or objects with description/instruction
    #[serde(default, deserialize_with = "deserialize_commands")]
    pub commands: HashMap<String, Command>,
    /// Structured output configuration for JSON schema responses
    #[serde(default)]
    pub structured_output: Option<StructuredOutput>,
    /// Hook configuration for pre/post tool execution and session lifecycle
    #[serde(default)]
    pub hooks: Option<HooksConfig>,
    /// RAG (Retrieval-Augmented Generation) configuration
    #[serde(default)]
    pub rag: Option<RagConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsetConfig {
    #[serde(rename = "type")]
    pub toolset_type: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub instruction: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default, rename = "ref")]
    pub reference: Option<String>,
    #[serde(default)]
    pub remote: Option<RemoteConfig>,
    /// For the `script` toolset: custom shell tool definitions
    #[serde(default)]
    pub shell: HashMap<String, ScriptShellToolConfig>,
    /// For the `api` toolset: API tool configuration
    #[serde(default)]
    pub api_config: Option<ApiToolConfig>,
    /// For the `filesystem` toolset: commands to run after file edits
    #[serde(default)]
    pub post_edit: Vec<PostEditConfig>,
    /// For the `filesystem` toolset: whether to respect .gitignore (default: true)
    #[serde(default = "default_ignore_vcs")]
    pub ignore_vcs: bool,
    /// For the `todo` toolset: whether to share todos across all agents
    #[serde(default)]
    pub shared: bool,
    /// Defer loading of tools (lazy-load)
    /// Can be `true` to defer all tools or a list of specific tool names
    #[serde(default, deserialize_with = "deserialize_defer")]
    pub defer: DeferConfig,
    /// Docker sandbox configuration for shell tools
    #[serde(default)]
    pub sandbox: Option<SandboxConfig>,
    /// Tool "toon" (persona) - regex patterns for tools whose JSON output should be cartoon-encoded
    /// Comma-separated patterns, e.g., "fetch,search_*"
    #[serde(default)]
    pub toon: Option<String>,
    /// MCP config passthrough - arbitrary config passed to MCP servers
    #[serde(default, rename = "config")]
    pub mcp_config: Option<serde_json::Value>,
}

/// Configuration for a post-edit command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostEditConfig {
    /// File path pattern to match (supports glob patterns)
    pub path: String,
    /// Command to run after the file is edited
    /// The environment variable $path is set to the edited file path
    pub cmd: String,
}

/// Configuration for a custom shell script tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptShellToolConfig {
    /// The shell command to execute (can use $VAR for argument substitution)
    pub cmd: String,
    /// Description of what this tool does
    #[serde(default)]
    pub description: Option<String>,
    /// Arguments schema: map of arg_name -> { "type": "string", "description": "..." }
    #[serde(default)]
    pub args: HashMap<String, serde_json::Value>,
    /// List of required argument names
    #[serde(default)]
    pub required: Vec<String>,
    /// Environment variables to set when running the command
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Working directory for the command
    #[serde(default)]
    pub working_dir: Option<String>,
}

/// Configuration for an API tool
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiToolConfig {
    /// The name of the tool
    pub name: String,
    /// Description/instruction for the tool
    #[serde(default)]
    pub instruction: Option<String>,
    /// HTTP endpoint URL (can contain template variables for GET)
    pub endpoint: String,
    /// HTTP method (GET or POST)
    #[serde(default = "default_api_method")]
    pub method: String,
    /// Request headers
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// Arguments schema: map of arg_name -> { "type": "string", "description": "..." }
    #[serde(default)]
    pub args: HashMap<String, serde_json::Value>,
    /// List of required argument names
    #[serde(default)]
    pub required: Vec<String>,
    /// Optional output schema for the response
    #[serde(default)]
    pub output_schema: Option<serde_json::Value>,
}

fn default_api_method() -> String {
    "GET".to_string()
}

fn default_ignore_vcs() -> bool {
    true
}

/// Defer configuration for lazy-loading tools
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeferConfig {
    /// True when all tools should be deferred
    #[serde(default)]
    pub defer_all: bool,
    /// List of specific tool names to defer (empty if defer_all is true)
    #[serde(default)]
    pub tools: Vec<String>,
}

impl DeferConfig {
    /// Create a config that defers all tools
    pub fn all() -> Self {
        Self {
            defer_all: true,
            tools: Vec::new(),
        }
    }

    /// Create a config that defers specific tools
    pub fn specific(tools: Vec<String>) -> Self {
        Self {
            defer_all: false,
            tools,
        }
    }

    /// Check if this config is empty (no deferral)
    pub fn is_empty(&self) -> bool {
        !self.defer_all && self.tools.is_empty()
    }

    /// Check if a specific tool should be deferred
    pub fn should_defer(&self, tool_name: &str) -> bool {
        self.defer_all || self.tools.iter().any(|t| t == tool_name)
    }
}

/// Custom deserializer for defer config that supports:
/// - `true` to defer all tools
/// - `["tool1", "tool2"]` to defer specific tools
fn deserialize_defer<'de, D>(deserializer: D) -> Result<DeferConfig, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct DeferVisitor;

    impl<'de> Visitor<'de> for DeferVisitor {
        type Value = DeferConfig;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a boolean or array of strings")
        }

        fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value {
                Ok(DeferConfig::all())
            } else {
                Ok(DeferConfig::default())
            }
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut tools = Vec::new();
            while let Some(tool) = seq.next_element::<String>()? {
                tools.push(tool);
            }
            Ok(DeferConfig::specific(tools))
        }
    }

    deserializer.deserialize_any(DeferVisitor)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteConfig {
    pub url: String,
    #[serde(default)]
    pub transport_type: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(skip)]
    pub name: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    #[serde(default)]
    pub presence_penalty: Option<f32>,
    /// Token budget for reasoning/thinking (for models that support it)
    /// Set to 0 to disable thinking. Providers will use their own defaults if not set.
    #[serde(default)]
    pub thinking_budget: Option<u32>,
    /// Override the base URL for the API (for custom endpoints)
    #[serde(default)]
    pub base_url: Option<String>,
    /// Environment variable name for the API token (defaults to provider-specific key)
    #[serde(default)]
    pub token_key: Option<String>,
    /// Whether to track token usage for this model
    #[serde(default)]
    pub track_usage: Option<bool>,
    /// Provider-specific options that aren't covered by the standard fields.
    /// Examples:
    /// - DMR: `runtime_flags`, `speculative_draft_model`, `speculative_num_tokens`
    /// - Others: `api_type` to override the API compatibility layer
    #[serde(default)]
    pub provider_opts: HashMap<String, serde_json::Value>,
    /// Routing rules for rule-based model selection
    /// Each rule maps example phrases to a target model
    #[serde(default)]
    pub routing: Vec<RoutingRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub base_url: String,
    #[serde(default)]
    pub api_type: Option<String>,
    #[serde(default)]
    pub token_key: Option<String>,
}

impl Config {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        Self::from_yaml(&std::fs::read_to_string(path)?)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, ConfigError> {
        let mut config: Config = serde_yaml::from_str(yaml)?;

        // Populate names from map keys
        for (name, agent) in &mut config.agents {
            agent.name = name.clone();
        }
        for (name, model) in &mut config.models {
            model.name = name.clone();
        }

        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        for (name, agent) in &self.agents {
            // Check all agent references exist
            for ref_name in agent.sub_agents.iter().chain(&agent.handoffs) {
                if !self.agents.contains_key(ref_name) {
                    return Err(ConfigError::InvalidSubAgent(name.clone(), ref_name.clone()));
                }
            }
        }
        Ok(())
    }

    /// Create a minimal config with a single agent
    pub fn default_agent() -> Self {
        let mut config = Self::default();
        config.agents.insert(
            "root".to_string(),
            AgentConfig {
                name: "root".to_string(),
                model: Some("openai/gpt-4o".to_string()),
                description: Some("A helpful AI assistant".to_string()),
                instruction: Some("You are a helpful AI assistant.".to_string()),
                add_date: true,
                add_environment_info: true,
                ..Default::default()
            },
        );
        config
    }
}

/// Parse "provider/model" into (provider, model)
pub fn parse_model_ref(model_ref: &str) -> Option<(&str, &str)> {
    model_ref.split_once('/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    toolsets:
      - type: filesystem
"#;
        let config = Config::from_yaml(yaml).unwrap();
        assert!(config.agents.contains_key("root"));
        assert_eq!(config.agents["root"].toolsets.len(), 1);
    }

    #[test]
    fn test_parse_model_ref() {
        assert_eq!(parse_model_ref("openai/gpt-4o"), Some(("openai", "gpt-4o")));
        assert_eq!(parse_model_ref("invalid"), None);
    }

    #[test]
    fn test_invalid_sub_agent() {
        let yaml = r#"
agents:
  root:
    sub_agents: [missing]
"#;
        assert!(matches!(
            Config::from_yaml(yaml),
            Err(ConfigError::InvalidSubAgent(_, _))
        ));
    }

    #[test]
    fn test_metadata_parsing() {
        let yaml = r#"
metadata:
  author: "Test Author"
  license: "MIT"
  description: "A test agent"
  readme: "README.md"
  agent_version: "1.0.0"
agents:
  root:
    model: openai/gpt-4o
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let meta = config.metadata.expect("metadata should be present");
        assert_eq!(meta.author.as_deref(), Some("Test Author"));
        assert_eq!(meta.license.as_deref(), Some("MIT"));
        assert_eq!(meta.description.as_deref(), Some("A test agent"));
        assert_eq!(meta.readme.as_deref(), Some("README.md"));
        assert_eq!(meta.agent_version.as_deref(), Some("1.0.0"));
    }

    #[test]
    fn test_model_config_with_provider_opts() {
        let yaml = r#"
agents:
  root:
    model: my_model
models:
  my_model:
    provider: dmr
    model: ai/llama3.2
    base_url: "http://localhost:8080"
    token_key: MY_CUSTOM_KEY
    track_usage: true
    provider_opts:
      runtime_flags:
        - "-c"
        - "2G"
      speculative_draft_model: "ai/draft-model"
      speculative_num_tokens: 5
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let model = &config.models["my_model"];

        assert_eq!(model.provider, "dmr");
        assert_eq!(model.model, "ai/llama3.2");
        assert_eq!(model.base_url.as_deref(), Some("http://localhost:8080"));
        assert_eq!(model.token_key.as_deref(), Some("MY_CUSTOM_KEY"));
        assert_eq!(model.track_usage, Some(true));

        // Check provider_opts
        assert!(model.provider_opts.contains_key("runtime_flags"));
        assert!(model.provider_opts.contains_key("speculative_draft_model"));
        assert!(model.provider_opts.contains_key("speculative_num_tokens"));

        // Verify runtime_flags is an array
        let flags = model.provider_opts.get("runtime_flags").unwrap();
        assert!(flags.is_array());
        let flags_arr = flags.as_array().unwrap();
        assert_eq!(flags_arr.len(), 2);
        assert_eq!(flags_arr[0].as_str(), Some("-c"));
        assert_eq!(flags_arr[1].as_str(), Some("2G"));

        // Verify speculative_num_tokens is a number
        let num_tokens = model.provider_opts.get("speculative_num_tokens").unwrap();
        assert_eq!(num_tokens.as_i64(), Some(5));
    }

    #[test]
    fn test_commands_parsing_simple() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    commands:
      df: "check disk space"
      ls: "list files in current directory"
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];

        assert_eq!(agent.commands.len(), 2);
        assert_eq!(
            agent.commands["df"].get_instruction(),
            "check disk space"
        );
        assert_eq!(
            agent.commands["ls"].get_instruction(),
            "list files in current directory"
        );
    }

    #[test]
    fn test_commands_parsing_advanced() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    commands:
      fix-lint:
        description: "Fix linting errors"
        instruction: "Fix the lint issues in the codebase"
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];

        let cmd = &agent.commands["fix-lint"];
        assert_eq!(cmd.description.as_deref(), Some("Fix linting errors"));
        assert_eq!(
            cmd.instruction.as_deref(),
            Some("Fix the lint issues in the codebase")
        );
        assert_eq!(cmd.display_text(), "Fix linting errors");
    }

    #[test]
    fn test_commands_mixed_format() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    commands:
      simple: "A simple command"
      advanced:
        description: "An advanced command"
        instruction: "Do something complex"
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];

        assert_eq!(agent.commands.len(), 2);
        assert_eq!(
            agent.commands["simple"].get_instruction(),
            "A simple command"
        );
        assert_eq!(
            agent.commands["advanced"].get_instruction(),
            "Do something complex"
        );
        assert_eq!(
            agent.commands["advanced"].display_text(),
            "An advanced command"
        );
    }

    #[test]
    fn test_structured_output_parsing() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    structured_output:
      name: person
      description: A person object
      strict: true
      schema:
        type: object
        properties:
          name:
            type: string
          age:
            type: integer
        required:
          - name
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];

        let so = agent.structured_output.as_ref().expect("should have structured_output");
        assert_eq!(so.name, "person");
        assert_eq!(so.description.as_deref(), Some("A person object"));
        assert!(so.strict);

        // Check the schema
        assert_eq!(so.schema.get("type").and_then(|v| v.as_str()), Some("object"));
        let props = so.schema.get("properties").expect("should have properties");
        assert!(props.get("name").is_some());
        assert!(props.get("age").is_some());
    }

    #[test]
    fn test_defer_all_tools() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    toolsets:
      - type: filesystem
        defer: true
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];
        let toolset = &agent.toolsets[0];

        assert!(toolset.defer.defer_all);
        assert!(toolset.defer.should_defer("read_file"));
        assert!(toolset.defer.should_defer("any_tool"));
    }

    #[test]
    fn test_defer_specific_tools() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    toolsets:
      - type: mcp
        command: python
        defer:
          - expensive_tool
          - slow_tool
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];
        let toolset = &agent.toolsets[0];

        assert!(!toolset.defer.defer_all);
        assert_eq!(toolset.defer.tools.len(), 2);
        assert!(toolset.defer.should_defer("expensive_tool"));
        assert!(toolset.defer.should_defer("slow_tool"));
        assert!(!toolset.defer.should_defer("other_tool"));
    }

    #[test]
    fn test_defer_empty() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    toolsets:
      - type: filesystem
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];
        let toolset = &agent.toolsets[0];

        assert!(toolset.defer.is_empty());
        assert!(!toolset.defer.should_defer("read_file"));
    }

    #[test]
    fn test_hooks_config_parsing() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    hooks:
      pre_tool_use:
        - matcher: "shell|edit_file"
          hooks:
            - type: command
              command: "echo 'About to execute tool'"
              timeout: 30
      post_tool_use:
        - matcher: "*"
          hooks:
            - type: command
              command: "echo 'Tool executed'"
      session_start:
        - type: command
          command: "echo 'Session started'"
      session_end:
        - type: command
          command: "echo 'Session ended'"
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];

        let hooks = agent.hooks.as_ref().expect("should have hooks");
        assert!(!hooks.is_empty());

        // Check pre_tool_use
        assert_eq!(hooks.pre_tool_use.len(), 1);
        assert_eq!(hooks.pre_tool_use[0].matcher, "shell|edit_file");
        assert_eq!(hooks.pre_tool_use[0].hooks.len(), 1);
        assert_eq!(hooks.pre_tool_use[0].hooks[0].hook_type, "command");
        assert_eq!(hooks.pre_tool_use[0].hooks[0].timeout, Some(30));

        // Check post_tool_use
        assert_eq!(hooks.post_tool_use.len(), 1);
        assert_eq!(hooks.post_tool_use[0].matcher, "*");

        // Check session hooks
        assert_eq!(hooks.session_start.len(), 1);
        assert_eq!(hooks.session_end.len(), 1);
    }

    #[test]
    fn test_sandbox_config_parsing() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    toolsets:
      - type: shell
        sandbox:
          image: "python:3.11-slim"
          paths:
            - "."
            - "/tmp"
            - "/config:ro"
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];
        let toolset = &agent.toolsets[0];

        let sandbox = toolset.sandbox.as_ref().expect("should have sandbox");
        assert_eq!(sandbox.image.as_deref(), Some("python:3.11-slim"));
        assert_eq!(sandbox.paths.len(), 3);
        assert_eq!(sandbox.paths[0], ".");
        assert_eq!(sandbox.paths[1], "/tmp");
        assert_eq!(sandbox.paths[2], "/config:ro");
    }

    #[test]
    fn test_rag_config_parsing() {
        let yaml = r#"
agents:
  root:
    model: openai/gpt-4o
    rag:
      tool:
        name: search_docs
        description: Search documentation
      docs:
        - "./docs/**/*.md"
        - "./README.md"
      respect_vcs: true
      strategies:
        - type: chunked-embeddings
          embedding_model: text-embedding-3-small
          database:
            path: "./rag.db"
            collection: docs
          chunking:
            chunk_size: 512
            chunk_overlap: 50
          limit: 10
        - type: bm25
          k1: 1.2
          b: 0.75
          limit: 10
      results:
        limit: 5
        fusion: rrf
        reranker: cross-encoder/ms-marco-MiniLM-L-6-v2
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let agent = &config.agents["root"];

        let rag = agent.rag.as_ref().expect("should have rag config");

        // Check tool config
        let tool = rag.tool.as_ref().expect("should have tool config");
        assert_eq!(tool.name.as_deref(), Some("search_docs"));
        assert_eq!(tool.description.as_deref(), Some("Search documentation"));

        // Check docs
        assert_eq!(rag.docs.len(), 2);
        assert_eq!(rag.docs[0], "./docs/**/*.md");

        // Check respect_vcs
        assert!(rag.get_respect_vcs());

        // Check strategies
        assert_eq!(rag.strategies.len(), 2);
        assert_eq!(rag.strategies[0].strategy_type, "chunked-embeddings");
        assert_eq!(rag.strategies[1].strategy_type, "bm25");

        // Check chunking on first strategy
        let chunking = rag.strategies[0].chunking.as_ref().expect("should have chunking");
        assert_eq!(chunking.chunk_size, Some(512));
        assert_eq!(chunking.chunk_overlap, Some(50));

        // Check results config
        let results = rag.results.as_ref().expect("should have results");
        assert_eq!(results.limit, Some(5));
        assert_eq!(results.fusion.as_deref(), Some("rrf"));
    }

    #[test]
    fn test_routing_config_parsing() {
        let yaml = r#"
models:
  router:
    provider: openai
    model: gpt-4o
    routing:
      - model: openai/gpt-4o-mini
        examples:
          - "what is 2+2"
          - "simple math"
      - model: anthropic/claude-sonnet-4-0
        examples:
          - "write a complex essay"
          - "analyze this document"
agents:
  root:
    model: router
"#;
        let config = Config::from_yaml(yaml).unwrap();
        let model = &config.models["router"];

        assert_eq!(model.routing.len(), 2);
        assert_eq!(model.routing[0].model, "openai/gpt-4o-mini");
        assert_eq!(model.routing[0].examples.len(), 2);
        assert_eq!(model.routing[0].examples[0], "what is 2+2");

        assert_eq!(model.routing[1].model, "anthropic/claude-sonnet-4-0");
        assert_eq!(model.routing[1].examples.len(), 2);
    }
}
