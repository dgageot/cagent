//! Lifecycle hooks for agent tool execution
//!
//! Hooks allow running shell commands at various points during the agent's
//! execution lifecycle, providing deterministic control over agent behavior.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, warn};

use crate::config::{HookDefinition, HookMatcherConfig, HooksConfig};

/// Result tuple from executing a single hook
type HookExecResult = (Option<HookOutput>, String, String, i32, Option<String>);

/// Event type for hooks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    PreToolUse,
    PostToolUse,
    SessionStart,
    SessionEnd,
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EventType::PreToolUse => write!(f, "pre_tool_use"),
            EventType::PostToolUse => write!(f, "post_tool_use"),
            EventType::SessionStart => write!(f, "session_start"),
            EventType::SessionEnd => write!(f, "session_end"),
        }
    }
}

/// Input passed to hooks via stdin as JSON
#[derive(Debug, Clone, Serialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: String,
    pub hook_event_name: EventType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<HashMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_response: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Output from a hook command (parsed from JSON stdout)
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookOutput {
    /// Whether to continue execution (default: true)
    #[serde(rename = "continue")]
    pub continue_execution: Option<bool>,
    /// Message when continue=false
    pub stop_reason: Option<String>,
    /// Hide stdout from transcript
    #[serde(default)]
    pub suppress_output: bool,
    /// Warning to show the user
    pub system_message: Option<String>,
    /// Block decision
    pub decision: Option<String>,
    /// Reason for decision
    pub reason: Option<String>,
    /// Hook-specific output
    pub hook_specific_output: Option<HookSpecificOutput>,
}

impl HookOutput {
    /// Returns whether execution should continue
    pub fn should_continue(&self) -> bool {
        self.continue_execution.unwrap_or(true)
    }

    /// Returns true if the decision is to block
    pub fn is_blocked(&self) -> bool {
        self.decision.as_deref() == Some("block")
    }
}

/// Hook-specific output fields
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HookSpecificOutput {
    pub hook_event_name: Option<String>,
    pub permission_decision: Option<String>,
    pub permission_decision_reason: Option<String>,
    pub updated_input: Option<HashMap<String, serde_json::Value>>,
    pub additional_context: Option<String>,
}

/// Result of executing hooks
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// Whether the operation should proceed
    pub allowed: bool,
    /// Feedback to include in the response
    pub message: Option<String>,
    /// Modifications to tool input (PreToolUse only)
    pub modified_input: Option<HashMap<String, serde_json::Value>>,
    /// Context to add (PostToolUse/SessionStart)
    pub additional_context: Option<String>,
    /// Warning to show the user
    pub system_message: Option<String>,
    /// Exit code from the hook command
    pub exit_code: i32,
    /// Error output from the hook
    pub stderr: Option<String>,
}

impl HookResult {
    /// Create a result indicating the operation is allowed
    pub fn allowed() -> Self {
        Self {
            allowed: true,
            ..Default::default()
        }
    }

    /// Create a result indicating the operation is blocked
    pub fn blocked(message: impl Into<String>) -> Self {
        Self {
            allowed: false,
            message: Some(message.into()),
            ..Default::default()
        }
    }
}

/// Compiled hook matcher with regex pattern
struct CompiledMatcher {
    config: HookMatcherConfig,
    pattern: Option<Regex>,
}

impl CompiledMatcher {
    fn new(config: HookMatcherConfig) -> Self {
        let pattern = if config.matcher.is_empty() || config.matcher == "*" {
            None // Matches all
        } else {
            match Regex::new(&format!("^(?:{})$", config.matcher)) {
                Ok(p) => Some(p),
                Err(e) => {
                    warn!("Invalid hook matcher pattern '{}': {}", config.matcher, e);
                    None
                }
            }
        };
        Self { config, pattern }
    }

    fn matches(&self, tool_name: &str) -> bool {
        if self.config.matcher.is_empty() || self.config.matcher == "*" {
            return true;
        }
        match &self.pattern {
            Some(p) => p.is_match(tool_name),
            None => false,
        }
    }
}

/// Hook executor that runs hooks at various lifecycle points
pub struct Executor {
    config: HooksConfig,
    working_dir: String,
    env: Vec<(String, String)>,
    pre_tool_use_matchers: Vec<CompiledMatcher>,
    post_tool_use_matchers: Vec<CompiledMatcher>,
}

impl Executor {
    /// Create a new hook executor
    pub fn new(config: HooksConfig, working_dir: impl Into<String>) -> Self {
        let working_dir = working_dir.into();
        let env: Vec<(String, String)> = std::env::vars().collect();

        let pre_tool_use_matchers = config
            .pre_tool_use
            .iter()
            .cloned()
            .map(CompiledMatcher::new)
            .collect();
        let post_tool_use_matchers = config
            .post_tool_use
            .iter()
            .cloned()
            .map(CompiledMatcher::new)
            .collect();

        Self {
            config,
            working_dir,
            env,
            pre_tool_use_matchers,
            post_tool_use_matchers,
        }
    }

    /// Execute pre-tool-use hooks for a tool
    pub async fn execute_pre_tool_use(&self, input: &mut HookInput) -> HookResult {
        if self.pre_tool_use_matchers.is_empty() {
            return HookResult::allowed();
        }

        input.hook_event_name = EventType::PreToolUse;

        let tool_name = input.tool_name.as_deref().unwrap_or("");
        let hooks: Vec<&HookDefinition> = self
            .pre_tool_use_matchers
            .iter()
            .filter(|m| m.matches(tool_name))
            .flat_map(|m| &m.config.hooks)
            .collect();

        if hooks.is_empty() {
            return HookResult::allowed();
        }

        self.execute_hooks(&hooks, input, EventType::PreToolUse).await
    }

    /// Execute post-tool-use hooks for a tool
    pub async fn execute_post_tool_use(&self, input: &mut HookInput) -> HookResult {
        if self.post_tool_use_matchers.is_empty() {
            return HookResult::allowed();
        }

        input.hook_event_name = EventType::PostToolUse;

        let tool_name = input.tool_name.as_deref().unwrap_or("");
        let hooks: Vec<&HookDefinition> = self
            .post_tool_use_matchers
            .iter()
            .filter(|m| m.matches(tool_name))
            .flat_map(|m| &m.config.hooks)
            .collect();

        if hooks.is_empty() {
            return HookResult::allowed();
        }

        self.execute_hooks(&hooks, input, EventType::PostToolUse).await
    }

    /// Execute session start hooks
    pub async fn execute_session_start(&self, input: &mut HookInput) -> HookResult {
        if self.config.session_start.is_empty() {
            return HookResult::allowed();
        }

        input.hook_event_name = EventType::SessionStart;
        let hooks: Vec<&HookDefinition> = self.config.session_start.iter().collect();
        self.execute_hooks(&hooks, input, EventType::SessionStart).await
    }

    /// Execute session end hooks
    pub async fn execute_session_end(&self, input: &mut HookInput) -> HookResult {
        if self.config.session_end.is_empty() {
            return HookResult::allowed();
        }

        input.hook_event_name = EventType::SessionEnd;
        let hooks: Vec<&HookDefinition> = self.config.session_end.iter().collect();
        self.execute_hooks(&hooks, input, EventType::SessionEnd).await
    }

    /// Check if pre-tool-use hooks are configured
    pub fn has_pre_tool_use_hooks(&self) -> bool {
        !self.pre_tool_use_matchers.is_empty()
    }

    /// Check if post-tool-use hooks are configured
    pub fn has_post_tool_use_hooks(&self) -> bool {
        !self.post_tool_use_matchers.is_empty()
    }

    /// Check if session start hooks are configured
    pub fn has_session_start_hooks(&self) -> bool {
        !self.config.session_start.is_empty()
    }

    /// Check if session end hooks are configured
    pub fn has_session_end_hooks(&self) -> bool {
        !self.config.session_end.is_empty()
    }

    async fn execute_hooks(
        &self,
        hooks: &[&HookDefinition],
        input: &HookInput,
        event_type: EventType,
    ) -> HookResult {
        // Deduplicate hooks by command
        let mut seen = std::collections::HashSet::new();
        let unique_hooks: Vec<&HookDefinition> = hooks
            .iter()
            .filter(|h| {
                let key = format!("{}:{}", h.hook_type, h.command.as_deref().unwrap_or(""));
                if seen.contains(&key) {
                    false
                } else {
                    seen.insert(key);
                    true
                }
            })
            .copied()
            .collect();

        if unique_hooks.is_empty() {
            return HookResult::allowed();
        }

        // Serialize input to JSON
        let input_json = match serde_json::to_vec(input) {
            Ok(j) => j,
            Err(e) => {
                return HookResult {
                    allowed: false,
                    message: Some(format!("Failed to serialize hook input: {}", e)),
                    ..Default::default()
                };
            }
        };

        // Execute hooks concurrently
        let futures: Vec<_> = unique_hooks
            .iter()
            .map(|hook| self.execute_hook(hook, &input_json))
            .collect();

        let results = futures::future::join_all(futures).await;

        // Aggregate results
        self.aggregate_results(&results, event_type)
    }

    async fn execute_hook(
        &self,
        hook: &HookDefinition,
        input_json: &[u8],
    ) -> (Option<HookOutput>, String, String, i32, Option<String>) {
        if hook.hook_type != "command" {
            return (
                None,
                String::new(),
                String::new(),
                -1,
                Some(format!("Unsupported hook type: {}", hook.hook_type)),
            );
        }

        let command = match &hook.command {
            Some(c) => c,
            None => {
                return (
                    None,
                    String::new(),
                    String::new(),
                    -1,
                    Some("No command specified".to_string()),
                );
            }
        };

        let timeout_secs = hook.timeout.unwrap_or(60);
        let timeout_duration = Duration::from_secs(timeout_secs);

        // Determine shell and args based on OS
        let (shell, shell_args) = if cfg!(windows) {
            (
                std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string()),
                vec!["/C".to_string()],
            )
        } else {
            (
                std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()),
                vec!["-c".to_string()],
            )
        };

        let mut cmd = Command::new(&shell);
        cmd.args(&shell_args);
        cmd.arg(command);
        cmd.current_dir(&self.working_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set environment
        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return (
                    None,
                    String::new(),
                    String::new(),
                    -1,
                    Some(format!("Failed to spawn hook: {}", e)),
                );
            }
        };

        // Write input to stdin and close it
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(input_json).await {
                warn!("Failed to write to hook stdin: {}", e);
            }
            // Stdin is dropped here, closing it
        }

        // Wait for completion with timeout
        let output = match timeout(timeout_duration, child.wait_with_output()).await {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => {
                return (
                    None,
                    String::new(),
                    String::new(),
                    -1,
                    Some(format!("Hook execution error: {}", e)),
                );
            }
            Err(_) => {
                return (
                    None,
                    String::new(),
                    String::new(),
                    -1,
                    Some("Hook timed out".to_string()),
                );
            }
        };

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        // Parse output if exit code is 0
        let parsed_output = if exit_code == 0 && !stdout.trim().is_empty() {
            let trimmed = stdout.trim();
            if trimmed.starts_with('{') {
                serde_json::from_str::<HookOutput>(trimmed).ok()
            } else {
                None
            }
        } else {
            None
        };

        debug!(
            "Hook executed: exit_code={}, stdout_len={}, stderr_len={}",
            exit_code,
            stdout.len(),
            stderr.len()
        );

        (parsed_output, stdout, stderr, exit_code, None)
    }

    fn aggregate_results(
        &self,
        results: &[HookExecResult],
        event_type: EventType,
    ) -> HookResult {
        let mut final_result = HookResult::allowed();
        let mut messages = Vec::new();
        let mut additional_contexts = Vec::new();
        let mut system_messages = Vec::new();

        for (output, stdout, stderr, exit_code, err) in results {
            if let Some(e) = err {
                warn!("Hook execution error: {}", e);
                continue;
            }

            // Exit code 2 is a blocking error
            if *exit_code == 2 {
                final_result.allowed = false;
                final_result.exit_code = 2;
                if !stderr.is_empty() {
                    final_result.stderr = Some(stderr.clone());
                    messages.push(stderr.trim().to_string());
                }
                continue;
            }

            // Non-zero, non-2 exit codes are non-blocking errors
            if *exit_code != 0 {
                debug!(
                    "Hook returned non-zero exit code: {}, stderr: {}",
                    exit_code, stderr
                );
                continue;
            }

            // Process successful output
            if let Some(out) = output {
                // Check continue flag
                if !out.should_continue() {
                    final_result.allowed = false;
                    if let Some(reason) = &out.stop_reason {
                        messages.push(reason.clone());
                    }
                }

                // Check decision
                if out.is_blocked() {
                    final_result.allowed = false;
                    if let Some(reason) = &out.reason {
                        messages.push(reason.clone());
                    }
                }

                // Collect system messages
                if let Some(msg) = &out.system_message {
                    system_messages.push(msg.clone());
                }

                // Process hook-specific output
                if let Some(hso) = &out.hook_specific_output {
                    // PreToolUse permission decision
                    if event_type == EventType::PreToolUse {
                        if let Some(decision) = &hso.permission_decision {
                            if decision == "deny" {
                                final_result.allowed = false;
                                if let Some(reason) = &hso.permission_decision_reason {
                                    messages.push(reason.clone());
                                }
                            }
                        }

                        // Merge updated input
                        if let Some(updated) = &hso.updated_input {
                            let modified = final_result
                                .modified_input
                                .get_or_insert_with(HashMap::new);
                            for (k, v) in updated {
                                modified.insert(k.clone(), v.clone());
                            }
                        }
                    }

                    // Additional context
                    if let Some(ctx) = &hso.additional_context {
                        additional_contexts.push(ctx.clone());
                    }
                }
            } else if !stdout.is_empty() {
                // Plain text stdout is added as context for some events
                if event_type == EventType::SessionStart || event_type == EventType::PostToolUse {
                    additional_contexts.push(stdout.trim().to_string());
                }
            }
        }

        // Combine messages
        if !messages.is_empty() {
            final_result.message = Some(messages.join("\n"));
        }
        if !additional_contexts.is_empty() {
            final_result.additional_context = Some(additional_contexts.join("\n"));
        }
        if !system_messages.is_empty() {
            final_result.system_message = Some(system_messages.join("\n"));
        }

        final_result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_type_display() {
        assert_eq!(EventType::PreToolUse.to_string(), "pre_tool_use");
        assert_eq!(EventType::PostToolUse.to_string(), "post_tool_use");
        assert_eq!(EventType::SessionStart.to_string(), "session_start");
        assert_eq!(EventType::SessionEnd.to_string(), "session_end");
    }

    #[test]
    fn test_hook_output_defaults() {
        let output = HookOutput::default();
        assert!(output.should_continue());
        assert!(!output.is_blocked());
    }

    #[test]
    fn test_hook_output_blocked() {
        let output = HookOutput {
            decision: Some("block".to_string()),
            ..Default::default()
        };
        assert!(output.is_blocked());
    }

    #[test]
    fn test_compiled_matcher_wildcard() {
        let config = HookMatcherConfig {
            matcher: "*".to_string(),
            hooks: vec![],
        };
        let matcher = CompiledMatcher::new(config);
        assert!(matcher.matches("any_tool"));
        assert!(matcher.matches("shell"));
    }

    #[test]
    fn test_compiled_matcher_specific() {
        let config = HookMatcherConfig {
            matcher: "shell|edit_file".to_string(),
            hooks: vec![],
        };
        let matcher = CompiledMatcher::new(config);
        assert!(matcher.matches("shell"));
        assert!(matcher.matches("edit_file"));
        assert!(!matcher.matches("read_file"));
    }

    #[test]
    fn test_hook_result_allowed() {
        let result = HookResult::allowed();
        assert!(result.allowed);
    }

    #[test]
    fn test_hook_result_blocked() {
        let result = HookResult::blocked("Permission denied");
        assert!(!result.allowed);
        assert_eq!(result.message.as_deref(), Some("Permission denied"));
    }
}
