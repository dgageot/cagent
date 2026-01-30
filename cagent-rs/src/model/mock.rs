//! Mock model provider for testing.
//!
//! Provides a configurable mock provider that can be used in tests
//! without making actual API calls.

use async_trait::async_trait;
use futures::stream::{self, BoxStream};

use crate::chat::{Choice, ChoiceDelta, FinishReason, Message, StreamResponse, Usage};
use crate::model::{MessageStream, Provider, ProviderError};
use crate::tools::{FunctionCall, Tool, ToolCall, ToolType};

/// A mock provider for testing purposes.
///
/// # Example
///
/// ```ignore
/// use cagent::model::mock::MockProvider;
///
/// let provider = MockProvider::new()
///     .with_response("Hello, how can I help you?");
/// ```
pub struct MockProvider {
    name: String,
    responses: Vec<String>,
    tool_calls: Vec<ToolCall>,
    usage: Usage,
    response_index: std::sync::atomic::AtomicUsize,
}

impl MockProvider {
    /// Create a new mock provider with no configured responses.
    pub fn new() -> Self {
        Self {
            name: "mock/model".to_string(),
            responses: Vec::new(),
            tool_calls: Vec::new(),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 20,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
            },
            response_index: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Set a custom name for this mock provider.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Add a response that will be returned for the next call.
    /// Multiple responses can be added; they will be returned in order.
    pub fn with_response(mut self, response: impl Into<String>) -> Self {
        self.responses.push(response.into());
        self
    }

    /// Add multiple responses at once.
    pub fn with_responses(mut self, responses: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.responses.extend(responses.into_iter().map(|s| s.into()));
        self
    }

    /// Add a tool call that will be included in the response.
    pub fn with_tool_call(mut self, tool_call: ToolCall) -> Self {
        self.tool_calls.push(tool_call);
        self
    }

    /// Configure the usage statistics returned.
    pub fn with_usage(mut self, input_tokens: i64, output_tokens: i64) -> Self {
        self.usage = Usage {
            input_tokens,
            output_tokens,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
        };
        self
    }

    fn get_next_response(&self) -> String {
        if self.responses.is_empty() {
            return "Mock response".to_string();
        }
        let index = self.response_index.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.responses
            .get(index % self.responses.len())
            .cloned()
            .unwrap_or_else(|| "Mock response".to_string())
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Provider for MockProvider {
    fn id(&self) -> String {
        self.name.clone()
    }

    async fn create_chat_completion_stream(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        let response_text = self.get_next_response();
        let tool_calls = self.tool_calls.clone();
        let usage = self.usage.clone();

        // Create a stream that emits chunks of the response
        let chunks: Vec<StreamResponse> = vec![
            // First chunk: content
            StreamResponse {
                id: "mock-response-id".to_string(),
                model: "mock-model".to_string(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta {
                        role: None,
                        content: response_text.clone(),
                        reasoning_content: String::new(),
                        tool_calls: tool_calls.clone(),
                    },
                    finish_reason: None,
                }],
                usage: None,
                rate_limit: None,
            },
            // Second chunk: finish
            StreamResponse {
                id: "mock-response-id".to_string(),
                model: "mock-model".to_string(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta {
                        role: None,
                        content: String::new(),
                        reasoning_content: String::new(),
                        tool_calls: Vec::new(),
                    },
                    finish_reason: Some(FinishReason::Stop),
                }],
                usage: Some(usage),
                rate_limit: None,
            },
        ];

        let stream = stream::iter(chunks.into_iter().map(Ok));
        Ok(Box::pin(stream) as BoxStream<'static, Result<StreamResponse, ProviderError>>)
    }
}

impl std::fmt::Debug for MockProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockProvider")
            .field("name", &self.name)
            .field("responses", &self.responses.len())
            .finish()
    }
}

/// Create a tool call for use with MockProvider.
pub fn mock_tool_call(name: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: format!("call_{}", uuid::Uuid::new_v4()),
        call_type: ToolType::Function,
        function: FunctionCall {
            name: name.to_string(),
            arguments: arguments.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn test_mock_provider_basic() {
        let provider = MockProvider::new()
            .with_response("Hello, world!");

        let messages = vec![Message::user("Hi")];
        let stream = provider
            .create_chat_completion_stream(messages, vec![])
            .await
            .unwrap();

        let responses: Vec<_> = stream.collect().await;
        assert_eq!(responses.len(), 2);

        // First chunk has content
        let first = responses[0].as_ref().unwrap();
        assert_eq!(first.choices[0].delta.content, "Hello, world!");

        // Second chunk has finish reason
        let second = responses[1].as_ref().unwrap();
        assert_eq!(second.choices[0].finish_reason, Some(FinishReason::Stop));
    }

    #[tokio::test]
    async fn test_mock_provider_multiple_responses() {
        let provider = MockProvider::new()
            .with_responses(["First", "Second", "Third"]);

        // First call
        let stream = provider
            .create_chat_completion_stream(vec![Message::user("1")], vec![])
            .await
            .unwrap();
        let responses: Vec<_> = stream.collect().await;
        assert_eq!(responses[0].as_ref().unwrap().choices[0].delta.content, "First");

        // Second call
        let stream = provider
            .create_chat_completion_stream(vec![Message::user("2")], vec![])
            .await
            .unwrap();
        let responses: Vec<_> = stream.collect().await;
        assert_eq!(responses[0].as_ref().unwrap().choices[0].delta.content, "Second");
    }

    #[tokio::test]
    async fn test_mock_provider_with_tool_call() {
        let tool_call = mock_tool_call("test_tool", r#"{"arg": "value"}"#);
        let provider = MockProvider::new()
            .with_response("Calling tool...")
            .with_tool_call(tool_call);

        let stream = provider
            .create_chat_completion_stream(vec![Message::user("Do something")], vec![])
            .await
            .unwrap();

        let responses: Vec<_> = stream.collect().await;
        let first = responses[0].as_ref().unwrap();

        assert!(!first.choices[0].delta.tool_calls.is_empty());
        let tool_calls = &first.choices[0].delta.tool_calls;
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].function.name, "test_tool");
    }
}
