//! Rate limiting utilities for model providers

use std::time::Duration;
use tracing::{debug, warn};

use crate::chat::RateLimit;

/// Parse rate limit information from HTTP headers
///
/// Supports common header formats:
/// - `X-RateLimit-Limit` / `X-RateLimit-Remaining` / `X-RateLimit-Reset` (OpenAI/standard)
/// - `Retry-After` (HTTP standard for 429 responses)
/// - `anthropic-ratelimit-*` headers
pub fn parse_rate_limit_headers(headers: &reqwest::header::HeaderMap) -> Option<RateLimit> {
    // Try OpenAI-style headers first
    let limit = parse_header_i64(headers, "x-ratelimit-limit-requests")
        .or_else(|| parse_header_i64(headers, "x-ratelimit-limit"));

    let remaining = parse_header_i64(headers, "x-ratelimit-remaining-requests")
        .or_else(|| parse_header_i64(headers, "x-ratelimit-remaining"));

    let reset = parse_header_i64(headers, "x-ratelimit-reset-requests")
        .or_else(|| parse_header_i64(headers, "x-ratelimit-reset"));

    let retry_after = parse_header_i64(headers, "retry-after");

    if limit.is_some() || remaining.is_some() || reset.is_some() || retry_after.is_some() {
        Some(RateLimit {
            limit: limit.unwrap_or(0),
            remaining: remaining.unwrap_or(0),
            reset: reset.unwrap_or(0),
            retry_after,
        })
    } else {
        None
    }
}

fn parse_header_i64(headers: &reqwest::header::HeaderMap, key: &str) -> Option<i64> {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

/// Retry configuration for rate limiting
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: u32,
    /// Base delay for exponential backoff
    pub base_delay: Duration,
    /// Maximum delay between retries
    pub max_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
        }
    }
}

/// Calculate delay for retry attempt using exponential backoff
pub fn calculate_retry_delay(
    attempt: u32,
    config: &RetryConfig,
    retry_after: Option<i64>,
) -> Duration {
    // If server specified retry-after, use that
    if let Some(seconds) = retry_after {
        let duration = Duration::from_secs(seconds.max(0) as u64);
        debug!("Using server-provided retry-after: {:?}", duration);
        return duration.min(config.max_delay);
    }

    // Otherwise use exponential backoff: base * 2^attempt
    let backoff = config.base_delay * 2u32.pow(attempt);
    let delay = backoff.min(config.max_delay);
    debug!(
        "Using exponential backoff: attempt={}, delay={:?}",
        attempt, delay
    );
    delay
}

/// Check if an error is retryable (rate limit or transient error)
pub fn is_retryable_error(status: u16) -> bool {
    match status {
        429 => true, // Too Many Requests
        500 => true, // Internal Server Error
        502 => true, // Bad Gateway
        503 => true, // Service Unavailable
        504 => true, // Gateway Timeout
        _ => false,
    }
}

/// Rate limit error with retry information
#[derive(Debug)]
pub struct RateLimitError {
    pub status: u16,
    pub message: String,
    pub retry_after: Option<i64>,
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rate limit error ({}): {}", self.status, self.message)?;
        if let Some(retry_after) = self.retry_after {
            write!(f, " (retry after {} seconds)", retry_after)?;
        }
        Ok(())
    }
}

impl std::error::Error for RateLimitError {}

/// Async retry helper for HTTP requests
///
/// Returns Ok(response) on success, or the last error after all retries exhausted
pub async fn retry_with_backoff<F, Fut, T, E>(
    config: &RetryConfig,
    mut make_request: F,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut last_error = None;

    for attempt in 0..=config.max_retries {
        match make_request().await {
            Ok(response) => return Ok(response),
            Err(e) => {
                warn!(
                    "Request failed (attempt {}/{}): {:?}",
                    attempt + 1,
                    config.max_retries + 1,
                    e
                );

                if attempt < config.max_retries {
                    let delay = calculate_retry_delay(attempt, config, None);
                    debug!("Waiting {:?} before retry", delay);
                    tokio::time::sleep(delay).await;
                }

                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn test_parse_rate_limit_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-limit", HeaderValue::from_static("100"));
        headers.insert("x-ratelimit-remaining", HeaderValue::from_static("50"));
        headers.insert("x-ratelimit-reset", HeaderValue::from_static("1234567890"));

        let rate_limit = parse_rate_limit_headers(&headers).unwrap();
        assert_eq!(rate_limit.limit, 100);
        assert_eq!(rate_limit.remaining, 50);
        assert_eq!(rate_limit.reset, 1234567890);
        assert!(rate_limit.retry_after.is_none());
    }

    #[test]
    fn test_parse_rate_limit_with_retry_after() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("30"));

        let rate_limit = parse_rate_limit_headers(&headers).unwrap();
        assert_eq!(rate_limit.retry_after, Some(30));
    }

    #[test]
    fn test_parse_empty_headers() {
        let headers = HeaderMap::new();
        assert!(parse_rate_limit_headers(&headers).is_none());
    }

    #[test]
    fn test_calculate_retry_delay() {
        let config = RetryConfig::default();

        // First attempt: 1 second
        let delay = calculate_retry_delay(0, &config, None);
        assert_eq!(delay, Duration::from_secs(1));

        // Second attempt: 2 seconds
        let delay = calculate_retry_delay(1, &config, None);
        assert_eq!(delay, Duration::from_secs(2));

        // Third attempt: 4 seconds
        let delay = calculate_retry_delay(2, &config, None);
        assert_eq!(delay, Duration::from_secs(4));

        // With retry-after: use server value
        let delay = calculate_retry_delay(0, &config, Some(30));
        assert_eq!(delay, Duration::from_secs(30));

        // Retry-after exceeds max: cap at max
        let delay = calculate_retry_delay(0, &config, Some(120));
        assert_eq!(delay, Duration::from_secs(60)); // max_delay
    }

    #[test]
    fn test_is_retryable_error() {
        assert!(is_retryable_error(429));
        assert!(is_retryable_error(500));
        assert!(is_retryable_error(502));
        assert!(is_retryable_error(503));
        assert!(is_retryable_error(504));
        assert!(!is_retryable_error(400));
        assert!(!is_retryable_error(401));
        assert!(!is_retryable_error(404));
    }
}
