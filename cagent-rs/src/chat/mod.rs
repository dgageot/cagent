//! Chat types for message handling

use crate::tools::ToolCall;
use serde::{Deserialize, Serialize};

/// Message role in a conversation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

/// A single message in a conversation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: MessageRole,
    pub content: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,

    #[serde(default)]
    pub cache_control: bool,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::System,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            usage: None,
            model: None,
            cost: None,
            cache_control: false,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::User,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            usage: None,
            model: None,
            cost: None,
            cache_control: false,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Assistant,
            content: content.into(),
            name: None,
            tool_call_id: None,
            tool_calls: Vec::new(),
            reasoning_content: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            usage: None,
            model: None,
            cost: None,
            cache_control: false,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: MessageRole::Tool,
            content: content.into(),
            name: None,
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: Vec::new(),
            reasoning_content: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            usage: None,
            model: None,
            cost: None,
            cache_control: false,
        }
    }

    pub fn with_tool_calls(mut self, tool_calls: Vec<ToolCall>) -> Self {
        self.tool_calls = tool_calls;
        self
    }
}

/// Token usage statistics
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Usage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    #[serde(default)]
    pub cached_input_tokens: i64,
    #[serde(default)]
    pub cache_write_tokens: i64,
}

/// Rate limit information
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RateLimit {
    /// Maximum requests per time window
    pub limit: i64,
    /// Remaining requests in current window
    pub remaining: i64,
    /// Time until rate limit resets (seconds since epoch or duration)
    pub reset: i64,
    /// Seconds to wait before retry (when rate limited)
    pub retry_after: Option<i64>,
}

/// Finish reason for a completion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    #[serde(other)]
    Unknown,
}

/// A choice from a chat completion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Choice {
    pub index: usize,
    pub delta: ChoiceDelta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
}

/// Delta content in a streaming choice
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChoiceDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<MessageRole>,

    #[serde(default)]
    pub content: String,

    #[serde(default)]
    pub reasoning_content: String,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tool_calls: Vec<ToolCall>,
}

/// A streamed chat completion response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamResponse {
    pub id: String,
    pub model: String,
    pub choices: Vec<Choice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limit: Option<RateLimit>,
}
