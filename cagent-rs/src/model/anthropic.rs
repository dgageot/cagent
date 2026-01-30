//! Anthropic provider implementation

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, error};

use crate::chat::{Choice, ChoiceDelta, FinishReason, Message, MessageRole, StreamResponse, Usage};
use crate::gateway::{self, GatewayConfig};
use crate::tools::{FunctionCall, Tool, ToolCall, ToolType};

use super::{MessageStream, Provider, ProviderError, ProviderOptions};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Base URL for forwarding headers (without endpoint path)
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Anthropic provider
#[derive(Debug)]
pub struct AnthropicProvider {
    api_key: String,
    model: String,
    options: ProviderOptions,
    client: Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String, model: String, options: ProviderOptions) -> Self {
        // Create client with timeout settings
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout for long responses
            .build()
            .unwrap_or_else(|_| Client::new());
        
        Self {
            api_key,
            model,
            options,
            client,
        }
    }

    fn build_request_body(&self, messages: &[Message], tools: &[Tool]) -> serde_json::Value {
        // Separate system messages from conversation messages
        let system_content: String = messages
            .iter()
            .filter(|m| m.role == MessageRole::System)
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n\n");

        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .filter(|m| m.role != MessageRole::System)
            .map(|m| {
                match m.role {
                    MessageRole::User => json!({
                        "role": "user",
                        "content": m.content,
                    }),
                    MessageRole::Assistant => {
                        if m.tool_calls.is_empty() {
                            json!({
                                "role": "assistant",
                                "content": m.content,
                            })
                        } else {
                            // Assistant message with tool use
                            let mut content: Vec<serde_json::Value> = Vec::new();
                            if !m.content.is_empty() {
                                content.push(json!({
                                    "type": "text",
                                    "text": m.content
                                }));
                            }
                            for tc in &m.tool_calls {
                                content.push(json!({
                                    "type": "tool_use",
                                    "id": tc.id,
                                    "name": tc.function.name,
                                    "input": serde_json::from_str::<serde_json::Value>(&tc.function.arguments).unwrap_or(json!({}))
                                }));
                            }
                            json!({
                                "role": "assistant",
                                "content": content
                            })
                        }
                    }
                    MessageRole::Tool => json!({
                        "role": "user",
                        "content": [{
                            "type": "tool_result",
                            "tool_use_id": m.tool_call_id,
                            "content": m.content
                        }]
                    }),
                    _ => json!({
                        "role": "user",
                        "content": m.content,
                    }),
                }
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "messages": messages_json,
            "stream": true,
            "max_tokens": self.options.max_tokens.unwrap_or(8192),
        });

        if !system_content.is_empty() {
            body["system"] = json!(system_content);
        }

        if let Some(temp) = self.options.temperature {
            body["temperature"] = json!(temp);
        }

        // Enable extended thinking if budget is set
        // https://docs.anthropic.com/en/docs/build-with-claude/extended-thinking
        if let Some(budget) = self.options.thinking_budget {
            if budget > 0 {
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": budget
                });
            }
        }

        if !tools.is_empty() {
            let tools_json: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.parameters
                    })
                })
                .collect();
            body["tools"] = json!(tools_json);
        }

        body
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum AnthropicEvent {
    #[serde(rename = "message_start")]
    MessageStart { message: AnthropicMessage },

    #[serde(rename = "content_block_start")]
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },

    #[serde(rename = "content_block_delta")]
    ContentBlockDelta { index: usize, delta: ContentDelta },

    #[serde(rename = "content_block_stop")]
    ContentBlockStop { index: usize },

    #[serde(rename = "message_delta")]
    MessageDelta {
        delta: MessageDeltaContent,
        usage: Option<DeltaUsage>,
    },

    #[serde(rename = "message_stop")]
    MessageStop,

    #[serde(rename = "ping")]
    Ping,

    #[serde(rename = "error")]
    Error { error: AnthropicError },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessage {
    id: String,
    model: String,
    usage: Option<AnthropicUsage>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: i64,
    output_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct DeltaUsage {
    output_tokens: i64,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },

    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum ContentDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },

    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },

    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
}

#[derive(Debug, Deserialize)]
struct MessageDeltaContent {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    message: String,
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn id(&self) -> String {
        format!("anthropic/{}", self.model)
    }

    async fn create_chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        let body = self.build_request_body(&messages, &tools);

        debug!(
            "Anthropic request: {}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );

        // Check if gateway is configured
        let (url, auth_token) = if let Some(gateway_config) = gateway::get_gateway_config() {
            // Use gateway with Docker Desktop token
            let token = GatewayConfig::get_auth_token().ok_or_else(|| {
                ProviderError::MissingApiKey(
                    "Docker Desktop token required for gateway (DOCKER_TOKEN)".to_string(),
                )
            })?;
            let gateway_url = gateway_config.get_url("/v1/messages");
            debug!("Using gateway: {}", gateway_url);
            (gateway_url, token)
        } else {
            // Use direct API
            (ANTHROPIC_API_URL.to_string(), self.api_key.clone())
        };

        let mut request = self
            .client
            .post(&url)
            .header("x-api-key", &auth_token)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header("Cache-Control", "no-cache");

        // Add Authorization header when using gateway (needed for gateway auth)
        if gateway::is_gateway_configured() {
            request = request.header("Authorization", format!("Bearer {}", auth_token));
        }

        // Add gateway headers if using gateway
        if gateway::is_gateway_configured() {
            for (key, value) in
                gateway::create_gateway_headers("anthropic", &self.model, ANTHROPIC_BASE_URL)
            {
                request = request.header(&key, &value);
            }
            debug!(
                "Gateway headers added: provider=anthropic, model={}",
                self.model
            );
        }

        let request = request.json(&body);

        let es = EventSource::new(request).map_err(|e| ProviderError::Stream(e.to_string()))?;

        let model = self.model.clone();

        let state = std::sync::Arc::new(std::sync::Mutex::new(StreamState {
            message_id: String::new(),
            model: model.clone(),
            current_tool_id: None,
            current_tool_name: None,
            input_tokens: 0,
            output_tokens: 0,
        }));

        // Use async_stream to properly handle the EventSource
        let stream = async_stream::stream! {
            let mut es = es;
            
            while let Some(event) = es.next().await {
                debug!("EventSource event: {:?}", event);
                
                match event {
                    Ok(Event::Open) => {
                        debug!("EventSource connection opened");
                        continue;
                    }
                    Ok(Event::Message(msg)) => {
                        if msg.event == "ping" {
                            continue;
                        }

                        match serde_json::from_str::<AnthropicEvent>(&msg.data) {
                            Ok(evt) => {
                                if let Some(response) = process_anthropic_event(evt, &state) {
                                    yield response;
                                }
                            }
                            Err(e) => {
                                error!("Failed to parse Anthropic event: {} - {}", e, msg.data);
                                yield Err(ProviderError::Json(e));
                            }
                        }
                    }
                    Err(reqwest_eventsource::Error::StreamEnded) => {
                        debug!("EventSource stream ended normally");
                        break;
                    }
                    Err(e) => {
                        error!("EventSource error: {:?}", e);
                        yield Err(ProviderError::Stream(e.to_string()));
                        break;
                    }
                }
            }
            
            debug!("Anthropic stream finished");
        };

        Ok(Box::pin(stream))
    }
}

// StreamState for tracking state across SSE events
struct StreamState {
    message_id: String,
    model: String,
    current_tool_id: Option<String>,
    current_tool_name: Option<String>,
    input_tokens: i64,
    output_tokens: i64,
}

fn process_anthropic_event(
    evt: AnthropicEvent,
    state: &std::sync::Arc<std::sync::Mutex<StreamState>>,
) -> Option<Result<StreamResponse, ProviderError>> {
    let mut state = state.lock().unwrap();

    match evt {
        AnthropicEvent::MessageStart { message } => {
            state.message_id = message.id;
            state.model = message.model;
            if let Some(usage) = message.usage {
                state.input_tokens = usage.input_tokens;
                state.output_tokens = usage.output_tokens;
            }
            None
        }

        AnthropicEvent::ContentBlockStart { content_block, .. } => {
            if let ContentBlock::ToolUse { id, name } = content_block {
                state.current_tool_id = Some(id.clone());
                state.current_tool_name = Some(name.clone());

                return Some(Ok(StreamResponse {
                    id: state.message_id.clone(),
                    model: state.model.clone(),
                    choices: vec![Choice {
                        index: 0,
                        delta: ChoiceDelta {
                            role: Some(MessageRole::Assistant),
                            content: String::new(),
                            reasoning_content: String::new(),
                            tool_calls: vec![ToolCall {
                                id,
                                call_type: ToolType::Function,
                                function: FunctionCall {
                                    name,
                                    arguments: String::new(),
                                },
                            }],
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                    rate_limit: None,
                }));
            }
            None
        }

        AnthropicEvent::ContentBlockDelta { delta, .. } => match delta {
            ContentDelta::Text { text } => Some(Ok(StreamResponse {
                id: state.message_id.clone(),
                model: state.model.clone(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta {
                        role: Some(MessageRole::Assistant),
                        content: text,
                        reasoning_content: String::new(),
                        tool_calls: Vec::new(),
                    },
                    finish_reason: None,
                }],
                usage: None,
                rate_limit: None,
            })),
            ContentDelta::InputJson { partial_json } => {
                if let (Some(id), Some(name)) = (
                    state.current_tool_id.clone(),
                    state.current_tool_name.clone(),
                ) {
                    Some(Ok(StreamResponse {
                        id: state.message_id.clone(),
                        model: state.model.clone(),
                        choices: vec![Choice {
                            index: 0,
                            delta: ChoiceDelta {
                                role: Some(MessageRole::Assistant),
                                content: String::new(),
                                reasoning_content: String::new(),
                                tool_calls: vec![ToolCall {
                                    id,
                                    call_type: ToolType::Function,
                                    function: FunctionCall {
                                        name,
                                        arguments: partial_json,
                                    },
                                }],
                            },
                            finish_reason: None,
                        }],
                        usage: None,
                        rate_limit: None,
                    }))
                } else {
                    None
                }
            }
            ContentDelta::Thinking { thinking } => Some(Ok(StreamResponse {
                id: state.message_id.clone(),
                model: state.model.clone(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta {
                        role: Some(MessageRole::Assistant),
                        content: String::new(),
                        reasoning_content: thinking,
                        tool_calls: Vec::new(),
                    },
                    finish_reason: None,
                }],
                usage: None,
                rate_limit: None,
            })),
        },

        AnthropicEvent::ContentBlockStop { .. } => {
            state.current_tool_id = None;
            state.current_tool_name = None;
            None
        }

        AnthropicEvent::MessageDelta { delta, usage } => {
            if let Some(u) = usage {
                state.output_tokens = u.output_tokens;
            }

            let finish_reason = delta.stop_reason.and_then(|r| match r.as_str() {
                "end_turn" | "stop_sequence" => Some(FinishReason::Stop),
                "max_tokens" => Some(FinishReason::Length),
                "tool_use" => Some(FinishReason::ToolCalls),
                _ => None,
            });

            Some(Ok(StreamResponse {
                id: state.message_id.clone(),
                model: state.model.clone(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta::default(),
                    finish_reason,
                }],
                usage: Some(Usage {
                    input_tokens: state.input_tokens,
                    output_tokens: state.output_tokens,
                    cached_input_tokens: 0,
                    cache_write_tokens: 0,
                }),
                rate_limit: None,
            }))
        }

        AnthropicEvent::MessageStop => None,

        AnthropicEvent::Ping => None,

        AnthropicEvent::Error { error } => Some(Err(ProviderError::Api {
            status: 400,
            message: error.message,
        })),
    }
}

