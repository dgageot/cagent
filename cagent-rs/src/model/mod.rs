//! Model provider abstraction and implementations

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use crate::chat::{Message, StreamResponse};
use crate::tools::Tool;

pub mod anthropic;
pub mod bedrock;
pub mod dmr;
pub mod gemini;
pub mod mock;
pub mod openai;
pub mod rate_limit;

/// Provider error types
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("API error: {status} - {message}")]
    Api { status: u16, message: String },

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Missing API key for provider: {0}")]
    MissingApiKey(String),

    #[error("Unknown provider: {0}")]
    UnknownProvider(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

/// A streamed chat completion
pub type MessageStream = Pin<Box<dyn Stream<Item = Result<StreamResponse, ProviderError>> + Send>>;

/// Model provider trait
#[async_trait]
pub trait Provider: Send + Sync + std::fmt::Debug {
    /// Get the provider ID (e.g., "openai/gpt-4o")
    fn id(&self) -> String;

    /// Get the context window limit in tokens (if known)
    fn context_limit(&self) -> Option<i64> {
        None
    }

    /// Create a streaming chat completion
    async fn create_chat_completion_stream(
        &self,
        messages: Vec<Message>,
        tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError>;
}

/// Provider options for configuration
#[derive(Debug, Clone, Default)]
pub struct ProviderOptions {
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub thinking_budget: Option<u32>,
    pub parallel_tool_calls: Option<bool>,

    // OpenAI-compatible sampling options
    pub top_p: Option<f32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,

    // Override API endpoint
    pub base_url: Option<String>,
    // Override API token environment variable name
    pub token_key: Option<String>,

    /// Provider-specific options not covered by standard fields
    /// Examples for DMR:
    /// - "runtime_flags": ["-c", "2G"]
    /// - "speculative_draft_model": "ai/draft-model"
    /// - "speculative_num_tokens": 5
    pub provider_opts: std::collections::HashMap<String, serde_json::Value>,
}

/// Built-in provider aliases
/// These use OpenAI-compatible APIs with different base URLs and token env vars
#[allow(dead_code)]
struct ProviderAlias {
    api_type: &'static str,
    base_url: &'static str,
    token_env_var: &'static str,
}

fn get_provider_alias(name: &str) -> Option<ProviderAlias> {
    match name {
        "mistral" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "https://api.mistral.ai/v1",
            token_env_var: "MISTRAL_API_KEY",
        }),
        "xai" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "https://api.x.ai/v1",
            token_env_var: "XAI_API_KEY",
        }),
        "nebius" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "https://api.studio.nebius.ai/v1",
            token_env_var: "NEBIUS_API_KEY",
        }),
        "ollama" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "http://localhost:11434/v1",
            token_env_var: "", // Ollama doesn't need an API key
        }),
        "groq" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "https://api.groq.com/openai/v1",
            token_env_var: "GROQ_API_KEY",
        }),
        "together" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "https://api.together.xyz/v1",
            token_env_var: "TOGETHER_API_KEY",
        }),
        "openrouter" => Some(ProviderAlias {
            api_type: "openai",
            base_url: "https://openrouter.ai/api/v1",
            token_env_var: "OPENROUTER_API_KEY",
        }),
        _ => None,
    }
}

/// Create a provider for built-in providers, or for OpenAI-compatible custom providers.
///
/// Built-in `provider_name` values: openai, anthropic, google/gemini.
///
/// For unknown `provider_name`, `provider_cfg` must be provided and describes
/// an OpenAI-compatible endpoint.
///
/// The `options.base_url` and `options.token_key` fields from `ProviderOptions`
/// take precedence over `provider_cfg` settings.
pub fn create_provider_from_parts(
    provider_name: &str,
    model_name: &str,
    options: ProviderOptions,
    provider_cfg: Option<&crate::config::ProviderConfig>,
) -> Result<Arc<dyn Provider>, ProviderError> {
    match provider_name {
        "openai" => {
            // token_key override: use options.token_key if set
            let token_key = options
                .token_key
                .clone()
                .unwrap_or_else(|| "OPENAI_API_KEY".to_string());
            let api_key = std::env::var(&token_key)
                .map_err(|_| ProviderError::MissingApiKey("openai".to_string()))?;
            Ok(Arc::new(openai::OpenAIProvider::new(
                "openai".to_string(),
                api_key,
                model_name.to_string(),
                options.clone(),
                options.base_url.clone(),
            )))
        }
        "anthropic" => {
            let token_key = options
                .token_key
                .clone()
                .unwrap_or_else(|| "ANTHROPIC_API_KEY".to_string());
            let api_key = std::env::var(&token_key)
                .map_err(|_| ProviderError::MissingApiKey("anthropic".to_string()))?;
            Ok(Arc::new(anthropic::AnthropicProvider::new(
                api_key,
                model_name.to_string(),
                options,
            )))
        }
        "google" | "gemini" => {
            let token_key = options
                .token_key
                .clone()
                .unwrap_or_else(|| "GOOGLE_API_KEY".to_string());
            let api_key = std::env::var(&token_key)
                .or_else(|_| std::env::var("GEMINI_API_KEY"))
                .map_err(|_| ProviderError::MissingApiKey("google".to_string()))?;
            Ok(Arc::new(gemini::GeminiProvider::new(
                api_key,
                model_name.to_string(),
                options,
            )))
        }
        "dmr" => {
            // DMR is async, so we need to use a blocking call here
            // In production, prefer using create_dmr_provider directly
            let rt = tokio::runtime::Handle::try_current()
                .map_err(|_| ProviderError::Config("DMR provider requires async runtime".into()))?;
            let provider = rt.block_on(dmr::DmrProvider::new(model_name.to_string(), options))?;
            Ok(Arc::new(provider))
        }
        "bedrock" => {
            let provider = bedrock::BedrockProvider::new(model_name.to_string(), options)?;
            Ok(Arc::new(provider))
        }
        other => {
            // Check for built-in alias first
            if let Some(alias) = get_provider_alias(other) {
                let token_key = options
                    .token_key
                    .clone()
                    .unwrap_or_else(|| alias.token_env_var.to_string());
                let api_key = if token_key.is_empty() {
                    // Ollama doesn't need an API key
                    String::new()
                } else {
                    std::env::var(&token_key)
                        .map_err(|_| ProviderError::MissingApiKey(other.to_string()))?
                };

                // options.base_url overrides the alias base_url
                let base_url = options
                    .base_url
                    .clone()
                    .unwrap_or_else(|| alias.base_url.to_string());

                return Ok(Arc::new(openai::OpenAIProvider::new(
                    other.to_string(),
                    api_key,
                    model_name.to_string(),
                    options,
                    Some(base_url),
                )));
            }

            // Otherwise, require a custom provider config
            let cfg =
                provider_cfg.ok_or_else(|| ProviderError::UnknownProvider(other.to_string()))?;

            if let Some(api_type) = cfg.api_type.as_deref() {
                if api_type != "openai" {
                    return Err(ProviderError::Config(format!(
                        "Unsupported provider api_type for '{}': {}",
                        other, api_type
                    )));
                }
            }

            // options fields override provider_cfg fields
            let token_key = options
                .token_key
                .clone()
                .or_else(|| cfg.token_key.clone())
                .unwrap_or_else(|| "OPENAI_API_KEY".to_string());

            let api_key = std::env::var(&token_key)
                .map_err(|_| ProviderError::MissingApiKey(other.to_string()))?;

            let base_url = options
                .base_url
                .clone()
                .unwrap_or_else(|| cfg.base_url.clone());

            Ok(Arc::new(openai::OpenAIProvider::new(
                other.to_string(),
                api_key,
                model_name.to_string(),
                options,
                Some(base_url),
            )))
        }
    }
}

/// Create a provider from a model reference like `openai/gpt-4o`.
///
/// This only supports built-in providers; for config-defined providers, prefer
/// `create_provider_from_parts`.
pub fn create_provider(
    model_ref: &str,
    options: ProviderOptions,
) -> Result<Arc<dyn Provider>, ProviderError> {
    let parts: Vec<&str> = model_ref.splitn(2, '/').collect();
    if parts.len() != 2 {
        return Err(ProviderError::Config(format!(
            "Invalid model reference: {}. Expected format: provider/model",
            model_ref
        )));
    }

    let (provider_name, model_name) = (parts[0], parts[1]);

    create_provider_from_parts(provider_name, model_name, options, None)
}

/// Configuration snapshot for a provider, used for cloning
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    /// Provider name (e.g., "openai", "anthropic")
    pub provider_name: String,
    /// Model name (e.g., "gpt-4o", "claude-sonnet-4-0")
    pub model_name: String,
    /// Provider options
    pub options: ProviderOptions,
    /// Custom provider configuration (for OpenAI-compatible endpoints)
    pub custom_config: Option<crate::config::ProviderConfig>,
}

/// Trait extension for providers that can be cloned with modified options
pub trait ClonableProvider: Provider {
    /// Get the base configuration for this provider
    fn base_config(&self) -> ProviderConfig;
}

/// Clone a provider with modified options.
///
/// Creates a new provider instance using the same provider/model as the base,
/// but with the provided options overriding the base options.
///
/// If cloning fails, returns the original provider.
pub fn clone_provider_with_options(
    base: Arc<dyn Provider>,
    new_options: ProviderOptions,
) -> Arc<dyn Provider> {
    // Try to downcast to specific provider types that implement ClonableProvider
    // For now, we'll try to re-create from the ID
    let id = base.id();
    let parts: Vec<&str> = id.splitn(2, '/').collect();
    
    if parts.len() != 2 {
        tracing::debug!("Cannot clone provider with non-standard ID: {}", id);
        return base;
    }

    let (provider_name, model_name) = (parts[0], parts[1]);

    // Merge options - new_options takes precedence
    let merged_options = merge_options(&ProviderOptions::default(), &new_options);

    match create_provider_from_parts(provider_name, model_name, merged_options, None) {
        Ok(new_provider) => new_provider,
        Err(e) => {
            tracing::debug!("Failed to clone provider {}: {}", id, e);
            base
        }
    }
}

/// Merge two ProviderOptions, with `override_opts` taking precedence
fn merge_options(base: &ProviderOptions, override_opts: &ProviderOptions) -> ProviderOptions {
    ProviderOptions {
        temperature: override_opts.temperature.or(base.temperature),
        max_tokens: override_opts.max_tokens.or(base.max_tokens),
        thinking_budget: override_opts.thinking_budget.or(base.thinking_budget),
        parallel_tool_calls: override_opts.parallel_tool_calls.or(base.parallel_tool_calls),
        top_p: override_opts.top_p.or(base.top_p),
        frequency_penalty: override_opts.frequency_penalty.or(base.frequency_penalty),
        presence_penalty: override_opts.presence_penalty.or(base.presence_penalty),
        base_url: override_opts.base_url.clone().or_else(|| base.base_url.clone()),
        token_key: override_opts.token_key.clone().or_else(|| base.token_key.clone()),
        provider_opts: {
            let mut merged = base.provider_opts.clone();
            merged.extend(override_opts.provider_opts.clone());
            merged
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merge_options_empty() {
        let base = ProviderOptions::default();
        let override_opts = ProviderOptions::default();
        let merged = merge_options(&base, &override_opts);
        
        assert!(merged.temperature.is_none());
        assert!(merged.max_tokens.is_none());
    }

    #[test]
    fn test_merge_options_override_takes_precedence() {
        let base = ProviderOptions {
            temperature: Some(0.5),
            max_tokens: Some(100),
            ..Default::default()
        };
        let override_opts = ProviderOptions {
            temperature: Some(0.9),
            ..Default::default()
        };
        let merged = merge_options(&base, &override_opts);
        
        assert_eq!(merged.temperature, Some(0.9)); // override wins
        assert_eq!(merged.max_tokens, Some(100)); // base preserved
    }

    #[test]
    fn test_merge_options_provider_opts_merged() {
        let mut base_opts = std::collections::HashMap::new();
        base_opts.insert("key1".to_string(), serde_json::json!("value1"));
        
        let mut override_opts_map = std::collections::HashMap::new();
        override_opts_map.insert("key2".to_string(), serde_json::json!("value2"));
        
        let base = ProviderOptions {
            provider_opts: base_opts,
            ..Default::default()
        };
        let override_opts = ProviderOptions {
            provider_opts: override_opts_map,
            ..Default::default()
        };
        let merged = merge_options(&base, &override_opts);
        
        assert_eq!(merged.provider_opts.len(), 2);
        assert!(merged.provider_opts.contains_key("key1"));
        assert!(merged.provider_opts.contains_key("key2"));
    }

    #[test]
    fn test_create_provider_invalid_ref() {
        let result = create_provider("invalid", ProviderOptions::default());
        assert!(result.is_err());
    }
}
