//! LLM-as-a-judge for relevance checking

use std::sync::Arc;

use anyhow::Result;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::chat::Message;
use crate::model::Provider;

/// Prompt template for the judge model
const RELEVANCE_PROMPT: &str = r#"You are an evaluation judge. Check if the response matches the given relevance criteria.

Response to evaluate:
<response>
{response}
</response>

Criteria to check:
<criteria>
{criteria}
</criteria>

Evaluate whether the response satisfies the criteria and respond with your judgment."#;

/// Structured response from the judge model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JudgeResponse {
    /// Whether the criterion passed or failed
    pub result: String,
    /// Explanation for the result
    pub reason: String,
}

/// JSON schema for structured output from the judge model
pub fn judge_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "result": {
                "type": "string",
                "enum": ["pass", "fail"],
                "description": "Whether the response satisfies the criterion"
            },
            "reason": {
                "type": "string",
                "description": "Brief explanation of why the criterion passed or failed"
            }
        },
        "required": ["result", "reason"],
        "additionalProperties": false
    })
}

/// Judge for running LLM-as-a-judge relevance checks
pub struct Judge {
    model: Arc<dyn Provider + Send + Sync>,
    concurrency: usize,
}

impl Judge {
    /// Create a new Judge with the given model and concurrency
    pub fn new(model: Arc<dyn Provider + Send + Sync>, concurrency: usize) -> Self {
        let concurrency = if concurrency < 1 { 1 } else { concurrency };
        Self { model, concurrency }
    }

    /// Check relevance of a response against multiple criteria
    ///
    /// Returns (passed_count, failed_criteria, errors)
    pub async fn check_relevance(
        &self,
        response: &str,
        criteria: &[String],
    ) -> (usize, Vec<String>, Vec<String>) {
        if criteria.is_empty() {
            return (0, vec![], vec![]);
        }

        let response = response.to_string();

        // Run checks concurrently with limited concurrency
        let results: Vec<_> = stream::iter(criteria.iter().enumerate())
            .map(|(idx, criterion)| {
                let response = response.clone();
                let criterion = criterion.clone();
                let model = Arc::clone(&self.model);
                async move {
                    let result = check_single(&model, &response, &criterion).await;
                    (idx, criterion, result)
                }
            })
            .buffer_unordered(self.concurrency)
            .collect()
            .await;

        let mut passed = 0;
        let mut failed = Vec::new();
        let mut errors = Vec::new();

        for (_idx, criterion, result) in results {
            match result {
                Ok(true) => passed += 1,
                Ok(false) => failed.push(criterion),
                Err(e) => {
                    errors.push(format!("error checking {:?}: {}", criterion, e));
                }
            }
        }

        (passed, failed, errors)
    }
}

/// Check a single relevance criterion
async fn check_single(
    model: &Arc<dyn Provider + Send + Sync>,
    response: &str,
    criterion: &str,
) -> Result<bool> {
    let prompt = RELEVANCE_PROMPT
        .replace("{response}", response)
        .replace("{criteria}", criterion);

    let messages = vec![Message::user(&prompt)];

    debug!(criterion = %criterion, "Checking relevance criterion");

    // Create the chat completion
    let mut stream = model.create_chat_completion_stream(messages, vec![]).await?;

    // Collect the full response
    let mut full_response = String::new();
    while let Some(event) = stream.next().await {
        let resp = event?;
        for choice in &resp.choices {
            full_response.push_str(&choice.delta.content);
        }
    }

    // Parse the response
    Ok(parse_judge_response(&full_response))
}

/// Parse the judge's response to determine pass/fail
fn parse_judge_response(text: &str) -> bool {
    let text = text.trim();

    // Try to parse as JSON first (structured output)
    if let Ok(response) = serde_json::from_str::<JudgeResponse>(text) {
        return response.result.eq_ignore_ascii_case("pass");
    }

    // Fallback: look for pass/fail keywords
    let lower = text.to_lowercase();
    if lower.contains("\"result\": \"pass\"") || lower.contains("\"result\":\"pass\"") {
        return true;
    }
    if lower.contains("\"result\": \"fail\"") || lower.contains("\"result\":\"fail\"") {
        return false;
    }

    // Last resort: check for pass/fail at word boundaries
    if lower.contains("pass") && !lower.contains("fail") {
        warn!(response = %text, "Could not parse judge response as JSON, using keyword fallback");
        return true;
    }

    warn!(response = %text, "Could not determine pass/fail from judge response");
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_judge_response_json() {
        let json = r#"{"result": "pass", "reason": "The response mentions Rust"}"#;
        assert!(parse_judge_response(json));

        let json = r#"{"result": "fail", "reason": "The response does not mention Rust"}"#;
        assert!(!parse_judge_response(json));
    }

    #[test]
    fn test_parse_judge_response_case_insensitive() {
        let json = r#"{"result": "PASS", "reason": "test"}"#;
        assert!(parse_judge_response(json));

        let json = r#"{"result": "Pass", "reason": "test"}"#;
        assert!(parse_judge_response(json));
    }

    #[test]
    fn test_parse_judge_response_fallback() {
        // Malformed JSON with keywords
        let text = r#"Based on my analysis, the result is "pass" because..."#;
        assert!(parse_judge_response(text));
    }

    #[test]
    fn test_judge_response_schema() {
        let schema = judge_response_schema();
        assert!(schema.get("properties").is_some());
        assert!(schema.get("required").is_some());
    }
}
