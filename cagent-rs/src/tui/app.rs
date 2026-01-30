//! TUI Application - Full-featured terminal interface
//!
//! Features:
//! - Sidebar with session info, token usage, agent info, tools
//! - Main chat area with message list
//! - Multi-line input editor
//! - Status bar with shortcuts
//! - Tool approval dialogs
//! - Markdown-like rendering

use std::io;
use std::time::Duration;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use rand::prelude::*;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use tokio::sync::mpsc;
use tui_textarea::TextArea;

use super::markdown::{highlight_json, render_markdown};
use super::theme::Theme;
use crate::agent::Team;
use crate::runtime::{Event as RuntimeEvent, LocalRuntime, ResumeType, RuntimeConfig};
use crate::session::{Session, SessionItem, SessionMessage};
use crate::tools::Tool;

// ============================================================================
// Constants & Colors
// ============================================================================

const SIDEBAR_WIDTH: u16 = 35;
#[allow(dead_code)]
const MIN_SIDEBAR_WIDTH: u16 = 25;
#[allow(dead_code)]
const MAX_SIDEBAR_WIDTH: u16 = 60;
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Clamp sidebar width to valid bounds (25-50% of window width, min 25, max 60)
fn clamp_sidebar_width(width: u16, window_width: u16) -> u16 {
    let min_percent = (window_width * 25 / 100).max(MIN_SIDEBAR_WIDTH);
    let max_percent = (window_width * 50 / 100).min(MAX_SIDEBAR_WIDTH);
    width.clamp(min_percent, max_percent)
}

// Random working messages (similar to Go TUI)
const WORKING_MESSAGES: &[&str] = &[
    "Thinking...",
    "Processing...",
    "Working on it...",
    "Analyzing...",
    "Computing...",
    "Reticulating splines...",
    "Herding cats...",
    "Consulting the oracle...",
    "Summoning knowledge...",
    "Crunching numbers...",
];

// Available slash commands
const COMMANDS: &[(&str, &str)] = &[
    ("/agent", "Switch to a different agent"),
    ("/clear", "Clear the screen"),
    ("/compact", "Compact session history"),
    ("/config", "Open config in editor"),
    ("/copy", "Copy conversation to clipboard"),
    ("/eval", "Save session as evaluation file"),
    ("/exit", "Exit cagent"),
    ("/export", "Export conversation to file"),
    ("/filter", "Filter messages by type"),
    ("/goto", "Jump to message number"),
    ("/help", "Show available commands"),
    ("/model", "Switch to a different model"),
    ("/new", "Start a new session"),
    ("/quit", "Exit cagent"),
    ("/reset", "Reset current session"),
    ("/search", "Search in conversation"),
    ("/theme", "Change color theme"),
    ("/think", "Toggle thinking/reasoning mode"),
    ("/title", "Set session title"),
    ("/undo", "Remove last message exchange"),
    ("/usage", "Show token usage statistics"),
    ("/wrap", "Toggle word wrap display"),
    ("/yolo", "Toggle auto-approve tools"),
];

// Color scheme - aligned with Go cagent default theme
const COLOR_PRIMARY: Color = Color::Rgb(122, 162, 247); // #7AA2F7 - Accent blue
const COLOR_ACCENT: Color = Color::Rgb(158, 206, 106); // #9ECE6A - Success green
const COLOR_WARNING: Color = Color::Rgb(224, 175, 104); // #E0AF68 - Warning yellow
const COLOR_ERROR: Color = Color::Rgb(247, 118, 142); // #F7768E - Error red
const COLOR_ERROR_STRONG: Color = Color::Rgb(255, 80, 100); // Brighter error for emphasis
const COLOR_ERROR_DARK: Color = Color::Rgb(120, 50, 60);   // Darker error for backgrounds
const COLOR_USER: Color = Color::Rgb(125, 207, 255); // #7DCFFF - Info cyan
const COLOR_ASSISTANT: Color = Color::Rgb(158, 206, 106); // #9ECE6A - Success green
const COLOR_TOOL: Color = Color::Rgb(176, 131, 234); // #B083EA - Badge purple
const COLOR_BRAND: Color = Color::Rgb(29, 99, 237); // #1D63ED - Brand blue
const COLOR_TEXT_PRIMARY: Color = Color::Rgb(192, 192, 192); // #C0C0C0
const COLOR_TEXT_SECONDARY: Color = Color::Rgb(128, 128, 128); // #808080
const COLOR_BACKGROUND: Color = Color::Rgb(28, 28, 34); // #1C1C22
const COLOR_BACKGROUND_ALT: Color = Color::Rgb(38, 38, 48); // #262630 - Alternate background for cards/panels
const COLOR_BORDER: Color = Color::Rgb(107, 117, 168); // #6B75A8

// Selection colors for highlighted items
const COLOR_SELECTED: Color = Color::Rgb(60, 80, 120);       // Selection background
const COLOR_SELECTED_FG: Color = Color::Rgb(220, 220, 255);  // Selection foreground

// Spinner gradient colors (4-level gradient like Go TUI)
const COLOR_SPINNER_DIM: Color = Color::Rgb(80, 80, 100);       // Dimmest
const COLOR_SPINNER_MID: Color = Color::Rgb(120, 120, 160);     // Mid dim
const COLOR_SPINNER_BRIGHT: Color = Color::Rgb(160, 160, 210);  // Bright
const COLOR_SPINNER_BRIGHTEST: Color = Color::Rgb(200, 200, 250); // Brightest

// Diff colors for displaying file differences
const COLOR_DIFF_ADD_BG: Color = Color::Rgb(30, 60, 30);        // Dark green background for additions
const COLOR_DIFF_ADD_FG: Color = Color::Rgb(120, 200, 120);     // Light green foreground for additions
const COLOR_DIFF_REMOVE_BG: Color = Color::Rgb(60, 30, 30);     // Dark red background for removals
const COLOR_DIFF_REMOVE_FG: Color = Color::Rgb(200, 120, 120);  // Light red foreground for removals

// Scrollbar colors (like Go TUI's TrackStyle, ThumbStyle)
const COLOR_SCROLLBAR_TRACK: Color = Color::Rgb(50, 50, 60);     // Dim track
const COLOR_SCROLLBAR_THUMB: Color = Color::Rgb(120, 120, 140);  // Visible thumb
const COLOR_SCROLLBAR_THUMB_ACTIVE: Color = Color::Rgb(160, 160, 200); // Brighter when scrolling

// ============================================================================
// Helper Functions
// ============================================================================

/// Calculate optimal foreground color for a given background
/// Uses relative luminance to determine if black or white provides better contrast
/// Similar to Go TUI's bestForegroundHex()
fn best_foreground_for_bg(bg: Color) -> Color {
    let (r, g, b) = match bg {
        Color::Rgb(r, g, b) => (r as f64, g as f64, b as f64),
        _ => return Color::White, // Default to white for non-RGB colors
    };
    
    // Calculate relative luminance using sRGB formula
    // https://www.w3.org/TR/WCAG20/#relativeluminancedef
    let luminance = 0.2126 * (r / 255.0) + 0.7152 * (g / 255.0) + 0.0722 * (b / 255.0);
    
    // Use black text for bright backgrounds (luminance > 0.5), white for dark
    if luminance > 0.5 {
        Color::Rgb(0, 0, 0) // Black
    } else {
        Color::Rgb(255, 255, 255) // White
    }
}

/// Get spinner color based on frame for gradient animation effect
fn spinner_color_for_frame(frame: usize) -> Color {
    // Cycle through 4 gradient levels based on frame
    match frame % 4 {
        0 => COLOR_SPINNER_DIM,
        1 => COLOR_SPINNER_MID,
        2 => COLOR_SPINNER_BRIGHT,
        _ => COLOR_SPINNER_BRIGHTEST,
    }
}

/// Replace home directory with ~/
fn shorten_home_dir(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return format!("~{}", &path[home_str.len()..]);
        }
    }
    path.to_string()
}

// ============================================================================
// Chat Message
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Error,
    Thinking,
    Tool,
    ShellOutput,
    Cancelled,
    Welcome,
    /// Loading indicator with spinner (shows during async operations)
    Loading,
}

/// Tool execution status for tool messages
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ToolStatus {
    /// Tool is pending (waiting to execute)
    #[default]
    Pending,
    /// Tool is currently running
    Running,
    /// Tool completed successfully
    Completed,
    /// Tool encountered an error
    Error,
    /// Tool awaiting user confirmation
    Confirmation,
}

#[derive(Debug, Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
    pub agent: Option<String>,
    pub tool_name: Option<String>,
    pub tool_description: Option<String>,
    pub timestamp: String,
    /// For tool messages: whether the output is collapsed
    pub collapsed: bool,
    /// For tool messages: the full output (may be truncated in display)
    pub tool_output: Option<String>,
    /// For tool messages: execution status
    pub tool_status: ToolStatus,
}

impl ChatMessage {
    fn new(role: MessageRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            agent: None,
            tool_name: None,
            tool_description: None,
            timestamp: chrono::Local::now().format("%H:%M").to_string(),
            collapsed: true, // Tool outputs are collapsed by default
            tool_output: None,
            tool_status: ToolStatus::default(),
        }
    }

    fn with_agent(mut self, agent: &str) -> Self {
        self.agent = Some(agent.to_string());
        self
    }

    fn with_tool_name(mut self, name: &str) -> Self {
        self.tool_name = Some(name.to_string());
        self
    }

    fn with_tool_description(mut self, description: &str) -> Self {
        self.tool_description = Some(description.to_string());
        self
    }

    pub fn user(content: &str) -> Self {
        Self::new(MessageRole::User, content)
    }

    pub fn assistant(content: &str, agent: &str) -> Self {
        Self::new(MessageRole::Assistant, content).with_agent(agent)
    }

    pub fn system(content: &str) -> Self {
        Self::new(MessageRole::System, content)
    }

    pub fn error(content: &str) -> Self {
        Self::new(MessageRole::Error, content)
    }

        pub fn tool(name: &str, description: &str, agent: &str) -> Self {
        let mut msg = Self::new(MessageRole::Tool, format!("Running {}...", name))
            .with_agent(agent)
            .with_tool_name(name)
            .with_tool_description(description);
        msg.tool_status = ToolStatus::Running;
        msg
    }

    pub fn tool_result(name: &str, agent: &str, output: &str, is_error: bool) -> Self {
        let preview = if output.len() > 100 {
            format!("{}...", &output[..100])
        } else {
            output.to_string()
        };
        let status = if is_error { "✗" } else { "✓" };
        let mut msg = Self::new(MessageRole::Tool, format!("{} {}", status, preview))
            .with_agent(agent)
            .with_tool_name(name);
        msg.tool_output = Some(output.to_string());
        msg.collapsed = true; // Collapsed by default
        msg.tool_status = if is_error {
            ToolStatus::Error
        } else {
            ToolStatus::Completed
        };
        msg
    }

    pub fn thinking(content: &str, agent: &str) -> Self {
        Self::new(MessageRole::Thinking, content).with_agent(agent)
    }

    pub fn shell_output(content: &str) -> Self {
        Self::new(MessageRole::ShellOutput, content)
    }

    pub fn cancelled(agent: &str) -> Self {
        Self::new(MessageRole::Cancelled, "⚠ stream cancelled ⚠").with_agent(agent)
    }

    pub fn welcome(content: &str) -> Self {
        Self::new(MessageRole::Welcome, content)
    }

    /// Create a loading message with description (shows spinner during render)
    pub fn loading(description: &str, agent: &str) -> Self {
        Self::new(MessageRole::Loading, description).with_agent(agent)
    }
}

// ============================================================================
// Pending Confirmation
// ============================================================================

#[derive(Debug, Clone)]
pub struct PendingConfirmation {
    pub tool_name: String,
    pub tool_args: String,
    /// Tool description (from tool definition)
    pub tool_description: Option<String>,
    /// For edit_file: contains the diff preview
    pub diff_preview: Option<DiffPreview>,
}

/// Pending elicitation request from an MCP server
#[derive(Debug, Clone)]
pub struct PendingElicitation {
    /// Unique request ID for matching responses
    pub request_id: String,
    /// Human-readable message explaining what information is needed
    pub message: String,
    /// Name of the MCP server making the request
    pub server_name: String,
    /// The JSON schema describing the expected input format
    pub requested_schema: serde_json::Value,
    /// User's input (for form-based elicitation)
    pub user_input: String,
    /// Current cursor position in the input field (for multi-field forms)
    pub field_index: usize,
    /// Parsed schema fields (for form-based rendering)
    pub fields: Vec<ElicitationField>,
}

/// A field in an elicitation form
#[derive(Debug, Clone)]
pub struct ElicitationField {
    pub name: String,
    pub description: String,
    pub field_type: ElicitationFieldType,
    pub required: bool,
    pub value: String,
}

/// Field types for elicitation forms
#[derive(Debug, Clone)]
pub enum ElicitationFieldType {
    Text,
    Password,
    Number,
    Boolean,
    Select(Vec<String>),
}

impl ElicitationField {
    /// Parse JSON schema properties into ElicitationFields
    pub fn from_schema(schema: &serde_json::Value) -> Vec<Self> {
        let mut fields = Vec::new();
        
        // Get properties from schema
        let properties = match schema.get("properties") {
            Some(serde_json::Value::Object(props)) => props,
            _ => return fields,
        };
        
        // Get required fields list
        let required: Vec<&str> = schema
            .get("required")
            .and_then(|r| r.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        
        for (name, prop) in properties {
            let description = prop
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            
            let field_type = match prop.get("type").and_then(|t| t.as_str()) {
                Some("string") => {
                    // Check for format or enum
                    if prop.get("format").and_then(|f| f.as_str()) == Some("password") {
                        ElicitationFieldType::Password
                    } else if let Some(serde_json::Value::Array(opts)) = prop.get("enum") {
                        let options: Vec<String> = opts
                            .iter()
                            .filter_map(|v| v.as_str())
                            .map(|s| s.to_string())
                            .collect();
                        if options.is_empty() {
                            ElicitationFieldType::Text
                        } else {
                            ElicitationFieldType::Select(options)
                        }
                    } else {
                        ElicitationFieldType::Text
                    }
                }
                Some("number") | Some("integer") => ElicitationFieldType::Number,
                Some("boolean") => ElicitationFieldType::Boolean,
                _ => ElicitationFieldType::Text,
            };
            
            fields.push(Self {
                name: name.clone(),
                description,
                field_type,
                required: required.contains(&name.as_str()),
                value: String::new(),
            });
        }
        
        // Sort fields: required first, then alphabetically
        fields.sort_by(|a, b| {
            match (a.required, b.required) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.cmp(&b.name),
            }
        });
        
        fields
    }
}

/// Diff preview for file edit operations
#[derive(Debug, Clone)]
pub struct DiffPreview {
    pub file_path: String,
    pub hunks: Vec<DiffHunk>,
}

/// A single diff hunk showing context and changes
#[derive(Debug, Clone)]
pub struct DiffHunk {
    pub old_start: usize,
    pub new_start: usize,
    pub lines: Vec<DiffLine>,
}

/// A single line in a diff
#[derive(Debug, Clone)]
pub enum DiffLine {
    Context(String), // Unchanged line (shown for context)
    Removed(String), // Line being removed (red, prefixed with -)
    Added(String),   // Line being added (green, prefixed with +)
}

// ============================================================================
// Application State
// ============================================================================

pub struct App {
    // Input
    input: TextArea<'static>,
    #[allow(dead_code)]
    input_mode: InputMode,

    // Messages
    messages: Vec<ChatMessage>,
    messages_scroll_offset: usize, // Line-based scroll offset
    messages_state: ListState,
    messages_view_height: usize, // Cached view height for scrolling
    messages_total_lines: usize, // Cached total lines for scrollbar

    // Sidebar state
    sidebar_collapsed: bool,
    sidebar_width: u16,
    #[allow(dead_code)]
    sidebar_scroll: usize,

    // Session info
    session_title: String,
    working_directory: String,
    session_created_at: chrono::DateTime<chrono::Local>,
    message_count: usize,

    // Token usage
    input_tokens: i64,
    output_tokens: i64,
    cost: f64,

    // Context window tracking
    context_length: i64,
    context_limit: i64,

    // Agent info
    current_agent: String,
    agent_model: String,
    agent_description: String,
    available_agents: Vec<String>,
    available_tools: Vec<String>,
    tools_loading: bool, // True when MCP tools are being initialized
    agent_switching: bool, // True when agent is being switched

    // Status
    working: bool,
    working_message: String,
    spinner_frame: usize,
    status_message: String,

    // Tool confirmation
    pending_confirmation: Option<PendingConfirmation>,
    tools_approved: bool,

    /// Pending MCP elicitation request
    pending_elicitation: Option<PendingElicitation>,

    /// Track which tools have been individually approved (when not in YOLO mode)
    approved_tools: std::collections::HashSet<String>,

    // App state
    should_quit: bool,
    show_exit_confirmation: bool,
    /// Tracks if ctrl-c was pressed recently (for double ctrl-c to exit)
    #[allow(dead_code)]
    last_ctrl_c_time: Option<std::time::Instant>,

    // Command completion
    showing_completions: bool,
    completion_index: usize,

    // File completion (for @ references)
    showing_file_completions: bool,
    file_completions: Vec<String>,
    file_completion_index: usize,
    file_completion_prefix: String,

    // Sidebar section collapse state
    sidebar_sections_collapsed: [bool; 4], // [Session, TokenUsage, Agent, Tools]

    // Sidebar focus state
    sidebar_focused: bool,
    sidebar_selected_section: usize, // 0-3 for the four sections

    // Word wrap toggle for messages
    word_wrap_enabled: bool,

    // Command history
    command_history: Vec<String>,
    history_index: Option<usize>,
    history_temp: String, // Temp storage for current input when browsing history

    // Theme
    theme: Theme,

    // Reasoning/thinking mode indicator
    thinking_mode: bool,

    // Accessibility
    accessibility_announcements: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputMode {
    Normal,
    Insert,
    Command,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        let mut input = TextArea::default();
        input.set_cursor_line_style(Style::default());
        input.set_placeholder_text("Type a message... (Enter to send, Ctrl+C to quit)");

        let working_dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "~".to_string());

        Self {
            input,
            input_mode: InputMode::Insert,
            messages: Vec::new(),
            messages_scroll_offset: 0,
            messages_state: ListState::default(),
            messages_view_height: 0,
            messages_total_lines: 0,
            sidebar_collapsed: false,
            sidebar_width: SIDEBAR_WIDTH,
            sidebar_scroll: 0,
            session_title: "New Session".to_string(),
            working_directory: working_dir,
            session_created_at: chrono::Local::now(),
            message_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            cost: 0.0,
            context_length: 0,
            context_limit: 0,
            current_agent: "root".to_string(),
            agent_model: String::new(),
            agent_description: String::new(),
            available_agents: Vec::new(),
            available_tools: Vec::new(),
            tools_loading: true, // Start with loading state
            agent_switching: false,
            working: false,
            working_message: WORKING_MESSAGES[0].to_string(),
            spinner_frame: 0,
            status_message: "Ready".to_string(),
            pending_confirmation: None,
            tools_approved: false,
            pending_elicitation: None,
            approved_tools: std::collections::HashSet::new(),
            should_quit: false,
            show_exit_confirmation: false,
            showing_completions: false,
            completion_index: 0,
            showing_file_completions: false,
            file_completions: Vec::new(),
            file_completion_index: 0,
            file_completion_prefix: String::new(),
            sidebar_sections_collapsed: [false; 4],
            sidebar_focused: false,
            sidebar_selected_section: 0,
            word_wrap_enabled: true, // Default to on
            command_history: Vec::new(),
            history_index: None,
            history_temp: String::new(),
            theme: Theme::detect_preferred(),
            thinking_mode: false,
            accessibility_announcements: std::env::var("CAGENT_A11Y").is_ok(),
            last_ctrl_c_time: None,
        }
    }

    pub fn add_message(&mut self, msg: ChatMessage) {
        self.messages.push(msg);
        self.message_count = self.messages.len();
        self.scroll_to_bottom();
    }

    pub fn scroll_to_bottom(&mut self) {
        // Scroll to show the last content
        let max_offset = self
            .messages_total_lines
            .saturating_sub(self.messages_view_height);
        self.messages_scroll_offset = max_offset;
        // Also update list state for compatibility
        if !self.messages.is_empty() {
            self.messages_state.select(Some(self.messages.len() - 1));
        }
    }

    pub fn scroll_up(&mut self) {
        if self.messages_scroll_offset > 0 {
            self.messages_scroll_offset = self.messages_scroll_offset.saturating_sub(3);
        }
    }

    pub fn scroll_down(&mut self) {
        let max_offset = self
            .messages_total_lines
            .saturating_sub(self.messages_view_height);
        if self.messages_scroll_offset < max_offset {
            self.messages_scroll_offset = (self.messages_scroll_offset + 3).min(max_offset);
        }
    }

    pub fn scroll_page_up(&mut self) {
        let page_size = self.messages_view_height.max(1);
        self.messages_scroll_offset = self.messages_scroll_offset.saturating_sub(page_size);
    }

    pub fn scroll_page_down(&mut self) {
        let page_size = self.messages_view_height.max(1);
        let max_offset = self
            .messages_total_lines
            .saturating_sub(self.messages_view_height);
        self.messages_scroll_offset = (self.messages_scroll_offset + page_size).min(max_offset);
    }

    /// Toggle the collapsed state of the currently selected message (for tool outputs)
    pub fn toggle_selected_message_collapse(&mut self) {
        if let Some(selected) = self.messages_state.selected() {
            if let Some(msg) = self.messages.get_mut(selected) {
                if msg.role == MessageRole::Tool && msg.tool_output.is_some() {
                    msg.collapsed = !msg.collapsed;
                }
            }
        }
    }

    /// Select the previous message in the list
    pub fn select_previous_message(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let new_idx = match self.messages_state.selected() {
            Some(idx) => idx.saturating_sub(1),
            None => self.messages.len().saturating_sub(1),
        };
        self.messages_state.select(Some(new_idx));
    }

    /// Select the next message in the list
    pub fn select_next_message(&mut self) {
        if self.messages.is_empty() {
            return;
        }
        let new_idx = match self.messages_state.selected() {
            Some(idx) => (idx + 1).min(self.messages.len().saturating_sub(1)),
            None => 0,
        };
        self.messages_state.select(Some(new_idx));
    }

    /// Clear message selection
    pub fn clear_message_selection(&mut self) {
        self.messages_state.select(None);
    }

    /// Increase sidebar width
    pub fn increase_sidebar_width(&mut self) {
        self.sidebar_width = (self.sidebar_width + 2).min(MAX_SIDEBAR_WIDTH);
    }

    /// Decrease sidebar width
    pub fn decrease_sidebar_width(&mut self) {
        self.sidebar_width = self.sidebar_width.saturating_sub(2).max(MIN_SIDEBAR_WIDTH);
    }

    fn get_input_content(&self) -> String {
        self.input.lines().join("\n")
    }

    fn clear_input(&mut self) {
        self.input = TextArea::default();
        self.input.set_cursor_line_style(Style::default());
        self.input
            .set_placeholder_text("Type a message... (Enter to send, Ctrl+C to quit)");
    }

    pub fn set_tools(&mut self, tools: Vec<Tool>) {
        self.available_tools = tools.iter().map(|t| t.name.clone()).collect();
        self.tools_loading = false;
    }

    /// Get the current theme
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// Set a new theme
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }

    /// Announce a message for screen readers (writes to stderr if enabled)
    pub fn announce(&self, message: &str) {
        if self.accessibility_announcements {
            // Write to stderr so screen readers can pick it up
            eprintln!("[cagent] {}", message);
        }
    }

    /// Announce and add a message
    pub fn add_message_with_announcement(&mut self, msg: ChatMessage, announcement: &str) {
        self.announce(announcement);
        self.add_message(msg);
    }
}

// ============================================================================
// Session Restoration
// ============================================================================

fn restore_messages_from_session(app: &mut App, session: &Session) {
    // Reset message view
    app.messages.clear();
    app.messages_state.select(None);

    // Build a mapping from tool_call_id -> tool name by scanning assistant tool calls.
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut flattened: Vec<SessionMessage> = Vec::new();

    fn flatten_items(out: &mut Vec<SessionMessage>, items: &[SessionItem]) {
        for item in items {
            match item {
                SessionItem::Message { message } => out.push((**message).clone()),
                SessionItem::SubSession { sub_session } => {
                    flatten_items(out, &sub_session.messages)
                }
                SessionItem::Summary { .. } => {}
            }
        }
    }

    flatten_items(&mut flattened, &session.messages);

    for sm in &flattened {
        for tc in &sm.message.tool_calls {
            if !tc.id.is_empty() && !tc.function.name.is_empty() {
                tool_names
                    .entry(tc.id.clone())
                    .or_insert_with(|| tc.function.name.clone());
            }
        }
    }

    fn timestamp_for(created_at: &Option<String>) -> String {
        let Some(ref ts) = created_at else {
            return chrono::Local::now().format("%H:%M").to_string();
        };
        chrono::DateTime::parse_from_rfc3339(ts)
            .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
            .unwrap_or_else(|_| chrono::Local::now().format("%H:%M").to_string())
    }

    for sm in flattened {
        let agent = sm
            .agent_name
            .clone()
            .unwrap_or_else(|| app.current_agent.clone());
        let ts = timestamp_for(&sm.message.created_at);

        let mut chat_msg = match sm.message.role {
            crate::chat::MessageRole::System => {
                // The session contains internal system messages; don't render them in the TUI.
                continue;
            }
            crate::chat::MessageRole::User => ChatMessage::user(&sm.message.content),
            crate::chat::MessageRole::Assistant => {
                ChatMessage::assistant(&sm.message.content, &agent)
            }
            crate::chat::MessageRole::Tool => {
                let name = sm
                    .message
                    .tool_call_id
                    .as_ref()
                    .and_then(|id| tool_names.get(id))
                    .cloned()
                    .unwrap_or_else(|| "tool".to_string());

                // Best-effort error detection.
                let is_error = sm.message.content.starts_with("Error:")
                    || sm.message.content.starts_with("Failed")
                    || sm.message.content.starts_with("Rejected");

                ChatMessage::tool_result(&name, &agent, &sm.message.content, is_error)
            }
        };

        chat_msg.timestamp = ts;
        app.add_message(chat_msg);
    }

    app.scroll_to_bottom();
}

// ============================================================================
// TUI Runner
// ============================================================================

pub async fn run_tui(team: Team, yolo: bool, hide_tool_results: bool) -> anyhow::Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app state
    let mut app = App::new();
    app.tools_approved = yolo;

    // Set agent info from team
    if let Some(agent) = team.default_agent() {
        app.current_agent = agent.name.clone();
        app.agent_description = agent.description.clone().unwrap_or_default();
        app.agent_model = agent.get_model().map(|m| m.id()).unwrap_or_default();

        // Get tools
        if let Ok(tools) = futures::executor::block_on(agent.tools()) {
            app.set_tools(tools);
        }
    }

    // Create runtime
    let runtime = LocalRuntime::new(team, RuntimeConfig::default())?;
    let store = runtime.session_store();

    // Create / load session
    let mut session = store
        .get_last()
        .await?
        .unwrap_or_else(Session::new)
        .with_working_dir(&app.working_directory)
        .with_tools_approved(yolo);

    session.hide_tool_results = hide_tool_results;

    // Ensure per-agent iteration limit is configured for this session.
    // (Used by runtime AgentLoop)
    if session.max_iterations == 0 {
        if let Some(agent) = runtime.current_agent().await {
            session.max_iterations = agent.max_iterations;
        }
    }

    // If we loaded an existing session, restore message view from it
    if !session.messages.is_empty() {
        restore_messages_from_session(&mut app, &session);
        app.session_title = session.title.clone();
        app.session_created_at = session.created_at.with_timezone(&chrono::Local);
        app.input_tokens = session.input_tokens;
        app.output_tokens = session.output_tokens;
        app.cost = session.cost;
    }

    // Persist session immediately (sets last_session pointer)
    store.save(&session).await?;

    // Create channel for runtime events
    let (event_tx, mut event_rx) = mpsc::channel::<RuntimeEvent>(128);

    // Add welcome message
    app.add_message(ChatMessage::system(
        "Welcome to cagent! Type a message to get started.\n\n\
         Shortcuts: Ctrl+C quit | ↑/↓ scroll | /help commands",
    ));

    // Main event loop
    let tick_rate = Duration::from_millis(100);

    loop {
        // Draw UI
        terminal.draw(|f| draw_ui(f, &mut app))?;

        // Check for runtime events (non-blocking)
        // Process all events but working flag controls whether we show them
        while let Ok(event) = event_rx.try_recv() {
            // Always process state-changing events (StreamStopped, Error)
            // But skip content events if we've already cancelled
            match &event {
                RuntimeEvent::StreamStopped { .. } | RuntimeEvent::Error { .. } => {
                    handle_runtime_event(&mut app, event, session.hide_tool_results);
                }
                _ if app.working => {
                    handle_runtime_event(&mut app, event, session.hide_tool_results);
                }
                _ => {
                    // Discard content events from cancelled stream
                }
            }
        }

        // Handle input events with timeout
        if event::poll(tick_rate)? {
            match event::read()? {
                Event::Mouse(mouse_event) => {
                    use crossterm::event::{MouseButton, MouseEventKind};
                    match mouse_event.kind {
                        MouseEventKind::ScrollUp => {
                            // Scroll up by 3 lines (like Go's mouseScrollAmount * defaultScrollAmount)
                            app.scroll_up();
                            app.scroll_up();
                        }
                        MouseEventKind::ScrollDown => {
                            // Scroll down by 3 lines
                            app.scroll_down();
                            app.scroll_down();
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            // Could add click-to-scroll or drag support here
                        }
                        _ => {}
                    }
                }
                Event::Key(key) => {
                    // Handle exit confirmation dialog first
                    if app.show_exit_confirmation {
                        match (key.code, key.modifiers) {
                            (KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter, _) => {
                                app.should_quit = true;
                            }
                            // Ctrl+C while in the confirmation dialog confirms exit.
                            (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                                app.should_quit = true;
                            }
                            (KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc, _) => {
                                app.show_exit_confirmation = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Handle tool confirmation dialog
                    if app.pending_confirmation.is_some() {
                        match key.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                // Track this tool as approved for this session
                                if let Some(ref conf) = app.pending_confirmation {
                                    app.approved_tools.insert(conf.tool_name.clone());
                                }
                                app.pending_confirmation = None;
                                runtime.resume(ResumeType::Approve).await;
                            }
                            KeyCode::Char('a') | KeyCode::Char('A') => {
                                app.pending_confirmation = None;
                                app.tools_approved = true;
                                runtime.resume(ResumeType::ApproveSession).await;
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                app.pending_confirmation = None;
                                runtime
                                    .resume(ResumeType::Reject {
                                        reason: Some("User declined".to_string()),
                                    })
                                    .await;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Global shortcuts
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            // Always show exit confirmation dialog
                            app.show_exit_confirmation = true;
                        }
                        (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
                            app.sidebar_collapsed = !app.sidebar_collapsed;
                        }
                        // Ctrl+P - Command palette (show help for now)
                        (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
                            handle_command(&mut app, "/help", &mut session);
                        }
                        // Ctrl+G - Open external editor for multi-line input
                        (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
                            // Get current input content
                            let current_content = app.get_input_content();
                            
                            // Create a temporary file
                            let temp_dir = std::env::temp_dir();
                            let temp_file = temp_dir.join(format!("cagent_input_{}.txt", std::process::id()));
                            
                            // Write current content to temp file
                            if let Err(e) = std::fs::write(&temp_file, &current_content) {
                                app.add_message(ChatMessage::error(&format!("Failed to create temp file: {}", e)));
                            } else {
                                // Get editor from environment
                                let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
                                    if cfg!(target_os = "macos") {
                                        "nano".to_string()
                                    } else if cfg!(target_os = "windows") {
                                        "notepad".to_string()
                                    } else {
                                        "vi".to_string()
                                    }
                                });
                                
                                // Need to restore terminal before opening editor
                                let _ = crossterm::terminal::disable_raw_mode();
                                let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
                                
                                // Open editor
                                let status = std::process::Command::new(&editor)
                                    .arg(&temp_file)
                                    .status();
                                
                                // Restore terminal
                                let _ = crossterm::terminal::enable_raw_mode();
                                let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen);
                                
                                match status {
                                    Ok(exit_status) if exit_status.success() => {
                                        // Read edited content back
                                        if let Ok(new_content) = std::fs::read_to_string(&temp_file) {
                                            app.clear_input();
                                            app.input.insert_str(&new_content);
                                        }
                                    }
                                    Ok(_) => {
                                        app.add_message(ChatMessage::system("Editor exited without saving"));
                                    }
                                    Err(e) => {
                                        app.add_message(ChatMessage::error(&format!("Failed to open editor: {}", e)));
                                    }
                                }
                                
                                // Clean up temp file
                                let _ = std::fs::remove_file(&temp_file);
                            }
                        }
                        // Ctrl+T - Toggle theme (cycle through themes)
                        (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
                            let current = app.theme.name;
                            let themes = Theme::available_themes();
                            let current_idx =
                                themes.iter().position(|&t| t == current).unwrap_or(0);
                            let next_idx = (current_idx + 1) % themes.len();
                            if let Some(theme) = Theme::by_name(themes[next_idx]) {
                                app.theme = theme;
                                app.add_message(ChatMessage::system(&format!(
                                    "✓ Theme: {}",
                                    themes[next_idx]
                                )));
                            }
                        }
                        // Ctrl+Z - Undo last exchange
                        (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
                            handle_command(&mut app, "/undo", &mut session);
                        }
                        // Ctrl+F - Search (prompts in input)
                        (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
                            app.clear_input();
                            app.input.insert_str("/search ");
                        }
                        // Alt+1-4 to toggle sidebar sections
                        (KeyCode::Char('1'), KeyModifiers::ALT) => {
                            app.sidebar_sections_collapsed[0] = !app.sidebar_sections_collapsed[0];
                        }
                        (KeyCode::Char('2'), KeyModifiers::ALT) => {
                            app.sidebar_sections_collapsed[1] = !app.sidebar_sections_collapsed[1];
                        }
                        (KeyCode::Char('3'), KeyModifiers::ALT) => {
                            app.sidebar_sections_collapsed[2] = !app.sidebar_sections_collapsed[2];
                        }
                        (KeyCode::Char('4'), KeyModifiers::ALT) => {
                            app.sidebar_sections_collapsed[3] = !app.sidebar_sections_collapsed[3];
                        }
                        (KeyCode::Tab, KeyModifiers::NONE) if app.showing_completions => {
                            // Cycle through command completions
                            let content = app.get_input_content();
                            let matches = get_matching_commands(&content);
                            if !matches.is_empty() {
                                app.completion_index = (app.completion_index + 1) % matches.len();
                            }
                        }
                        (KeyCode::Tab, KeyModifiers::NONE) if app.showing_file_completions => {
                            // Cycle through file completions
                            if !app.file_completions.is_empty() {
                                app.file_completion_index =
                                    (app.file_completion_index + 1) % app.file_completions.len();
                            }
                        }
                        (KeyCode::Tab, KeyModifiers::SHIFT) if app.showing_completions => {
                            // Cycle backwards through command completions
                            let content = app.get_input_content();
                            let matches = get_matching_commands(&content);
                            if !matches.is_empty() {
                                app.completion_index = app
                                    .completion_index
                                    .checked_sub(1)
                                    .unwrap_or(matches.len() - 1);
                            }
                        }
                        (KeyCode::Tab, KeyModifiers::SHIFT) if app.showing_file_completions => {
                            // Cycle backwards through file completions
                            if !app.file_completions.is_empty() {
                                app.file_completion_index = app
                                    .file_completion_index
                                    .checked_sub(1)
                                    .unwrap_or(app.file_completions.len() - 1);
                            }
                        }
                        (KeyCode::Tab, _)
                            if !app.showing_completions && !app.showing_file_completions =>
                        {
                            // Toggle focus between sidebar and main area
                            if !app.sidebar_collapsed {
                                app.sidebar_focused = !app.sidebar_focused;
                            }
                        }
                        _ => {}
                    }

                    // Input handling (only when sidebar is not focused)
                    if app.sidebar_focused {
                        // Sidebar navigation
                        match key.code {
                            KeyCode::Up => {
                                if app.sidebar_selected_section > 0 {
                                    app.sidebar_selected_section -= 1;
                                }
                            }
                            KeyCode::Down => {
                                if app.sidebar_selected_section < 4 {
                                    app.sidebar_selected_section += 1;
                                }
                            }
                            KeyCode::Enter | KeyCode::Char(' ') => {
                                // Toggle collapse for the selected section
                                app.sidebar_sections_collapsed[app.sidebar_selected_section] =
                                    !app.sidebar_sections_collapsed[app.sidebar_selected_section];
                            }
                            KeyCode::Left
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                // Ctrl+Left: Decrease sidebar width
                                app.decrease_sidebar_width();
                            }
                            KeyCode::Right
                                if key.modifiers.contains(KeyModifiers::CONTROL) =>
                            {
                                // Ctrl+Right: Increase sidebar width
                                app.increase_sidebar_width();
                            }
                            KeyCode::Left => {
                                // In Session section: scroll to previous message
                                if app.sidebar_selected_section == 0 {
                                    app.scroll_up();
                                }
                            }
                            KeyCode::Right => {
                                // In Session section: scroll to next message
                                if app.sidebar_selected_section == 0 {
                                    app.scroll_down();
                                }
                            }
                            KeyCode::Home => {
                                // Jump to first message
                                if !app.messages.is_empty() {
                                    app.messages_state.select(Some(0));
                                }
                            }
                            KeyCode::End => {
                                // Jump to last message
                                app.scroll_to_bottom();
                            }
                            KeyCode::Esc => {
                                // Exit sidebar focus
                                app.sidebar_focused = false;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Input handling
                    match key.code {
                        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            // Shift+Enter: Insert new line with smart indent
                            let lines = app.input.lines();
                            let (row, _) = app.input.cursor();

                            // Get current line's indentation
                            let indent = if let Some(line) = lines.get(row) {
                                let leading_spaces: String =
                                    line.chars().take_while(|c| c.is_whitespace()).collect();
                                leading_spaces
                            } else {
                                String::new()
                            };

                            // Insert newline with indentation
                            app.input.insert_newline();
                            if !indent.is_empty() {
                                app.input.insert_str(&indent);
                            }
                        }
                        KeyCode::Enter => {
                            let content = app.get_input_content();

                            // If showing file completions and pressing Enter, accept the selected file
                            if app.showing_file_completions {
                                if let Some(file) =
                                    app.file_completions.get(app.file_completion_index)
                                {
                                    // Replace the @ reference with the selected file
                                    let file_to_insert = file.clone();

                                    // Find the @ position and replace
                                    let lines = app.input.lines();
                                    let (row, col) = app.input.cursor();
                                    let mut cursor_pos = 0;
                                    for (i, line) in lines.iter().enumerate() {
                                        if i < row {
                                            cursor_pos += line.len() + 1;
                                        } else {
                                            cursor_pos += col;
                                            break;
                                        }
                                    }

                                    if let Some((_at_pos, prefix)) =
                                        extract_at_reference(&content, cursor_pos)
                                    {
                                        // Delete the prefix and insert the file
                                        for _ in 0..prefix.len() {
                                            app.input.delete_char();
                                        }
                                        app.input.insert_str(&file_to_insert);
                                    }

                                    app.showing_file_completions = false;
                                    continue;
                                }
                            }

                            // If showing command completions and pressing Enter, accept the selected completion
                            if app.showing_completions {
                                let matches = get_matching_commands(&content);
                                if let Some((cmd, _)) = matches.get(app.completion_index) {
                                    app.clear_input();
                                    app.input.insert_str(*cmd);
                                    app.showing_completions = false;
                                    continue;
                                }
                            }

                            if !content.trim().is_empty() {
                                // Add to command history
                                if app
                                    .command_history
                                    .last()
                                    .map(|s| s != &content)
                                    .unwrap_or(true)
                                {
                                    app.command_history.push(content.clone());
                                    // Keep only last 100 commands
                                    if app.command_history.len() > 100 {
                                        app.command_history.remove(0);
                                    }
                                }
                                app.history_index = None;

                                app.clear_input();
                                app.showing_completions = false;

                                // Handle commands
                                if content.starts_with('/') {
                                    handle_command(&mut app, &content, &mut session);
                                } else {
                                    // Send message
                                    app.add_message(ChatMessage::user(&content));
                                    app.working = true;
                                    // Select a random working message
                                    let mut rng = rand::rng();
                                    app.working_message = WORKING_MESSAGES.choose(&mut rng)
                                        .unwrap_or(&WORKING_MESSAGES[0])
                                        .to_string();
                                    app.status_message = app.working_message.clone();

                                    session = session.with_user_message(&content);

                                    // Run agent
                                    let mut events = runtime.run_stream(&mut session).await;
                                    let tx = event_tx.clone();
                                    tokio::spawn(async move {
                                        while let Some(event) = events.recv().await {
                                            if tx.send(event).await.is_err() {
                                                break;
                                            }
                                        }
                                    });
                                }
                            }
                        }
                        KeyCode::Esc => {
                            // Close any completion popup
                            app.showing_completions = false;
                            app.showing_file_completions = false;
                        }
                        KeyCode::Up if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Command history - previous
                            if !app.command_history.is_empty() {
                                match app.history_index {
                                    None => {
                                        // Save current input and start browsing history
                                        app.history_temp = app.get_input_content();
                                        app.history_index = Some(app.command_history.len() - 1);
                                        let cmd = app.command_history.last().unwrap().clone();
                                        app.clear_input();
                                        app.input.insert_str(&cmd);
                                    }
                                    Some(idx) if idx > 0 => {
                                        app.history_index = Some(idx - 1);
                                        let cmd = app.command_history[idx - 1].clone();
                                        app.clear_input();
                                        app.input.insert_str(&cmd);
                                    }
                                    _ => {}
                                }
                            }
                        }
                        KeyCode::Down if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Command history - next
                            if let Some(idx) = app.history_index {
                                if idx < app.command_history.len() - 1 {
                                    app.history_index = Some(idx + 1);
                                    let cmd = app.command_history[idx + 1].clone();
                                    app.clear_input();
                                    app.input.insert_str(&cmd);
                                } else {
                                    // Return to current input
                                    app.history_index = None;
                                    let temp = app.history_temp.clone();
                                    app.clear_input();
                                    app.input.insert_str(&temp);
                                }
                            }
                        }
                        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.scroll_up();
                        }
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            app.scroll_down();
                        }
                        // Ctrl+Shift+Up/Down for message selection (not line scrolling)
                        KeyCode::Up if key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
                            app.select_previous_message();
                        }
                        KeyCode::Down if key.modifiers.contains(KeyModifiers::CONTROL | KeyModifiers::SHIFT) => {
                            app.select_next_message();
                        }
                        // Escape clears message selection
                        KeyCode::Esc if app.messages_state.selected().is_some() && !app.working => {
                            app.clear_message_selection();
                        }
                        KeyCode::Esc if app.working => {
                            // Interrupt the running agent stream
                            // Add a cancelled message to show the user
                            app.add_message(ChatMessage::cancelled(&app.current_agent));
                            app.working = false;
                            app.status_message = "Stream cancelled".to_string();
                            // Note: The background task will continue but its events will be ignored
                            // since we set working = false
                        }
                        KeyCode::PageUp => {
                            for _ in 0..5 {
                                app.scroll_up();
                            }
                        }
                        KeyCode::PageDown => {
                            for _ in 0..5 {
                                app.scroll_down();
                            }
                        }
                        KeyCode::Home if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl+Home: Scroll to top
                            app.messages_scroll_offset = 0;
                        }
                        KeyCode::End if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            // Ctrl+End: Scroll to bottom
                            app.scroll_to_bottom();
                        }
                        // Space key toggles collapsed state for tool messages
                        KeyCode::Char(' ')
                            if key.modifiers.is_empty()
                                && app.messages_state.selected().is_some() =>
                        {
                            // Only toggle if we're scrolled to a message (not in input mode)
                            // Check if the selected message is a tool message with output
                            if let Some(selected) = app.messages_state.selected() {
                                if let Some(msg) = app.messages.get(selected) {
                                    if msg.role == MessageRole::Tool && msg.tool_output.is_some() {
                                        app.toggle_selected_message_collapse();
                                        continue; // Don't insert space into input
                                    }
                                }
                            }
                            // If not a tool message, insert space normally
                            app.input.insert_char(' ');
                            update_completion_state(&mut app);
                        }
                        KeyCode::Char(c) => {
                            // Check if we should skip over a closing bracket/quote
                            let skip_chars = [')', ']', '}', '"', '\'', '`'];
                            if skip_chars.contains(&c) {
                                // Get character after cursor
                                let lines = app.input.lines();
                                let (row, col) = app.input.cursor();
                                if let Some(line) = lines.get(row) {
                                    if let Some(next_char) = line.chars().nth(col) {
                                        if next_char == c {
                                            // Skip over the closing character
                                            app.input
                                                .move_cursor(tui_textarea::CursorMove::Forward);
                                            // Update completion state and continue
                                            update_completion_state(&mut app);
                                            continue;
                                        }
                                    }
                                }
                            }

                            // Auto-pairing for brackets and quotes
                            let pair = match c {
                                '(' => Some(')'),
                                '[' => Some(']'),
                                '{' => Some('}'),
                                '"' => Some('"'),
                                '\'' => Some('\''),
                                '`' => Some('`'),
                                _ => None,
                            };

                            if let Some(closing) = pair {
                                app.input.insert_char(c);
                                app.input.insert_char(closing);
                                // Move cursor back between the pair
                                app.input.move_cursor(tui_textarea::CursorMove::Back);
                            } else {
                                app.input.insert_char(c);
                            }

                            // Update completion state (commands and files)
                            update_completion_state(&mut app);
                        }
                        KeyCode::Backspace => {
                            app.input.delete_char();
                            // Update completion state
                            update_completion_state(&mut app);
                        }
                        KeyCode::Left => {
                            app.input.move_cursor(tui_textarea::CursorMove::Back);
                        }
                        KeyCode::Right => {
                            app.input.move_cursor(tui_textarea::CursorMove::Forward);
                        }
                        KeyCode::Home => {
                            // Move cursor to beginning of current line
                            app.input.move_cursor(tui_textarea::CursorMove::Head);
                        }
                        KeyCode::End => {
                            // Move cursor to end of current line
                            app.input.move_cursor(tui_textarea::CursorMove::End);
                        }
                        _ => {}
                    }
                } // End of Event::Key
                _ => {} // Ignore other events (resize, etc.)
            } // End of match event::read()
        }

        // Update spinner
        if app.working {
            app.spinner_frame = (app.spinner_frame + 1) % SPINNER_FRAMES.len();
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    Ok(())
}

fn handle_command(app: &mut App, command: &str, session: &mut Session) {
    let working_dir = app.working_directory.clone();
    let yolo = app.tools_approved;

    // Parse command and arguments
    let parts: Vec<&str> = command.trim().splitn(2, ' ').collect();
    let cmd = parts[0];
    let args = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd {
        "/agent" | "/switch_agent" => {
            let agents = &app.available_agents;
            if args.is_empty() {
                app.add_message(ChatMessage::system(&format!(
                    "👥 Available Agents\n\n\
                     Current: {}\n\n\
                     Agents:\n  {}\n\n\
                     Usage: /agent <name>\n\n\
                     Note: Agent switching at runtime will take effect on the next message.",
                    app.current_agent,
                    agents.join("\n  ")
                )));
            } else if agents.iter().any(|a| a == args) {
                app.agent_switching = true; // Show spinner indicator
                app.current_agent = args.to_string();
                app.agent_switching = false;
                app.add_message(ChatMessage::system(&format!(
                    "✓ Switched to agent: {}\n\nYour next message will be handled by this agent.",
                    args
                )));
            } else {
                app.add_message(ChatMessage::error(&format!(
                    "Unknown agent: {}\n\nAvailable agents: {}",
                    args,
                    agents.join(", ")
                )));
            }
        }
        "/exit" | "/quit" | "/q" => {
            app.should_quit = true;
        }
        "/new" | "/reset" => {
            *session = Session::new()
                .with_working_dir(&working_dir)
                .with_tools_approved(yolo);
            app.messages.clear();
            app.message_count = 0;
            app.input_tokens = 0;
            app.output_tokens = 0;
            app.cost = 0.0;
            app.context_length = 0;
            app.session_title = "New Session".to_string();
            app.session_created_at = chrono::Local::now();
            app.add_message(ChatMessage::system("✓ Session reset."));
        }
        "/usage" => {
            let context_info = if app.context_limit > 0 {
                let ratio = (app.context_length as f64 / app.context_limit as f64) * 100.0;
                format!("\n└─ Context: {:.1}% used", ratio)
            } else {
                String::new()
            };
            app.add_message(ChatMessage::system(&format!(
                "📊 Token Usage\n\
                 ├─ Input:  {} tokens\n\
                 ├─ Output: {} tokens\n\
                 ├─ Total:  {} tokens\n\
                 ├─ Cost:   ${:.4}{}",
                app.input_tokens,
                app.output_tokens,
                app.input_tokens + app.output_tokens,
                app.cost,
                context_info
            )));
        }
        "/yolo" => {
            app.tools_approved = !app.tools_approved;
            session.tools_approved = app.tools_approved;
            if app.tools_approved {
                app.add_message(ChatMessage::system(
                    "🚀 YOLO mode enabled - tools auto-approved",
                ));
            } else {
                app.add_message(ChatMessage::system(
                    "🔒 YOLO mode disabled - tools require approval",
                ));
            }
        }
        "/think" => {
            session.thinking = !session.thinking;
            app.thinking_mode = session.thinking; // Update app state for sidebar
            if session.thinking {
                app.add_message(ChatMessage::system(
                    "🧠 Thinking mode enabled - model will show reasoning",
                ));
            } else {
                app.add_message(ChatMessage::system(
                    "💤 Thinking mode disabled - model reasoning hidden",
                ));
            }
        }
        "/wrap" => {
            app.word_wrap_enabled = !app.word_wrap_enabled;
            if app.word_wrap_enabled {
                app.add_message(ChatMessage::system("✓ Word wrap enabled"));
            } else {
                app.add_message(ChatMessage::system("✓ Word wrap disabled"));
            }
        }
        "/title" => {
            if args.is_empty() {
                app.add_message(ChatMessage::system(&format!(
                    "Current title: {}",
                    app.session_title
                )));
            } else {
                app.session_title = args.to_string();
                session.title = args.to_string();
                app.add_message(ChatMessage::system(&format!(
                    "✓ Session title set to: {}",
                    args
                )));
            }
        }
        "/copy" => match copy_conversation_to_clipboard(app) {
            Ok(_) => app.add_message(ChatMessage::system("✓ Conversation copied to clipboard")),
            Err(e) => app.add_message(ChatMessage::error(&format!("Failed to copy: {}", e))),
        },
        "/eval" => {
            // Save session as evaluation file
            let dir = if args.is_empty() {
                "evals".to_string()
            } else {
                args.to_string()
            };

            // Update session with current state
            session.title = app.session_title.clone();
            session.input_tokens = app.input_tokens;
            session.output_tokens = app.output_tokens;
            session.cost = app.cost;

            // Save synchronously (block TUI) - we could make this async but it's fast
            match std::fs::create_dir_all(&dir) {
                Ok(_) => {
                    // Generate filename
                    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
                    let title_slug: String = app.session_title
                        .to_lowercase()
                        .chars()
                        .map(|c| if c.is_alphanumeric() { c } else { '_' })
                        .collect::<String>()
                        .split('_')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("_")
                        .chars()
                        .take(50)
                        .collect();

                    let filename = if title_slug.is_empty() {
                        format!("eval_{}.json", timestamp)
                    } else {
                        format!("{}_{}.json", title_slug, timestamp)
                    };
                    let path = std::path::Path::new(&dir).join(&filename);

                    // Convert session to eval format
                    let eval_session = crate::evaluation::EvalSession::from(session as &_);
                    match serde_json::to_string_pretty(&eval_session) {
                        Ok(json) => match std::fs::write(&path, json) {
                            Ok(_) => app.add_message(ChatMessage::system(&format!(
                                "✓ Evaluation saved to {}",
                                path.display()
                            ))),
                            Err(e) => app.add_message(ChatMessage::error(&format!(
                                "Failed to write file: {}",
                                e
                            ))),
                        },
                        Err(e) => app.add_message(ChatMessage::error(&format!(
                            "Failed to serialize: {}",
                            e
                        ))),
                    }
                }
                Err(e) => app.add_message(ChatMessage::error(&format!(
                    "Failed to create directory: {}",
                    e
                ))),
            }
        }
        "/export" => {
            let filename = if args.is_empty() {
                format!(
                    "cagent-export-{}.md",
                    chrono::Local::now().format("%Y%m%d-%H%M%S")
                )
            } else {
                args.to_string()
            };
            match export_conversation(app, &filename) {
                Ok(_) => app.add_message(ChatMessage::system(&format!(
                    "✓ Conversation exported to {}",
                    filename
                ))),
                Err(e) => app.add_message(ChatMessage::error(&format!("Failed to export: {}", e))),
            }
        }
        "/compact" => {
            // Real compaction: summarize older messages into session state.
            // Keep the last N message items verbatim to preserve recent context.
            let keep_last_messages = 20;

            let total_message_items = session.message_item_count();
            if total_message_items <= keep_last_messages {
                app.add_message(ChatMessage::system(
                    "Session is small, no compaction needed.",
                ));
                return;
            }

            // Build a simple summary from the oldest messages in the current tail.
            // (We can later replace this with a model-generated summary.)
            let mut summary_lines = Vec::new();
            let mut summarized = 0usize;
            for item in &session.messages {
                if summarized >= total_message_items.saturating_sub(keep_last_messages) {
                    break;
                }
                if let SessionItem::Message { message } = item {
                    let msg = message.as_ref();
                    if msg.implicit {
                        continue;
                    }
                    match msg.message.role {
                        crate::chat::MessageRole::User => {
                            summarized += 1;
                            let text = msg.message.content.replace('\n', " ");
                            summary_lines.push(format!("User: {}", truncate(&text, 120)));
                        }
                        crate::chat::MessageRole::Assistant => {
                            summarized += 1;
                            let text = msg.message.content.replace('\n', " ");
                            summary_lines.push(format!("Assistant: {}", truncate(&text, 120)));
                        }
                        crate::chat::MessageRole::Tool => {
                            // Tool messages tend to be noisy; skip from summary.
                        }
                        crate::chat::MessageRole::System => {}
                    }
                }
            }

            let summary_text = if summary_lines.is_empty() {
                "(no content to summarize)".to_string()
            } else {
                summary_lines.join("\n")
            };

            let (removed_items, kept_message_items) =
                session.compact_with_summary(summary_text, keep_last_messages);

            // Rebuild message view from the updated session.
            restore_messages_from_session(app, session);

            app.add_message(ChatMessage::system(&format!(
                "✓ Compacted session: summarized {} message(s), removed {} item(s), kept last {} message(s)",
                summarized,
                removed_items,
                kept_message_items
            )));
        }
        "/undo" => {
            // Find and remove the last user message and all messages after it
            let mut last_user_idx = None;
            for (idx, msg) in app.messages.iter().enumerate().rev() {
                if msg.role == MessageRole::User {
                    last_user_idx = Some(idx);
                    break;
                }
            }

            if let Some(idx) = last_user_idx {
                let removed_count = app.messages.len() - idx;
                app.messages.truncate(idx);
                app.add_message(ChatMessage::system(&format!(
                    "✓ Undid last exchange ({} messages removed)",
                    removed_count
                )));
                // Also remove from session
                session.undo_last_exchange();
            } else {
                app.add_message(ChatMessage::system("Nothing to undo."));
            }
        }
        "/config" => {
            // Try to open config in the default editor
            let config_path = dirs::config_dir()
                .map(|p| p.join("cagent").join("config.yaml"))
                .unwrap_or_else(|| std::path::PathBuf::from("cagent.yaml"));

            let editor = std::env::var("EDITOR").unwrap_or_else(|_| {
                if cfg!(target_os = "macos") {
                    "open".to_string()
                } else if cfg!(target_os = "windows") {
                    "notepad".to_string()
                } else {
                    "xdg-open".to_string()
                }
            });

            if config_path.exists() {
                match std::process::Command::new(&editor)
                    .arg(&config_path)
                    .spawn()
                {
                    Ok(_) => app.add_message(ChatMessage::system(&format!(
                        "✓ Opening {} in {}",
                        config_path.display(),
                        editor
                    ))),
                    Err(e) => app
                        .add_message(ChatMessage::error(&format!("Failed to open editor: {}", e))),
                }
            } else {
                app.add_message(ChatMessage::error(&format!(
                    "Config file not found: {}",
                    config_path.display()
                )));
            }
        }
        "/clear" => {
            // Clear the screen (but keep messages in history)
            app.messages.clear();
            app.message_count = 0;
            app.add_message(ChatMessage::system(
                "✓ Screen cleared. Session history preserved.",
            ));
        }
        "/theme" => {
            if args.is_empty() {
                let available = Theme::available_themes().join(", ");
                app.add_message(ChatMessage::system(&format!(
                    "🎨 Theme Settings\n\n\
                     Current theme: {}\n\n\
                     Available themes: {}\n\n\
                     Usage: /theme <name>\n\n\
                     You can also set CAGENT_THEME environment variable.",
                    app.theme.name, available
                )));
            } else {
                match Theme::by_name(args) {
                    Some(theme) => {
                        let theme_name = theme.name;
                        app.theme = theme;
                        app.add_message(ChatMessage::system(&format!(
                            "✓ Theme changed to: {}",
                            theme_name
                        )));
                    }
                    None => {
                        let available = Theme::available_themes().join(", ");
                        app.add_message(ChatMessage::error(&format!(
                            "Unknown theme: {}\n\nAvailable themes: {}",
                            args, available
                        )));
                    }
                }
            }
        }
        "/model" | "/switch_model" => {
            // NOTE: This is a UI-only change indicator. Actual model switching
            // requires runtime support which is not yet implemented.
            // For now, we show a message about the current model.
            if args.is_empty() {
                app.add_message(ChatMessage::system(&format!(
                    "🤖 Current Model\n\n\
                     Agent: {}\n\
                     Model: {}\n\n\
                     To switch models, edit your agent configuration file.\n\
                     Use /config to open the configuration.",
                    app.current_agent, app.agent_model
                )));
            } else {
                // TODO: When runtime model switching is implemented, handle it here
                app.add_message(ChatMessage::system(&format!(
                    "Model switching at runtime is not yet implemented.\n\n\
                     Requested model: {}\n\n\
                     To use a different model, edit your agent configuration.\n\
                     Use /config to open the configuration.",
                    args
                )));
            }
        }
        "/search" => {
            if args.is_empty() {
                app.add_message(ChatMessage::system(
                    "Usage: /search <query>\n\nSearch for text in the conversation.",
                ));
            } else {
                let query = args.to_lowercase();
                let mut matches = Vec::new();

                for (idx, msg) in app.messages.iter().enumerate() {
                    if msg.content.to_lowercase().contains(&query) {
                        let role = match msg.role {
                            MessageRole::User => "You",
                            MessageRole::Assistant => msg.agent.as_deref().unwrap_or("Assistant"),
                            MessageRole::System => "System",
                            MessageRole::Error => "Error",
                            MessageRole::Thinking => "Thinking",
                            MessageRole::Tool => msg.tool_name.as_deref().unwrap_or("Tool"),
                            MessageRole::ShellOutput => "Shell",
                            MessageRole::Cancelled => "Cancelled",
                            MessageRole::Welcome => "Welcome",
                            MessageRole::Loading => "Loading",
                        };

                        // Get a snippet around the match
                        let content = &msg.content;
                        let lower_content = content.to_lowercase();
                        if let Some(pos) = lower_content.find(&query) {
                            let start = pos.saturating_sub(20);
                            let end = (pos + query.len() + 20).min(content.len());
                            let snippet = &content[start..end];
                            let snippet = if start > 0 {
                                format!("...{}", snippet)
                            } else {
                                snippet.to_string()
                            };
                            let snippet = if end < content.len() {
                                format!("{}...", snippet)
                            } else {
                                snippet
                            };

                            matches.push(format!(
                                "  #{} {} [{}]: {}",
                                idx + 1,
                                msg.timestamp,
                                role,
                                snippet
                            ));
                        }
                    }
                }

                if matches.is_empty() {
                    app.add_message(ChatMessage::system(&format!(
                        "No results found for: {}",
                        args
                    )));
                } else {
                    let result = format!(
                        "🔍 Search Results for \"{}\"\n\nFound {} match{}:\n{}",
                        args,
                        matches.len(),
                        if matches.len() == 1 { "" } else { "es" },
                        matches.join("\n")
                    );
                    app.add_message(ChatMessage::system(&result));
                }
            }
        }
        "/filter" => {
            if args.is_empty() {
                app.add_message(ChatMessage::system(
                    "Usage: /filter <type>\n\n\
                     Filter messages by type:\n\
                     • user     - Show only your messages\n\
                     • assistant - Show only assistant messages\n\
                     • system   - Show only system messages\n\
                     • error    - Show only error messages\n\
                     • tool     - Show only tool messages\n\
                     • all      - Show all messages (reset filter)",
                ));
            } else {
                let filter_type = args.to_lowercase();
                let filtered: Vec<&ChatMessage> = match filter_type.as_str() {
                    "user" => app
                        .messages
                        .iter()
                        .filter(|m| m.role == MessageRole::User)
                        .collect(),
                    "assistant" => app
                        .messages
                        .iter()
                        .filter(|m| m.role == MessageRole::Assistant)
                        .collect(),
                    "system" => app
                        .messages
                        .iter()
                        .filter(|m| m.role == MessageRole::System)
                        .collect(),
                    "error" => app
                        .messages
                        .iter()
                        .filter(|m| m.role == MessageRole::Error)
                        .collect(),
                    "tool" => app
                        .messages
                        .iter()
                        .filter(|m| m.role == MessageRole::Tool)
                        .collect(),
                    "all" => {
                        app.add_message(ChatMessage::system(
                            "✓ Filter reset - showing all messages",
                        ));
                        return;
                    }
                    _ => {
                        app.add_message(ChatMessage::error(&format!(
                            "Unknown filter type: {}\nUse /filter for help.",
                            args
                        )));
                        return;
                    }
                };

                if filtered.is_empty() {
                    app.add_message(ChatMessage::system(&format!(
                        "No {} messages found.",
                        filter_type
                    )));
                } else {
                    let summary: Vec<String> = filtered
                        .iter()
                        .enumerate()
                        .map(|(i, m)| {
                            let preview = if m.content.len() > 50 {
                                format!("{}...", &m.content[..50])
                            } else {
                                m.content.clone()
                            };
                            format!(
                                "  #{} [{}]: {}",
                                i + 1,
                                m.timestamp,
                                preview.replace('\n', " ")
                            )
                        })
                        .collect();

                    app.add_message(ChatMessage::system(&format!(
                        "📝 {} {} message{}:\n{}",
                        filtered.len(),
                        filter_type,
                        if filtered.len() == 1 { "" } else { "s" },
                        summary.join("\n")
                    )));
                }
            }
        }
        "/goto" => {
            if args.is_empty() {
                app.add_message(ChatMessage::system(
                    "Usage: /goto <number>\n\nJump to a specific message by number.",
                ));
            } else {
                match args.parse::<usize>() {
                    Ok(num) if num > 0 && num <= app.messages.len() => {
                        app.messages_state.select(Some(num - 1));
                        app.add_message(ChatMessage::system(&format!(
                            "✓ Jumped to message #{}",
                            num
                        )));
                    }
                    Ok(num) => {
                        app.add_message(ChatMessage::error(&format!(
                            "Message #{} doesn't exist. Valid range: 1-{}",
                            num,
                            app.messages.len()
                        )));
                    }
                    Err(_) => {
                        app.add_message(ChatMessage::error(
                            "Invalid number. Usage: /goto <number>",
                        ));
                    }
                }
            }
        }
        "/help" | "/?" => {
            app.add_message(ChatMessage::system(
                "📚 Available Commands\n\n\
                 /new, /reset       - Start new session\n\
                 /undo              - Undo last message exchange\n\
                 /usage             - Show token usage\n\
                 /model             - Show/switch model info\n\
                 /search <query>    - Search in conversation\n\
                 /filter <type>     - Filter messages (user/assistant/system/error/tool/all)\n\
                 /goto <number>     - Jump to message number\n\
                 /clear             - Clear the screen\n\
                 /theme <name>      - Change color theme\n\
                 /yolo              - Toggle auto-approve tools\n\
                 /wrap              - Toggle word wrap\n\
                 /title <name>      - Set session title\n\
                 /copy              - Copy conversation to clipboard\n\
                 /eval [dir]        - Save session as evaluation file\n\
                 /export [file]     - Export to markdown file\n\
                 /compact           - Compact session history\n\
                 /config            - Open config in editor\n\
                 /help              - Show this help\n\
                 /exit              - Exit cagent\n\n\
                 ⌨️  Shortcuts\n\n\
                 Enter      - Send message\n\
                 Shift+⏎   - New line with smart indent\n\
                 ↑/↓        - Browse command history\n\
                 Ctrl+C     - Quit\n\
                 Ctrl+B     - Toggle sidebar\n\
                 Ctrl+T     - Cycle themes\n\
                 Ctrl+P     - Command palette (help)\n\
                 Ctrl+G     - Open external editor\n\
                 Ctrl+Z     - Undo last exchange\n\
                 Ctrl+F     - Search\n\
                 Alt+1-5    - Toggle sidebar sections\n\
                 Tab        - Cycle completions / focus sidebar\n\
                 Ctrl+↑/↓   - Scroll messages\n\
                 PgUp/PgDn  - Scroll fast\n\
                 Space      - Expand/collapse tool output\n\
                 Esc        - Close popups / exit sidebar",
            ));
        }
        _ => {
            app.add_message(ChatMessage::error(&format!(
                "Unknown command: {}\nType /help for available commands.",
                command
            )));
        }
    }
}

fn handle_runtime_event(app: &mut App, event: RuntimeEvent, hide_tool_results: bool) {
    match event {
        RuntimeEvent::AgentInfo {
            name,
            model,
            description,
            welcome_message,
        } => {
            app.current_agent = name;
            app.agent_model = model;
            app.agent_description = description;

            if let Some(msg) = welcome_message {
                let msg = msg.trim();
                if !msg.is_empty() {
                    app.add_message(ChatMessage::assistant(msg, &app.current_agent));
                }
            }
        }
        RuntimeEvent::AgentChoice { content, agent } => {
            // Append to last assistant message or create new one
            if let Some(last) = app.messages.last_mut() {
                if last.role == MessageRole::Assistant && last.agent.as_deref() == Some(&agent) {
                    last.content.push_str(&content);
                    return;
                }
            }
            app.add_message(ChatMessage::assistant(&content, &agent));
        }
        RuntimeEvent::AgentReasoning { content, agent } => {
            app.add_message(ChatMessage::thinking(&content, &agent));
        }
        RuntimeEvent::ToolCall {
            tool,
            agent,
            tool_call: _,
        } => {
            app.add_message(ChatMessage::tool(&tool.name, &tool.description, &agent));
        }
        RuntimeEvent::ToolCallResponse {
            result,
            tool_call,
            agent,
            ..
        } => {
            // Hide tool results if requested by the session.
            if hide_tool_results {
                return;
            }

            // Create tool result message with full output stored for expansion
            let mut msg = ChatMessage::tool_result(
                &tool_call.function.name,
                &agent,
                &result.output,
                result.is_error,
            );
            msg.collapsed = true; // Start collapsed
            app.add_message(msg);
        }
        RuntimeEvent::ToolCallConfirmation {
            tool, tool_call, ..
        } => {
            // Announce for screen readers
            app.announce(&format!("Tool confirmation required: {}", tool.name));

            // Parse arguments to check if this is an edit_file tool call
            let diff_preview = if tool.name == "edit_file" {
                compute_edit_file_diff(&tool_call.function.arguments, &app.working_directory)
            } else {
                None
            };

            app.pending_confirmation = Some(PendingConfirmation {
                tool_name: tool.name.clone(),
                tool_args: tool_call.function.arguments.clone(),
                tool_description: Some(tool.description.clone()),
                diff_preview,
            });
        }
        RuntimeEvent::TokenUsage {
            input_tokens,
            output_tokens,
            cost,
            context_length,
            context_limit,
            ..
        } => {
            app.input_tokens = input_tokens;
            app.output_tokens = output_tokens;
            app.cost = cost;
            if let Some(ctx_len) = context_length {
                app.context_length = ctx_len;
            }
            if let Some(ctx_limit) = context_limit {
                app.context_limit = ctx_limit;
            }
        }
        RuntimeEvent::StreamStopped { .. } => {
            app.announce("Response complete");
            app.working = false;
            app.status_message = "Ready".to_string();
        }
        RuntimeEvent::Error { message } => {
            app.announce(&format!("Error: {}", message));
            app.add_message(ChatMessage::error(&message));
            app.working = false;
            app.status_message = "Error".to_string();
        }
        RuntimeEvent::AgentSwitching {
            active,
            to_agent,
            from_agent,
        } => {
            if active {
                app.add_message(ChatMessage::system(&format!(
                    "→ Delegating to: {}",
                    to_agent
                )));
                app.current_agent = to_agent;
            } else {
                app.add_message(ChatMessage::system(&format!(
                    "← Returned from: {}",
                    from_agent
                )));
            }
        }
        RuntimeEvent::HookBlocked {
            agent,
            tool_name,
            hook_type: _,
            reason,
        } => {
            let msg = reason.unwrap_or_else(|| "Blocked by hook".to_string());
            app.announce(&format!("Tool {} blocked: {}", tool_name, msg));
            app.add_message(ChatMessage::system(&format!(
                "⚠️ Tool '{}' was blocked by {} hook: {}",
                tool_name, agent, msg
            )));
        }
        RuntimeEvent::SessionTitle {
            session_id: _,
            title,
        } => {
            app.session_title = title;
        }
        RuntimeEvent::SessionSummary {
            session_id: _,
            summary,
        } => {
            app.add_message(ChatMessage::system(&format!(
                "📝 Session summary: {}",
                summary
            )));
        }
        RuntimeEvent::SessionCompaction {
            session_id: _,
            items_before,
            items_after,
        } => {
            app.add_message(ChatMessage::system(&format!(
                "📦 Session compacted: {} → {} items",
                items_before, items_after
            )));
        }
        RuntimeEvent::McpInitStarted { toolset } => {
            app.add_message(ChatMessage::system(&format!(
                "⏳ Starting MCP server: {}",
                toolset
            )));
        }
        RuntimeEvent::McpInitFinished {
            toolset,
            success,
            error,
        } => {
            if success {
                app.add_message(ChatMessage::system(&format!(
                    "✅ MCP server ready: {}",
                    toolset
                )));
            } else {
                let err_msg = error.as_deref().unwrap_or("unknown error");
                app.add_message(ChatMessage::system(&format!(
                    "❌ MCP server failed: {} - {}",
                    toolset, err_msg
                )));
            }
        }
        RuntimeEvent::TeamInfo { agents, default_agent } => {
            app.available_agents = agents.clone();
            if agents.len() > 1 {
                app.add_message(ChatMessage::system(&format!(
                    "👥 Team: {} (default: {})\n\nUse /agent <name> to switch agents.",
                    agents.join(", "),
                    default_agent
                )));
            }
        }
        RuntimeEvent::ToolsetInfo { agent, tools } => {
            // Only show this for verbose mode or debugging
            // For now, just log it
            tracing::debug!(agent = %agent, tool_count = tools.len(), "Toolset info received");
        }
        _ => {}
    }
}

// ============================================================================
// UI Drawing
// ============================================================================

fn draw_ui(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Minimum terminal width handling
    const MIN_TERMINAL_WIDTH: u16 = 60;
    const MIN_TERMINAL_HEIGHT: u16 = 10;
    
    // Show warning if terminal is too narrow
    if size.width < MIN_TERMINAL_WIDTH || size.height < MIN_TERMINAL_HEIGHT {
        let warning = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Terminal too small",
                Style::default().fg(COLOR_WARNING).bold(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("Minimum: {}x{}", MIN_TERMINAL_WIDTH, MIN_TERMINAL_HEIGHT),
                Style::default().fg(COLOR_TEXT_SECONDARY),
            )),
            Line::from(Span::styled(
                format!("Current: {}x{}", size.width, size.height),
                Style::default().fg(COLOR_TEXT_SECONDARY),
            )),
        ])
        .alignment(ratatui::layout::Alignment::Center);
        f.render_widget(warning, size);
        return;
    }

    // Auto-collapse sidebar on narrow terminals
    let effective_sidebar_collapsed = app.sidebar_collapsed || size.width < 80;

    // Add 1 character left padding (like Go TUI's AppPaddingLeft)
    let padded_area = Rect::new(
        size.x + 1,
        size.y,
        size.width.saturating_sub(1),
        size.height,
    );

    // Main layout: chat | sidebar (sidebar on right)
    let main_chunks = if effective_sidebar_collapsed {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(0)])
            .split(padded_area)
    } else {
        // Clamp sidebar width to valid bounds (25-50% of window)
        let sidebar_width = clamp_sidebar_width(app.sidebar_width, size.width);
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(40), Constraint::Length(sidebar_width)])
            .split(padded_area)
    };

    // Chat area layout: messages | input | status
    // Make input area height dynamic based on content
    let input_lines = app.input.lines().len();
    let input_height = (input_lines as u16 + 2).clamp(3, 10); // Min 3, max 10 lines

    let chat_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(main_chunks[0]);

    draw_messages(f, app, chat_chunks[0]);
    draw_input(f, app, chat_chunks[1]);
    draw_status_bar(f, app, chat_chunks[2]);

    // Draw sidebar if not collapsed and terminal is wide enough
    let effective_sidebar_collapsed = app.sidebar_collapsed || f.area().width < 80;
    if !effective_sidebar_collapsed {
        draw_sidebar(f, app, main_chunks[1]);
    }

    // Draw command completions popup if showing
    if app.showing_completions {
        draw_completions_popup(f, app, chat_chunks[1]);
    }

    // Draw file completions popup if showing
    if app.showing_file_completions {
        draw_file_completions_popup(f, app, chat_chunks[1]);
    }

    // Draw confirmation dialog if needed
    if app.pending_confirmation.is_some() {
        draw_confirmation_dialog(f, app, size);
    }

    // Draw elicitation dialog if needed
    if app.pending_elicitation.is_some() {
        draw_elicitation_dialog(f, app, size);
    }

    // Draw exit confirmation dialog if needed
    if app.show_exit_confirmation {
        draw_exit_confirmation_dialog(f, size);
    }
}

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    let theme = &app.theme;

    // Draw left border
    let border_color = if app.sidebar_focused {
        theme.primary
    } else {
        theme.border
    };
    let border = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(border_color));
    let inner = border.inner(area);
    f.render_widget(border, area);

    // Add padding inside the sidebar
    let padded = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let content_width = padded.width as usize;

    // Calculate section heights based on collapse state
    // Each section has: title (1) + content lines + bottom padding (1)
    let session_height = if app.sidebar_sections_collapsed[0] {
        1
    } else {
        5 // title + star/title + empty + workdir + padding
    };
    let token_height = if app.sidebar_sections_collapsed[1] {
        1
    } else {
        4 // title + tokens line + context bar + padding
    };
    let agent_height = if app.sidebar_sections_collapsed[2] {
        1
    } else if app.thinking_mode {
        6 // title + agent name + description + model + reasoning indicator + padding
    } else {
        5 // title + agent name + description + model + padding
    };

    // Sidebar sections (no MCP section - matches Go version)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(session_height), // Session info
            Constraint::Length(token_height),   // Token usage
            Constraint::Length(agent_height),   // Agent info
            Constraint::Min(3),                 // Tools
        ])
        .split(padded);

    // Session section
    let session_selected = app.sidebar_focused && app.sidebar_selected_section == 0;
    let session_title_val = app.session_title.clone();
    let working_directory = app.working_directory.clone();
    let session_duration = format_duration(chrono::Local::now() - app.session_created_at);
    // Capture theme colors for the closure
    let text_secondary = theme.text_secondary;
    let text_primary = theme.text_primary;
    let accent = theme.accent;
    draw_tab_section(
        f,
        "Session",
        chunks[0],
        content_width,
        session_selected,
        !app.sidebar_sections_collapsed[0],
        theme,
        move || {
            // Star indicator + title (like Go version)
            // TODO: Add starred state tracking
            let star = "☆ "; // Unstarred by default, use ★ for starred

            let mut lines = vec![
                // Title line with star
                Line::from(vec![
                    Span::styled(star, Style::default().fg(text_secondary)),
                    Span::styled(session_title_val, Style::default().fg(text_primary)),
                ]),
                // Session duration
                Line::from(vec![
                    Span::styled("⌛ ", Style::default().fg(text_secondary)),
                    Span::styled(session_duration.clone(), Style::default().fg(text_secondary)),
                ]),
            ];

            // Working directory: █ (accent) + " path" (primary)
            // Replace home directory with ~/
            if !working_directory.is_empty() {
                let display_path = shorten_home_dir(&working_directory);
                lines.push(Line::from(vec![
                    Span::styled("█", Style::default().fg(accent)),
                    Span::styled(
                        format!(" {}", display_path),
                        Style::default().fg(text_primary),
                    ),
                ]));
            }

            lines
        },
    );

    // Token usage section - single line like Go version
    let token_selected = app.sidebar_focused && app.sidebar_selected_section == 1;
    let context_limit = app.context_limit;
    let context_length = app.context_length;
    let input_tokens = app.input_tokens;
    let output_tokens = app.output_tokens;
    let cost = app.cost;
    // Capture theme colors for the closure
    let text_primary = theme.text_primary;
    let accent = theme.accent;
    draw_tab_section(
        f,
        "Token Usage",
        chunks[1],
        content_width,
        token_selected,
        !app.sidebar_sections_collapsed[1],
        theme,
        move || {
            let total = input_tokens + output_tokens;
            let mut lines = Vec::new();

            // Format: "1.2K tokens $0.05" like Go version
            let token_text = format!("{} tokens", format_tokens(total));
            lines.push(Line::from(vec![
                Span::styled(token_text, Style::default().fg(text_primary)),
                Span::styled(format!(" ${:.2}", cost), Style::default().fg(accent)),
            ]));

            // Add context percentage with progress bar if we have context info
            if context_limit > 0 {
                let percent = ((context_length as f64 / context_limit as f64) * 100.0) as i32;
                let percent = percent.min(100).max(0);

                // Create simple progress bar (10 chars wide)
                let filled = (percent / 10) as usize;
                let empty = 10 - filled;
                let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(empty));

                lines.push(Line::from(vec![
                    Span::styled(bar, Style::default().fg(if percent > 80 {
                        COLOR_WARNING
                    } else {
                        accent
                    })),
                    Span::styled(format!(" {}% context", percent), Style::default().fg(text_secondary)),
                ]));
            }

            lines
        },
    );

    // Agent section
    let agent_selected = app.sidebar_focused && app.sidebar_selected_section == 2;
    let working = app.working;
    let spinner_frame = app.spinner_frame;
    let current_agent = app.current_agent.clone();
    let agent_description = app.agent_description.clone();
    let agent_model = app.agent_model.clone();
    let thinking_mode = app.thinking_mode;
    // Capture theme colors for the closure
    let accent = theme.accent;
    let text_secondary = theme.text_secondary;
    let primary = theme.primary;
    draw_tab_section(
        f,
        "Agent",
        chunks[2],
        content_width,
        agent_selected,
        !app.sidebar_sections_collapsed[2],
        theme,
        move || {
            // First line: indicator + agent name (all accent) + right-aligned ^1 hint
            let hint = "^1";
            let hint_width = hint.len();

            let mut first_line_spans = Vec::new();
            
            if working {
                // Spinner with gradient color
                let spinner_color = spinner_color_for_frame(spinner_frame);
                first_line_spans.push(Span::styled(
                    format!("{} ", SPINNER_FRAMES[spinner_frame]),
                    Style::default().fg(spinner_color),
                ));
            } else {
                first_line_spans.push(Span::styled("▶ ", Style::default().fg(accent)));
            }
            
            first_line_spans.push(Span::styled(current_agent.clone(), Style::default().fg(accent)));
            
            // Calculate padding
            let indicator_width = if working { 2 } else { 2 }; // spinner or arrow + space
            let agent_width = current_agent.chars().count();
            let total_width = indicator_width + agent_width;
            let padding = content_width.saturating_sub(total_width + hint_width);
            
            first_line_spans.push(Span::raw(" ".repeat(padding)));
            first_line_spans.push(Span::styled(hint, Style::default().fg(text_secondary)));

            let mut lines = vec![Line::from(first_line_spans)];

            let max_width = content_width.saturating_sub(2); // Account for tree prefix

            // Description (if present) with ├ prefix
            if !agent_description.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("├ ", Style::default().fg(text_secondary)),
                    Span::raw(truncate(&agent_description, max_width)),
                ]));
            }

            // Model line - use ├ if there's a thinking indicator, else └
            let model_prefix = if thinking_mode { "├ " } else { "└ " };
            lines.push(Line::from(vec![
                Span::styled(model_prefix, Style::default().fg(text_secondary)),
                Span::raw(truncate(&format!("Model: {}", agent_model), max_width)),
            ]));

            // Reasoning mode indicator (if enabled)
            if thinking_mode {
                lines.push(Line::from(vec![
                    Span::styled("└ ", Style::default().fg(text_secondary)),
                    Span::styled("🧠 ", Style::default()),
                    Span::styled("Reasoning", Style::default().fg(primary)),
                ]));
            }

            lines
        },
    );

    // Tools section - matches Go version exactly
    let tools_title = "Tools".to_string();
    let tools_selected = app.sidebar_focused && app.sidebar_selected_section == 3;
    let tool_count = app.available_tools.len();
    let tools_approved = app.tools_approved;
    let tools_loading = app.tools_loading;
    let spinner_frame = app.spinner_frame;
    // Capture theme colors for the closure
    let accent = theme.accent;
    let text_primary = theme.text_primary;
    let text_secondary = theme.text_secondary;
    let warning = theme.warning;
    draw_tab_section(
        f,
        &tools_title,
        chunks[3],
        content_width,
        tools_selected,
        !app.sidebar_sections_collapsed[3],
        theme,
        move || {
            let mut lines = Vec::new();

            // Show loading indicator or tool count
            if tools_loading {
                let spinner = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
                lines.push(Line::from(vec![
                    Span::styled(format!("{} ", spinner), Style::default().fg(warning)),
                    Span::styled("Loading tools…", Style::default().fg(text_secondary)),
                ]));
            } else if tool_count > 0 {
                lines.push(Line::from(vec![
                    Span::styled("█", Style::default().fg(accent)),
                    Span::styled(
                        format!(" {} tools available", tool_count),
                        Style::default().fg(text_primary),
                    ),
                ]));
            } else {
                lines.push(Line::from(vec![Span::styled(
                    "No tools available",
                    Style::default().fg(text_secondary),
                )]));
            }

            // YOLO mode: ✓ (accent) + " YOLO mode enabled" (primary) + "^y" right-aligned (muted)
            if tools_approved {
                let indicator = "✓ YOLO mode enabled";
                let hint = "^y";
                let indicator_width = indicator.chars().count();
                let hint_width = hint.len();
                let padding = content_width.saturating_sub(indicator_width + hint_width);

                lines.push(Line::from(vec![
                    Span::styled("✓", Style::default().fg(accent)),
                    Span::styled(
                        " YOLO mode enabled",
                        Style::default().fg(text_primary),
                    ),
                    Span::raw(" ".repeat(padding)),
                    Span::styled(hint, Style::default().fg(text_secondary)),
                ]));
            }

            lines
        },
    );
}

/// Draw a tab section with title─────────── style like the Go version
fn draw_tab_section<F>(
    f: &mut Frame,
    title: &str,
    area: Rect,
    content_width: usize,
    selected: bool,
    expanded: bool,
    theme: &Theme,
    content_fn: F,
) where
    F: FnOnce() -> Vec<Line<'static>>,
{
    if area.height == 0 {
        return;
    }

    // Title color based on selection
    let title_color = if selected {
        theme.primary
    } else {
        theme.text_secondary
    };

    // Focus indicator (left border) for selected section
    let focus_indicator = if selected { "│ " } else { "  " };

    // Build title line: "│ Title ─────────────" or "  Title ─────────────"
    let title_text = format!("{}{} ", focus_indicator, title);
    let divider_len = content_width.saturating_sub(title_text.chars().count());
    let divider = "─".repeat(divider_len);

    let title_line = Line::from(vec![
        Span::styled(
            focus_indicator,
            Style::default().fg(if selected { theme.accent } else { theme.text_secondary }),
        ),
        Span::styled(format!("{} ", title), Style::default().fg(title_color)),
        Span::styled(divider, Style::default().fg(title_color)),
    ]);

    // Render title
    let title_para = Paragraph::new(title_line);
    f.render_widget(title_para, Rect::new(area.x, area.y, area.width, 1));

    // Render content if expanded (leave 1 line for bottom padding)
    if expanded && area.height > 2 {
        let content_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(2),
        );
        let mut lines = content_fn();
        // Add focus indicator to content lines too
        if selected {
            lines = lines
                .into_iter()
                .map(|line| {
                    let mut spans = vec![Span::styled("│ ", Style::default().fg(theme.accent))];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect();
        } else {
            lines = lines
                .into_iter()
                .map(|line| {
                    let mut spans = vec![Span::raw("  ")];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect();
        }
        let content = Paragraph::new(lines);
        f.render_widget(content, content_area);
    }
}

fn draw_messages(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_BORDER))
        .border_type(ratatui::widgets::BorderType::Rounded);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Update view height for scrolling calculations
    app.messages_view_height = inner.height as usize;

    if app.messages.is_empty() {
        // Show centered welcome text with double border (like Go TUI)
        let welcome_text = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  ✨ Welcome to cagent ✨  ",
                Style::default().fg(COLOR_PRIMARY).bold(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Type a message to get started",
                Style::default().fg(COLOR_TEXT_SECONDARY),
            )),
            Line::from(Span::styled(
                "/help for commands",
                Style::default().fg(COLOR_TEXT_SECONDARY),
            )),
            Line::from(""),
        ];

        // Calculate centered position
        let welcome_height = welcome_text.len() as u16 + 2; // +2 for border
        let welcome_width = 40.min(inner.width.saturating_sub(4));
        let x = inner.x + (inner.width.saturating_sub(welcome_width)) / 2;
        let y = inner.y + (inner.height.saturating_sub(welcome_height)) / 2;

        let welcome_block = Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Double)
            .border_style(Style::default().fg(COLOR_PRIMARY));

        let welcome = Paragraph::new(welcome_text)
            .block(welcome_block)
            .alignment(Alignment::Center);

        let welcome_area = Rect::new(x, y, welcome_width, welcome_height);
        f.render_widget(welcome, welcome_area);
        return;
    }

    // Render all messages into lines, tracking message boundaries for selection highlighting
    let content_width = (inner.width as usize).saturating_sub(3); // Reserve space for scrollbar
    let mut all_lines: Vec<Line<'static>> = Vec::new();
    let mut prev_agent: Option<String> = None;
    let selected_idx = app.messages_state.selected();

    // Track line ranges for each message (start_line, end_line)
    let mut message_line_ranges: Vec<(usize, usize)> = Vec::new();

    for (msg_idx, msg) in app.messages.iter().enumerate() {
        let start_line = all_lines.len();
        
        // Determine if we should show the badge (only when agent changes)
        let show_badge = match msg.role {
            MessageRole::Assistant | MessageRole::Tool | MessageRole::Thinking => {
                let current_agent = msg.agent.as_ref();
                let should_show = prev_agent.as_ref() != current_agent;
                prev_agent = current_agent.cloned();
                should_show
            }
            MessageRole::User => {
                prev_agent = None; // Reset on user message
                true
            }
            _ => {
                prev_agent = None; // Reset on system/error
                true
            }
        };
        
        let mut msg_lines =
            render_message_lines(msg, content_width, app.word_wrap_enabled, show_badge, app.spinner_frame);
        
        // Apply selection highlighting if this is the selected message
        if selected_idx == Some(msg_idx) {
            // Add green left border indicator to show selection
            msg_lines = msg_lines
                .into_iter()
                .map(|line| {
                    let mut spans = vec![Span::styled("│ ", Style::default().fg(COLOR_ACCENT))];
                    spans.extend(line.spans);
                    Line::from(spans)
                })
                .collect();
        }
        
        let end_line = start_line + msg_lines.len();
        message_line_ranges.push((start_line, end_line));
        all_lines.extend(msg_lines);
    }

    // Update total lines for scrollbar
    app.messages_total_lines = all_lines.len();

    // Clamp scroll offset
    let max_offset = app
        .messages_total_lines
        .saturating_sub(app.messages_view_height);
    if app.messages_scroll_offset > max_offset {
        app.messages_scroll_offset = max_offset;
    }

    // Get visible lines based on scroll offset
    let start = app.messages_scroll_offset;
    let end = (start + inner.height as usize).min(all_lines.len());
    let visible_lines: Vec<Line<'static>> = all_lines[start..end].to_vec();

    // Render content
    let content_area = Rect::new(
        inner.x,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let content = Paragraph::new(visible_lines);
    f.render_widget(content, content_area);

    // Scrollbar (custom rendering like Go version)
    if app.messages_total_lines > app.messages_view_height {
        let scrollbar_x = inner.x + inner.width - 1;
        let view_height = inner.height as usize;
        let total_height = app.messages_total_lines;

        // Calculate thumb size and position
        let thumb_height = ((view_height * view_height) / total_height)
            .max(1)
            .min(view_height);
        let scrollable_track = view_height.saturating_sub(thumb_height);
        let thumb_top = if max_offset > 0 {
            (app.messages_scroll_offset * scrollable_track) / max_offset
        } else {
            0
        };

        // Draw scrollbar with distinct track and thumb colors
        for i in 0..view_height {
            let y = inner.y + i as u16;
            let (char, style) = if i >= thumb_top && i < thumb_top + thumb_height {
                // Thumb (visible part)
                ("█", Style::default().fg(COLOR_SCROLLBAR_THUMB)) // Full block for thumb
            } else {
                // Track (background)
                ("│", Style::default().fg(COLOR_SCROLLBAR_TRACK)) // Thin line for track
            };
            let span = Span::styled(char, style);
            f.render_widget(
                Paragraph::new(Line::from(span)),
                Rect::new(scrollbar_x, y, 1, 1),
            );
        }
    }
}

/// Render a message as lines (for line-based scrolling)
fn render_message_lines(
    msg: &ChatMessage,
    content_width: usize,
    word_wrap_enabled: bool,
    show_badge: bool,
    spinner_frame: usize,
) -> Vec<Line<'static>> {
    match msg.role {
        MessageRole::User => render_user_message_lines(msg, content_width, word_wrap_enabled),
        MessageRole::Assistant => render_assistant_message_lines(msg, content_width, show_badge),
        MessageRole::System => render_system_message_lines(msg, content_width),
        MessageRole::Error => render_error_message_lines(msg, content_width),
        MessageRole::Thinking => render_thinking_message_lines(msg, content_width, show_badge),
        MessageRole::Tool => render_tool_message_lines(msg, content_width, show_badge),
        MessageRole::ShellOutput => render_shell_output_lines(msg, content_width),
        MessageRole::Cancelled => render_cancelled_message_lines(msg, content_width),
        MessageRole::Welcome => render_welcome_message_lines(msg, content_width),
        MessageRole::Loading => render_loading_message_lines(msg, content_width, spinner_frame),
    }
}

/// Render a single message with appropriate styling (legacy, for List widget)
#[allow(dead_code)]
fn render_message(
    msg: &ChatMessage,
    _idx: usize,
    width: u16,
    word_wrap_enabled: bool,
    spinner_frame: usize,
) -> ListItem<'static> {
    let content_width = (width as usize).saturating_sub(6);

    match msg.role {
        MessageRole::User => render_user_message(msg, content_width, word_wrap_enabled),
        MessageRole::Assistant => render_assistant_message(msg, content_width),
        MessageRole::System => render_system_message(msg, content_width),
        MessageRole::Error => render_error_message(msg, content_width),
        MessageRole::Thinking => render_thinking_message(msg, content_width),
        MessageRole::Tool => render_tool_message(msg, content_width),
        MessageRole::ShellOutput => render_shell_output(msg, content_width),
        MessageRole::Cancelled => render_cancelled_message(msg, content_width),
        MessageRole::Welcome => render_welcome_message(msg, content_width),
        MessageRole::Loading => render_loading_message(msg, content_width, spinner_frame),
    }
}

fn render_user_message_lines(
    msg: &ChatMessage,
    width: usize,
    word_wrap_enabled: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let timestamp = msg.timestamp.clone();
    let content = msg.content.clone();

    // Header with user badge
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            " You ",
            Style::default().bg(COLOR_USER).fg(COLOR_BACKGROUND).bold(),
        ),
        Span::styled(
            format!(" {}", timestamp),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ),
    ]));

    // Content with thick border on left and alternate background
    // Using "█" (full block) for thick border like Go's ThickBorder
    let border_char = "█";
    let content_lines: Vec<String> = if word_wrap_enabled {
        word_wrap(&content, width.saturating_sub(6))
    } else {
        content.lines().map(|s| s.to_string()).collect()
    };

    for line in content_lines {
        // Calculate padding to fill width for background color effect
        let padding = width.saturating_sub(line.len() + 5);
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                border_char,
                Style::default().fg(COLOR_USER),
            ),
            Span::styled(
                format!(" {}{}", line, " ".repeat(padding)),
                Style::default().fg(COLOR_TEXT_PRIMARY).bg(COLOR_BACKGROUND_ALT),
            ),
        ]));
    }

    // Spacing
    lines.push(Line::from(""));

    lines
}

#[allow(dead_code)]
fn render_user_message(
    msg: &ChatMessage,
    width: usize,
    word_wrap_enabled: bool,
) -> ListItem<'static> {
    let mut lines = Vec::new();
    let timestamp = msg.timestamp.clone();
    let content = msg.content.clone();

    // Header with user badge
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            " You ",
            Style::default().bg(COLOR_USER).fg(COLOR_BACKGROUND).bold(),
        ),
        Span::styled(
            format!(" {}", timestamp),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ),
    ]));

    // Content with thick border on left and alternate background
    let border_char = "█";
    let content_lines: Vec<String> = if word_wrap_enabled {
        word_wrap(&content, width.saturating_sub(6))
    } else {
        content.lines().map(|s| s.to_string()).collect()
    };

    for line in content_lines {
        let padding = width.saturating_sub(line.len() + 5);
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                border_char,
                Style::default().fg(COLOR_USER),
            ),
            Span::styled(
                format!(" {}{}", line, " ".repeat(padding)),
                Style::default().fg(COLOR_TEXT_PRIMARY).bg(COLOR_BACKGROUND_ALT),
            ),
        ]));
    }

    // Spacing
    lines.push(Line::from(""));

    ListItem::new(lines)
}

fn render_assistant_message_lines(
    msg: &ChatMessage,
    _width: usize,
    show_badge: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let agent = msg.agent.clone().unwrap_or_else(|| "Assistant".to_string());
    let timestamp = msg.timestamp.clone();
    let content = msg.content.clone();

    // Header with agent badge (only show if badge should be visible)
    // Uses brand color for agent badge like Go TUI's AgentBadgeStyle
    if show_badge {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(" {} ", agent),
                Style::default()
                    .bg(COLOR_BRAND)  // Brand blue for agent badge
                    .fg(best_foreground_for_bg(COLOR_BRAND))  // Auto-calculated contrast
                    .bold(),
            ),
            Span::styled(
                format!(" {}", timestamp),
                Style::default().fg(COLOR_TEXT_SECONDARY),
            ),
        ]));
    }

    // Render content with markdown
    let rendered = render_markdown(&content, Style::default().fg(COLOR_TEXT_PRIMARY));
    for line in rendered {
        let mut indented_spans = vec![Span::raw("    ")];
        indented_spans.extend(line.spans);
        lines.push(Line::from(indented_spans));
    }

    // Spacing
    lines.push(Line::from(""));

    lines
}

#[allow(dead_code)]
fn render_assistant_message(msg: &ChatMessage, _width: usize) -> ListItem<'static> {
    ListItem::new(render_assistant_message_lines(msg, _width, true))
}

fn render_system_message_lines(msg: &ChatMessage, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let content = msg.content.clone();

    // Subtle divider
    let divider = "─".repeat(width.min(50));
    lines.push(Line::from(Span::styled(
        format!("  {}", divider),
        Style::default().fg(COLOR_BORDER),
    )));

    // Content with markdown support for help text
    let rendered = render_markdown(&content, Style::default().fg(COLOR_TEXT_SECONDARY));
    for line in rendered {
        let mut indented_spans = vec![Span::raw("  ")];
        indented_spans.extend(line.spans);
        lines.push(Line::from(indented_spans));
    }

    // Bottom divider
    lines.push(Line::from(Span::styled(
        format!("  {}", divider),
        Style::default().fg(COLOR_BORDER),
    )));
    lines.push(Line::from(""));

    lines
}

#[allow(dead_code)]
fn render_system_message(msg: &ChatMessage, width: usize) -> ListItem<'static> {
    ListItem::new(render_system_message_lines(msg, width))
}

fn render_error_message_lines(msg: &ChatMessage, _width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let timestamp = msg.timestamp.clone();
    let content = msg.content.clone();

    // Error header
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            " ✗ Error ",
            Style::default().bg(COLOR_ERROR).fg(COLOR_BACKGROUND).bold(),
        ),
        Span::styled(
            format!(" {}", timestamp),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ),
    ]));

    // Error content
    for line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(line.to_string(), Style::default().fg(COLOR_ERROR)),
        ]));
    }

    lines.push(Line::from(""));

    lines
}

#[allow(dead_code)]
fn render_error_message(msg: &ChatMessage, _width: usize) -> ListItem<'static> {
    ListItem::new(render_error_message_lines(msg, _width))
}

fn render_thinking_message_lines(
    msg: &ChatMessage,
    _width: usize,
    show_badge: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let timestamp = msg.timestamp.clone();
    let content = msg.content.clone();

    // Thinking header (only show if badge should be visible)
    if show_badge {
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                " 🤔 Thinking ",
                Style::default()
                    .bg(COLOR_WARNING)
                    .fg(COLOR_BACKGROUND)
                    .bold(),
            ),
            Span::styled(
                format!(" {}", timestamp),
                Style::default().fg(COLOR_TEXT_SECONDARY),
            ),
        ]));
    }

    // Thinking content (italicized)
    for line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("    ", Style::default()),
            Span::styled(
                line.to_string(),
                Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
            ),
        ]));
    }

    lines.push(Line::from(""));

    lines
}

#[allow(dead_code)]
fn render_thinking_message(msg: &ChatMessage, _width: usize) -> ListItem<'static> {
    ListItem::new(render_thinking_message_lines(msg, _width, true))
}

fn render_tool_message_lines(
    msg: &ChatMessage,
    width: usize,
    show_badge: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let tool_name = msg.tool_name.clone().unwrap_or_else(|| "tool".to_string());
    let timestamp = msg.timestamp.clone();
    let collapsed = msg.collapsed;
    let tool_output = msg.tool_output.clone();

    // Determine icon and color based on tool status
    let (icon, badge_bg) = match msg.tool_status {
        ToolStatus::Pending => ("⋯", COLOR_WARNING),  // Three dots for pending
        ToolStatus::Running => ("•", COLOR_PRIMARY),   // Bullet for running (spinner handled elsewhere)
        ToolStatus::Completed => ("✓", COLOR_ACCENT), // Checkmark for completed
        ToolStatus::Error => ("✗", COLOR_ERROR_DARK),  // X for error with ErrorDark background
        ToolStatus::Confirmation => ("❓", COLOR_WARNING), // Question mark for awaiting confirmation
    };

    // For error status, use stronger foreground color
    let badge_fg = match msg.tool_status {
        ToolStatus::Error => COLOR_ERROR_STRONG,
        ToolStatus::Confirmation => COLOR_BACKGROUND,
        _ => COLOR_BACKGROUND,
    };

    // Tool header with collapse indicator (only show if badge should be visible)
    if show_badge {
        let collapse_indicator = if tool_output.is_some() {
            if collapsed {
                "▶ "
            } else {
                "▼ "
            }
        } else {
            ""
        };

        lines.push(Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                format!(" {} {} ", icon, tool_name),
                Style::default().bg(badge_bg).fg(badge_fg).bold(),
            ),
            Span::styled(
                format!(" {}{}", collapse_indicator, timestamp),
                Style::default().fg(COLOR_TEXT_SECONDARY),
            ),
        ]));

        // Show tool description below the header if available
        if let Some(ref desc) = msg.tool_description {
            if !desc.is_empty() {
                let desc_truncated = if desc.len() > width.saturating_sub(4) {
                    format!("{}...", &desc[..width.saturating_sub(7)])
                } else {
                    desc.clone()
                };
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(desc_truncated, Style::default().fg(COLOR_TEXT_SECONDARY).italic()),
                ]));
            }
        }
    }

    // Show content or collapsed summary
    if let Some(ref output) = tool_output {
        if collapsed {
            // Show truncated preview
            let preview = if output.len() > 60 {
                format!("{}...", &output[..60])
            } else {
                output.clone()
            };
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(preview, Style::default().fg(COLOR_TEXT_SECONDARY)),
            ]));
        } else {
            // Show full output (limited lines)
            let max_lines = 20;
            let output_lines: Vec<&str> = output.lines().take(max_lines).collect();
            for line in &output_lines {
                let display_line = if line.len() > width {
                    format!("{}...", &line[..width.saturating_sub(3)])
                } else {
                    line.to_string()
                };
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(display_line, Style::default().fg(COLOR_TEXT_SECONDARY)),
                ]));
            }
            if output.lines().count() > max_lines {
                lines.push(Line::from(vec![
                    Span::styled("    ", Style::default()),
                    Span::styled(
                        format!("... ({} more lines)", output.lines().count() - max_lines),
                        Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
                    ),
                ]));
            }
        }
    } else {
        // Simple content display
        for line in msg.content.lines() {
            let display_line = if line.len() > width {
                format!("{}...", &line[..width.saturating_sub(3)])
            } else {
                line.to_string()
            };
            lines.push(Line::from(vec![
                Span::styled("    ", Style::default()),
                Span::styled(display_line, Style::default().fg(COLOR_TEXT_SECONDARY)),
            ]));
        }
    }

    lines.push(Line::from(""));

    lines
}

#[allow(dead_code)]
fn render_tool_message(msg: &ChatMessage, width: usize) -> ListItem<'static> {
    ListItem::new(render_tool_message_lines(msg, width, true))
}

/// Render shell output as a fenced console code block
fn render_shell_output_lines(msg: &ChatMessage, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let content = msg.content.clone();

    // Header showing "Shell Output"
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            " ☰ Shell Output ",
            Style::default().bg(COLOR_TEXT_SECONDARY).fg(COLOR_BACKGROUND).bold(),
        ),
        Span::styled(
            format!(" {}", msg.timestamp),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ),
    ]));

    // Code block styling for the output
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled("┌─ console", Style::default().fg(COLOR_TEXT_SECONDARY)),
    ]));

    // Content lines with code block prefix
    for line in content.lines().take(20) {
        let display_line = if line.len() > width.saturating_sub(6) {
            format!("{}...", &line[..width.saturating_sub(9)])
        } else {
            line.to_string()
        };
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(COLOR_TEXT_SECONDARY)),
            Span::styled(display_line, Style::default().fg(COLOR_TEXT_PRIMARY)),
        ]));
    }

    if content.lines().count() > 20 {
        lines.push(Line::from(vec![
            Span::styled("  │ ", Style::default().fg(COLOR_TEXT_SECONDARY)),
            Span::styled(
                format!("... ({} more lines)", content.lines().count() - 20),
                Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
            ),
        ]));
    }

    lines.push(Line::from(vec![
        Span::styled("  └─", Style::default().fg(COLOR_TEXT_SECONDARY)),
    ]));

    lines.push(Line::from(""));
    lines
}

#[allow(dead_code)]
fn render_shell_output(msg: &ChatMessage, width: usize) -> ListItem<'static> {
    ListItem::new(render_shell_output_lines(msg, width))
}

/// Render cancelled stream message with warning style
fn render_cancelled_message_lines(msg: &ChatMessage, _width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Warning styled message
    lines.push(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            " ⚠ stream cancelled ⚠ ",
            Style::default().bg(COLOR_WARNING).fg(COLOR_BACKGROUND).bold(),
        ),
        Span::styled(
            format!(" {}", msg.timestamp),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ),
    ]));

    lines.push(Line::from(""));
    lines
}

#[allow(dead_code)]
fn render_cancelled_message(msg: &ChatMessage, width: usize) -> ListItem<'static> {
    ListItem::new(render_cancelled_message_lines(msg, width))
}

/// Render welcome message with double border style like Go TUI
fn render_welcome_message_lines(msg: &ChatMessage, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let content = msg.content.clone();
    
    // Top border (double line)
    let border_width = width.min(60).saturating_sub(4);
    lines.push(Line::from(vec![
        Span::styled("  ╔", Style::default().fg(COLOR_PRIMARY)),
        Span::styled("═".repeat(border_width), Style::default().fg(COLOR_PRIMARY)),
        Span::styled("╗", Style::default().fg(COLOR_PRIMARY)),
    ]));
    
    // Title line
    let title = "✨ Welcome ✨";
    let title_padding = (border_width.saturating_sub(title.chars().count())) / 2;
    lines.push(Line::from(vec![
        Span::styled("  ║", Style::default().fg(COLOR_PRIMARY)),
        Span::raw(" ".repeat(title_padding)),
        Span::styled(title, Style::default().fg(COLOR_PRIMARY).bold()),
        Span::raw(" ".repeat(border_width.saturating_sub(title_padding + title.chars().count()))),
        Span::styled("║", Style::default().fg(COLOR_PRIMARY)),
    ]));
    
    // Empty line
    lines.push(Line::from(vec![
        Span::styled("  ║", Style::default().fg(COLOR_PRIMARY)),
        Span::raw(" ".repeat(border_width)),
        Span::styled("║", Style::default().fg(COLOR_PRIMARY)),
    ]));
    
    // Content lines
    for line in content.lines() {
        let line_text = if line.len() > border_width - 2 {
            format!("{}...", &line[..border_width.saturating_sub(5)])
        } else {
            line.to_string()
        };
        let line_padding = border_width.saturating_sub(line_text.len());
        lines.push(Line::from(vec![
            Span::styled("  ║", Style::default().fg(COLOR_PRIMARY)),
            Span::styled(format!(" {}", line_text), Style::default().fg(COLOR_TEXT_PRIMARY)),
            Span::raw(" ".repeat(line_padding.saturating_sub(1))),
            Span::styled("║", Style::default().fg(COLOR_PRIMARY)),
        ]));
    }
    
    // Empty line before bottom border
    lines.push(Line::from(vec![
        Span::styled("  ║", Style::default().fg(COLOR_PRIMARY)),
        Span::raw(" ".repeat(border_width)),
        Span::styled("║", Style::default().fg(COLOR_PRIMARY)),
    ]));
    
    // Bottom border (double line)
    lines.push(Line::from(vec![
        Span::styled("  ╚", Style::default().fg(COLOR_PRIMARY)),
        Span::styled("═".repeat(border_width), Style::default().fg(COLOR_PRIMARY)),
        Span::styled("╝", Style::default().fg(COLOR_PRIMARY)),
    ]));
    
    lines.push(Line::from(""));
    lines
}

#[allow(dead_code)]
fn render_welcome_message(msg: &ChatMessage, width: usize) -> ListItem<'static> {
    ListItem::new(render_welcome_message_lines(msg, width))
}

/// Render loading message with spinner animation (like Go TUI's MessageTypeLoading)
/// Shows spinner + truncated description, used during async operations
fn render_loading_message_lines(
    msg: &ChatMessage,
    width: usize,
    spinner_frame: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    
    // Get the spinner character for current frame
    let spinner = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
    let spinner_color = spinner_color_for_frame(spinner_frame);
    
    // Agent badge (if present)
    if let Some(agent) = &msg.agent {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!(" {} ", agent),
                Style::default()
                    .fg(best_foreground_for_bg(COLOR_BRAND))
                    .bg(COLOR_BRAND)
                    .bold(),
            ),
        ]));
        lines.push(Line::from(""));
    }
    
    // Truncate description if needed (leaving room for spinner)
    let max_desc_len = width.saturating_sub(8); // space for spinner + padding
    let description = if msg.content.len() > max_desc_len {
        format!("{}…", &msg.content[..max_desc_len.saturating_sub(1)])
    } else {
        msg.content.clone()
    };
    
    // Loading line with spinner and description
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(spinner, Style::default().fg(spinner_color)),
        Span::raw(" "),
        Span::styled(description, Style::default().fg(COLOR_TEXT_SECONDARY)),
    ]));
    
    lines.push(Line::from(""));
    lines
}

#[allow(dead_code)]
fn render_loading_message(
    msg: &ChatMessage,
    width: usize,
    spinner_frame: usize,
) -> ListItem<'static> {
    ListItem::new(render_loading_message_lines(msg, width, spinner_frame))
}

fn draw_input(f: &mut Frame, app: &mut App, area: Rect) {
    // Go version: No border, just a textarea with top padding
    // Uses InputStyle with accent-colored cursor
    // Placeholder: "Type your message here…"

    // Add 1 line top padding like Go's EditorStyle.Padding(1, 0, 0, 0)
    let padded_area = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    // No block/border - just the raw textarea
    app.input.set_block(Block::default());
    app.input
        .set_cursor_style(Style::default().bg(COLOR_ACCENT));
    app.input.set_placeholder_text("Type your message here…");
    app.input
        .set_placeholder_style(Style::default().fg(COLOR_TEXT_SECONDARY));

    f.render_widget(&app.input, padded_area);
}

fn draw_status_bar(f: &mut Frame, app: &App, area: Rect) {
    // Format: "Key desc  Key desc  ..." on left, "cagent version" on right
    // Keys are highlighted (white/bold), descriptions are secondary (muted)

    let width = area.width as usize;

    // Build help bindings based on current focus
    let bindings: Vec<(&str, &str)> = if app.sidebar_focused {
        vec![
            ("Tab", "switch focus"),
            ("↑↓", "navigate"),
            ("Ctrl+←→", "resize"),
            ("Enter", "toggle"),
        ]
    } else if app.working {
        vec![("Esc", "interrupt"), ("Tab", "switch focus")]
    } else {
        vec![
            ("Tab", "switch focus"),
            ("Shift+Enter", "newline"),
            ("Ctrl+g", "external editor"),
        ]
    };

    // Build help text spans
    let mut help_spans: Vec<Span> = Vec::new();
    for (i, (key, desc)) in bindings.iter().enumerate() {
        if i > 0 {
            help_spans.push(Span::raw("  ")); // Two spaces between bindings
        }
        help_spans.push(Span::styled(
            *key,
            Style::default().fg(COLOR_TEXT_PRIMARY).bold(),
        ));
        help_spans.push(Span::styled(
            format!(" {}", desc),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ));
    }

    // Version text - use actual package version
    let version_text = format!("cagent {}", env!("CARGO_PKG_VERSION"));
    let version_span = Span::styled(&version_text, Style::default().fg(COLOR_TEXT_SECONDARY));

    // Calculate widths
    let help_width: usize = bindings
        .iter()
        .enumerate()
        .map(|(i, (k, d))| {
            let sep = if i > 0 { 2 } else { 0 };
            sep + k.len() + 1 + d.len()
        })
        .sum();
    let version_width = version_text.len();

    // Spacer between help and version
    let spacer_width = width.saturating_sub(help_width + version_width + 2); // +2 for padding
    let spacer = " ".repeat(spacer_width);

    // Build final line: " help...  spacer  version "
    let mut spans = vec![Span::raw(" ")]; // Left padding
    spans.extend(help_spans);
    spans.push(Span::raw(spacer));
    spans.push(version_span);
    spans.push(Span::raw(" ")); // Right padding

    let line = Line::from(spans);
    let status_bar = Paragraph::new(line);

    f.render_widget(status_bar, area);
}

// ============================================================================
// Dialog Helpers
// ============================================================================

/// Render a horizontal separator line
fn render_separator(width: usize) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(COLOR_BORDER),
    ))
}

/// Render help keys in dialog style: [K] description
fn render_help_keys(bindings: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, desc)) in bindings.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!("[{}]", key),
            Style::default().fg(COLOR_ACCENT).bold(),
        ));
        spans.push(Span::styled(
            format!(" {}", desc),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        ));
    }
    Line::from(spans)
}

/// Draw a styled dialog with rounded borders, centered title, and proper content
fn draw_styled_dialog(
    f: &mut Frame,
    title: &str,
    content_lines: Vec<Line>,
    help_keys: &[(&str, &str)],
    area: Rect,
    width_percent: u16,
    border_color: Color,
) {
    // Calculate dialog dimensions
    let dialog_width = (area.width * width_percent / 100).clamp(40, 80);
    let content_width = dialog_width.saturating_sub(4) as usize; // -4 for borders and padding

    // Build the full content
    let mut lines = Vec::new();

    // Title (centered)
    let title_text = format!(" {} ", title);
    let title_padding = (content_width.saturating_sub(title_text.len())) / 2;
    lines.push(Line::from(Span::styled(
        format!(
            "{}{}{}",
            " ".repeat(title_padding),
            title_text,
            " ".repeat(title_padding)
        ),
        Style::default().fg(COLOR_TEXT_SECONDARY).bold(),
    )));

    // Separator
    lines.push(render_separator(content_width));
    lines.push(Line::from(""));

    // Content
    for line in content_lines {
        lines.push(line);
    }

    // Spacing before help
    lines.push(Line::from(""));

    // Help keys (centered)
    if !help_keys.is_empty() {
        lines.push(render_help_keys(help_keys));
    }

    // Calculate height based on content
    let dialog_height = (lines.len() as u16 + 2).min(area.height - 2); // +2 for borders

    // Center the dialog
    let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
    let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);

    // Clear background
    f.render_widget(Clear, dialog_area);

    // Render dialog with rounded border
    let dialog = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .style(Style::default().bg(COLOR_BACKGROUND)),
        )
        .alignment(Alignment::Center);

    f.render_widget(dialog, dialog_area);
}

fn draw_confirmation_dialog(f: &mut Frame, app: &App, area: Rect) {
    if let Some(ref conf) = app.pending_confirmation {
        // Check if we have a diff preview
        if let Some(ref diff) = conf.diff_preview {
            draw_diff_confirmation_dialog(f, conf, diff, area);
            return;
        }

        // Format tool arguments (truncate if too long)
        let args_preview = if conf.tool_args.len() > 100 {
            format!("{}...", &conf.tool_args[..100])
        } else {
            conf.tool_args.clone()
        };

        // Build content lines
        let mut content = vec![
            Line::from(Span::styled(
                "Do you want to allow this tool call?",
                Style::default().fg(COLOR_TEXT_PRIMARY).bold(),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Tool: ", Style::default().fg(COLOR_TEXT_SECONDARY)),
                Span::styled(&conf.tool_name, Style::default().fg(COLOR_TOOL).bold()),
            ]),
        ];

        // Add tool description if available
        if let Some(ref desc) = conf.tool_description {
            let desc_preview = if desc.len() > 80 {
                format!("{}...", &desc[..80])
            } else {
                desc.clone()
            };
            content.push(Line::from(""));
            content.push(Line::from(Span::styled(
                desc_preview,
                Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
            )));
        }

        content.push(Line::from(""));
        content.push(Line::from(Span::styled(
            "Arguments:",
            Style::default().fg(COLOR_TEXT_SECONDARY),
        )));
        // Highlight tool arguments as JSON
        let args_spans = highlight_json(&args_preview);
        content.push(Line::from(args_spans));

        let help_keys = &[
            ("Y", "yes"),
            ("N", "no"),
            ("A", "all (approve all this session)"),
        ];

        draw_styled_dialog(
            f,
            "Tool Confirmation",
            content,
            help_keys,
            area,
            60,
            COLOR_WARNING,
        );
    }
}

/// Draw a specialized confirmation dialog with diff preview for edit_file operations
fn draw_diff_confirmation_dialog(
    f: &mut Frame,
    _conf: &PendingConfirmation,
    diff: &DiffPreview,
    area: Rect,
) {
    // Calculate dialog dimensions - make it wider for diffs
    let dialog_width = (area.width * 80 / 100).clamp(60, 100);
    let content_width = dialog_width.saturating_sub(4) as usize;

    let mut lines = vec![
        // Title
        Line::from(Span::styled(
            "  File Edit Confirmation  ",
            Style::default().fg(COLOR_TEXT_SECONDARY).bold(),
        )),
        render_separator(content_width),
        Line::from(""),
        // File path
        Line::from(vec![
            Span::styled("File: ", Style::default().fg(COLOR_TEXT_SECONDARY)),
            Span::styled(&diff.file_path, Style::default().fg(COLOR_PRIMARY).bold()),
        ]),
        Line::from(""),
    ];

    // Diff hunks
    for (hunk_idx, hunk) in diff.hunks.iter().enumerate() {
        if hunk_idx > 0 {
            lines.push(Line::from(Span::styled(
                "───────────────",
                Style::default().fg(COLOR_BORDER),
            )));
        }

        // Hunk header
        lines.push(Line::from(Span::styled(
            format!(
                "@@ -{},{} +{},{} @@",
                hunk.old_start,
                hunk.lines
                    .iter()
                    .filter(|l| !matches!(l, DiffLine::Added(_)))
                    .count(),
                hunk.new_start,
                hunk.lines
                    .iter()
                    .filter(|l| !matches!(l, DiffLine::Removed(_)))
                    .count(),
            ),
            Style::default().fg(COLOR_PRIMARY),
        )));

        // Diff lines (limit to prevent dialog from being too tall)
        let max_lines_per_hunk = 15;
        let displayed_lines = hunk.lines.iter().take(max_lines_per_hunk);
        let total_lines = hunk.lines.len();

        for diff_line in displayed_lines {
            let (prefix, text, style) = match diff_line {
                DiffLine::Context(text) => (
                    " ",
                    text.as_str(),
                    Style::default().fg(COLOR_TEXT_SECONDARY),
                ),
                DiffLine::Removed(text) => (
                    "-",
                    text.as_str(),
                    Style::default().fg(COLOR_DIFF_REMOVE_FG).bg(COLOR_DIFF_REMOVE_BG),
                ),
                DiffLine::Added(text) => (
                    "+",
                    text.as_str(),
                    Style::default().fg(COLOR_DIFF_ADD_FG).bg(COLOR_DIFF_ADD_BG),
                ),
            };

            // Truncate long lines
            let display_text = if text.len() > content_width.saturating_sub(4) {
                format!("{}...", &text[..content_width.saturating_sub(7)])
            } else {
                text.to_string()
            };

            lines.push(Line::from(vec![
                Span::styled(prefix, style.bold()),
                Span::styled(" ", Style::default()),
                Span::styled(display_text, style),
            ]));
        }

        // Show truncation indicator
        if total_lines > max_lines_per_hunk {
            lines.push(Line::from(Span::styled(
                format!("  ... {} more lines", total_lines - max_lines_per_hunk),
                Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
            )));
        }
    }

    lines.push(Line::from(""));

    // Help keys
    lines.push(render_help_keys(&[
        ("Y", "yes, apply"),
        ("N", "no, reject"),
        ("A", "approve all this session"),
    ]));

    // Calculate height based on content (cap at 80% of screen)
    let dialog_height = (lines.len() as u16 + 2).min(area.height * 80 / 100);

    // Center the dialog
    let dialog_x = (area.width.saturating_sub(dialog_width)) / 2;
    let dialog_y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(dialog_x, dialog_y, dialog_width, dialog_height);

    // Clear background
    f.render_widget(Clear, dialog_area);

    // Render dialog with rounded border
    let dialog = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(ratatui::widgets::BorderType::Rounded)
                .border_style(Style::default().fg(COLOR_WARNING))
                .style(Style::default().bg(COLOR_BACKGROUND)),
        )
        .alignment(Alignment::Left);

    f.render_widget(dialog, dialog_area);
}

fn draw_exit_confirmation_dialog(f: &mut Frame, area: Rect) {
    let content = vec![
        Line::from(Span::styled(
            "Are you sure you want to exit?",
            Style::default().fg(COLOR_TEXT_PRIMARY).bold(),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Your session history will be lost.",
            Style::default().fg(COLOR_TEXT_SECONDARY),
        )),
    ];

    let help_keys = &[("Y", "yes, exit"), ("N", "no, stay")];

    draw_styled_dialog(
        f,
        "Exit Confirmation",
        content,
        help_keys,
        area,
        50,
        COLOR_ERROR,
    );
}

fn draw_elicitation_dialog(f: &mut Frame, app: &App, area: Rect) {
    if let Some(ref elicit) = app.pending_elicitation {
        // Build content lines based on elicitation request
        let mut content = vec![
            Line::from(Span::styled(
                &elicit.message,
                Style::default().fg(COLOR_TEXT_PRIMARY).bold(),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Server: ", Style::default().fg(COLOR_TEXT_SECONDARY)),
                Span::styled(&elicit.server_name, Style::default().fg(COLOR_TOOL).bold()),
            ]),
            Line::from(""),
        ];

        // If we have parsed fields, show them as a form
        if !elicit.fields.is_empty() {
            content.push(Line::from(Span::styled(
                "Please provide the following information:",
                Style::default().fg(COLOR_TEXT_SECONDARY),
            )));
            content.push(Line::from(""));

            for (i, field) in elicit.fields.iter().enumerate() {
                let is_current = i == elicit.field_index;
                let indicator = if is_current { "▸ " } else { "  " };
                let required = if field.required { "*" } else { "" };

                // Field name with indicator
                let name_style = if is_current {
                    Style::default().fg(COLOR_PRIMARY).bold()
                } else {
                    Style::default().fg(COLOR_TEXT_PRIMARY)
                };

                content.push(Line::from(vec![
                    Span::styled(indicator, Style::default().fg(COLOR_PRIMARY)),
                    Span::styled(&field.name, name_style),
                    Span::styled(required, Style::default().fg(COLOR_ERROR)),
                    Span::styled(": ", Style::default().fg(COLOR_TEXT_SECONDARY)),
                    Span::styled(
                        if field.value.is_empty() {
                            "(empty)".to_string()
                        } else if matches!(field.field_type, ElicitationFieldType::Password) {
                            "*".repeat(field.value.len())
                        } else {
                            field.value.clone()
                        },
                        if is_current {
                            Style::default().fg(COLOR_ACCENT)
                        } else {
                            Style::default().fg(COLOR_TEXT_SECONDARY)
                        },
                    ),
                ]));

                // Show description if available
                if !field.description.is_empty() {
                    content.push(Line::from(vec![
                        Span::raw("     "),
                        Span::styled(
                            &field.description,
                            Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
                        ),
                    ]));
                }
            }
        } else {
            // Simple text input for schema-less elicitation
            content.push(Line::from(vec![
                Span::styled("Input: ", Style::default().fg(COLOR_TEXT_SECONDARY)),
                Span::styled(
                    if elicit.user_input.is_empty() {
                        "(type your response)".to_string()
                    } else {
                        elicit.user_input.clone()
                    },
                    Style::default().fg(COLOR_ACCENT),
                ),
            ]));
        }

        let help_keys = &[
            ("Enter", "submit"),
            ("Tab", "next field"),
            ("Esc", "cancel"),
        ];

        draw_styled_dialog(
            f,
            "MCP Server Request",
            content,
            help_keys,
            area,
            65,
            COLOR_TOOL,
        );
    }
}
fn draw_completions_popup(f: &mut Frame, app: &App, input_area: Rect) {
    let content = app.input.lines().join("\n");
    let matches = get_matching_commands(&content);

    if matches.is_empty() {
        return;
    }

    // Position popup above the input area
    let popup_height = (matches.len() as u16).min(8) + 2; // +2 for borders
    let popup_width = 45;
    let popup_x = input_area.x + 1;
    let popup_y = input_area.y.saturating_sub(popup_height);

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    // Get search pattern (without leading /)
    let search_pattern = content.trim_start_matches('/');
    
    let items: Vec<ListItem> = matches
        .iter()
        .enumerate()
        .map(|(i, (cmd, desc))| {
            let is_selected = i == app.completion_index;
            let (base_cmd_style, highlight_style, desc_style) = if is_selected {
                (
                    Style::default().fg(COLOR_BACKGROUND).bg(COLOR_BRAND),
                    Style::default().fg(COLOR_BACKGROUND).bg(COLOR_BRAND).bold().underlined(),
                    Style::default().fg(COLOR_BACKGROUND).bg(COLOR_BRAND),
                )
            } else {
                (
                    Style::default().fg(COLOR_PRIMARY),
                    Style::default().fg(COLOR_ACCENT).bold(),
                    Style::default().fg(COLOR_TEXT_SECONDARY),
                )
            };
            
            // Get match indices for highlighting (skip the leading /)
            let cmd_without_slash = &cmd[1..];
            let match_indices = fuzzy_match_indices(search_pattern, cmd_without_slash);
            
            // Build command text with highlighted characters
            let mut cmd_spans: Vec<Span> = vec![Span::styled(" /", base_cmd_style)];
            for (idx, c) in cmd_without_slash.chars().enumerate() {
                let style = if match_indices.contains(&idx) {
                    highlight_style
                } else {
                    base_cmd_style
                };
                cmd_spans.push(Span::styled(c.to_string(), style));
            }
            
            // Pad command to fixed width
            let cmd_len = cmd.len();
            let padding = 12usize.saturating_sub(cmd_len);
            cmd_spans.push(Span::styled(" ".repeat(padding), base_cmd_style));
            
            cmd_spans.push(Span::styled(format!(" {}", desc), desc_style));

            ListItem::new(Line::from(cmd_spans))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_BORDER))
            .style(Style::default().bg(COLOR_BACKGROUND))
            .title(Span::styled(
                " Commands ",
                Style::default().fg(COLOR_TEXT_SECONDARY),
            ))
            .title_bottom(Span::styled(
                " Tab: select • Enter: confirm • Esc: close ",
                Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
            )),
    );

    f.render_widget(list, popup_area);
}

// ============================================================================
// Helpers
// ============================================================================

fn draw_file_completions_popup(f: &mut Frame, app: &App, input_area: Rect) {
    if app.file_completions.is_empty() {
        return;
    }

    // Position popup above the input area
    let popup_height = (app.file_completions.len() as u16).min(10) + 2; // +2 for borders
    let popup_width = 55;
    let popup_x = input_area.x + 1;
    let popup_y = input_area.y.saturating_sub(popup_height);

    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    f.render_widget(Clear, popup_area);

    let items: Vec<ListItem> = app
        .file_completions
        .iter()
        .enumerate()
        .map(|(i, file)| {
            let is_dir = file.ends_with('/');
            let (icon_style, name_style) = if i == app.file_completion_index {
                (
                    Style::default().fg(COLOR_BACKGROUND).bg(COLOR_BRAND),
                    Style::default().fg(COLOR_BACKGROUND).bg(COLOR_BRAND),
                )
            } else if is_dir {
                (
                    Style::default().fg(COLOR_PRIMARY),
                    Style::default().fg(COLOR_PRIMARY),
                )
            } else {
                (
                    Style::default().fg(COLOR_TEXT_SECONDARY),
                    Style::default().fg(COLOR_TEXT_PRIMARY),
                )
            };

            let icon = if is_dir { " 📁 " } else { " 📄 " };

            ListItem::new(Line::from(vec![
                Span::styled(icon, icon_style),
                Span::styled(file, name_style),
            ]))
        })
        .collect();

    let title = format!(
        " Files: @{} ",
        if app.file_completion_prefix.len() > 20 {
            format!(
                "...{}",
                &app.file_completion_prefix[app.file_completion_prefix.len() - 20..]
            )
        } else {
            app.file_completion_prefix.clone()
        }
    );

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(COLOR_BORDER))
            .style(Style::default().bg(COLOR_BACKGROUND))
            .title(Span::styled(title, Style::default().fg(COLOR_PRIMARY)))
            .title_bottom(Span::styled(
                " Tab: select • Enter: confirm • Esc: close ",
                Style::default().fg(COLOR_TEXT_SECONDARY).italic(),
            )),
    );

    f.render_widget(list, popup_area);
}

/// Format a duration in a human-readable way
fn format_duration(duration: chrono::Duration) -> String {
    let secs = duration.num_seconds();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        let mins = secs / 60;
        let remaining_secs = secs % 60;
        if remaining_secs == 0 {
            format!("{}m", mins)
        } else {
            format!("{}m {}s", mins, remaining_secs)
        }
    } else {
        let hours = secs / 3600;
        let mins = (secs % 3600) / 60;
        if mins == 0 {
            format!("{}h", hours)
        } else {
            format!("{}h {}m", hours, mins)
        }
    }
}

fn format_tokens(count: i64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}\u{2026}", &s[..max_len.saturating_sub(1)]) // Use ellipsis character
    }
}

/// Wrap text to fit within a given width
fn word_wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![text.to_string()];
    }

    let mut lines = Vec::new();

    for line in text.lines() {
        if line.len() <= width {
            lines.push(line.to_string());
            continue;
        }

        let mut current = String::new();
        for word in line.split_whitespace() {
            if current.is_empty() {
                if word.len() > width {
                    // Word is too long, break it
                    for chunk in word.chars().collect::<Vec<_>>().chunks(width) {
                        lines.push(chunk.iter().collect());
                    }
                } else {
                    current = word.to_string();
                }
            } else if current.len() + 1 + word.len() <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(current);
                current = word.to_string();
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Update completion state (both command and file completions)
fn update_completion_state(app: &mut App) {
    let content = app.get_input_content();

    // Calculate cursor position in content
    let lines = app.input.lines();
    let (row, col) = app.input.cursor();
    let mut cursor_pos = 0;
    for (i, line) in lines.iter().enumerate() {
        if i < row {
            cursor_pos += line.len() + 1; // +1 for newline
        } else {
            cursor_pos += col;
            break;
        }
    }

    // Check for @ file reference
    if let Some((at_pos, prefix)) = extract_at_reference(&content, cursor_pos) {
        app.file_completion_prefix = prefix.clone();
        app.file_completions = get_file_completions(&prefix, &app.working_directory, 10);
        app.showing_file_completions = !app.file_completions.is_empty();
        app.file_completion_index = 0;
        app.showing_completions = false;
        let _ = at_pos; // Used for positioning later
    } else {
        app.showing_file_completions = false;

        // Check for command completion
        let matches = get_matching_commands(&content);
        app.showing_completions = !matches.is_empty() && content.len() > 1;
        app.completion_index = 0;
    }
}

/// Get file completions for a given prefix
fn get_file_completions(prefix: &str, working_dir: &str, max_results: usize) -> Vec<String> {
    use std::path::Path;

    let base_path = if prefix.is_empty() {
        Path::new(working_dir).to_path_buf()
    } else if prefix.starts_with('/') || prefix.starts_with('~') {
        // Absolute path or home directory
        let expanded = if prefix.starts_with('~') {
            dirs::home_dir()
                .map(|h| h.join(&prefix[2..]))
                .unwrap_or_else(|| Path::new(prefix).to_path_buf())
        } else {
            Path::new(prefix).to_path_buf()
        };
        expanded
    } else {
        // Relative path
        Path::new(working_dir).join(prefix)
    };

    // Determine the directory to search and the prefix to match
    let (search_dir, name_prefix) = if base_path.is_dir() && prefix.ends_with('/') {
        (base_path.clone(), String::new())
    } else if let Some(parent) = base_path.parent() {
        let name = base_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        (parent.to_path_buf(), name)
    } else {
        (Path::new(working_dir).to_path_buf(), prefix.to_string())
    };

    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&search_dir) {
        for entry in entries.filter_map(|e| e.ok()) {
            let file_name = entry.file_name().to_string_lossy().to_string();

            // Skip hidden files unless explicitly searching for them
            if file_name.starts_with('.') && !name_prefix.starts_with('.') {
                continue;
            }

            // Match prefix (case-insensitive)
            if name_prefix.is_empty()
                || file_name
                    .to_lowercase()
                    .starts_with(&name_prefix.to_lowercase())
            {
                let path = entry.path();
                let display_path = if path.is_dir() {
                    format!("{}/", file_name)
                } else {
                    file_name
                };
                results.push(display_path);
            }

            if results.len() >= max_results {
                break;
            }
        }
    }

    results.sort();
    results
}

/// Extract the @ reference being typed at the cursor position
fn extract_at_reference(content: &str, cursor_pos: usize) -> Option<(usize, String)> {
    // Find the start of the @ reference
    let before_cursor = &content[..cursor_pos.min(content.len())];

    // Look backwards for @
    if let Some(at_pos) = before_cursor.rfind('@') {
        // Make sure it's not escaped or part of an email
        if at_pos > 0 {
            let prev_char = before_cursor.chars().nth(at_pos - 1);
            // Skip if preceded by alphanumeric (likely email)
            if prev_char.map(|c| c.is_alphanumeric()).unwrap_or(false) {
                return None;
            }
        }

        // Extract the path after @
        let path = &before_cursor[at_pos + 1..];

        // Don't complete if there's a space (reference ended)
        if path.contains(' ') {
            return None;
        }

        return Some((at_pos, path.to_string()));
    }

    None
}

/// Check if pattern fuzzy matches target (case-insensitive)
fn fuzzy_match(pattern: &str, target: &str) -> Option<i32> {
    let pattern = pattern.to_lowercase();
    let target = target.to_lowercase();

    // Empty pattern matches everything
    if pattern.is_empty() {
        return Some(0);
    }

    let mut pattern_chars = pattern.chars().peekable();
    let mut score = 0;
    let mut consecutive = 0;
    let mut last_match_idx: Option<usize> = None;

    for (idx, c) in target.chars().enumerate() {
        if let Some(&p) = pattern_chars.peek() {
            if c == p {
                pattern_chars.next();

                // Bonus for consecutive matches
                if last_match_idx == Some(idx.saturating_sub(1)) {
                    consecutive += 1;
                    score += consecutive * 2;
                } else {
                    consecutive = 0;
                }

                // Bonus for matching at start
                if idx == 0 {
                    score += 10;
                }

                // Bonus for matching after separator
                if idx > 0 {
                    let prev = target.chars().nth(idx - 1);
                    if prev == Some('/') || prev == Some(' ') || prev == Some('_') {
                        score += 5;
                    }
                }

                last_match_idx = Some(idx);
                score += 1;
            }
        }
    }

    // All pattern characters must be found
    if pattern_chars.peek().is_some() {
        None
    } else {
        Some(score)
    }
}

/// Get indices of characters that matched in a fuzzy match
fn fuzzy_match_indices(pattern: &str, target: &str) -> Vec<usize> {
    let pattern = pattern.to_lowercase();
    let target = target.to_lowercase();
    let mut indices = Vec::new();
    
    if pattern.is_empty() {
        return indices;
    }
    
    let mut pattern_chars = pattern.chars().peekable();
    
    for (idx, c) in target.chars().enumerate() {
        if let Some(&p) = pattern_chars.peek() {
            if c == p {
                pattern_chars.next();
                indices.push(idx);
            }
        }
        if pattern_chars.peek().is_none() {
            break;
        }
    }
    
    indices
}

/// Get commands that match the current input (fuzzy matching)
fn get_matching_commands(input: &str) -> Vec<(&'static str, &'static str)> {
    if !input.starts_with('/') {
        return Vec::new();
    }

    let search = &input[1..]; // Remove the leading '/'

    // If empty search, return all commands
    if search.is_empty() {
        return COMMANDS.to_vec();
    }

    let mut matches: Vec<((&'static str, &'static str), i32)> = COMMANDS
        .iter()
        .filter_map(|(cmd, desc)| {
            // Try fuzzy match on command name (without the '/')
            let cmd_name = &cmd[1..];
            fuzzy_match(search, cmd_name).map(|score| ((*cmd, *desc), score))
        })
        .collect();

    // Sort by score (highest first)
    matches.sort_by(|a, b| b.1.cmp(&a.1));

    matches.into_iter().map(|(cmd, _)| cmd).collect()
}

/// Copy the conversation to clipboard
fn copy_conversation_to_clipboard(app: &App) -> anyhow::Result<()> {
    let content = format_conversation_as_markdown(app);
    let mut clipboard = arboard::Clipboard::new()?;
    clipboard.set_text(content)?;
    Ok(())
}

/// Export conversation to a file
fn export_conversation(app: &App, filename: &str) -> anyhow::Result<()> {
    let content = format_conversation_as_markdown(app);
    std::fs::write(filename, content)?;
    Ok(())
}

/// Check if the cursor is currently inside a code block (between ``` markers)
#[allow(dead_code)]
fn is_in_code_block(content: &str) -> bool {
    let mut in_block = false;
    for line in content.lines() {
        if line.trim().starts_with("```") {
            in_block = !in_block;
        }
    }
    // If we end with an odd number of ``` markers, we're in a code block
    in_block
}

/// Format conversation as markdown
fn format_conversation_as_markdown(app: &App) -> String {
    let mut output = String::new();

    output.push_str(&format!(
        "# {}

",
        app.session_title
    ));
    output.push_str(&format!(
        "**Date:** {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    output.push_str(&format!("**Agent:** {}\n", app.current_agent));
    output.push_str(&format!("**Model:** {}\n\n", app.agent_model));
    output.push_str("---\n\n");

    for msg in &app.messages {
        let role = match msg.role {
            MessageRole::User => "**You**",
            MessageRole::Assistant => {
                &format!("**{}**", msg.agent.as_deref().unwrap_or("Assistant"))
            }
            MessageRole::System => "*System*",
            MessageRole::Error => "**Error**",
            MessageRole::Thinking => "*Thinking*",
            MessageRole::Tool => {
                &format!("*Tool: {}*", msg.tool_name.as_deref().unwrap_or("unknown"))
            }
            MessageRole::ShellOutput => "*Shell Output*",
            MessageRole::Cancelled => "*Cancelled*",
            MessageRole::Welcome => "*Welcome*",
            MessageRole::Loading => "*Loading*",
        };

        output.push_str(&format!(
            "### {} ({})\n\n{}\n\n",
            role, msg.timestamp, msg.content
        ));
    }

    output.push_str("---\n\n");
    output.push_str(&format!(
        "**Token Usage:** {} input, {} output, ${:.4} cost\n",
        app.input_tokens, app.output_tokens, app.cost
    ));

    output
}

/// Compute a diff preview for edit_file operations
fn compute_edit_file_diff(args_json: &str, working_dir: &str) -> Option<DiffPreview> {
    use std::path::Path;

    // Parse the edit_file arguments
    #[derive(serde::Deserialize)]
    struct EditFileArgs {
        path: String,
        edits: Vec<Edit>,
    }

    #[derive(serde::Deserialize)]
    struct Edit {
        #[serde(rename = "oldText")]
        old_text: String,
        #[serde(rename = "newText")]
        new_text: String,
    }

    let args: EditFileArgs = serde_json::from_str(args_json).ok()?;

    // Resolve the file path
    let file_path = if Path::new(&args.path).is_absolute() {
        args.path.clone()
    } else {
        Path::new(working_dir)
            .join(&args.path)
            .to_string_lossy()
            .to_string()
    };

    // Read the original file content
    let original_content = std::fs::read_to_string(&file_path).ok()?;

    // Generate hunks for each edit
    let mut hunks = Vec::new();
    let original_lines: Vec<&str> = original_content.lines().collect();

    for edit in &args.edits {
        // Find where the old_text appears in the original content
        if let Some(edit_start_pos) = original_content.find(&edit.old_text) {
            // Count which line number this is
            let line_num = original_content[..edit_start_pos].matches('\n').count();

            // Create the diff hunk
            let old_lines: Vec<&str> = edit.old_text.lines().collect();
            let new_lines: Vec<&str> = edit.new_text.lines().collect();

            let mut hunk_lines = Vec::new();

            // Add context lines before (up to 2)
            let context_start = line_num.saturating_sub(2);
            for i in context_start..line_num {
                if let Some(line) = original_lines.get(i) {
                    hunk_lines.push(DiffLine::Context(line.to_string()));
                }
            }

            // Add removed lines
            for line in &old_lines {
                hunk_lines.push(DiffLine::Removed(line.to_string()));
            }

            // Add added lines
            for line in &new_lines {
                hunk_lines.push(DiffLine::Added(line.to_string()));
            }

            // Add context lines after (up to 2)
            let context_end = (line_num + old_lines.len() + 2).min(original_lines.len());
            for i in (line_num + old_lines.len())..context_end {
                if let Some(line) = original_lines.get(i) {
                    hunk_lines.push(DiffLine::Context(line.to_string()));
                }
            }

            hunks.push(DiffHunk {
                old_start: context_start + 1, // 1-indexed
                new_start: context_start + 1,
                lines: hunk_lines,
            });
        }
    }

    if hunks.is_empty() {
        return None;
    }

    Some(DiffPreview {
        file_path: args.path,
        hunks,
    })
}
