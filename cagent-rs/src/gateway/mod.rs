//! Gateway support for routing model requests through a Docker AI Gateway.
//!
//! When `CAGENT_MODELS_GATEWAY` is set, requests are routed through the gateway
//! with special headers for authentication and routing.

use std::sync::OnceLock;

use crate::desktop;
use crate::environment::DOCKER_TOKEN;

/// Environment variable for the models gateway URL
pub const CAGENT_MODELS_GATEWAY_ENV: &str = "CAGENT_MODELS_GATEWAY";

/// Global gateway configuration
static GATEWAY_CONFIG: OnceLock<Option<GatewayConfig>> = OnceLock::new();

/// Gateway configuration
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    /// Gateway URL (from CAGENT_MODELS_GATEWAY)
    pub gateway_url: String,
}

impl GatewayConfig {
    /// Get the gateway config from environment
    pub fn from_env() -> Option<Self> {
        std::env::var(CAGENT_MODELS_GATEWAY_ENV)
            .ok()
            .filter(|s| !s.is_empty())
            .map(|url| Self {
                gateway_url: url.trim_end_matches('/').to_string(),
            })
    }

    /// Get the gateway URL for a specific provider and endpoint
    pub fn get_url(&self, endpoint_path: &str) -> String {
        format!("{}{}", self.gateway_url, endpoint_path)
    }

    /// Get the Docker Desktop token for authentication
    ///
    /// First checks the DOCKER_TOKEN environment variable, then falls back to
    /// retrieving the token from Docker Desktop's backend API.
    pub fn get_auth_token() -> Option<String> {
        // First try environment variable
        if let Ok(token) = std::env::var(DOCKER_TOKEN) {
            if !token.is_empty() {
                return Some(token);
            }
        }

        // Fall back to Docker Desktop
        desktop::get_token_blocking()
    }

    /// Check if gateway is available (has token)
    pub fn is_available() -> bool {
        Self::get_auth_token().is_some()
    }
}

/// Get the global gateway configuration
pub fn get_gateway_config() -> Option<&'static GatewayConfig> {
    GATEWAY_CONFIG.get_or_init(GatewayConfig::from_env).as_ref()
}

/// Check if a gateway is configured
pub fn is_gateway_configured() -> bool {
    get_gateway_config().is_some()
}

/// HTTP header names used by the gateway
pub mod headers {
    /// Header for forwarding requests to the original base URL
    pub const X_CAGENT_FORWARD: &str = "X-Cagent-Forward";
    /// Header for the provider name
    pub const X_CAGENT_PROVIDER: &str = "X-Cagent-Provider";
    /// Header for the model name
    pub const X_CAGENT_MODEL: &str = "X-Cagent-Model";
    /// Header indicating title generation (for analytics)
    pub const X_CAGENT_GENERATING_TITLE: &str = "X-Cagent-GeneratingTitle";
    /// Header for language
    pub const X_CAGENT_LANG: &str = "X-Cagent-Lang";
    /// Header for OS
    pub const X_CAGENT_OS: &str = "X-Cagent-OS";
    /// Header for architecture
    pub const X_CAGENT_ARCH: &str = "X-Cagent-Arch";
    /// Header for runtime
    pub const X_CAGENT_RUNTIME: &str = "X-Cagent-Runtime";
    /// Header for runtime version
    pub const X_CAGENT_RUNTIME_VERSION: &str = "X-Cagent-Runtime-Version";
}

/// Create headers for gateway requests
///
/// These headers are used by the Docker AI Gateway to route requests
/// to the appropriate provider.
pub fn create_gateway_headers(
    provider: &str,
    model: &str,
    original_base_url: &str,
) -> Vec<(String, String)> {
    vec![
        (
            headers::X_CAGENT_FORWARD.to_string(),
            original_base_url.to_string(),
        ),
        (headers::X_CAGENT_PROVIDER.to_string(), provider.to_string()),
        (headers::X_CAGENT_MODEL.to_string(), model.to_string()),
        (headers::X_CAGENT_LANG.to_string(), "rust".to_string()),
        (headers::X_CAGENT_OS.to_string(), std::env::consts::OS.to_string()),
        (headers::X_CAGENT_ARCH.to_string(), std::env::consts::ARCH.to_string()),
        (headers::X_CAGENT_RUNTIME.to_string(), "cagent".to_string()),
        (
            headers::X_CAGENT_RUNTIME_VERSION.to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gateway_url_formatting() {
        let config = GatewayConfig {
            gateway_url: "https://gateway.example.com".to_string(),
        };
        assert_eq!(
            config.get_url("/v1/chat/completions"),
            "https://gateway.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn test_create_gateway_headers() {
        let headers = create_gateway_headers("openai", "gpt-4o", "https://api.openai.com/v1");
        assert_eq!(headers.len(), 8);

        let header_map: std::collections::HashMap<_, _> = headers.into_iter().collect();
        assert_eq!(
            header_map.get(headers::X_CAGENT_FORWARD),
            Some(&"https://api.openai.com/v1".to_string())
        );
        assert_eq!(
            header_map.get(headers::X_CAGENT_PROVIDER),
            Some(&"openai".to_string())
        );
        assert_eq!(
            header_map.get(headers::X_CAGENT_MODEL),
            Some(&"gpt-4o".to_string())
        );
        assert_eq!(
            header_map.get(headers::X_CAGENT_LANG),
            Some(&"rust".to_string())
        );
        assert_eq!(
            header_map.get(headers::X_CAGENT_RUNTIME),
            Some(&"cagent".to_string())
        );
        // OS and ARCH depend on the build platform, so just check they exist
        assert!(header_map.contains_key(headers::X_CAGENT_OS));
        assert!(header_map.contains_key(headers::X_CAGENT_ARCH));
        assert!(header_map.contains_key(headers::X_CAGENT_RUNTIME_VERSION));
    }
}
