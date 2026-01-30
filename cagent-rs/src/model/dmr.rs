//! DMR (Docker Model Runner) provider implementation
//!
//! DMR provides local model inference through Docker. It exposes an OpenAI-compatible
//! API, so this provider delegates most functionality to the OpenAI provider with
//! appropriate configuration.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;
use tokio::process::Command;
use tracing::{debug, warn};

use crate::chat::Message;
use crate::tools::Tool;

use super::openai::OpenAIProvider;
use super::{MessageStream, Provider, ProviderError, ProviderOptions};

const DMR_INFERENCE_PREFIX: &str = "/engines";
const DMR_DEFAULT_PORT: &str = "12434";
const CONNECTIVITY_TIMEOUT: Duration = Duration::from_secs(2);

/// DMR provider that wraps the OpenAI provider with Docker Model Runner endpoints
#[derive(Debug)]
pub struct DmrProvider {
    inner: OpenAIProvider,
    model: String,
}

impl DmrProvider {
    /// Create a new DMR provider
    pub async fn new(model: String, options: ProviderOptions) -> Result<Self, ProviderError> {
        let base_url = resolve_dmr_base_url().await;

        debug!("DMR using base URL: {}", base_url);

        // DMR doesn't need an API key
        let inner = OpenAIProvider::new(
            "dmr".to_string(),
            String::new(),
            model.clone(),
            options,
            Some(base_url),
        );

        Ok(Self { inner, model })
    }

    /// Check if DMR is available by testing connectivity
    pub async fn is_available() -> bool {
        let base_url = resolve_dmr_base_url().await;
        test_dmr_connectivity(&base_url).await
    }
}

#[async_trait]
impl Provider for DmrProvider {
    fn id(&self) -> String {
        format!("dmr/{}", self.model)
    }

    fn context_limit(&self) -> Option<i64> {
        // DMR models typically have configurable context sizes
        // Return None to use provider defaults
        None
    }

    async fn create_chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        self.inner
            .create_chat_completion_stream(messages, tools)
            .await
    }
}

/// Resolve the DMR base URL by checking various sources
async fn resolve_dmr_base_url() -> String {
    // Check for explicit MODEL_RUNNER_HOST environment variable
    if let Ok(host) = std::env::var("MODEL_RUNNER_HOST") {
        let trimmed = host.trim_end_matches('/');
        let url = format!("{}{}/v1/", trimmed, DMR_INFERENCE_PREFIX);
        debug!("DMR using MODEL_RUNNER_HOST: {}", url);
        return url;
    }

    // Try to get endpoint from `docker model status --json`
    if let Some(endpoint) = get_docker_model_endpoint().await {
        debug!("DMR using endpoint from docker model status: {}", endpoint);
        return endpoint;
    }

    // Check if we're in a container
    let in_container = is_in_container();

    // Try fallback URLs based on environment
    let fallback_urls = get_dmr_fallback_urls(in_container);

    for url in &fallback_urls {
        if test_dmr_connectivity(url).await {
            debug!("DMR using fallback URL: {}", url);
            return url.clone();
        }
    }

    // Default to localhost
    let default_url = format!(
        "http://127.0.0.1:{}{}/v1/",
        DMR_DEFAULT_PORT, DMR_INFERENCE_PREFIX
    );
    debug!("DMR using default URL: {}", default_url);
    default_url
}

/// Check if we're running inside a Docker container
fn is_in_container() -> bool {
    Path::new("/.dockerenv").exists()
}

/// Get fallback URLs to try for DMR connectivity
fn get_dmr_fallback_urls(in_container: bool) -> Vec<String> {
    if in_container {
        vec![
            format!(
                "http://model-runner.docker.internal{}/v1/",
                DMR_INFERENCE_PREFIX
            ),
            format!(
                "http://host.docker.internal:{}{}/v1/",
                DMR_DEFAULT_PORT, DMR_INFERENCE_PREFIX
            ),
            format!(
                "http://172.17.0.1:{}{}/v1/",
                DMR_DEFAULT_PORT, DMR_INFERENCE_PREFIX
            ),
        ]
    } else {
        vec![format!(
            "http://127.0.0.1:{}{}/v1/",
            DMR_DEFAULT_PORT, DMR_INFERENCE_PREFIX
        )]
    }
}

/// Test connectivity to a DMR endpoint
async fn test_dmr_connectivity(base_url: &str) -> bool {
    let health_url = format!(
        "{}models",
        base_url.trim_end_matches('/').trim_end_matches("v1/")
    );

    let client = reqwest::Client::builder()
        .timeout(CONNECTIVITY_TIMEOUT)
        .build()
        .unwrap_or_default();

    match client.get(&health_url).send().await {
        Ok(resp) => {
            debug!(
                "DMR connectivity check success: {} -> {}",
                health_url,
                resp.status()
            );
            true
        }
        Err(e) => {
            debug!("DMR connectivity check failed: {} -> {}", health_url, e);
            false
        }
    }
}

/// Get the DMR endpoint from `docker model status --json`
async fn get_docker_model_endpoint() -> Option<String> {
    #[derive(Deserialize)]
    struct DockerModelStatus {
        #[serde(default)]
        running: bool,
        #[serde(default)]
        endpoint: String,
        #[serde(default)]
        #[allow(dead_code)]
        engine: String,
    }

    let output = Command::new("docker")
        .args(["model", "status", "--json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        debug!("docker model status failed");
        return None;
    }

    let status: DockerModelStatus = serde_json::from_slice(&output.stdout).ok()?;

    if !status.running {
        debug!("Docker Model Runner is not running");
        return None;
    }

    let endpoint = status.endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }

    // Handle legacy format (http://:0/engines/v1/)
    if endpoint == "http://:0/engines/v1/" {
        return Some(format!(
            "http://127.0.0.1:{}{}/v1/",
            DMR_DEFAULT_PORT, DMR_INFERENCE_PREFIX
        ));
    }

    // If the endpoint is model-runner.docker.internal and we're not in a container,
    // we need special handling (Unix socket), but for simplicity, try localhost
    if endpoint.contains("model-runner.docker.internal") && !is_in_container() {
        return Some(format!(
            "http://127.0.0.1:{}{}/v1/",
            DMR_DEFAULT_PORT, DMR_INFERENCE_PREFIX
        ));
    }

    // Normalize endpoint to have proper suffix
    let mut url = endpoint.to_string();
    if !url.ends_with('/') {
        url.push('/');
    }

    Some(url)
}

/// Pull a Docker model if needed (interactive only)
pub async fn pull_model_if_needed(model: &str) -> Result<(), ProviderError> {
    // Check if model exists
    let output = Command::new("docker")
        .args(["model", "inspect", model])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .await
        .map_err(|e| ProviderError::Config(format!("Failed to check model: {}", e)))?;

    if output.status.success() {
        debug!("Model {} already exists", model);
        return Ok(());
    }

    // Model doesn't exist - in non-interactive mode, just warn
    // In interactive mode, the TUI would handle prompting the user
    warn!(
        "Model {} not found locally. Pull it with: docker model pull {}",
        model, model
    );

    Err(ProviderError::Config(format!(
        "Model {} not found. Pull it with: docker model pull {}",
        model, model
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_in_container() {
        // This test will return false on most dev machines
        let _ = is_in_container();
    }

    #[test]
    fn test_get_dmr_fallback_urls() {
        let urls_container = get_dmr_fallback_urls(true);
        assert_eq!(urls_container.len(), 3);
        assert!(urls_container[0].contains("model-runner.docker.internal"));

        let urls_host = get_dmr_fallback_urls(false);
        assert_eq!(urls_host.len(), 1);
        assert!(urls_host[0].contains("127.0.0.1"));
    }
}
