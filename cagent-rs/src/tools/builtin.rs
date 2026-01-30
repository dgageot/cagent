//! Built-in tools: filesystem, shell, think, todo, fetch, etc.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use rusqlite::{params, Connection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use walkdir::WalkDir;
use ignore::WalkBuilder;

use super::{Tool, ToolAnnotations, ToolCall, ToolCallResult, ToolSet};

// ============================================================================
// Elicitation Types (for user_prompt tool)
// ============================================================================

/// Action taken by the user in response to an elicitation request
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

impl std::fmt::Display for ElicitationAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElicitationAction::Accept => write!(f, "accept"),
            ElicitationAction::Decline => write!(f, "decline"),
            ElicitationAction::Cancel => write!(f, "cancel"),
        }
    }
}

/// Request for user input/elicitation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    /// Message to display to the user
    pub message: String,
    /// Optional JSON schema for structured response
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Map<String, serde_json::Value>>,
}

/// Result of an elicitation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationResult {
    /// The action taken by the user
    pub action: ElicitationAction,
    /// The content provided (only present when action is Accept)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ElicitationResult {
    pub fn accept(content: serde_json::Map<String, serde_json::Value>) -> Self {
        Self {
            action: ElicitationAction::Accept,
            content: Some(content),
        }
    }

    pub fn decline() -> Self {
        Self {
            action: ElicitationAction::Decline,
            content: None,
        }
    }

    pub fn cancel() -> Self {
        Self {
            action: ElicitationAction::Cancel,
            content: None,
        }
    }
}

/// Type alias for elicitation handler function
pub type ElicitationHandler = Box<
    dyn Fn(ElicitationRequest) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<ElicitationResult>> + Send>>
        + Send
        + Sync,
>;

// ============================================================================
// Helper macros and functions
// ============================================================================

/// Create a tool definition with common patterns
fn make_tool(name: &str, description: &str, params: serde_json::Value, read_only: bool) -> Tool {
    Tool {
        name: name.to_string(),
        category: Some(name.split('_').next().unwrap_or(name).to_string()),
        description: description.to_string(),
        parameters: params,
        annotations: ToolAnnotations {
            read_only_hint: read_only,
            title: Some(name.replace('_', " ").to_string()),
        },
        output_schema: None,
    }
}

/// Resolve a path relative to a working directory
fn resolve_path(working_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        working_dir.join(p)
    }
}

/// Get the user's shell
fn get_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
}

// ============================================================================
// Filesystem Toolset
// ============================================================================

/// Configuration for a post-edit command (imported from config module)
pub use crate::config::PostEditConfig;

pub struct FilesystemToolset {
    working_dir: PathBuf,
    post_edit_commands: Vec<PostEditConfig>,
    ignore_vcs: bool,
}

impl FilesystemToolset {
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            post_edit_commands: Vec::new(),
            ignore_vcs: true, // Default to ignoring VCS files
        }
    }

    pub fn with_post_edit(mut self, commands: Vec<PostEditConfig>) -> Self {
        self.post_edit_commands = commands;
        self
    }

    pub fn with_ignore_vcs(mut self, ignore: bool) -> Self {
        self.ignore_vcs = ignore;
        self
    }

    fn resolve(&self, path: &str) -> PathBuf {
        resolve_path(&self.working_dir, path)
    }

    /// Execute post-edit commands for a file that was just modified
    fn execute_post_edit_commands(&self, file_path: &Path) {
        if self.post_edit_commands.is_empty() {
            return;
        }

        let file_name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        for post_edit in &self.post_edit_commands {
            // Check if the file matches the pattern
            let matched = glob::Pattern::new(&post_edit.path)
                .map(|p| p.matches(&file_name))
                .unwrap_or(false);

            if !matched {
                continue;
            }

            // Run the post-edit command
            let shell = get_shell();
            let mut cmd = std::process::Command::new(shell);
            cmd.arg("-c")
                .arg(&post_edit.cmd)
                .current_dir(&self.working_dir)
                .env("path", file_path.to_string_lossy().to_string());

            if let Err(e) = cmd.output() {
                tracing::warn!(
                    "Post-edit command failed for {}: {}",
                    file_path.display(),
                    e
                );
            }
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadFileArgs {
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WriteFileArgs {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct EditFileArgs {
    pub path: String,
    pub edits: Vec<Edit>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Edit {
    #[serde(rename = "oldText")]
    pub old_text: String,
    #[serde(rename = "newText")]
    pub new_text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PathArg {
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchFilesContentArgs {
    pub path: String,
    pub query: String,
    #[serde(default)]
    pub is_regex: bool,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadMultipleFilesArgs {
    pub paths: Vec<String>,
    #[serde(default)]
    pub json: bool,
}

#[async_trait::async_trait]
impl ToolSet for FilesystemToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![
            make_tool(
                "read_file",
                "Read the complete contents of a file.",
                crate::tool_schema!(ReadFileArgs),
                true,
            ),
            make_tool(
                "write_file",
                "Create or overwrite a file with new content.",
                crate::tool_schema!(WriteFileArgs),
                false,
            ),
            make_tool(
                "edit_file",
                "Make line-based edits to a text file.",
                crate::tool_schema!(EditFileArgs),
                false,
            ),
            make_tool(
                "directory_tree",
                "Get a recursive tree view of files and directories.",
                crate::tool_schema!(PathArg),
                true,
            ),
            make_tool(
                "list_directory",
                "Get a detailed listing of files in a directory.",
                crate::tool_schema!(PathArg),
                true,
            ),
            make_tool(
                "search_files_content",
                "Search for text or regex patterns in files.",
                crate::tool_schema!(SearchFilesContentArgs),
                true,
            ),
            make_tool(
                "read_multiple_files",
                "Read the contents of multiple files simultaneously.",
                crate::tool_schema!(ReadMultipleFilesArgs),
                true,
            ),
        ])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let name = &tool_call.function.name;
        let args = &tool_call.function.arguments;

        match name.as_str() {
            "read_file" => {
                let a: ReadFileArgs = serde_json::from_str(args)?;
                match fs::read_to_string(self.resolve(&a.path)) {
                    Ok(content) => Ok(ToolCallResult::success(content)),
                    Err(e) => Ok(ToolCallResult::error(format!("Error reading file: {}", e))),
                }
            }
            "write_file" => {
                let a: WriteFileArgs = serde_json::from_str(args)?;
                let path = self.resolve(&a.path);
                if let Some(parent) = path.parent() {
                    fs::create_dir_all(parent)?;
                }
                match fs::write(&path, &a.content) {
                    Ok(_) => {
                        // Execute post-edit commands
                        self.execute_post_edit_commands(&path);
                        Ok(ToolCallResult::success(format!(
                            "Written {} bytes to {}",
                            a.content.len(),
                            a.path
                        )))
                    }
                    Err(e) => Ok(ToolCallResult::error(format!("Error writing: {}", e))),
                }
            }
            "edit_file" => {
                let a: EditFileArgs = serde_json::from_str(args)?;
                let path = self.resolve(&a.path);
                let mut content =
                    fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("Read error: {}", e))?;

                for (i, edit) in a.edits.iter().enumerate() {
                    if !content.contains(&edit.old_text) {
                        return Ok(ToolCallResult::error(format!(
                            "Edit {} failed: text not found",
                            i + 1
                        )));
                    }
                    content = content.replacen(&edit.old_text, &edit.new_text, 1);
                }

                fs::write(&path, &content)?;
                // Execute post-edit commands
                self.execute_post_edit_commands(&path);
                Ok(ToolCallResult::success(format!(
                    "Applied {} edit(s) successfully",
                    a.edits.len()
                )))
            }
            "directory_tree" => {
                let a: PathArg = serde_json::from_str(args)?;
                let path = self.resolve(&a.path);

                fn build_tree(path: &Path, depth: usize) -> Option<serde_json::Value> {
                    if depth > 5 {
                        return None;
                    }
                    let name = path.file_name()?.to_string_lossy().to_string();
                    if path.is_file() {
                        return Some(json!({"name": name, "type": "file"}));
                    }
                    let children: Vec<_> = fs::read_dir(path)
                        .ok()?
                        .flatten()
                        .filter_map(|e| build_tree(&e.path(), depth + 1))
                        .collect();
                    Some(json!({"name": name, "type": "directory", "children": children}))
                }

                match build_tree(&path, 0) {
                    Some(tree) => Ok(ToolCallResult::success(serde_json::to_string_pretty(
                        &tree,
                    )?)),
                    None => Ok(ToolCallResult::error("Could not build tree")),
                }
            }
            "list_directory" => {
                let a: PathArg = serde_json::from_str(args)?;
                match fs::read_dir(self.resolve(&a.path)) {
                    Ok(entries) => {
                        let listing: String = entries
                            .flatten()
                            .map(|e| {
                                let prefix = if e.metadata().map(|m| m.is_dir()).unwrap_or(false) {
                                    "DIR  "
                                } else {
                                    "FILE "
                                };
                                format!("{}{}\n", prefix, e.file_name().to_string_lossy())
                            })
                            .collect();
                        Ok(ToolCallResult::success(listing))
                    }
                    Err(e) => Ok(ToolCallResult::error(format!("Error: {}", e))),
                }
            }
            "search_files_content" => {
                let a: SearchFilesContentArgs = serde_json::from_str(args)?;
                let regex = a
                    .is_regex
                    .then(|| regex::Regex::new(&a.query))
                    .transpose()?;

                // Compile exclude patterns
                let exclude_globs: Vec<glob::Pattern> = a
                    .exclude_patterns
                    .iter()
                    .filter_map(|p| glob::Pattern::new(p).ok())
                    .collect();

                // Build walker - use ignore crate if ignoring VCS
                let search_path = self.resolve(&a.path);
                let entries: Box<dyn Iterator<Item = walkdir::DirEntry>> = if self.ignore_vcs {
                    Box::new(
                        WalkBuilder::new(&search_path)
                            .hidden(false) // Still show hidden files
                            .git_ignore(true)
                            .git_global(true)
                            .git_exclude(true)
                            .build()
                            .filter_map(|e| e.ok())
                            .filter_map(|e| {
                                // Convert ignore::DirEntry to walkdir::DirEntry-like behavior
                                if e.file_type().map(|ft| ft.is_file()).unwrap_or(false) {
                                    // Create a pseudo-entry using walkdir
                                    WalkDir::new(e.path())
                                        .max_depth(0)
                                        .into_iter()
                                        .next()
                                        .and_then(|r| r.ok())
                                } else {
                                    None
                                }
                            })
                    )
                } else {
                    Box::new(
                        WalkDir::new(&search_path)
                            .into_iter()
                            .filter_map(|e| e.ok())
                            .filter(|e| e.file_type().is_file())
                    )
                };

                let results: Vec<String> = entries
                    .filter(|e| {
                        // Check exclude patterns against the path
                        let path_str = e.path().to_string_lossy();
                        !exclude_globs.iter().any(|glob| glob.matches(&path_str))
                    })
                    .flat_map(|entry| {
                        fs::read_to_string(entry.path())
                            .ok()
                            .map(|content| {
                                content
                                    .lines()
                                    .enumerate()
                                    .filter(|(_, line)| {
                                        regex
                                            .as_ref()
                                            .map(|r| r.is_match(line))
                                            .unwrap_or_else(|| line.contains(&a.query))
                                    })
                                    .map(|(num, line)| {
                                        let preview =
                                            if line.len() > 100 { &line[..100] } else { line };
                                        format!(
                                            "{}:{}:1: {}",
                                            entry.path().display(),
                                            num + 1,
                                            preview
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default()
                    })
                    .collect();

                Ok(ToolCallResult::success(if results.is_empty() {
                    "No results found".to_string()
                } else {
                    results.join("\n")
                }))
            }
            "read_multiple_files" => {
                let a: ReadMultipleFilesArgs = serde_json::from_str(args)?;

                if a.json {
                    let contents: Vec<_> = a.paths.iter()
                        .map(|p| json!({"path": p, "content": fs::read_to_string(self.resolve(p)).unwrap_or_else(|e| e.to_string())}))
                        .collect();
                    Ok(ToolCallResult::success(serde_json::to_string_pretty(
                        &contents,
                    )?))
                } else {
                    let result: String = a
                        .paths
                        .iter()
                        .map(|p| {
                            format!(
                                "=== {} ===\n{}\n\n",
                                p,
                                fs::read_to_string(self.resolve(p))
                                    .unwrap_or_else(|e| e.to_string())
                            )
                        })
                        .collect();
                    Ok(ToolCallResult::success(result))
                }
            }
            _ => Ok(ToolCallResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn instructions(&self) -> Option<String> {
        Some("## Filesystem Tools\nUse read_multiple_files for batch operations. Paths are relative to working directory.".to_string())
    }
}

// ============================================================================
// Shell Toolset
// ============================================================================

pub struct ShellToolset {
    working_dir: PathBuf,
}

impl ShellToolset {
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ShellArgs {
    pub cmd: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_timeout")]
    pub timeout: u64,
}

fn default_timeout() -> u64 {
    30
}

#[async_trait::async_trait]
impl ToolSet for ShellToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![make_tool(
            "shell",
            "Execute a shell command.",
            crate::tool_schema!(ShellArgs),
            false,
        )])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let a: ShellArgs = serde_json::from_str(&tool_call.function.arguments)?;
        let cwd = a
            .cwd
            .map(|p| resolve_path(&self.working_dir, &p))
            .unwrap_or_else(|| self.working_dir.clone());

        let timeout = Duration::from_secs(a.timeout.max(1));

        let mut cmd = tokio::process::Command::new(get_shell());
        cmd.arg("-c")
            .arg(&a.cmd)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Ensure the child process is killed when dropped (e.g., on timeout)
            .kill_on_drop(true);

        let child = cmd.spawn()?;

        let output = match tokio::time::timeout(timeout, child.wait_with_output()).await {
            Ok(res) => res?,
            Err(_) => {
                return Ok(ToolCallResult::error(format!(
                    "Timeout after {}s",
                    timeout.as_secs()
                )));
            }
        };

        let mut result = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.is_empty() {
            if !result.is_empty() {
                result.push('\n');
            }
            result.push_str(&stderr);
        }

        if output.status.success() {
            Ok(ToolCallResult::success(result))
        } else {
            Ok(ToolCallResult::error(format!(
                "Exit {}: {}",
                output.status.code().unwrap_or(-1),
                result
            )))
        }
    }

    fn instructions(&self) -> Option<String> {
        Some("## Shell Tool\nUse 'cwd' parameter for directory-specific commands.".to_string())
    }
}

// ============================================================================
// Simple Toolsets (Think, TransferTask)
// ============================================================================

pub struct ThinkToolset;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ThinkArgs {
    pub thought: String,
}

#[async_trait::async_trait]
impl ToolSet for ThinkToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![make_tool(
            "think",
            "Think through complex problems step by step.",
            crate::tool_schema!(ThinkArgs),
            true,
        )])
    }

    async fn execute(&self, _: &ToolCall) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult::success("Thought recorded."))
    }
}

pub struct TransferTaskToolset;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransferTaskArgs {
    pub agent: String,
    pub task: String,
    #[serde(default)]
    pub expected_output: Option<String>,
}

#[async_trait::async_trait]
impl ToolSet for TransferTaskToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![make_tool(
            "transfer_task",
            "Transfer a task to another agent.",
            crate::tool_schema!(TransferTaskArgs),
            false,
        )])
    }

    async fn execute(&self, _: &ToolCall) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult::error(
            "transfer_task should be handled by runtime",
        ))
    }
}

// ============================================================================
// Handoff Toolset
// ============================================================================

/// The handoff toolset allows an agent to hand off the conversation to another agent.
/// Unlike transfer_task, handoff permanently transfers control and the conversation
/// continues with the target agent.
#[derive(Debug)]
pub struct HandoffToolset;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HandoffArgs {
    /// The name of the agent to hand off the conversation to
    pub agent: String,
}

#[async_trait::async_trait]
impl ToolSet for HandoffToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![make_tool(
            "handoff",
            "Use this function to hand off the conversation to the selected agent.",
            crate::tool_schema!(HandoffArgs),
            true, // read_only since it doesn't modify external state
        )])
    }

    async fn execute(&self, _: &ToolCall) -> anyhow::Result<ToolCallResult> {
        Ok(ToolCallResult::error(
            "handoff should be handled by runtime",
        ))
    }

    fn instructions(&self) -> Option<String> {
        Some("## Handoff Tool\n\nUse the handoff tool to transfer the conversation to another agent. The target agent will take over and continue the conversation.".to_string())
    }
}

// ============================================================================
// Todo Toolset
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub description: String,
    pub status: String,
}

pub struct TodoToolset {
    todos: Arc<RwLock<Vec<Todo>>>,
}

/// Shared TodoToolset singleton - use when `shared: true` in config
static SHARED_TODOS: once_cell::sync::Lazy<Arc<RwLock<Vec<Todo>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(Vec::new())));

impl Default for TodoToolset {
    fn default() -> Self {
        Self::new()
    }
}

impl TodoToolset {
    pub fn new() -> Self {
        Self {
            todos: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Create a shared todo toolset that shares state across all agents
    pub fn shared() -> Self {
        Self {
            todos: SHARED_TODOS.clone(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTodoArgs {
    pub description: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTodosArgs {
    pub descriptions: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TodoUpdate {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateTodosArgs {
    pub updates: Vec<TodoUpdate>,
}

#[async_trait::async_trait]
impl ToolSet for TodoToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![
            make_tool(
                "create_todo",
                "Create a new todo item.",
                crate::tool_schema!(CreateTodoArgs),
                true,
            ),
            make_tool(
                "create_todos",
                "Create multiple todo items.",
                crate::tool_schema!(CreateTodosArgs),
                true,
            ),
            make_tool(
                "update_todos",
                "Update todo status (pending/in-progress/completed).",
                crate::tool_schema!(UpdateTodosArgs),
                true,
            ),
            make_tool(
                "list_todos",
                "List all current todos.",
                json!({"type": "object", "properties": {}}),
                true,
            ),
        ])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let name = &tool_call.function.name;
        let args = &tool_call.function.arguments;

        match name.as_str() {
            "create_todo" => {
                let a: CreateTodoArgs = serde_json::from_str(args)?;
                let mut todos = self.todos.write().unwrap();
                let id = format!("todo_{}", todos.len() + 1);
                todos.push(Todo {
                    id: id.clone(),
                    description: a.description.clone(),
                    status: "pending".to_string(),
                });
                Ok(ToolCallResult::success(format!(
                    "Created [{}]: {}",
                    id, a.description
                )))
            }
            "create_todos" => {
                let a: CreateTodosArgs = serde_json::from_str(args)?;
                let mut todos = self.todos.write().unwrap();
                let start = todos.len();
                let ids: Vec<_> = a
                    .descriptions
                    .iter()
                    .enumerate()
                    .map(|(i, desc)| {
                        let id = format!("todo_{}", start + i + 1);
                        todos.push(Todo {
                            id: id.clone(),
                            description: desc.clone(),
                            status: "pending".to_string(),
                        });
                        id
                    })
                    .collect();
                Ok(ToolCallResult::success(format!(
                    "Created {} todos: {}",
                    ids.len(),
                    ids.join(", ")
                )))
            }
            "update_todos" => {
                let a: UpdateTodosArgs = serde_json::from_str(args)?;
                let mut todos = self.todos.write().unwrap();
                let (updated, not_found): (Vec<_>, Vec<_>) = a
                    .updates
                    .iter()
                    .map(|u| {
                        if let Some(t) = todos.iter_mut().find(|t| t.id == u.id) {
                            t.status = u.status.clone();
                            (Some(format!("{} -> {}", u.id, u.status)), None)
                        } else {
                            (None, Some(u.id.clone()))
                        }
                    })
                    .unzip();

                // Clear if all completed
                if todos.iter().all(|t| t.status == "completed") && !todos.is_empty() {
                    todos.clear();
                }

                let updated: Vec<_> = updated.into_iter().flatten().collect();
                let not_found: Vec<_> = not_found.into_iter().flatten().collect();
                let mut msg = if !updated.is_empty() {
                    format!("Updated: {}", updated.join(", "))
                } else {
                    String::new()
                };
                if !not_found.is_empty() {
                    if !msg.is_empty() {
                        msg.push_str("; ");
                    }
                    msg.push_str(&format!("Not found: {}", not_found.join(", ")));
                }
                Ok(ToolCallResult::success(msg))
            }
            "list_todos" => {
                let todos = self.todos.read().unwrap();
                let output: String = todos
                    .iter()
                    .map(|t| format!("- [{}] {} ({})\n", t.id, t.description, t.status))
                    .collect();
                Ok(ToolCallResult::success(if output.is_empty() {
                    "No todos.".to_string()
                } else {
                    output
                }))
            }
            _ => Ok(ToolCallResult::error(format!("Unknown tool: {}", name))),
        }
    }

    fn instructions(&self) -> Option<String> {
        Some(
            "## Todo Tools\nUse todos to track task progress. Mark as 'completed' when done."
                .to_string(),
        )
    }
}

// ============================================================================
// Fetch Toolset
// ============================================================================

pub struct FetchToolset {
    timeout: Duration,
}

impl Default for FetchToolset {
    fn default() -> Self {
        Self::new()
    }
}

impl FetchToolset {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(30),
        }
    }
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FetchArgs {
    pub urls: Vec<String>,
    #[serde(default = "default_format")]
    pub format: String,
    #[serde(default)]
    pub timeout: Option<u64>,
}

fn default_format() -> String {
    "text".to_string()
}

#[derive(Debug, Serialize)]
struct FetchResult {
    url: String,
    status_code: Option<u16>,
    content_type: Option<String>,
    content_length: usize,
    body: String,
    error: Option<String>,
}

#[async_trait::async_trait]
impl ToolSet for FetchToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![make_tool(
            "fetch",
            "Fetch content from HTTP/HTTPS URLs.",
            crate::tool_schema!(FetchArgs),
            true,
        )])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let a: FetchArgs = serde_json::from_str(&tool_call.function.arguments)?;
        if a.urls.is_empty() {
            return Ok(ToolCallResult::error("At least one URL required"));
        }

        let timeout = a.timeout.map(Duration::from_secs).unwrap_or(self.timeout);
        let client = reqwest::Client::builder().timeout(timeout).build()?;

        static HTML_RE: std::sync::LazyLock<regex::Regex> =
            std::sync::LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());

        let results: Vec<FetchResult> = futures::future::join_all(a.urls.iter().map(|url| {
            let client = &client;
            let format = &a.format;
            async move {
                match client.get(url).send().await {
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let ct = resp
                            .headers()
                            .get("content-type")
                            .and_then(|v| v.to_str().ok())
                            .map(String::from);
                        match resp.text().await {
                            Ok(body) => {
                                let processed = if format == "html" {
                                    body.clone()
                                } else if ct
                                    .as_ref()
                                    .map(|c| c.contains("text/html"))
                                    .unwrap_or(false)
                                {
                                    HTML_RE.replace_all(&body, "").to_string()
                                } else {
                                    body.clone()
                                };
                                FetchResult {
                                    url: url.clone(),
                                    status_code: Some(status),
                                    content_type: ct,
                                    content_length: processed.len(),
                                    body: processed,
                                    error: None,
                                }
                            }
                            Err(e) => FetchResult {
                                url: url.clone(),
                                status_code: Some(status),
                                content_type: ct,
                                content_length: 0,
                                body: String::new(),
                                error: Some(e.to_string()),
                            },
                        }
                    }
                    Err(e) => FetchResult {
                        url: url.clone(),
                        status_code: None,
                        content_type: None,
                        content_length: 0,
                        body: String::new(),
                        error: Some(e.to_string()),
                    },
                }
            }
        }))
        .await;

        if a.urls.len() == 1 {
            let r = &results[0];
            if let Some(ref e) = r.error {
                return Ok(ToolCallResult::error(format!("Error: {}", e)));
            }
            Ok(ToolCallResult::success(format!(
                "Fetched {} ({} bytes):\n\n{}",
                r.url, r.content_length, r.body
            )))
        } else {
            Ok(ToolCallResult::success(serde_json::to_string_pretty(
                &results,
            )?))
        }
    }

    fn instructions(&self) -> Option<String> {
        Some("## Fetch Tool\nFetch HTTP/HTTPS URLs. Supports multiple URLs in batch.".to_string())
    }
}

// ============================================================================
// Background Jobs Toolset
// ============================================================================

#[derive(Debug, Clone)]
pub struct BackgroundJob {
    pub id: String,
    pub cmd: String,
    pub cwd: String,
    pub started_at: Instant,
    pub status: JobStatus,
    pub output: Arc<Mutex<String>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Running,
    Completed(i32),
    Failed(String),
}

pub struct BackgroundJobsToolset {
    working_dir: PathBuf,
    jobs: Arc<RwLock<HashMap<String, BackgroundJob>>>,
    job_counter: Arc<Mutex<u64>>,
}

impl BackgroundJobsToolset {
    pub fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            jobs: Arc::new(RwLock::new(HashMap::new())),
            job_counter: Arc::new(Mutex::new(0)),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunBackgroundJobArgs {
    pub cmd: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct JobIdArg {
    pub job_id: String,
}

#[async_trait::async_trait]
impl ToolSet for BackgroundJobsToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![
            make_tool(
                "run_background_job",
                "Start a command in background, returns job ID.",
                crate::tool_schema!(RunBackgroundJobArgs),
                false,
            ),
            make_tool(
                "list_background_jobs",
                "List all background jobs.",
                json!({"type": "object", "properties": {}}),
                true,
            ),
            make_tool(
                "view_background_job",
                "View output of a background job.",
                crate::tool_schema!(JobIdArg),
                true,
            ),
            make_tool(
                "stop_background_job",
                "Stop a running background job.",
                crate::tool_schema!(JobIdArg),
                false,
            ),
        ])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        match tool_call.function.name.as_str() {
            "run_background_job" => {
                let a: RunBackgroundJobArgs = serde_json::from_str(&tool_call.function.arguments)?;
                let cwd = a
                    .cwd
                    .map(|p| resolve_path(&self.working_dir, &p))
                    .unwrap_or_else(|| self.working_dir.clone());

                let job_id = {
                    let mut counter = self.job_counter.lock().unwrap();
                    *counter += 1;
                    format!("job_{}_{}", chrono::Utc::now().timestamp(), *counter)
                };

                let output_buf = Arc::new(Mutex::new(String::new()));
                let job = BackgroundJob {
                    id: job_id.clone(),
                    cmd: a.cmd.clone(),
                    cwd: cwd.to_string_lossy().to_string(),
                    started_at: Instant::now(),
                    status: JobStatus::Running,
                    output: output_buf.clone(),
                };

                self.jobs.write().unwrap().insert(job_id.clone(), job);

                let jobs = self.jobs.clone();
                let job_id_clone = job_id.clone();
                let cmd = a.cmd.clone();

                std::thread::spawn(move || {
                    let mut child = match Command::new(get_shell())
                        .arg("-c")
                        .arg(&cmd)
                        .current_dir(&cwd)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                    {
                        Ok(c) => c,
                        Err(e) => {
                            if let Some(j) = jobs.write().unwrap().get_mut(&job_id_clone) {
                                j.status = JobStatus::Failed(e.to_string());
                            }
                            return;
                        }
                    };

                    if let Some(stdout) = child.stdout.take() {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            let mut buf = output_buf.lock().unwrap();
                            buf.push_str(&line);
                            buf.push('\n');
                        }
                    }

                    let status = match child.wait() {
                        Ok(s) => JobStatus::Completed(s.code().unwrap_or(-1)),
                        Err(e) => JobStatus::Failed(e.to_string()),
                    };
                    if let Some(j) = jobs.write().unwrap().get_mut(&job_id_clone) {
                        j.status = status;
                    }
                });

                Ok(ToolCallResult::success(format!("Started: {}", job_id)))
            }
            "list_background_jobs" => {
                let jobs = self.jobs.read().unwrap();
                if jobs.is_empty() {
                    return Ok(ToolCallResult::success("No background jobs."));
                }
                let output: String = jobs
                    .values()
                    .map(|j| {
                        let status = match &j.status {
                            JobStatus::Running => "running".to_string(),
                            JobStatus::Completed(c) => format!("exit {}", c),
                            JobStatus::Failed(e) => format!("failed: {}", e),
                        };
                        format!(
                            "- [{}] {} ({}s, {})\n",
                            j.id,
                            j.cmd,
                            j.started_at.elapsed().as_secs(),
                            status
                        )
                    })
                    .collect();
                Ok(ToolCallResult::success(output))
            }
            "view_background_job" => {
                let a: JobIdArg = serde_json::from_str(&tool_call.function.arguments)?;
                let jobs = self.jobs.read().unwrap();
                match jobs.get(&a.job_id) {
                    Some(j) => {
                        let status = match &j.status {
                            JobStatus::Running => "running".to_string(),
                            JobStatus::Completed(c) => format!("exit {}", c),
                            JobStatus::Failed(e) => format!("failed: {}", e),
                        };
                        Ok(ToolCallResult::success(format!(
                            "Job: {}\nCmd: {}\nStatus: {}\nOutput:\n{}",
                            j.id,
                            j.cmd,
                            status,
                            j.output.lock().unwrap()
                        )))
                    }
                    None => Ok(ToolCallResult::error(format!(
                        "Job not found: {}",
                        a.job_id
                    ))),
                }
            }
            "stop_background_job" => {
                let a: JobIdArg = serde_json::from_str(&tool_call.function.arguments)?;
                match self.jobs.write().unwrap().get_mut(&a.job_id) {
                    Some(j) => {
                        j.status = JobStatus::Failed("Stopped".to_string());
                        Ok(ToolCallResult::success(format!("Stopped: {}", a.job_id)))
                    }
                    None => Ok(ToolCallResult::error(format!(
                        "Job not found: {}",
                        a.job_id
                    ))),
                }
            }
            _ => Ok(ToolCallResult::error(format!(
                "Unknown tool: {}",
                tool_call.function.name
            ))),
        }
    }

    fn instructions(&self) -> Option<String> {
        Some(
            "## Background Jobs\nRun long processes in background. Use for servers, watchers, etc."
                .to_string(),
        )
    }
}

// ============================================================================
// Memory Toolset (SQLite)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub created_at: String,
}

pub struct MemoryToolset {
    db_path: PathBuf,
}

impl MemoryToolset {
    pub fn new(db_path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let db_path = db_path.into();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent)?;
        }
        Connection::open(&db_path)?.execute(
            "CREATE TABLE IF NOT EXISTS memories (id TEXT PRIMARY KEY, content TEXT NOT NULL, created_at TEXT NOT NULL)",
            [],
        )?;
        Ok(Self { db_path })
    }

    fn conn(&self) -> anyhow::Result<Connection> {
        Ok(Connection::open(&self.db_path)?)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddMemoryArgs {
    pub memory: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteMemoryArgs {
    pub id: String,
}

#[async_trait::async_trait]
impl ToolSet for MemoryToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![
            make_tool(
                "add_memory",
                "Add a new memory.",
                crate::tool_schema!(AddMemoryArgs),
                false,
            ),
            make_tool(
                "get_memories",
                "Retrieve all memories.",
                json!({"type": "object", "properties": {}}),
                true,
            ),
            make_tool(
                "delete_memory",
                "Delete a memory by ID.",
                crate::tool_schema!(DeleteMemoryArgs),
                false,
            ),
        ])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        match tool_call.function.name.as_str() {
            "add_memory" => {
                let a: AddMemoryArgs = serde_json::from_str(&tool_call.function.arguments)?;
                let id = format!(
                    "mem_{}",
                    chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
                );
                self.conn()?.execute(
                    "INSERT INTO memories (id, content, created_at) VALUES (?1, ?2, ?3)",
                    params![id, a.memory, chrono::Utc::now().to_rfc3339()],
                )?;
                Ok(ToolCallResult::success(format!("Added memory: {}", id)))
            }
            "get_memories" => {
                let conn = self.conn()?;
                let mut stmt = conn.prepare(
                    "SELECT id, content, created_at FROM memories ORDER BY created_at DESC",
                )?;
                let memories: Vec<Memory> = stmt
                    .query_map([], |row| {
                        Ok(Memory {
                            id: row.get(0)?,
                            content: row.get(1)?,
                            created_at: row.get(2)?,
                        })
                    })?
                    .filter_map(|r| r.ok())
                    .collect();
                Ok(ToolCallResult::success(serde_json::to_string_pretty(
                    &memories,
                )?))
            }
            "delete_memory" => {
                let a: DeleteMemoryArgs = serde_json::from_str(&tool_call.function.arguments)?;
                let deleted = self
                    .conn()?
                    .execute("DELETE FROM memories WHERE id = ?1", params![a.id])?;
                if deleted > 0 {
                    Ok(ToolCallResult::success(format!("Deleted: {}", a.id)))
                } else {
                    Ok(ToolCallResult::error(format!("Not found: {}", a.id)))
                }
            }
            _ => Ok(ToolCallResult::error(format!(
                "Unknown tool: {}",
                tool_call.function.name
            ))),
        }
    }

    fn instructions(&self) -> Option<String> {
        Some("## Memory Tool\nUse get_memories before actions to recall user context.".to_string())
    }
}

// ============================================================================
// Script Toolset (Custom Shell Tools)
// ============================================================================

#[derive(Debug)]
pub struct ScriptToolset {
    working_dir: PathBuf,
    tools_config: HashMap<String, crate::config::ScriptShellToolConfig>,
    env: Vec<(String, String)>,
}

impl ScriptToolset {
    pub fn new(
        working_dir: impl Into<PathBuf>,
        tools_config: HashMap<String, crate::config::ScriptShellToolConfig>,
        env: HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        // Validate configuration
        for (tool_name, tool) in &tools_config {
            Self::validate_config(tool_name, tool)?;
        }

        Ok(Self {
            working_dir: working_dir.into(),
            tools_config,
            env: env.into_iter().collect(),
        })
    }

    fn validate_config(
        tool_name: &str,
        tool: &crate::config::ScriptShellToolConfig,
    ) -> anyhow::Result<()> {
        // Find undefined args used in the command
        let mut missing_args = Vec::new();
        let mut i = 0;
        let cmd_bytes = tool.cmd.as_bytes();
        while i < cmd_bytes.len() {
            if cmd_bytes[i] == b'$' && i + 1 < cmd_bytes.len() {
                // Check for ${VAR} or $VAR format
                let (var_name, end_pos) = if cmd_bytes[i + 1] == b'{' {
                    // ${VAR} format
                    let start = i + 2;
                    let mut end = start;
                    while end < cmd_bytes.len() && cmd_bytes[end] != b'}' {
                        end += 1;
                    }
                    if end < cmd_bytes.len() {
                        (
                            std::str::from_utf8(&cmd_bytes[start..end]).unwrap_or(""),
                            end + 1,
                        )
                    } else {
                        i += 1;
                        continue;
                    }
                } else {
                    // $VAR format
                    let start = i + 1;
                    let mut end = start;
                    while end < cmd_bytes.len()
                        && (cmd_bytes[end].is_ascii_alphanumeric() || cmd_bytes[end] == b'_')
                    {
                        end += 1;
                    }
                    (
                        std::str::from_utf8(&cmd_bytes[start..end]).unwrap_or(""),
                        end,
                    )
                };

                if !var_name.is_empty() && !tool.args.contains_key(var_name) {
                    // Check if it's a common env var we should ignore
                    let common_env_vars = ["HOME", "USER", "PATH", "SHELL", "PWD", "TERM"];
                    if !common_env_vars.contains(&var_name) {
                        missing_args.push(var_name.to_string());
                    }
                }
                i = end_pos;
            } else {
                i += 1;
            }
        }

        if !missing_args.is_empty() {
            anyhow::bail!(
                "tool '{}' uses undefined args: {:?}",
                tool_name,
                missing_args
            );
        }

        // Check that all required args are defined
        for req_arg in &tool.required {
            if !tool.args.contains_key(req_arg) {
                anyhow::bail!(
                    "tool '{}' has required arg '{}' which is not defined in args",
                    tool_name,
                    req_arg
                );
            }
        }

        Ok(())
    }
}

#[async_trait::async_trait]
impl ToolSet for ScriptToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        let mut tools_list = Vec::new();

        for (name, cfg) in &self.tools_config {
            let description = cfg
                .description
                .clone()
                .unwrap_or_else(|| format!("Execute shell command: {}", cfg.cmd));

            // Build parameters schema
            let parameters = json!({
                "type": "object",
                "properties": cfg.args,
                "required": cfg.required,
            });

            tools_list.push(Tool {
                name: name.clone(),
                category: Some("script".to_string()),
                description,
                parameters,
                annotations: ToolAnnotations {
                    read_only_hint: false,
                    title: Some(name.clone()),
                },
                output_schema: None,
            });
        }

        Ok(tools_list)
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let tool_name = &tool_call.function.name;
        let cfg = self
            .tools_config
            .get(tool_name)
            .ok_or_else(|| anyhow::anyhow!("Script tool '{}' not found", tool_name))?;

        // Parse arguments
        let params: HashMap<String, serde_json::Value> = if tool_call.function.arguments.is_empty()
        {
            HashMap::new()
        } else {
            serde_json::from_str(&tool_call.function.arguments)?
        };

        // Get shell
        let shell = get_shell();

        // Build environment
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        env.extend(self.env.clone());
        for (k, v) in &cfg.env {
            env.push((k.clone(), v.clone()));
        }
        // Add arguments as environment variables
        for (key, value) in &params {
            if let Some(s) = value.as_str() {
                env.push((key.clone(), s.to_string()));
            } else {
                env.push((key.clone(), value.to_string()));
            }
        }

        // Determine working directory
        let cwd = cfg
            .working_dir
            .as_ref()
            .map(|p| resolve_path(&self.working_dir, p))
            .unwrap_or_else(|| self.working_dir.clone());

        // Execute command
        let output = Command::new(&shell)
            .arg("-c")
            .arg(&cfg.cmd)
            .current_dir(&cwd)
            .envs(env)
            .output();

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);
                let combined = if stderr.is_empty() {
                    stdout.to_string()
                } else {
                    format!("{}\n{}", stdout, stderr)
                };

                // Limit output size (same as Go implementation)
                let limited = limit_output(&combined);

                if output.status.success() {
                    Ok(ToolCallResult::success(limited))
                } else {
                    Ok(ToolCallResult::error(format!(
                        "Error executing command '{}': exit code {}\nOutput: {}",
                        cfg.cmd,
                        output.status.code().unwrap_or(-1),
                        limited
                    )))
                }
            }
            Err(e) => Ok(ToolCallResult::error(format!(
                "Error executing command '{}': {}",
                cfg.cmd, e
            ))),
        }
    }

    fn instructions(&self) -> Option<String> {
        let mut instructions = String::from("## Custom Shell Tools\n\n");
        instructions.push_str("The following custom shell tools are available:\n\n");

        for (name, tool) in &self.tools_config {
            instructions.push_str(&format!("### {}\n", name));
            if let Some(ref desc) = tool.description {
                instructions.push_str(&format!("{}\n\n", desc));
            } else {
                instructions.push_str(&format!("Execute: `{}`\n\n", tool.cmd));
            }

            if !tool.args.is_empty() {
                instructions.push_str("**Parameters:**\n");
                for (arg_name, arg_def) in &tool.args {
                    let required = if tool.required.contains(arg_name) {
                        " (required)"
                    } else {
                        ""
                    };
                    let description = arg_def
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    instructions
                        .push_str(&format!("- `{}`: {}{}\n", arg_name, description, required));
                }
                instructions.push('\n');
            }
        }

        Some(instructions)
    }
}

/// Limit output to avoid overly large tool responses
fn limit_output(output: &str) -> String {
    const MAX_OUTPUT_BYTES: usize = 100_000; // 100KB limit
    if output.len() > MAX_OUTPUT_BYTES {
        let truncated = &output[..MAX_OUTPUT_BYTES];
        format!(
            "{}\n... (output truncated, {} bytes total)",
            truncated,
            output.len()
        )
    } else {
        output.to_string()
    }
}

// ============================================================================
// API Toolset (HTTP API endpoint tools)
// ============================================================================

#[derive(Debug)]
pub struct ApiToolset {
    config: crate::config::ApiToolConfig,
    client: reqwest::Client,
}

impl ApiToolset {
    pub fn new(config: crate::config::ApiToolConfig) -> anyhow::Result<Self> {
        // Validate URL
        let parsed = reqwest::Url::parse(&config.endpoint)?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            anyhow::bail!("Only HTTP and HTTPS URLs are supported");
        }

        // Validate method
        let method = config.method.to_uppercase();
        if method != "GET" && method != "POST" {
            anyhow::bail!("Only GET and POST methods are supported");
        }

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self { config, client })
    }

    fn expand_url(&self, endpoint: &str, params: &HashMap<String, String>) -> String {
        let mut result = endpoint.to_string();
        for (key, value) in params {
            // Replace ${key} or $key patterns
            result = result.replace(&format!("${{{}}}", key), value);
            result = result.replace(&format!("${}", key), value);
        }
        result
    }
}

#[async_trait::async_trait]
impl ToolSet for ApiToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        let parameters = json!({
            "type": "object",
            "properties": self.config.args,
            "required": self.config.required,
        });

        Ok(vec![Tool {
            name: self.config.name.clone(),
            category: Some("api".to_string()),
            description: self.config.instruction.clone().unwrap_or_default(),
            parameters,
            annotations: ToolAnnotations {
                read_only_hint: true,
                title: Some(self.config.name.clone()),
            },
            output_schema: self.config.output_schema.clone(),
        }])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let method = self.config.method.to_uppercase();

        let response = match method.as_str() {
            "GET" => {
                let endpoint = if !tool_call.function.arguments.is_empty() {
                    let params: HashMap<String, String> =
                        serde_json::from_str(&tool_call.function.arguments)?;
                    self.expand_url(&self.config.endpoint, &params)
                } else {
                    self.config.endpoint.clone()
                };

                let mut req = self.client.get(&endpoint);
                for (key, value) in &self.config.headers {
                    req = req.header(key, value);
                }
                req.send().await
            }
            "POST" => {
                let body: serde_json::Value = if tool_call.function.arguments.is_empty() {
                    serde_json::json!({})
                } else {
                    serde_json::from_str(&tool_call.function.arguments)?
                };

                let mut req = self
                    .client
                    .post(&self.config.endpoint)
                    .header("Content-Type", "application/json")
                    .json(&body);
                for (key, value) in &self.config.headers {
                    req = req.header(key, value);
                }
                req.send().await
            }
            _ => {
                return Ok(ToolCallResult::error(format!(
                    "Unsupported method: {}",
                    method
                )));
            }
        };

        match response {
            Ok(resp) => {
                let status = resp.status();
                match resp.text().await {
                    Ok(body) => {
                        let limited = limit_output(&body);
                        if status.is_success() {
                            Ok(ToolCallResult::success(limited))
                        } else {
                            Ok(ToolCallResult::error(format!(
                                "HTTP {}: {}",
                                status.as_u16(),
                                limited
                            )))
                        }
                    }
                    Err(e) => Ok(ToolCallResult::error(format!(
                        "Failed to read response: {}",
                        e
                    ))),
                }
            }
            Err(e) => Ok(ToolCallResult::error(format!("Request failed: {}", e))),
        }
    }

    fn instructions(&self) -> Option<String> {
        self.config.instruction.clone()
    }
}

// ============================================================================
// User Prompt Toolset
// ============================================================================

/// The user_prompt tool allows an agent to request input from the user.
/// This is useful when the agent needs additional information that wasn't
/// provided in the initial request.
pub struct UserPromptToolset {
    handler: Arc<tokio::sync::Mutex<Option<ElicitationHandler>>>,
}

impl std::fmt::Debug for UserPromptToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UserPromptToolset")
            .field("handler", &"<handler>")
            .finish()
    }
}

impl Default for UserPromptToolset {
    fn default() -> Self {
        Self::new()
    }
}

impl UserPromptToolset {
    pub fn new() -> Self {
        Self {
            handler: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Create a new UserPromptToolset with an elicitation handler
    pub fn with_handler(handler: ElicitationHandler) -> Self {
        Self {
            handler: Arc::new(tokio::sync::Mutex::new(Some(handler))),
        }
    }

    /// Set the elicitation handler
    pub async fn set_handler(&self, handler: ElicitationHandler) {
        let mut guard = self.handler.lock().await;
        *guard = Some(handler);
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UserPromptArgs {
    /// The message or question to display to the user
    pub message: String,
    /// Optional JSON schema for validating the user's response
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}

#[async_trait::async_trait]
impl ToolSet for UserPromptToolset {
    async fn tools(&self) -> anyhow::Result<Vec<Tool>> {
        Ok(vec![make_tool(
            "user_prompt",
            "Request input from the user. Use this when you need additional information that wasn't provided in the initial request.",
            crate::tool_schema!(UserPromptArgs),
            true, // read_only since it doesn't modify external state
        )])
    }

    async fn execute(&self, tool_call: &ToolCall) -> anyhow::Result<ToolCallResult> {
        let args: UserPromptArgs = serde_json::from_str(&tool_call.function.arguments)?;

        let handler_guard = self.handler.lock().await;
        let Some(ref handler) = *handler_guard else {
            return Ok(ToolCallResult::error(
                "No elicitation handler configured. User input is not available in this context.",
            ));
        };

        // Convert schema if provided
        let schema = args.schema.and_then(|v| v.as_object().cloned());

        let request = ElicitationRequest {
            message: args.message.clone(),
            schema,
        };

        // Call the handler
        match handler(request).await {
            Ok(result) => match result.action {
                ElicitationAction::Accept => {
                    if let Some(content) = result.content {
                        Ok(ToolCallResult::success(serde_json::to_string(&content)?))
                    } else {
                        Ok(ToolCallResult::success("User accepted without providing content."))
                    }
                }
                ElicitationAction::Decline => {
                    Ok(ToolCallResult::error("User declined to provide the requested input."))
                }
                ElicitationAction::Cancel => {
                    Ok(ToolCallResult::error("User cancelled the request."))
                }
            },
            Err(e) => Ok(ToolCallResult::error(format!(
                "Failed to get user input: {}",
                e
            ))),
        }
    }

    fn instructions(&self) -> Option<String> {
        Some(
            "## User Prompt Tool\n\nUse the user_prompt tool when you need additional information from the user that wasn't provided in the initial request. The user will be prompted with your message and can provide a response.".to_string(),
        )
    }
}
