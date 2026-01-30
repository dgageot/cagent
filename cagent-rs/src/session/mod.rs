//! Session management

mod title;

pub use title::TitleGenerator;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::Agent;
use crate::chat::{Message, MessageRole};

// Re-export PermissionsConfig from the permissions module
pub use crate::permissions::PermissionsConfig;

/// A session item - either a message or a sub-session
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionItem {
    Message { message: Box<SessionMessage> },
    SubSession { sub_session: Box<Session> },
    Summary { summary: String },
}

impl SessionItem {
    pub fn is_message(&self) -> bool {
        matches!(self, SessionItem::Message { .. })
    }

    pub fn is_sub_session(&self) -> bool {
        matches!(self, SessionItem::SubSession { .. })
    }

    pub fn as_message(&self) -> Option<&SessionMessage> {
        match self {
            SessionItem::Message { message } => Some(message.as_ref()),
            _ => None,
        }
    }
}

/// A message associated with an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    #[serde(rename = "agentName")]
    pub agent_name: Option<String>,
    pub message: Message,
    #[serde(default)]
    pub implicit: bool,
}

impl SessionMessage {
    pub fn new(agent: Option<&Agent>, message: Message) -> Self {
        Self {
            agent_name: agent.map(|a| a.name.clone()),
            message,
            implicit: false,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            agent_name: None,
            message: Message::user(content),
            implicit: false,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self {
            agent_name: None,
            message: Message::system(content),
            implicit: false,
        }
    }

    pub fn implicit_user(content: impl Into<String>) -> Self {
        let mut msg = Self::user(content);
        msg.implicit = true;
        msg
    }
}

/// A conversation session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub messages: Vec<SessionItem>,
    pub created_at: chrono::DateTime<chrono::Utc>,

    #[serde(default)]
    pub tools_approved: bool,

    #[serde(default)]
    pub thinking: bool,

    #[serde(default)]
    pub hide_tool_results: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,

    #[serde(default)]
    pub send_user_message: bool,

    #[serde(default)]
    pub max_iterations: usize,

    #[serde(default)]
    pub starred: bool,

    #[serde(default)]
    pub input_tokens: i64,

    #[serde(default)]
    pub output_tokens: i64,

    #[serde(default)]
    pub cost: f64,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionsConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            title: String::new(),
            messages: Vec::new(),
            created_at: chrono::Utc::now(),
            tools_approved: false,
            thinking: false,
            hide_tool_results: false,
            working_dir: None,
            send_user_message: true,
            max_iterations: 0,
            starred: false,
            input_tokens: 0,
            output_tokens: 0,
            cost: 0.0,
            permissions: None,
            parent_id: None,
        }
    }
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    pub fn with_working_dir(mut self, dir: impl Into<String>) -> Self {
        self.working_dir = Some(dir.into());
        self
    }

    pub fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    pub fn with_tools_approved(mut self, approved: bool) -> Self {
        self.tools_approved = approved;
        self
    }

    pub fn with_thinking(mut self, thinking: bool) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn with_user_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(SessionItem::Message {
            message: Box::new(SessionMessage::user(content)),
        });
        self
    }

    pub fn with_system_message(mut self, content: impl Into<String>) -> Self {
        self.messages.push(SessionItem::Message {
            message: Box::new(SessionMessage::system(content)),
        });
        self
    }

    pub fn with_parent_id(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_permissions(mut self, permissions: PermissionsConfig) -> Self {
        self.permissions = Some(permissions);
        self
    }

    pub fn is_sub_session(&self) -> bool {
        self.parent_id.is_some()
    }

    /// Set tools_approved flag
    pub fn set_tools_approved(&mut self, approved: bool) {
        self.tools_approved = approved;
    }

    /// Add a user message to the session
    pub fn add_user_message(&mut self, content: impl Into<String>) {
        self.messages.push(SessionItem::Message {
            message: Box::new(SessionMessage::user(content)),
        });
    }

    /// Add a message to the session
    pub fn add_message(&mut self, msg: SessionMessage) {
        self.messages.push(SessionItem::Message {
            message: Box::new(msg),
        });
    }

    /// Add a sub-session to the session
    pub fn add_sub_session(&mut self, sub_session: Session) {
        self.messages.push(SessionItem::SubSession {
            sub_session: Box::new(sub_session),
        });
    }

    /// Get all messages (flattening sub-sessions)
    pub fn get_all_messages(&self) -> Vec<SessionMessage> {
        let mut result = Vec::new();
        for item in &self.messages {
            match item {
                SessionItem::Message { message } => {
                    let message = message.as_ref();
                    if message.message.role != MessageRole::System {
                        result.push(message.clone());
                    }
                }
                SessionItem::SubSession { sub_session } => {
                    result.extend(sub_session.get_all_messages());
                }
                SessionItem::Summary { .. } => {}
            }
        }
        result
    }

    /// Get messages formatted for the model
    pub fn get_messages(&self, agent: &Agent) -> Vec<Message> {
        let mut messages = Vec::new();

        // Add invariant system messages
        messages.extend(self.build_invariant_system_messages(agent));

        // Add context-specific system messages
        messages.extend(self.build_context_system_messages(agent));

        // Find last summary index
        let last_summary_idx = self
            .messages
            .iter()
            .rposition(|item| matches!(item, SessionItem::Summary { .. }));

        // Add summary if exists
        if let Some(idx) = last_summary_idx {
            if let SessionItem::Summary { summary } = &self.messages[idx] {
                messages.push(Message::system(format!("Session Summary: {}", summary)));
            }
        }

        // Add conversation messages after the summary
        let start_idx = last_summary_idx.map(|i| i + 1).unwrap_or(0);
        for item in self.messages.iter().skip(start_idx) {
            if let SessionItem::Message { message } = item {
                messages.push(message.as_ref().message.clone());
            }
        }

        // Trim if needed
        if agent.num_history_items > 0 {
            messages = self.trim_messages(messages, agent.num_history_items);
        }

        messages
    }

    fn build_invariant_system_messages(&self, agent: &Agent) -> Vec<Message> {
        let mut messages = Vec::new();

        // Add sub-agents info if present
        if !agent.sub_agents.is_empty() {
            let mut text = String::from("You are a multi-agent system. Available sub-agents:\n");
            for sub in &agent.sub_agents {
                text.push_str(&format!(
                    "Name: {} | Description: {}\n",
                    sub.name,
                    sub.description.as_deref().unwrap_or("")
                ));
            }
            text.push_str("\nUse transfer_task to delegate tasks to appropriate sub-agents.");
            messages.push(Message::system(text));
        }

        // Add agent instructions
        if let Some(ref instructions) = agent.instruction {
            messages.push(Message::system(instructions.clone()));
        }

        // Add toolset instructions
        for toolset in &agent.toolsets {
            if let Some(instr) = toolset.instructions() {
                messages.push(Message::system(instr));
            }
        }

        // Mark last message for cache control
        if let Some(last) = messages.last_mut() {
            last.cache_control = true;
        }

        messages
    }

    fn build_context_system_messages(&self, agent: &Agent) -> Vec<Message> {
        let mut messages = Vec::new();

        if agent.add_date {
            messages.push(Message::system(format!(
                "Today's date: {}",
                chrono::Utc::now().format("%Y-%m-%d")
            )));
        }

        if agent.add_environment_info {
            if let Some(ref wd) = self.working_dir {
                messages.push(Message::system(format!("Working directory: {}", wd)));
            }
        }

        // Load additional prompt files
        if !agent.add_prompt_files.is_empty() {
            let base_dir = self.working_dir.as_deref().unwrap_or(".");
            for file_pattern in &agent.add_prompt_files {
                match load_prompt_files(base_dir, file_pattern) {
                    Ok(contents) => {
                        for content in contents {
                            if !content.trim().is_empty() {
                                messages.push(Message::system(content));
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load prompt file '{}': {}", file_pattern, e);
                    }
                }
            }
        }

        // Mark last message for cache control
        if let Some(last) = messages.last_mut() {
            last.cache_control = true;
        }

        messages
    }

    fn trim_messages(&self, messages: Vec<Message>, max_items: usize) -> Vec<Message> {
        let mut system_messages = Vec::new();
        let mut conversation_messages = Vec::new();

        for msg in messages {
            if msg.role == MessageRole::System {
                system_messages.push(msg);
            } else {
                conversation_messages.push(msg);
            }
        }

        if conversation_messages.len() <= max_items {
            let mut result = system_messages;
            result.extend(conversation_messages);
            return result;
        }

        // Keep only the most recent messages
        let to_remove = conversation_messages.len() - max_items;

        // Track tool calls to remove their responses
        let mut tool_calls_to_remove = std::collections::HashSet::new();
        for msg in conversation_messages.iter().take(to_remove) {
            for tc in &msg.tool_calls {
                tool_calls_to_remove.insert(tc.id.clone());
            }
        }

        let mut result = system_messages;
        for (i, msg) in conversation_messages.into_iter().enumerate() {
            if i < to_remove {
                continue;
            }
            if msg.role == MessageRole::Tool {
                if let Some(ref id) = msg.tool_call_id {
                    if tool_calls_to_remove.contains(id) {
                        continue;
                    }
                }
            }
            result.push(msg);
        }

        result
    }

    /// Get the last assistant message content
    pub fn get_last_assistant_content(&self) -> Option<String> {
        for item in self.messages.iter().rev() {
            if let SessionItem::Message { message } = item {
                let message = message.as_ref();
                if message.message.role == MessageRole::Assistant {
                    return Some(message.message.content.clone());
                }
            }
        }
        None
    }

    /// Undo the last user message exchange (removes user message and all responses)
    pub fn undo_last_exchange(&mut self) -> usize {
        // Find the last user message
        let mut last_user_idx = None;
        for (idx, item) in self.messages.iter().enumerate().rev() {
            if let SessionItem::Message { message } = item {
                let message = message.as_ref();
                if message.message.role == MessageRole::User {
                    last_user_idx = Some(idx);
                    break;
                }
            }
        }

        if let Some(idx) = last_user_idx {
            let removed = self.messages.len() - idx;
            self.messages.truncate(idx);
            removed
        } else {
            0
        }
    }

    /// Returns the number of `SessionItem::Message` items currently stored.
    pub fn message_item_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|it| matches!(it, SessionItem::Message { .. }))
            .count()
    }

    /// Compact the session history by inserting/updating a `SessionItem::Summary` and keeping only
    /// the last `keep_last_messages` message items since the last summary.
    ///
    /// This function only manipulates the stored session items; it does *not* generate the summary.
    ///
    /// Returns `(removed_items, kept_message_items)`.
    pub fn compact_with_summary(
        &mut self,
        summary: impl Into<String>,
        keep_last_messages: usize,
    ) -> (usize, usize) {
        let summary = summary.into();

        // Find last summary index so we only compact the "tail" of the session.
        let last_summary_idx = self
            .messages
            .iter()
            .rposition(|item| matches!(item, SessionItem::Summary { .. }));

        let start_idx = last_summary_idx.map(|i| i + 1).unwrap_or(0);

        // Collect indexes of message items in the compactable range.
        let message_item_indexes: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .skip(start_idx)
            .filter_map(|(i, it)| matches!(it, SessionItem::Message { .. }).then_some(i))
            .collect();

        if keep_last_messages == 0 || message_item_indexes.len() <= keep_last_messages {
            // Nothing to do.
            // Still update existing summary if present and the caller provided new non-empty content.
            if !summary.trim().is_empty() {
                if let Some(idx) = last_summary_idx {
                    if let Some(SessionItem::Summary { summary: existing }) =
                        self.messages.get_mut(idx)
                    {
                        if existing.trim().is_empty() {
                            *existing = summary;
                        } else {
                            existing.push_str("\n\n");
                            existing.push_str(&summary);
                        }
                    }
                }
            }
            return (0, message_item_indexes.len());
        }

        // Determine where to cut the compactable range: keep the last N message items.
        let keep_from_message_idx = message_item_indexes.len() - keep_last_messages;
        let keep_from_item_idx = message_item_indexes[keep_from_message_idx];

        // Build the kept tail (includes non-message items that happen to be after the cut).
        let kept_tail: Vec<SessionItem> = self.messages.drain(keep_from_item_idx..).collect();

        // Remove everything after the last summary (or from start) up to the cut.
        // At this point `self.messages` ends at `keep_from_item_idx`.
        let removed_items = self.messages.len().saturating_sub(start_idx);
        self.messages.truncate(start_idx);

        // Insert or update summary.
        match last_summary_idx {
            Some(idx) => {
                // There is already a summary entry at `idx`. Update it (merge).
                if let Some(SessionItem::Summary { summary: existing }) = self.messages.get_mut(idx)
                {
                    if existing.trim().is_empty() {
                        *existing = summary;
                    } else {
                        existing.push_str("\n\n");
                        existing.push_str(&summary);
                    }
                }
            }
            None => {
                self.messages.push(SessionItem::Summary { summary });
            }
        }

        // Append the kept tail.
        self.messages.extend(kept_tail);

        (removed_items, keep_last_messages)
    }
}

/// Session store trait
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    async fn save(&self, session: &Session) -> anyhow::Result<()>;
    async fn get(&self, id: &str) -> anyhow::Result<Option<Session>>;
    async fn list(&self) -> anyhow::Result<Vec<Session>>;
    async fn delete(&self, id: &str) -> anyhow::Result<()>;

    async fn get_last(&self) -> anyhow::Result<Option<Session>> {
        Ok(None)
    }
}

/// In-memory session store
pub struct InMemorySessionStore {
    sessions: RwLock<HashMap<String, Session>>,
    last_id: RwLock<Option<String>>,
}

impl Default for InMemorySessionStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            last_id: RwLock::new(None),
        }
    }
}

#[async_trait::async_trait]
impl SessionStore for InMemorySessionStore {
    async fn save(&self, session: &Session) -> anyhow::Result<()> {
        self.sessions
            .write()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        *self.last_id.write().unwrap() = Some(session.id.clone());
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<Session>> {
        Ok(self.sessions.read().unwrap().get(id).cloned())
    }

    async fn list(&self) -> anyhow::Result<Vec<Session>> {
        Ok(self.sessions.read().unwrap().values().cloned().collect())
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        self.sessions.write().unwrap().remove(id);
        if self.last_id.read().unwrap().as_deref() == Some(id) {
            *self.last_id.write().unwrap() = None;
        }
        Ok(())
    }

    async fn get_last(&self) -> anyhow::Result<Option<Session>> {
        let Some(id) = self.last_id.read().unwrap().clone() else {
            return Ok(None);
        };
        self.get(&id).await
    }
}

// ============================================================================
// File-backed store
// ============================================================================

pub struct FileSessionStore {
    dir: PathBuf,
}

impl FileSessionStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn default_dir() -> PathBuf {
        // Keep parity with the Go version: ~/.cagent
        let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join(".cagent").join("sessions")
    }

    pub fn new_default() -> Self {
        Self::new(Self::default_dir())
    }

    fn session_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{}.json", id))
    }

    fn last_path(&self) -> PathBuf {
        self.dir.join("last_session")
    }

    async fn ensure_dir(&self) -> anyhow::Result<()> {
        tokio::fs::create_dir_all(&self.dir).await?;
        Ok(())
    }

    async fn write_atomic(&self, path: &Path, contents: &[u8]) -> anyhow::Result<()> {
        let tmp = path.with_extension("tmp");
        tokio::fs::write(&tmp, contents).await?;
        tokio::fs::rename(&tmp, path).await?;
        Ok(())
    }

    async fn write_last_id(&self, id: &str) -> anyhow::Result<()> {
        self.ensure_dir().await?;
        self.write_atomic(self.last_path().as_path(), id.as_bytes())
            .await
    }

    async fn read_last_id(&self) -> anyhow::Result<Option<String>> {
        let path = self.last_path();
        let Ok(bytes) = tokio::fs::read(&path).await else {
            return Ok(None);
        };

        let id = String::from_utf8_lossy(&bytes).trim().to_string();
        if id.is_empty() {
            return Ok(None);
        }
        Ok(Some(id))
    }
}

#[async_trait::async_trait]
impl SessionStore for FileSessionStore {
    async fn save(&self, session: &Session) -> anyhow::Result<()> {
        self.ensure_dir().await?;

        let json = serde_json::to_vec_pretty(session)?;
        let path = self.session_path(&session.id);
        self.write_atomic(path.as_path(), &json).await?;

        // Update last session pointer
        self.write_last_id(&session.id).await?;
        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<Session>> {
        let path = self.session_path(id);
        let Ok(bytes) = tokio::fs::read(&path).await else {
            return Ok(None);
        };

        let session: Session = serde_json::from_slice(&bytes)?;
        Ok(Some(session))
    }

    async fn list(&self) -> anyhow::Result<Vec<Session>> {
        let mut sessions = Vec::new();
        let Ok(mut entries) = tokio::fs::read_dir(&self.dir).await else {
            return Ok(Vec::new());
        };

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Ok(bytes) = tokio::fs::read(&path).await {
                if let Ok(session) = serde_json::from_slice::<Session>(&bytes) {
                    sessions.push(session);
                }
            }
        }

        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sessions)
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let path = self.session_path(id);
        let _ = tokio::fs::remove_file(&path).await;

        if self.read_last_id().await?.as_deref() == Some(id) {
            let _ = tokio::fs::remove_file(self.last_path()).await;
        }
        Ok(())
    }

    async fn get_last(&self) -> anyhow::Result<Option<Session>> {
        let Some(id) = self.read_last_id().await? else {
            return Ok(None);
        };
        self.get(&id).await
    }
}

#[cfg(test)]
mod file_store_tests {
    use super::*;

    #[tokio::test]
    async fn file_store_roundtrip_and_last_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileSessionStore::new(tmp.path());

        let mut s = Session::new();
        s.title = "hello".to_string();
        store.save(&s).await.unwrap();

        let got = store.get(&s.id).await.unwrap().unwrap();
        assert_eq!(got.id, s.id);
        assert_eq!(got.title, "hello");

        let last = store.get_last().await.unwrap().unwrap();
        assert_eq!(last.id, s.id);

        let list = store.list().await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, s.id);
    }
}

// ============================================================================
// Prompt File Loading
// ============================================================================

/// Load prompt files from a base directory with glob pattern support.
/// Returns the contents of all matched files.
fn load_prompt_files(base_dir: &str, pattern: &str) -> anyhow::Result<Vec<String>> {
    let base = Path::new(base_dir);
    let full_pattern = base.join(pattern);
    let pattern_str = full_pattern.to_string_lossy();

    let mut results = Vec::new();

    // Check if it's a glob pattern
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        // Use glob to expand the pattern
        for entry in glob::glob(&pattern_str)? {
            match entry {
                Ok(path) => {
                    if path.is_file() {
                        let content = std::fs::read_to_string(&path)?;
                        results.push(content);
                    }
                }
                Err(e) => {
                    tracing::warn!("Glob pattern error: {}", e);
                }
            }
        }
    } else {
        // Direct file path
        let path = base.join(pattern);
        if path.is_file() {
            let content = std::fs::read_to_string(&path)?;
            results.push(content);
        } else if path.is_dir() {
            // Load all markdown/text files in directory
            for entry in std::fs::read_dir(&path)? {
                let entry = entry?;
                let entry_path = entry.path();
                if entry_path.is_file() {
                    if let Some(ext) = entry_path.extension() {
                        let ext = ext.to_string_lossy().to_lowercase();
                        if ext == "md" || ext == "txt" || ext == "prompt" {
                            let content = std::fs::read_to_string(&entry_path)?;
                            results.push(content);
                        }
                    }
                }
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod prompt_file_tests {
    use super::*;

    #[test]
    fn test_load_single_file() {
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("prompt.md");
        std::fs::write(&file_path, "Test prompt content").unwrap();

        let results = load_prompt_files(tmp.path().to_str().unwrap(), "prompt.md").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "Test prompt content");
    }

    #[test]
    fn test_load_glob_pattern() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.md"), "Content A").unwrap();
        std::fs::write(tmp.path().join("b.md"), "Content B").unwrap();
        std::fs::write(tmp.path().join("c.txt"), "Content C").unwrap();

        let results = load_prompt_files(tmp.path().to_str().unwrap(), "*.md").unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|c| c == "Content A"));
        assert!(results.iter().any(|c| c == "Content B"));
    }

    #[test]
    fn test_load_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let prompts_dir = tmp.path().join("prompts");
        std::fs::create_dir(&prompts_dir).unwrap();
        std::fs::write(prompts_dir.join("a.md"), "A").unwrap();
        std::fs::write(prompts_dir.join("b.txt"), "B").unwrap();
        std::fs::write(prompts_dir.join("c.json"), "C").unwrap(); // Should be skipped

        let results = load_prompt_files(tmp.path().to_str().unwrap(), "prompts").unwrap();
        assert_eq!(results.len(), 2);
    }
}

// ============================================================================
// SQLite Session Store
// ============================================================================

use rusqlite::{params, Connection};
use std::sync::Mutex;

/// SQLite-backed session store
pub struct SqliteSessionStore {
    conn: Mutex<Connection>,
}

impl SqliteSessionStore {
    /// Create a new SQLite session store at the given path
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let conn = Connection::open(path.as_ref())?;
        
        // Create tables
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT '',
                tools_approved INTEGER NOT NULL DEFAULT 0,
                thinking INTEGER NOT NULL DEFAULT 0,
                hide_tool_results INTEGER NOT NULL DEFAULT 0,
                working_dir TEXT,
                send_user_message INTEGER NOT NULL DEFAULT 1,
                max_iterations INTEGER NOT NULL DEFAULT 0,
                starred INTEGER NOT NULL DEFAULT 0,
                input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0.0,
                permissions TEXT,
                parent_id TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS session_items (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                position INTEGER NOT NULL,
                item_type TEXT NOT NULL,
                agent_name TEXT,
                message_json TEXT,
                implicit INTEGER NOT NULL DEFAULT 0,
                subsession_id TEXT,
                summary_text TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_session_items_session_id ON session_items(session_id);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent_id ON sessions(parent_id);
            "#,
        )?;

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Create a new SQLite session store at the default path (~/.cagent/sessions.db)
    pub fn new_default() -> anyhow::Result<Self> {
        let base = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let dir = base.join(".cagent");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("sessions.db");
        Self::new(path)
    }

    fn save_session_items(&self, conn: &Connection, session: &Session) -> anyhow::Result<()> {
        // Clear existing items for this session
        conn.execute(
            "DELETE FROM session_items WHERE session_id = ?",
            params![&session.id],
        )?;

        // Insert new items
        for (position, item) in session.messages.iter().enumerate() {
            match item {
                SessionItem::Message { message } => {
                    let message_json = serde_json::to_string(&message.message)?;
                    conn.execute(
                        "INSERT INTO session_items (session_id, position, item_type, agent_name, message_json, implicit) VALUES (?, ?, 'message', ?, ?, ?)",
                        params![
                            &session.id,
                            position as i64,
                            &message.agent_name,
                            message_json,
                            message.implicit
                        ],
                    )?;
                }
                SessionItem::SubSession { sub_session } => {
                    conn.execute(
                        "INSERT INTO session_items (session_id, position, item_type, subsession_id) VALUES (?, ?, 'subsession', ?)",
                        params![&session.id, position as i64, &sub_session.id],
                    )?;
                }
                SessionItem::Summary { summary } => {
                    conn.execute(
                        "INSERT INTO session_items (session_id, position, item_type, summary_text) VALUES (?, ?, 'summary', ?)",
                        params![&session.id, position as i64, summary],
                    )?;
                }
            }
        }

        Ok(())
    }

    fn load_session_items(&self, conn: &Connection, session_id: &str) -> anyhow::Result<Vec<SessionItem>> {
        let mut stmt = conn.prepare(
            "SELECT item_type, agent_name, message_json, implicit, subsession_id, summary_text FROM session_items WHERE session_id = ? ORDER BY position",
        )?;

        let rows = stmt.query_map(params![session_id], |row| {
            let item_type: String = row.get(0)?;
            let agent_name: Option<String> = row.get(1)?;
            let message_json: Option<String> = row.get(2)?;
            let implicit: bool = row.get(3)?;
            let subsession_id: Option<String> = row.get(4)?;
            let summary_text: Option<String> = row.get(5)?;
            Ok((item_type, agent_name, message_json, implicit, subsession_id, summary_text))
        })?;

        let mut items = Vec::new();
        for row in rows {
            let (item_type, agent_name, message_json, implicit, subsession_id, summary_text) = row?;

            match item_type.as_str() {
                "message" => {
                    if let Some(json) = message_json {
                        let message: Message = serde_json::from_str(&json)?;
                        items.push(SessionItem::Message {
                            message: Box::new(SessionMessage {
                                agent_name,
                                message,
                                implicit,
                            }),
                        });
                    }
                }
                "subsession" => {
                    if let Some(sub_id) = subsession_id {
                        // Recursively load sub-session
                        if let Some(sub_session) = self.load_session_sync(conn, &sub_id)? {
                            items.push(SessionItem::SubSession {
                                sub_session: Box::new(sub_session),
                            });
                        }
                    }
                }
                "summary" => {
                    if let Some(summary) = summary_text {
                        items.push(SessionItem::Summary { summary });
                    }
                }
                _ => {}
            }
        }

        Ok(items)
    }

    fn load_session_sync(&self, conn: &Connection, id: &str) -> anyhow::Result<Option<Session>> {
        let mut stmt = conn.prepare(
            "SELECT id, title, tools_approved, thinking, hide_tool_results, working_dir, send_user_message, max_iterations, starred, input_tokens, output_tokens, cost, permissions, parent_id, created_at FROM sessions WHERE id = ?",
        )?;

        let mut rows = stmt.query(params![id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        let permissions_json: Option<String> = row.get(12)?;
        let permissions: Option<PermissionsConfig> = permissions_json
            .and_then(|json| serde_json::from_str(&json).ok());

        let created_at_str: String = row.get(14)?;
        let created_at = chrono::DateTime::parse_from_rfc3339(&created_at_str)
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());

        let mut session = Session {
            id: row.get(0)?,
            title: row.get(1)?,
            tools_approved: row.get(2)?,
            thinking: row.get(3)?,
            hide_tool_results: row.get(4)?,
            working_dir: row.get(5)?,
            send_user_message: row.get(6)?,
            max_iterations: row.get::<_, i64>(7)? as usize,
            starred: row.get(8)?,
            input_tokens: row.get(9)?,
            output_tokens: row.get(10)?,
            cost: row.get(11)?,
            permissions,
            parent_id: row.get(13)?,
            created_at,
            messages: Vec::new(),
        };

        // Load messages
        session.messages = self.load_session_items(conn, &session.id)?;

        Ok(Some(session))
    }
}

#[async_trait::async_trait]
impl SessionStore for SqliteSessionStore {
    async fn save(&self, session: &Session) -> anyhow::Result<()> {
        // Collect sub-sessions first
        let sub_sessions: Vec<Session> = session
            .messages
            .iter()
            .filter_map(|item| {
                if let SessionItem::SubSession { sub_session } = item {
                    Some(sub_session.as_ref().clone())
                } else {
                    None
                }
            })
            .collect();

        // Save main session synchronously
        {
            let conn = self.conn.lock().unwrap();

            // Serialize permissions
            let permissions_json = session
                .permissions
                .as_ref()
                .map(|p| serde_json::to_string(p))
                .transpose()?;

            let created_at_str = session.created_at.to_rfc3339();

            // Upsert session
            conn.execute(
                r#"
                INSERT INTO sessions (id, title, tools_approved, thinking, hide_tool_results, working_dir, send_user_message, max_iterations, starred, input_tokens, output_tokens, cost, permissions, parent_id, created_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET
                    title = excluded.title,
                    tools_approved = excluded.tools_approved,
                    thinking = excluded.thinking,
                    hide_tool_results = excluded.hide_tool_results,
                    working_dir = excluded.working_dir,
                    send_user_message = excluded.send_user_message,
                    max_iterations = excluded.max_iterations,
                    starred = excluded.starred,
                    input_tokens = excluded.input_tokens,
                    output_tokens = excluded.output_tokens,
                    cost = excluded.cost,
                    permissions = excluded.permissions,
                    parent_id = excluded.parent_id
                "#,
                params![
                    &session.id,
                    &session.title,
                    session.tools_approved,
                    session.thinking,
                    session.hide_tool_results,
                    &session.working_dir,
                    session.send_user_message,
                    session.max_iterations as i64,
                    session.starred,
                    session.input_tokens,
                    session.output_tokens,
                    session.cost,
                    &permissions_json,
                    &session.parent_id,
                    &created_at_str,
                ],
            )?;

            // Save session items
            self.save_session_items(&conn, session)?;
        } // Release lock before recursive calls

        // Save sub-sessions recursively
        for sub in sub_sessions {
            Box::pin(self.save(&sub)).await?;
        }

        Ok(())
    }

    async fn get(&self, id: &str) -> anyhow::Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();
        self.load_session_sync(&conn, id)
    }

    async fn list(&self) -> anyhow::Result<Vec<Session>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id FROM sessions WHERE parent_id IS NULL OR parent_id = '' ORDER BY created_at DESC",
        )?;

        let ids: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        let mut sessions = Vec::new();
        for id in ids {
            if let Some(session) = self.load_session_sync(&conn, &id)? {
                sessions.push(session);
            }
        }

        Ok(sessions)
    }

    async fn delete(&self, id: &str) -> anyhow::Result<()> {
        let conn = self.conn.lock().unwrap();

        // Delete session items (CASCADE should handle this, but be explicit)
        conn.execute("DELETE FROM session_items WHERE session_id = ?", params![id])?;

        // Delete the session
        conn.execute("DELETE FROM sessions WHERE id = ?", params![id])?;

        Ok(())
    }

    async fn get_last(&self) -> anyhow::Result<Option<Session>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare(
            "SELECT id FROM sessions WHERE parent_id IS NULL OR parent_id = '' ORDER BY created_at DESC LIMIT 1",
        )?;

        let mut rows = stmt.query([])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };

        let id: String = row.get(0)?;
        self.load_session_sync(&conn, &id)
    }
}

#[cfg(test)]
mod sqlite_store_tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_store_roundtrip() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = SqliteSessionStore::new(tmp.path()).unwrap();

        let mut session = Session::new();
        session.title = "Test Session".to_string();
        session.add_message(SessionMessage::user("Hello"));

        // Save
        store.save(&session).await.unwrap();

        // Get
        let loaded = store.get(&session.id).await.unwrap().unwrap();
        assert_eq!(loaded.title, "Test Session");
        assert_eq!(loaded.messages.len(), 1);
        if let SessionItem::Message { message } = &loaded.messages[0] {
            assert_eq!(message.message.content, "Hello");
        } else {
            panic!("Expected message item");
        }

        // List
        let sessions = store.list().await.unwrap();
        assert_eq!(sessions.len(), 1);

        // Get last
        let last = store.get_last().await.unwrap().unwrap();
        assert_eq!(last.id, session.id);

        // Delete
        store.delete(&session.id).await.unwrap();
        let deleted = store.get(&session.id).await.unwrap();
        assert!(deleted.is_none());
    }

    #[tokio::test]
    async fn sqlite_store_with_summary() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let store = SqliteSessionStore::new(tmp.path()).unwrap();

        let mut session = Session::new();
        session.messages.push(SessionItem::Summary {
            summary: "This is a summary".to_string(),
        });
        session.add_message(SessionMessage::user("Hello"));

        store.save(&session).await.unwrap();

        let loaded = store.get(&session.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
        if let SessionItem::Summary { summary } = &loaded.messages[0] {
            assert_eq!(summary, "This is a summary");
        } else {
            panic!("Expected summary item");
        }
    }
}
