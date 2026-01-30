//! OpenAI provider implementation

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

const OPENAI_API_BASE: &str = "https://api.openai.com/v1";
const OPENAI_API_URL: &str = "https://api.openai.com/v1/chat/completions";

/// OpenAI provider
#[derive(Debug)]
pub struct OpenAIProvider {
    provider: String,
    api_key: String,
    model: String,
    options: ProviderOptions,
    client: Client,
    base_url: String,
}

impl OpenAIProvider {
    pub fn new(
        provider: String,
        api_key: String,
        model: String,
        options: ProviderOptions,
        base_url: Option<String>,
    ) -> Self {
        Self {
            provider,
            api_key,
            model,
            options,
            client: Client::new(),
            base_url: base_url.unwrap_or_else(|| OPENAI_API_URL.to_string()),
        }
    }

    #[cfg(test)]
    pub fn build_request_body_for_test(
        &self,
        messages: &[Message],
        tools: &[Tool],
    ) -> serde_json::Value {
        self.build_request_body(messages, tools)
    }

    fn build_request_body(&self, messages: &[Message], tools: &[Tool]) -> serde_json::Value {
        let messages_json: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                let mut msg = json!({
                    "role": match m.role {
                        MessageRole::System => "system",
                        MessageRole::User => "user",
                        MessageRole::Assistant => "assistant",
                        MessageRole::Tool => "tool",
                    },
                    "content": m.content,
                });

                if let Some(ref tool_call_id) = m.tool_call_id {
                    msg["tool_call_id"] = json!(tool_call_id);
                }

                if !m.tool_calls.is_empty() {
                    msg["tool_calls"] = json!(m
                        .tool_calls
                        .iter()
                        .map(|tc| {
                            json!({
                                "id": tc.id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments
                                }
                            })
                        })
                        .collect::<Vec<_>>());
                }

                msg
            })
            .collect();

        let mut body = json!({
            "model": self.model,
            "messages": messages_json,
            "stream": true,
            "stream_options": {
                "include_usage": true
            }
        });

        if let Some(temp) = self.options.temperature {
            body["temperature"] = json!(temp);
        }

        if let Some(top_p) = self.options.top_p {
            body["top_p"] = json!(top_p);
        }

        if let Some(frequency_penalty) = self.options.frequency_penalty {
            body["frequency_penalty"] = json!(frequency_penalty);
        }

        if let Some(presence_penalty) = self.options.presence_penalty {
            body["presence_penalty"] = json!(presence_penalty);
        }

        if let Some(max_tokens) = self.options.max_tokens {
            body["max_tokens"] = json!(max_tokens);
        }

        if !tools.is_empty() {
            let tools_json: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }
                    })
                })
                .collect();
            body["tools"] = json!(tools_json);

            if self.options.parallel_tool_calls.unwrap_or(true) {
                body["parallel_tool_calls"] = json!(true);
            }
        }

        body
    }
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamChunk {
    id: String,
    model: String,
    choices: Vec<OpenAIChoice>,
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    index: usize,
    delta: OpenAIDelta,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct OpenAIDelta {
    role: Option<String>,
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    call_type: Option<String>,
    function: Option<OpenAIFunctionDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAIFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAIUsage {
    prompt_tokens: i64,
    completion_tokens: i64,
    total_tokens: i64,
}

fn parse_finish_reason(s: &str) -> Option<FinishReason> {
    match s {
        "stop" => Some(FinishReason::Stop),
        "length" => Some(FinishReason::Length),
        "tool_calls" => Some(FinishReason::ToolCalls),
        "content_filter" => Some(FinishReason::ContentFilter),
        _ => None,
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    fn id(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }

    async fn create_chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        let body = self.build_request_body(&messages, &tools);

        debug!(
            "OpenAI request: {}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );

        // Check if gateway is configured
        let (url, auth_token) = if let Some(gateway_config) = gateway::get_gateway_config() {
            // Use gateway
            let token = GatewayConfig::get_auth_token().ok_or_else(|| {
                ProviderError::MissingApiKey(
                    "Docker Desktop token required for gateway (DOCKER_TOKEN)".to_string(),
                )
            })?;
            let gateway_url = gateway_config.get_url("/v1/chat/completions");
            debug!("Using gateway: {}", gateway_url);
            (gateway_url, token)
        } else {
            // Use direct API
            (self.base_url.clone(), self.api_key.clone())
        };

        let mut request = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", auth_token))
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");

        // Add gateway headers if using gateway
        if gateway::is_gateway_configured() {
            let original_base = if self.base_url.ends_with("/chat/completions") {
                self.base_url
                    .trim_end_matches("/chat/completions")
                    .to_string()
            } else {
                OPENAI_API_BASE.to_string()
            };

            for (key, value) in
                gateway::create_gateway_headers(&self.provider, &self.model, &original_base)
            {
                request = request.header(&key, &value);
            }
            debug!(
                "Gateway headers added: provider={}, model={}",
                self.provider, self.model
            );
        }

        let request = request.json(&body);

        let es = EventSource::new(request).map_err(|e| ProviderError::Stream(e.to_string()))?;

        let model = self.model.clone();

        // Use async_stream to properly handle the EventSource
        let stream = async_stream::stream! {
            let mut es = es;
            let _model = model.clone();

            while let Some(event) = es.next().await {
                debug!("OpenAI EventSource event: {:?}", event);

                match event {
                    Ok(Event::Open) => {
                        debug!("OpenAI EventSource connection opened");
                        continue;
                    }
                    Ok(Event::Message(msg)) => {
                        if msg.data == "[DONE]" {
                            debug!("OpenAI stream received [DONE]");
                            break;
                        }

                        match serde_json::from_str::<OpenAIStreamChunk>(&msg.data) {
                            Ok(chunk) => {
                                let choices: Vec<Choice> = chunk
                                    .choices
                                    .into_iter()
                                    .map(|c| {
                                        let tool_calls: Vec<ToolCall> = c
                                            .delta
                                            .tool_calls
                                            .unwrap_or_default()
                                            .into_iter()
                                            .map(|tc| ToolCall {
                                                id: tc.id.unwrap_or_default(),
                                                call_type: ToolType::Function,
                                                function: FunctionCall {
                                                    name: tc
                                                        .function
                                                        .as_ref()
                                                        .and_then(|f| f.name.clone())
                                                        .unwrap_or_default(),
                                                    arguments: tc
                                                        .function
                                                        .as_ref()
                                                        .and_then(|f| f.arguments.clone())
                                                        .unwrap_or_default(),
                                                },
                                            })
                                            .collect();

                                        Choice {
                                            index: c.index,
                                            delta: ChoiceDelta {
                                                role: c.delta.role.and_then(|r| match r.as_str() {
                                                    "assistant" => Some(MessageRole::Assistant),
                                                    "user" => Some(MessageRole::User),
                                                    "system" => Some(MessageRole::System),
                                                    "tool" => Some(MessageRole::Tool),
                                                    _ => None,
                                                }),
                                                content: c.delta.content.unwrap_or_default(),
                                                reasoning_content: String::new(),
                                                tool_calls,
                                            },
                                            finish_reason: c
                                                .finish_reason
                                                .and_then(|r| parse_finish_reason(&r)),
                                        }
                                    })
                                    .collect();

                                let usage = chunk.usage.map(|u| Usage {
                                    input_tokens: u.prompt_tokens,
                                    output_tokens: u.completion_tokens,
                                    cached_input_tokens: 0,
                                    cache_write_tokens: 0,
                                });

                                yield Ok(StreamResponse {
                                    id: chunk.id,
                                    model: chunk.model,
                                    choices,
                                    usage,
                                    rate_limit: None,
                                });
                            }
                            Err(e) => {
                                error!("Failed to parse OpenAI chunk: {} - {}", e, msg.data);
                                yield Err(ProviderError::Json(e));
                            }
                        }
                    }
                    Err(reqwest_eventsource::Error::StreamEnded) => {
                        debug!("OpenAI EventSource stream ended normally");
                        break;
                    }
                    Err(e) => {
                        error!("OpenAI stream error: {:?}", e);
                        yield Err(ProviderError::Stream(e.to_string()));
                        break;
                    }
                }
            }

            debug!("OpenAI stream finished");
        };

        Ok(Box::pin(stream))
    }
}
