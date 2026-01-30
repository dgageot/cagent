//! AWS Bedrock provider implementation
//!
//! Supports Amazon Bedrock models including:
//! - Anthropic Claude models (claude-3-*, claude-sonnet-4-*)
//! - Amazon Titan models
//! - Meta Llama models
//! - Other models available in Bedrock

use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::chat::{
    Choice, ChoiceDelta, FinishReason, Message, MessageRole, StreamResponse, Usage,
};
use crate::tools::{FunctionCall, Tool, ToolCall, ToolType};

use super::{MessageStream, Provider, ProviderError, ProviderOptions};

/// AWS Bedrock provider
#[derive(Debug)]
pub struct BedrockProvider {
    region: String,
    model_id: String,
    options: ProviderOptions,
    client: reqwest::Client,
}

impl BedrockProvider {
    /// Create a new Bedrock provider
    pub fn new(model_id: String, options: ProviderOptions) -> Result<Self, ProviderError> {
        // Region can be specified in options or from environment
        let region = options
            .provider_opts
            .get("region")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| std::env::var("AWS_REGION").ok())
            .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
            .unwrap_or_else(|| "us-east-1".to_string());

        debug!(
            model_id = %model_id,
            region = %region,
            "Creating Bedrock provider"
        );

        Ok(Self {
            region,
            model_id,
            options,
            client: reqwest::Client::new(),
        })
    }

    /// Get the Bedrock runtime endpoint URL
    fn endpoint_url(&self) -> String {
        format!("https://bedrock-runtime.{}.amazonaws.com", self.region)
    }

    /// Convert messages to Bedrock Converse API format
    fn build_converse_request(
        &self,
        messages: &[Message],
        tools: &[Tool],
    ) -> BedrockConverseRequest {
        let mut system_parts = Vec::new();
        let mut bedrock_messages = Vec::new();

        for msg in messages {
            match msg.role {
                MessageRole::System => {
                    if !msg.content.is_empty() {
                        system_parts.push(SystemContentBlock {
                            text: msg.content.clone(),
                        });
                    }
                }
                MessageRole::User => {
                    if !msg.content.is_empty() {
                        bedrock_messages.push(BedrockMessage {
                            role: "user".to_string(),
                            content: vec![ContentBlock::Text(TextBlock {
                                text: msg.content.clone(),
                            })],
                        });
                    }
                }
                MessageRole::Assistant => {
                    let mut content = vec![];

                    if !msg.content.is_empty() {
                        content.push(ContentBlock::Text(TextBlock {
                            text: msg.content.clone(),
                        }));
                    }

                    // Add tool use blocks for tool calls
                    for tc in &msg.tool_calls {
                        let input: serde_json::Value =
                            serde_json::from_str(&tc.function.arguments)
                                .unwrap_or(serde_json::json!({}));
                        content.push(ContentBlock::ToolUse(ToolUseBlock {
                            tool_use_id: tc.id.clone(),
                            name: tc.function.name.clone(),
                            input,
                        }));
                    }

                    if !content.is_empty() {
                        bedrock_messages.push(BedrockMessage {
                            role: "assistant".to_string(),
                            content,
                        });
                    }
                }
                MessageRole::Tool => {
                    if let Some(ref tool_call_id) = msg.tool_call_id {
                        bedrock_messages.push(BedrockMessage {
                            role: "user".to_string(),
                            content: vec![ContentBlock::ToolResult(ToolResultBlock {
                                tool_use_id: tool_call_id.clone(),
                                content: vec![ToolResultContentBlock::Text {
                                    text: msg.content.clone(),
                                }],
                                status: None,
                            })],
                        });
                    }
                }
            }
        }

        // Build tool config if tools are provided
        let tool_config = if tools.is_empty() {
            None
        } else {
            let bedrock_tools: Vec<BedrockTool> = tools
                .iter()
                .map(|t| BedrockTool {
                    tool_spec: ToolSpec {
                        name: t.name.clone(),
                        description: Some(t.description.clone()),
                        input_schema: ToolInputSchema {
                            json: t.parameters.clone(),
                        },
                    },
                })
                .collect();
            Some(ToolConfig {
                tools: bedrock_tools,
                tool_choice: None,
            })
        };

        // Build inference config
        let mut inference_config = InferenceConfig::default();
        if let Some(temp) = self.options.temperature {
            inference_config.temperature = Some(temp);
        }
        if let Some(max_tokens) = self.options.max_tokens {
            inference_config.max_tokens = Some(max_tokens);
        }
        if let Some(top_p) = self.options.top_p {
            inference_config.top_p = Some(top_p);
        }

        BedrockConverseRequest {
            model_id: self.model_id.clone(),
            system: if system_parts.is_empty() {
                None
            } else {
                Some(system_parts)
            },
            messages: bedrock_messages,
            tool_config,
            inference_config: Some(inference_config),
        }
    }

    /// Sign a request with AWS Signature Version 4
    async fn sign_request(
        &self,
        method: &str,
        path: &str,
        body: &[u8],
    ) -> Result<Vec<(String, String)>, ProviderError> {
        // Get credentials from environment
        let access_key = std::env::var("AWS_ACCESS_KEY_ID")
            .map_err(|_| ProviderError::MissingApiKey("AWS_ACCESS_KEY_ID".to_string()))?;
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY")
            .map_err(|_| ProviderError::MissingApiKey("AWS_SECRET_ACCESS_KEY".to_string()))?;
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();

        let host = format!("bedrock-runtime.{}.amazonaws.com", self.region);
        let service = "bedrock";

        // Get current time in required formats
        let now = chrono::Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        // Create canonical request
        let content_hash = sha256_hex(body);

        let mut signed_headers = "content-type;host;x-amz-content-sha256;x-amz-date".to_string();
        let mut canonical_headers = format!(
            "content-type:application/json\nhost:{}\nx-amz-content-sha256:{}\nx-amz-date:{}\n",
            host, content_hash, amz_date
        );

        if session_token.is_some() {
            signed_headers.push_str(";x-amz-security-token");
            canonical_headers.push_str(&format!(
                "x-amz-security-token:{}\n",
                session_token.as_ref().unwrap()
            ));
        }

        let canonical_request = format!(
            "{}\n{}\n\n{}\n{}\n{}",
            method, path, canonical_headers, signed_headers, content_hash
        );

        // Create string to sign
        let algorithm = "AWS4-HMAC-SHA256";
        let credential_scope = format!("{}/{}/{}/aws4_request", date_stamp, self.region, service);
        let string_to_sign = format!(
            "{}\n{}\n{}\n{}",
            algorithm,
            amz_date,
            credential_scope,
            sha256_hex(canonical_request.as_bytes())
        );

        // Calculate signature
        let k_date = hmac_sha256(format!("AWS4{}", secret_key).as_bytes(), date_stamp.as_bytes());
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, service.as_bytes());
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex_encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        // Build authorization header
        let authorization = format!(
            "{} Credential={}/{}, SignedHeaders={}, Signature={}",
            algorithm, access_key, credential_scope, signed_headers, signature
        );

        let mut headers = vec![
            ("Authorization".to_string(), authorization),
            ("X-Amz-Date".to_string(), amz_date),
            ("X-Amz-Content-Sha256".to_string(), content_hash),
            ("Content-Type".to_string(), "application/json".to_string()),
        ];

        if let Some(token) = session_token {
            headers.push(("X-Amz-Security-Token".to_string(), token));
        }

        Ok(headers)
    }
}

#[async_trait]
impl Provider for BedrockProvider {
    fn id(&self) -> String {
        format!("bedrock/{}", self.model_id)
    }

    fn context_limit(&self) -> Option<i64> {
        // Context limits vary by model
        if self.model_id.contains("claude-3") {
            Some(200_000)
        } else if self.model_id.contains("claude-sonnet-4")
            || self.model_id.contains("claude-opus-4")
        {
            Some(200_000)
        } else if self.model_id.contains("titan") {
            Some(8_000)
        } else if self.model_id.contains("llama") {
            Some(128_000)
        } else {
            Some(8_000) // Safe default
        }
    }

    async fn create_chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        let request_body = self.build_converse_request(&messages, &tools);
        let body_bytes = serde_json::to_vec(&request_body)?;

        // Use streaming endpoint
        let path = format!("/model/{}/converse-stream", self.model_id);
        let url = format!("{}{}", self.endpoint_url(), path);

        debug!(
            model_id = %self.model_id,
            url = %url,
            "Sending Bedrock Converse Stream request"
        );

        let headers = self.sign_request("POST", &path, &body_bytes).await?;

        let mut req = self.client.post(&url);
        for (key, value) in headers {
            req = req.header(&key, &value);
        }
        req = req.body(body_bytes);

        let es = EventSource::new(req).map_err(|e| ProviderError::Stream(e.to_string()))?;

        let model_id = self.model_id.clone();

        // Track state across events for tool call assembly
        let stream = es
            .take_while(|event| {
                let should_continue = !matches!(event, Err(reqwest_eventsource::Error::StreamEnded));
                std::future::ready(should_continue)
            })
            .filter_map(move |event| {
            let model = model_id.clone();
            async move {
                match event {
                    Ok(Event::Message(msg)) => {
                        if msg.data == "[DONE]" {
                            return None;
                        }

                        // Try to parse as Bedrock event
                        match serde_json::from_str::<BedrockStreamEvent>(&msg.data) {
                            Ok(bedrock_event) => {
                                convert_bedrock_event_to_stream_response(bedrock_event, &model)
                            }
                            Err(e) => {
                                warn!("Failed to parse Bedrock event: {} - {}", e, msg.data);
                                None
                            }
                        }
                    }
                    Ok(Event::Open) => None,
                    Err(e) => Some(Err(ProviderError::Stream(e.to_string()))),
                }
            }
        });

        Ok(Box::pin(stream))
    }
}

/// Convert Bedrock streaming event to StreamResponse
fn convert_bedrock_event_to_stream_response(
    event: BedrockStreamEvent,
    model: &str,
) -> Option<Result<StreamResponse, ProviderError>> {
    match event {
        BedrockStreamEvent::ContentBlockStart {
            content_block,
            index,
        } => {
            // Tool use start creates a tool call
            if let Some(tool_use) = content_block.tool_use {
                let tool_call = ToolCall {
                    id: tool_use.tool_use_id,
                    call_type: ToolType::Function,
                    function: FunctionCall {
                        name: tool_use.name,
                        arguments: String::new(),
                    },
                };
                return Some(Ok(StreamResponse {
                    id: uuid::Uuid::new_v4().to_string(),
                    model: model.to_string(),
                    choices: vec![Choice {
                        index,
                        delta: ChoiceDelta {
                            role: Some(MessageRole::Assistant),
                            content: String::new(),
                            reasoning_content: String::new(),
                            tool_calls: vec![tool_call],
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                    rate_limit: None,
                }));
            }
            None
        }
        BedrockStreamEvent::ContentBlockDelta { delta, index } => {
            if let Some(text) = delta.text {
                return Some(Ok(StreamResponse {
                    id: uuid::Uuid::new_v4().to_string(),
                    model: model.to_string(),
                    choices: vec![Choice {
                        index,
                        delta: ChoiceDelta {
                            role: None,
                            content: text,
                            reasoning_content: String::new(),
                            tool_calls: vec![],
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                    rate_limit: None,
                }));
            }
            if let Some(input) = delta.tool_use {
                // Tool use delta - append to arguments
                let tool_call = ToolCall {
                    id: String::new(), // ID was in start
                    call_type: ToolType::Function,
                    function: FunctionCall {
                        name: String::new(), // Name was in start
                        arguments: input.input,
                    },
                };
                return Some(Ok(StreamResponse {
                    id: uuid::Uuid::new_v4().to_string(),
                    model: model.to_string(),
                    choices: vec![Choice {
                        index,
                        delta: ChoiceDelta {
                            role: None,
                            content: String::new(),
                            reasoning_content: String::new(),
                            tool_calls: vec![tool_call],
                        },
                        finish_reason: None,
                    }],
                    usage: None,
                    rate_limit: None,
                }));
            }
            None
        }
        BedrockStreamEvent::ContentBlockStop { .. } => None,
        BedrockStreamEvent::MessageStart { .. } => None,
        BedrockStreamEvent::MessageDelta { delta, usage } => {
            let finish_reason = delta.stop_reason.and_then(|r| match r.as_str() {
                "end_turn" => Some(FinishReason::Stop),
                "tool_use" => Some(FinishReason::ToolCalls),
                "max_tokens" => Some(FinishReason::Length),
                _ => None,
            });

            Some(Ok(StreamResponse {
                id: uuid::Uuid::new_v4().to_string(),
                model: model.to_string(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta {
                        role: None,
                        content: String::new(),
                        reasoning_content: String::new(),
                        tool_calls: vec![],
                    },
                    finish_reason,
                }],
                usage: usage.map(|u| Usage {
                    input_tokens: 0,
                    output_tokens: u.output_tokens,
                    cached_input_tokens: 0,
                    cache_write_tokens: 0,
                }),
                rate_limit: None,
            }))
        }
        BedrockStreamEvent::MessageStop { stop_reason } => {
            let finish_reason = match stop_reason.as_deref() {
                Some("end_turn") => Some(FinishReason::Stop),
                Some("tool_use") => Some(FinishReason::ToolCalls),
                Some("max_tokens") => Some(FinishReason::Length),
                _ => Some(FinishReason::Stop),
            };

            Some(Ok(StreamResponse {
                id: uuid::Uuid::new_v4().to_string(),
                model: model.to_string(),
                choices: vec![Choice {
                    index: 0,
                    delta: ChoiceDelta {
                        role: None,
                        content: String::new(),
                        reasoning_content: String::new(),
                        tool_calls: vec![],
                    },
                    finish_reason,
                }],
                usage: None,
                rate_limit: None,
            }))
        }
        BedrockStreamEvent::Metadata { usage, .. } => {
            if let Some(u) = usage {
                return Some(Ok(StreamResponse {
                    id: uuid::Uuid::new_v4().to_string(),
                    model: model.to_string(),
                    choices: vec![],
                    usage: Some(Usage {
                        input_tokens: u.input_tokens,
                        output_tokens: u.output_tokens,
                        cached_input_tokens: 0,
                        cache_write_tokens: 0,
                    }),
                    rate_limit: None,
                }));
            }
            None
        }
    }
}

// ============================================================================
// Bedrock Converse API Request/Response Types
// ============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockConverseRequest {
    model_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<SystemContentBlock>>,
    messages: Vec<BedrockMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_config: Option<ToolConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    inference_config: Option<InferenceConfig>,
}

#[derive(Debug, Serialize)]
struct SystemContentBlock {
    text: String,
}

#[derive(Debug, Serialize)]
struct BedrockMessage {
    role: String,
    content: Vec<ContentBlock>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ContentBlock {
    Text(TextBlock),
    ToolUse(ToolUseBlock),
    ToolResult(ToolResultBlock),
}

#[derive(Debug, Serialize)]
struct TextBlock {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolUseBlock {
    tool_use_id: String,
    name: String,
    input: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolResultBlock {
    tool_use_id: String,
    content: Vec<ToolResultContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ToolResultContentBlock {
    Text { text: String },
}

#[derive(Debug, Serialize)]
struct ToolConfig {
    tools: Vec<BedrockTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BedrockTool {
    tool_spec: ToolSpec,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolSpec {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    input_schema: ToolInputSchema,
}

#[derive(Debug, Serialize)]
struct ToolInputSchema {
    json: serde_json::Value,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct InferenceConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stop_sequences: Option<Vec<String>>,
}

// ============================================================================
// Bedrock Streaming Event Types
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
#[allow(clippy::large_enum_variant)]
enum BedrockStreamEvent {
    #[serde(rename = "contentBlockStart")]
    ContentBlockStart {
        #[serde(rename = "contentBlockIndex")]
        index: usize,
        #[serde(rename = "start")]
        content_block: ContentBlockStartData,
    },
    #[serde(rename = "contentBlockDelta")]
    ContentBlockDelta {
        #[serde(rename = "contentBlockIndex")]
        index: usize,
        delta: ContentBlockDeltaData,
    },
    #[serde(rename = "contentBlockStop")]
    ContentBlockStop {
        #[serde(rename = "contentBlockIndex")]
        index: usize,
    },
    #[serde(rename = "messageStart")]
    MessageStart { role: Option<String> },
    #[serde(rename = "messageDelta")]
    MessageDelta {
        delta: MessageDeltaData,
        usage: Option<DeltaUsage>,
    },
    #[serde(rename = "messageStop")]
    MessageStop {
        #[serde(rename = "stopReason")]
        stop_reason: Option<String>,
    },
    #[serde(rename = "metadata")]
    Metadata {
        usage: Option<MetadataUsage>,
        #[allow(dead_code)]
        metrics: Option<serde_json::Value>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockStartData {
    #[serde(rename = "toolUse")]
    tool_use: Option<ToolUseStartData>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ToolUseStartData {
    tool_use_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentBlockDeltaData {
    text: Option<String>,
    #[serde(rename = "toolUse")]
    tool_use: Option<ToolUseDeltaData>,
}

#[derive(Debug, Deserialize)]
struct ToolUseDeltaData {
    input: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MessageDeltaData {
    stop_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeltaUsage {
    output_tokens: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MetadataUsage {
    input_tokens: i64,
    output_tokens: i64,
}

// ============================================================================
// AWS Signature V4 Helpers
// ============================================================================

fn sha256_hex(data: &[u8]) -> String {
    let hash = sha256_hash(data);
    hex_encode(hash)
}

fn sha256_hash(data: &[u8]) -> [u8; 32] {
    use std::num::Wrapping;

    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [Wrapping<u32>; 8] = [
        Wrapping(0x6a09e667),
        Wrapping(0xbb67ae85),
        Wrapping(0x3c6ef372),
        Wrapping(0xa54ff53a),
        Wrapping(0x510e527f),
        Wrapping(0x9b05688c),
        Wrapping(0x1f83d9ab),
        Wrapping(0x5be0cd19),
    ];

    // Pad message
    let bit_len = (data.len() as u64) * 8;
    let mut padded = data.to_vec();
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0x00);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    // Process each 512-bit block
    for chunk in padded.chunks(64) {
        let mut w = [Wrapping(0u32); 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = Wrapping(u32::from_be_bytes([word[0], word[1], word[2], word[3]]));
        }
        for i in 16..64 {
            let s0 = w[i - 15].0.rotate_right(7)
                ^ w[i - 15].0.rotate_right(18)
                ^ (w[i - 15].0 >> 3);
            let s1 =
                w[i - 2].0.rotate_right(17) ^ w[i - 2].0.rotate_right(19) ^ (w[i - 2].0 >> 10);
            w[i] = w[i - 16] + Wrapping(s0) + w[i - 7] + Wrapping(s1);
        }

        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);

        for i in 0..64 {
            let s1 = e.0.rotate_right(6) ^ e.0.rotate_right(11) ^ e.0.rotate_right(25);
            let ch = (e.0 & f.0) ^ ((!e.0) & g.0);
            let temp1 = hh + Wrapping(s1) + Wrapping(ch) + Wrapping(K[i]) + w[i];
            let s0 = a.0.rotate_right(2) ^ a.0.rotate_right(13) ^ a.0.rotate_right(22);
            let maj = (a.0 & b.0) ^ (a.0 & c.0) ^ (b.0 & c.0);
            let temp2 = Wrapping(s0) + Wrapping(maj);

            hh = g;
            g = f;
            f = e;
            e = d + temp1;
            d = c;
            c = b;
            b = a;
            a = temp1 + temp2;
        }

        h[0] += a;
        h[1] += b;
        h[2] += c;
        h[3] += d;
        h[4] += e;
        h[5] += f;
        h[6] += g;
        h[7] += hh;
    }

    let mut result = [0u8; 32];
    for (i, &Wrapping(val)) in h.iter().enumerate() {
        result[i * 4..(i + 1) * 4].copy_from_slice(&val.to_be_bytes());
    }
    result
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];

    let key_block = if key.len() > 64 {
        sha256_hash(key).to_vec()
    } else {
        key.to_vec()
    };

    for (i, &k) in key_block.iter().enumerate() {
        ipad[i] ^= k;
        opad[i] ^= k;
    }

    let mut inner = ipad.to_vec();
    inner.extend_from_slice(data);
    let inner_hash = sha256_hash(&inner);

    let mut outer = opad.to_vec();
    outer.extend_from_slice(&inner_hash);
    sha256_hash(&outer)
}

fn hex_encode(data: impl AsRef<[u8]>) -> String {
    const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";
    let data = data.as_ref();
    let mut result = String::with_capacity(data.len() * 2);
    for &byte in data {
        result.push(HEX_CHARS[(byte >> 4) as usize] as char);
        result.push(HEX_CHARS[(byte & 0x0f) as usize] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_basic() {
        let result = sha256_hex(b"");
        assert_eq!(
            result,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_sha256_hello() {
        let result = sha256_hex(b"hello");
        assert_eq!(
            result,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode([0x00, 0xff, 0x12, 0xab]), "00ff12ab");
    }

    #[test]
    fn test_bedrock_provider_id() {
        let provider = BedrockProvider::new(
            "anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
            ProviderOptions::default(),
        )
        .unwrap();

        assert_eq!(
            provider.id(),
            "bedrock/anthropic.claude-3-sonnet-20240229-v1:0"
        );
    }

    #[test]
    fn test_bedrock_context_limits() {
        let claude3 = BedrockProvider::new(
            "anthropic.claude-3-sonnet".to_string(),
            ProviderOptions::default(),
        )
        .unwrap();
        assert_eq!(claude3.context_limit(), Some(200_000));

        let titan = BedrockProvider::new(
            "amazon.titan-text-express".to_string(),
            ProviderOptions::default(),
        )
        .unwrap();
        assert_eq!(titan.context_limit(), Some(8_000));
    }
}
