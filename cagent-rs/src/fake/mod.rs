//! VCR-style proxy for recording and replaying AI API interactions.
//!
//! This module provides a fake proxy that can:
//! - Record AI API calls to cassette files for later replay
//! - Replay recorded cassettes for deterministic testing
//! - Simulate streaming with configurable delays
//!
//! The proxy intercepts requests intended for AI APIs and either:
//! 1. In record mode: forwards to the real API and saves the response
//! 2. In replay mode: returns the saved response from the cassette

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

/// A recorded HTTP interaction
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interaction {
    pub request: RecordedRequest,
    pub response: RecordedResponse,
}

/// Recorded HTTP request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub body: String,
}

/// Recorded HTTP response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub body: String,
}

/// Cassette file for storing recorded interactions
#[derive(Debug, Serialize, Deserialize)]
pub struct Cassette {
    #[serde(skip)]
    path: PathBuf,
    pub interactions: Vec<Interaction>,
    #[serde(skip)]
    replay_index: usize,
}

impl Cassette {
    /// Create a new cassette at the given path
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            interactions: Vec::new(),
            replay_index: 0,
        }
    }

    /// Load an existing cassette from disk
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read cassette: {}", path.display()))?;
        let mut cassette: Cassette = serde_yaml::from_str(&contents)
            .with_context(|| format!("Failed to parse cassette: {}", path.display()))?;
        cassette.path = path.to_path_buf();
        Ok(cassette)
    }

    /// Save the cassette to disk
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = serde_yaml::to_string(&self)?;
        std::fs::write(&self.path, contents)?;
        info!("Saved cassette to {}", self.path.display());
        Ok(())
    }

    /// Add a new interaction
    pub fn add_interaction(&mut self, interaction: Interaction) {
        self.interactions.push(interaction);
    }

    /// Get the next interaction for replay
    pub fn next_interaction(&mut self) -> Option<&Interaction> {
        if self.replay_index < self.interactions.len() {
            let interaction = &self.interactions[self.replay_index];
            self.replay_index += 1;
            Some(interaction)
        } else {
            None
        }
    }

    /// Find a matching interaction by request
    pub fn find_match(&self, method: &str, url: &str, body: &str) -> Option<&Interaction> {
        let normalized_body = normalize_request_body(body);

        self.interactions.iter().find(|i| {
            i.request.method == method
                && urls_match(&i.request.url, url)
                && normalize_request_body(&i.request.body) == normalized_body
        })
    }
}

/// Normalize request body for matching (removes dynamic fields like tool call IDs)
fn normalize_request_body(body: &str) -> String {
    // This regex matches tool call IDs which are dynamic
    static CALL_ID_RE: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r#"call_[a-zA-Z0-9\-]+"#).unwrap());

    // Also normalize max_tokens which can vary
    static MAX_TOKENS_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r#""(?:max_(?:output_)?tokens|maxOutputTokens)":\d+,?"#).unwrap()
    });

    let result = CALL_ID_RE.replace_all(body, "call_ID");
    let result = MAX_TOKENS_RE.replace_all(&result, "");
    result.to_string()
}

/// Check if two URLs match (ignoring query parameter order)
fn urls_match(recorded: &str, actual: &str) -> bool {
    // Simple exact match for now
    recorded == actual
}

/// Proxy mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyMode {
    /// Only replay from cassette, fail if not found
    ReplayOnly,
    /// Record new interactions, overwriting existing cassette
    Record,
}

/// Options for the fake proxy
#[derive(Debug, Clone)]
pub struct ProxyOptions {
    /// Simulate streaming with delays between chunks
    pub simulate_stream: bool,
    /// Delay between SSE chunks when simulating streaming
    pub stream_chunk_delay: Duration,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            simulate_stream: false,
            stream_chunk_delay: Duration::from_millis(15),
        }
    }
}

/// Builder for proxy options
pub struct ProxyOptionsBuilder {
    options: ProxyOptions,
}

impl ProxyOptionsBuilder {
    pub fn new() -> Self {
        Self {
            options: ProxyOptions::default(),
        }
    }

    pub fn simulate_stream(mut self, enabled: bool) -> Self {
        self.options.simulate_stream = enabled;
        self
    }

    pub fn stream_chunk_delay(mut self, delay: Duration) -> Self {
        self.options.stream_chunk_delay = delay;
        self
    }

    pub fn build(self) -> ProxyOptions {
        self.options
    }
}

impl Default for ProxyOptionsBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// A running proxy instance
pub struct RunningProxy {
    pub url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
    cassette: Arc<Mutex<Cassette>>,
    mode: ProxyMode,
}

impl RunningProxy {
    /// Get the proxy URL for use as the models gateway
    pub fn gateway_url(&self) -> &str {
        &self.url
    }

    /// Stop the proxy and save the cassette if in record mode
    pub async fn stop(mut self) -> Result<()> {
        // Signal shutdown
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        // Save cassette if in record mode
        if self.mode == ProxyMode::Record {
            let cassette = self.cassette.lock().unwrap();
            cassette.save()?;
        }

        Ok(())
    }
}

/// Start a replay proxy that returns recorded responses
pub async fn start_replay_proxy(
    cassette_path: impl AsRef<Path>,
    options: ProxyOptions,
) -> Result<RunningProxy> {
    let cassette = Cassette::load(&cassette_path)?;
    info!(
        "Starting replay proxy with {} interactions",
        cassette.interactions.len()
    );
    start_proxy_internal(cassette, ProxyMode::ReplayOnly, options).await
}

/// Start a recording proxy that records API interactions
pub async fn start_recording_proxy(cassette_path: impl AsRef<Path>) -> Result<RunningProxy> {
    let cassette = Cassette::new(&cassette_path);
    info!(
        "Starting recording proxy, will save to {}",
        cassette_path.as_ref().display()
    );
    start_proxy_internal(cassette, ProxyMode::Record, ProxyOptions::default()).await
}

async fn start_proxy_internal(
    cassette: Cassette,
    mode: ProxyMode,
    options: ProxyOptions,
) -> Result<RunningProxy> {
    use axum::{
        body::Body,
        extract::{Request, State},
        http::{HeaderMap, StatusCode},
        response::{IntoResponse, Response},
        routing::any,
        Router,
    };
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    let cassette = Arc::new(Mutex::new(cassette));
    let cassette_clone = Arc::clone(&cassette);

    #[derive(Clone)]
    struct ProxyState {
        cassette: Arc<Mutex<Cassette>>,
        mode: ProxyMode,
        options: ProxyOptions,
    }

    let state = ProxyState {
        cassette: cassette_clone,
        mode,
        options,
    };

    async fn handle_request(
        State(state): State<ProxyState>,
        headers: HeaderMap,
        request: Request,
    ) -> Response {
        let method = request.method().to_string();
        let uri = request.uri().to_string();

        // Get forwarding host from header
        let forward_host = headers
            .get("x-cagent-forward")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .trim_end_matches('/');

        if forward_host.is_empty() {
            return (StatusCode::BAD_REQUEST, "Missing X-Cagent-Forward header").into_response();
        }

        // Read body
        let body_bytes = match axum::body::to_bytes(request.into_body(), 10 * 1024 * 1024).await {
            Ok(b) => b,
            Err(e) => {
                error!("Failed to read request body: {}", e);
                return (StatusCode::BAD_REQUEST, "Failed to read body").into_response();
            }
        };
        let body_str = String::from_utf8_lossy(&body_bytes).to_string();

        // Build target URL
        let target_url = format!("{}{}", forward_host, uri);

        debug!(
            mode = ?state.mode,
            method = method,
            url = target_url,
            "Proxy handling request"
        );

        match state.mode {
            ProxyMode::ReplayOnly => {
                // Find matching interaction in cassette
                let (response_body, response_status, response_headers) = {
                    let cassette = state.cassette.lock().unwrap();
                    match cassette.find_match(&method, &target_url, &body_str) {
                        Some(interaction) => {
                            let response = &interaction.response;
                            (
                                response.body.clone(),
                                response.status,
                                response.headers.clone(),
                            )
                        }
                        None => {
                            warn!(method = method, url = target_url, "No matching cassette entry");
                            return (StatusCode::NOT_FOUND, "No matching cassette entry").into_response();
                        }
                    }
                };

                debug!(status = response_status, "Replaying recorded response");

                let recorded_response = RecordedResponse {
                    status: response_status,
                    headers: response_headers.clone(),
                    body: response_body.clone(),
                };

                let mut builder = Response::builder().status(response_status);
                for (key, value) in &response_headers {
                    builder = builder.header(key, value);
                }

                if state.options.simulate_stream && is_sse_response(&recorded_response) {
                    // Simulate streaming with delays
                    simulate_stream_response(response_body, state.options.clone()).await
                } else {
                    builder
                        .body(Body::from(response_body))
                        .unwrap_or_else(|_| {
                            (StatusCode::INTERNAL_SERVER_ERROR, "Build error")
                                .into_response()
                        })
                }
            }
            ProxyMode::Record => {
                // Forward to real API
                let client = reqwest::Client::new();

                // Build request with API key
                let mut req_builder = client.request(
                    reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
                    &target_url,
                );

                // Add API key based on host
                req_builder = add_api_key(req_builder, forward_host);

                // Copy relevant headers
                for (key, value) in &headers {
                    if key != "host" && key != "x-cagent-forward" {
                        if let Ok(v) = value.to_str() {
                            req_builder = req_builder.header(key.as_str(), v);
                        }
                    }
                }

                req_builder = req_builder.body(body_bytes.to_vec());

                match req_builder.send().await {
                    Ok(response) => {
                        let status = response.status().as_u16();
                        let resp_headers: HashMap<String, String> = response
                            .headers()
                            .iter()
                            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
                            .collect();
                        
                        match response.bytes().await {
                            Ok(body) => {
                                let body_str = String::from_utf8_lossy(&body).to_string();
                                
                                // Record the interaction
                                let interaction = Interaction {
                                    request: RecordedRequest {
                                        method,
                                        url: target_url,
                                        body: body_str.clone(),
                                    },
                                    response: RecordedResponse {
                                        status,
                                        headers: HashMap::new(), // Don't store headers for security
                                        body: body_str.clone(),
                                    },
                                };

                                {
                                    let mut cassette = state.cassette.lock().unwrap();
                                    cassette.add_interaction(interaction);
                                }

                                let mut builder = Response::builder().status(status);
                                for (key, value) in resp_headers {
                                    builder = builder.header(key, value);
                                }
                                builder.body(Body::from(body)).unwrap_or_else(|_| {
                                    (StatusCode::INTERNAL_SERVER_ERROR, "Build error").into_response()
                                })
                            }
                            Err(e) => {
                                error!("Failed to read response body: {}", e);
                                (StatusCode::BAD_GATEWAY, "Failed to read response").into_response()
                            }
                        }
                    }
                    Err(e) => {
                        error!("Request to upstream failed: {}", e);
                        (StatusCode::BAD_GATEWAY, format!("Upstream error: {}", e)).into_response()
                    }
                }
            }
        }
    }

    let app = Router::new()
        .route("/*path", any(handle_request))
        .route("/", any(handle_request))
        .with_state(state);

    // Bind to a random available port
    let addr = SocketAddr::from(([127, 0, 0, 1], 0));
    let listener = TcpListener::bind(addr).await?;
    let actual_addr = listener.local_addr()?;
    let url = format!("http://{}", actual_addr);

    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    // Spawn server
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .ok();
    });

    info!("Fake proxy started on {}", url);

    Ok(RunningProxy {
        url,
        shutdown_tx: Some(shutdown_tx),
        cassette,
        mode,
    })
}

fn add_api_key(builder: reqwest::RequestBuilder, host: &str) -> reqwest::RequestBuilder {
    match host {
        "https://api.openai.com/v1" | "https://api.openai.com" => {
            if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                builder.header("Authorization", format!("Bearer {}", key))
            } else {
                builder
            }
        }
        "https://api.anthropic.com" => {
            if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                builder.header("X-Api-Key", key)
            } else {
                builder
            }
        }
        "https://generativelanguage.googleapis.com" => {
            if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
                builder.header("X-Goog-Api-Key", key)
            } else {
                builder
            }
        }
        "https://api.mistral.ai/v1" | "https://api.mistral.ai" => {
            if let Ok(key) = std::env::var("MISTRAL_API_KEY") {
                builder.header("Authorization", format!("Bearer {}", key))
            } else {
                builder
            }
        }
        _ => builder,
    }
}

fn is_sse_response(response: &RecordedResponse) -> bool {
    // Check if the body looks like SSE (starts with "data:")
    response.body.starts_with("data:")
        || response.body.starts_with("event:")
        || response.headers.get("content-type")
            .map(|ct| ct.contains("text/event-stream"))
            .unwrap_or(false)
}

async fn simulate_stream_response(body: String, options: ProxyOptions) -> axum::response::Response<axum::body::Body> {
    use axum::body::Body;
    use axum::response::Response;
    use tokio_stream::StreamExt;

    // Split into lines and add delays
    let lines: Vec<String> = body.lines().map(|l| format!("{}\n", l)).collect();
    let delay = options.stream_chunk_delay;

    let stream = tokio_stream::iter(lines).then(move |line| {
        let delay = delay;
        async move {
            if line.starts_with("data:") {
                tokio::time::sleep(delay).await;
            }
            Ok::<_, std::convert::Infallible>(line)
        }
    });

    Response::builder()
        .status(200)
        .header("content-type", "text/event-stream")
        .body(Body::from_stream(stream))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_request_body() {
        let body = r#"{"tool_calls":[{"id":"call_abc123def456"}]}"#;
        let normalized = normalize_request_body(body);
        assert!(normalized.contains("call_ID"));
        assert!(!normalized.contains("call_abc123def456"));
    }

    #[test]
    fn test_cassette_roundtrip() {
        let tmp_dir = tempfile::tempdir().unwrap();
        let path = tmp_dir.path().join("test.yaml");

        let mut cassette = Cassette::new(&path);
        cassette.add_interaction(Interaction {
            request: RecordedRequest {
                method: "POST".to_string(),
                url: "https://api.openai.com/v1/chat/completions".to_string(),
                body: r#"{"model":"gpt-4"}"#.to_string(),
            },
            response: RecordedResponse {
                status: 200,
                headers: HashMap::new(),
                body: r#"{"choices":[]}"#.to_string(),
            },
        });

        cassette.save().unwrap();

        let loaded = Cassette::load(&path).unwrap();
        assert_eq!(loaded.interactions.len(), 1);
    }

    #[test]
    fn test_cassette_find_match() {
        let mut cassette = Cassette::new("/tmp/test.yaml");
        cassette.add_interaction(Interaction {
            request: RecordedRequest {
                method: "POST".to_string(),
                url: "https://api.openai.com/v1/chat/completions".to_string(),
                body: r#"{"model":"gpt-4","messages":[{"role":"user","content":"hello"}]}"#.to_string(),
            },
            response: RecordedResponse {
                status: 200,
                headers: HashMap::new(),
                body: r#"{"choices":[{"message":{"content":"Hi!"}}]}"#.to_string(),
            },
        });

        // Exact match
        let matched = cassette.find_match(
            "POST",
            "https://api.openai.com/v1/chat/completions",
            r#"{"model":"gpt-4","messages":[{"role":"user","content":"hello"}]}"#,
        );
        assert!(matched.is_some());

        // Different body
        let not_matched = cassette.find_match(
            "POST",
            "https://api.openai.com/v1/chat/completions",
            r#"{"model":"gpt-4","messages":[{"role":"user","content":"goodbye"}]}"#,
        );
        assert!(not_matched.is_none());
    }
}
