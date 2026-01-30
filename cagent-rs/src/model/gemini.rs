//! Google Gemini provider implementation

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tracing::{debug, error};

use crate::chat::{Choice, ChoiceDelta, FinishReason, Message, MessageRole, StreamResponse, Usage};
use crate::tools::{FunctionCall, Tool, ToolCall, ToolType};

use super::{MessageStream, Provider, ProviderError, ProviderOptions};

const GEMINI_API_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";

/// Google Gemini provider
#[derive(Debug)]
pub struct GeminiProvider {
    api_key: String,
    model: String,
    options: ProviderOptions,
    client: Client,
}

impl GeminiProvider {
    pub fn new(api_key: String, model: String, options: ProviderOptions) -> Self {
        Self {
            api_key,
            model,
            options,
            client: Client::new(),
        }
    }

    fn build_request_body(&self, messages: &[Message], tools: &[Tool]) -> serde_json::Value {
        // Convert messages to Gemini format
        let mut contents = Vec::new();
        let mut system_instruction = None;

        for msg in messages {
            match msg.role {
                MessageRole::System => {
                    system_instruction = Some(json!({
                        "parts": [{"text": msg.content}]
                    }));
                }
                MessageRole::User => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{"text": msg.content}]
                    }));
                }
                MessageRole::Assistant => {
                    if msg.tool_calls.is_empty() {
                        contents.push(json!({
                            "role": "model",
                            "parts": [{"text": msg.content}]
                        }));
                    } else {
                        let mut parts: Vec<serde_json::Value> = Vec::new();
                        if !msg.content.is_empty() {
                            parts.push(json!({"text": msg.content}));
                        }
                        for tc in &msg.tool_calls {
                            parts.push(json!({
                                "functionCall": {
                                    "name": tc.function.name,
                                    "args": serde_json::from_str::<serde_json::Value>(&tc.function.arguments).unwrap_or(json!({}))
                                }
                            }));
                        }
                        contents.push(json!({
                            "role": "model",
                            "parts": parts
                        }));
                    }
                }
                MessageRole::Tool => {
                    contents.push(json!({
                        "role": "user",
                        "parts": [{
                            "functionResponse": {
                                "name": msg.name.as_ref().unwrap_or(&"unknown".to_string()),
                                "response": {"result": msg.content}
                            }
                        }]
                    }));
                }
            }
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": {}
        });

        if let Some(sys) = system_instruction {
            body["systemInstruction"] = sys;
        }

        if let Some(temp) = self.options.temperature {
            body["generationConfig"]["temperature"] = json!(temp);
        }

        if let Some(max_tokens) = self.options.max_tokens {
            body["generationConfig"]["maxOutputTokens"] = json!(max_tokens);
        }

        if !tools.is_empty() {
            let tool_declarations: Vec<serde_json::Value> = tools
                .iter()
                .map(|t| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters
                    })
                })
                .collect();
            body["tools"] = json!([{
                "functionDeclarations": tool_declarations
            }]);
        }

        body
    }
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    #[serde(rename = "usageMetadata")]
    usage_metadata: Option<GeminiUsage>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiCandidate {
    content: Option<GeminiContent>,
    #[serde(rename = "finishReason")]
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiContent {
    parts: Option<Vec<GeminiPart>>,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiPart {
    text: Option<String>,
    #[serde(rename = "functionCall")]
    function_call: Option<GeminiFunctionCall>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiFunctionCall {
    name: String,
    args: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct GeminiUsage {
    #[serde(rename = "promptTokenCount")]
    prompt_token_count: Option<i64>,
    #[serde(rename = "candidatesTokenCount")]
    candidates_token_count: Option<i64>,
    #[serde(rename = "totalTokenCount")]
    total_token_count: Option<i64>,
}

#[async_trait]
impl Provider for GeminiProvider {
    fn id(&self) -> String {
        format!("google/{}", self.model)
    }

    async fn create_chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        let body = self.build_request_body(&messages, &tools);

        debug!(
            "Gemini request: {}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );

        let url = format!(
            "{}/{}:streamGenerateContent?alt=sse&key={}",
            GEMINI_API_URL, self.model, self.api_key
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::Api {
                status,
                message: body,
            });
        }

        let model = self.model.clone();
        let stream = response.bytes_stream();
        let buffer = Arc::new(Mutex::new(String::new()));

        let stream = stream
            .filter_map(move |chunk| {
                let model = model.clone();
                let buffer = buffer.clone();
                async move {
                    match chunk {
                        Ok(bytes) => {
                            let text = String::from_utf8_lossy(&bytes);
                            let mut buf = buffer.lock().unwrap();
                            buf.push_str(&text);

                            // Parse SSE events
                            let mut results = Vec::new();

                            loop {
                                let current_buf = buf.clone();
                                if let Some(data_start) = current_buf.find("data: ") {
                                    let data_content = &current_buf[data_start + 6..];
                                    if let Some(end) = data_content.find("\n\n") {
                                        let json_str = &data_content[..end];
                                        *buf = data_content[end + 2..].to_string();

                                        if let Ok(gemini_resp) =
                                            serde_json::from_str::<GeminiResponse>(json_str)
                                        {
                                            if let Some(candidates) = gemini_resp.candidates {
                                                for candidate in candidates {
                                                    if let Some(content) = candidate.content {
                                                        let mut text_content = String::new();
                                                        let mut tool_calls = Vec::new();

                                                        if let Some(parts) = content.parts {
                                                            for part in parts {
                                                                if let Some(text) = part.text {
                                                                    text_content.push_str(&text);
                                                                }
                                                                if let Some(fc) = part.function_call
                                                                {
                                                                    tool_calls.push(ToolCall {
                                                                        id: format!(
                                                                            "call_{}",
                                                                            uuid::Uuid::new_v4()
                                                                        ),
                                                                        call_type:
                                                                            ToolType::Function,
                                                                        function: FunctionCall {
                                                                            name: fc.name,
                                                                            arguments: fc
                                                                                .args
                                                                                .to_string(),
                                                                        },
                                                                    });
                                                                }
                                                            }
                                                        }

                                                        let finish_reason = candidate
                                                            .finish_reason
                                                            .and_then(|r| match r.as_str() {
                                                                "STOP" => Some(FinishReason::Stop),
                                                                "MAX_TOKENS" => {
                                                                    Some(FinishReason::Length)
                                                                }
                                                                "SAFETY" => Some(
                                                                    FinishReason::ContentFilter,
                                                                ),
                                                                _ => None,
                                                            });

                                                        let usage = gemini_resp
                                                            .usage_metadata
                                                            .as_ref()
                                                            .map(|u| Usage {
                                                                input_tokens: u
                                                                    .prompt_token_count
                                                                    .unwrap_or(0),
                                                                output_tokens: u
                                                                    .candidates_token_count
                                                                    .unwrap_or(0),
                                                                cached_input_tokens: 0,
                                                                cache_write_tokens: 0,
                                                            });

                                                        results.push(Ok(StreamResponse {
                                                            id: format!(
                                                                "gemini_{}",
                                                                uuid::Uuid::new_v4()
                                                            ),
                                                            model: model.clone(),
                                                            choices: vec![Choice {
                                                                index: 0,
                                                                delta: ChoiceDelta {
                                                                    role: Some(
                                                                        MessageRole::Assistant,
                                                                    ),
                                                                    content: text_content,
                                                                    reasoning_content: String::new(
                                                                    ),
                                                                    tool_calls,
                                                                },
                                                                finish_reason,
                                                            }],
                                                            usage,
                                                            rate_limit: None,
                                                        }));
                                                    }
                                                }
                                            }
                                        }
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }

                            if results.is_empty() {
                                None
                            } else {
                                Some(futures::stream::iter(results))
                            }
                        }
                        Err(e) => {
                            error!("Gemini stream error: {:?}", e);
                            Some(futures::stream::iter(vec![Err(ProviderError::Stream(
                                e.to_string(),
                            ))]))
                        }
                    }
                }
            })
            .flatten();

        Ok(Box::pin(stream))
    }
}
