//! Types for the evaluation framework

use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Evaluation criteria for a test case
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvalCriteria {
    /// Statements that should be true about the response
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relevance: Vec<String>,
    /// Subdirectory under evals/working_dirs/
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    /// Expected response size: S, M, L, XL
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
}

/// Session item from evaluation JSON
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallItem>,
}

/// Tool call within a message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallItem {
    pub function: ToolCallFunction,
}

/// Tool call function details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
}

/// Session item (can be message or sub-session)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SessionItem {
    Message {
        message: SessionMessage,
    },
    SubSession {
        sub_session: Box<EvalSessionInner>,
    },
}

/// Inner session structure
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvalSessionInner {
    #[serde(default)]
    pub messages: Vec<SessionItem>,
}

/// Evaluation session with criteria
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSession {
    /// Title of the evaluation
    #[serde(default)]
    pub title: String,
    /// Evaluation criteria
    #[serde(default)]
    pub evals: EvalCriteria,
    /// Session messages
    #[serde(default)]
    pub messages: Vec<SessionItem>,
    /// Source path (not serialized, set at load time)
    #[serde(skip)]
    pub source_path: String,
}

impl EvalSession {
    /// Get the first user message from the session
    pub fn get_first_user_message(&self) -> Option<String> {
        find_first_user_message(&self.messages)
    }

    /// Extract all tool call names from the session
    pub fn extract_tool_calls(&self) -> Vec<String> {
        extract_tool_calls_from_items(&self.messages)
    }

    /// Estimate duration based on content (used for sorting)
    pub fn estimated_duration(&self) -> Duration {
        // Estimate based on number of relevance checks and content size
        let relevance_count = self.evals.relevance.len();
        let message_count = self.messages.len();
        Duration::from_secs((relevance_count * 5 + message_count * 2) as u64)
    }
}

fn find_first_user_message(items: &[SessionItem]) -> Option<String> {
    for item in items {
        match item {
            SessionItem::Message { message } if message.role == "user" => {
                return Some(message.content.clone());
            }
            SessionItem::SubSession { sub_session } => {
                if let Some(msg) = find_first_user_message(&sub_session.messages) {
                    return Some(msg);
                }
            }
            _ => {}
        }
    }
    None
}

fn extract_tool_calls_from_items(items: &[SessionItem]) -> Vec<String> {
    let mut names = Vec::new();
    for item in items {
        match item {
            SessionItem::Message { message } => {
                for tc in &message.tool_calls {
                    names.push(tc.function.name.clone());
                }
            }
            SessionItem::SubSession { sub_session } => {
                names.extend(extract_tool_calls_from_items(&sub_session.messages));
            }
        }
    }
    names
}

/// Result of a single evaluation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Result {
    /// Path to the input evaluation file
    pub input_path: String,
    /// Title of the evaluation
    pub title: String,
    /// The question/prompt that was evaluated
    pub question: String,
    /// The agent's response
    pub response: String,
    /// Cost in dollars
    pub cost: f64,
    /// Number of output tokens
    pub output_tokens: i64,
    /// Actual response size classification
    pub size: String,
    /// Expected response size
    pub size_expected: String,
    /// F1 score for tool calls
    pub tool_calls_score: f64,
    /// Expected tool calls score (1.0 if any expected)
    pub tool_calls_score_expected: f64,
    /// Whether handoffs matched
    pub handoffs: bool,
    /// Number of relevance checks that passed
    pub relevance: f64,
    /// Expected number of relevance checks
    pub relevance_expected: f64,
    /// Relevance criteria that failed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failed_relevance: Vec<String>,
    /// Error message if evaluation failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Raw output events from the container
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raw_output: Vec<HashMap<String, serde_json::Value>>,
}

impl Result {
    /// Check results and return successes and failures
    pub fn check_results(&self) -> (Vec<String>, Vec<String>) {
        let mut successes = Vec::new();
        let mut failures = Vec::new();

        if let Some(err) = &self.error {
            return (vec![], vec![err.clone()]);
        }

        // Check size
        if !self.size_expected.is_empty() {
            if self.size_expected == self.size {
                successes.push(format!("size {}", self.size));
            } else {
                failures.push(format!(
                    "size expected {}, got {}",
                    self.size_expected, self.size
                ));
            }
        }

        // Check tool calls
        if self.tool_calls_score_expected > 0.0 {
            if self.tool_calls_score >= 1.0 {
                successes.push("tool calls".to_string());
            } else {
                failures.push(format!("tool calls score {:.2}", self.tool_calls_score));
            }
        }

        // Check handoffs
        if self.handoffs {
            successes.push("handoffs".to_string());
        } else {
            failures.push("handoffs mismatch".to_string());
        }

        // Check relevance
        if self.relevance_expected > 0.0 {
            if self.relevance >= self.relevance_expected {
                successes.push(format!(
                    "relevance {:.0}/{:.0}",
                    self.relevance, self.relevance_expected
                ));
            } else {
                for criterion in &self.failed_relevance {
                    failures.push(format!("relevance: {}", criterion));
                }
            }
        }

        (successes, failures)
    }
}

/// Aggregate statistics across all evaluations
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub total_evals: usize,
    pub failed_evals: usize,
    pub total_cost: f64,
    pub sizes_passed: usize,
    pub sizes_total: usize,
    pub tools_passed: f64,
    pub tools_total: f64,
    pub handoffs_passed: usize,
    pub handoffs_total: usize,
    pub relevance_passed: f64,
    pub relevance_total: f64,
}

/// Results and metadata for an evaluation run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    pub name: String,
    pub timestamp: DateTime<Utc>,
    #[serde(with = "duration_serde")]
    pub duration: Duration,
    pub results: Vec<Result>,
    pub summary: Summary,
}

/// Configuration for evaluation runs
#[derive(Debug, Clone)]
pub struct Config {
    /// Path to the agent configuration file
    pub agent_filename: String,
    /// Directory containing evaluation files
    pub evals_dir: String,
    /// Model for relevance checking (format: provider/model, optional)
    pub judge_model: Option<String>,
    /// Number of concurrent runs (0 = number of CPUs)
    pub concurrency: usize,
    /// Only run evaluations matching these patterns
    pub only: Vec<String>,
    /// Custom base Docker image for running evaluations
    pub base_image: Option<String>,
    /// If true, don't remove containers after evaluation
    pub keep_containers: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            agent_filename: String::new(),
            evals_dir: String::new(),
            judge_model: None,
            concurrency: 0,
            only: Vec::new(),
            base_image: None,
            keep_containers: false,
        }
    }
}

/// Custom serialization for Duration
mod duration_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::time::Duration;

    pub fn serialize<S>(duration: &Duration, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        duration.as_secs_f64().serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let secs = f64::deserialize(deserializer)?;
        Ok(Duration::from_secs_f64(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_session_deserialize() {
        let json = r#"{
            "title": "Test Eval",
            "evals": {
                "relevance": ["The response should mention Rust"],
                "size": "M"
            },
            "messages": [
                {
                    "message": {
                        "role": "user",
                        "content": "What is Rust?"
                    }
                }
            ]
        }"#;

        let session: EvalSession = serde_json::from_str(json).unwrap();
        assert_eq!(session.title, "Test Eval");
        assert_eq!(session.evals.relevance.len(), 1);
        assert_eq!(session.get_first_user_message(), Some("What is Rust?".to_string()));
    }

    #[test]
    fn test_result_check_results() {
        let result = Result {
            size_expected: "M".to_string(),
            size: "M".to_string(),
            tool_calls_score_expected: 1.0,
            tool_calls_score: 1.0,
            handoffs: true,
            relevance_expected: 2.0,
            relevance: 2.0,
            ..Default::default()
        };

        let (successes, failures) = result.check_results();
        assert_eq!(successes.len(), 4);
        assert!(failures.is_empty());
    }

    #[test]
    fn test_result_check_results_with_failures() {
        let result = Result {
            size_expected: "M".to_string(),
            size: "L".to_string(),
            tool_calls_score_expected: 1.0,
            tool_calls_score: 0.5,
            handoffs: false,
            relevance_expected: 2.0,
            relevance: 1.0,
            failed_relevance: vec!["criterion 1".to_string()],
            ..Default::default()
        };

        let (successes, failures) = result.check_results();
        assert!(successes.is_empty());
        assert_eq!(failures.len(), 4);
    }
}
