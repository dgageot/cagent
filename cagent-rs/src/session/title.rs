//! Session title generation using one-shot LLM calls
//!
//! This module provides automatic session title generation based on user messages.
//! It uses a simple prompt to generate concise, descriptive titles.

use std::sync::Arc;

use futures::StreamExt;
use tracing::{debug, error};

use crate::chat::Message;
use crate::model::Provider;

const SYSTEM_PROMPT: &str = "You are a helpful AI assistant that generates concise, descriptive titles for conversations. You will be given up to 2 recent user messages and asked to create a single-line title that captures the main topic. Never use newlines or line breaks in your response.";

const USER_PROMPT_FORMAT: &str = "Based on the following recent user messages from a conversation with an AI assistant, generate a short, descriptive title (maximum 50 characters) that captures the main topic or purpose of the conversation. Return ONLY the title text on a single line, nothing else. Do not include any newlines, explanations, or formatting.

Recent user messages:
{}

";

/// Generator for session titles using one-shot LLM completion
pub struct TitleGenerator {
    model: Arc<dyn Provider>,
}

impl TitleGenerator {
    /// Create a new title generator with the given model provider
    pub fn new(model: Arc<dyn Provider>) -> Self {
        Self { model }
    }

    /// Generate a title for a session based on user messages
    ///
    /// Performs a one-shot LLM call to generate a concise title.
    /// Returns None if generation fails or no messages are provided.
    pub async fn generate(
        &self,
        session_id: &str,
        user_messages: &[String],
    ) -> Option<String> {
        if user_messages.is_empty() {
            return None;
        }

        debug!(
            session_id = session_id,
            message_count = user_messages.len(),
            "Generating title for session"
        );

        // Format messages for the prompt
        let formatted_messages: String = user_messages
            .iter()
            .enumerate()
            .map(|(i, msg)| format!("{}. {}", i + 1, msg))
            .collect::<Vec<_>>()
            .join("\n");

        let user_prompt = USER_PROMPT_FORMAT.replace("{}", &formatted_messages);

        // Build messages for completion
        let messages = vec![
            Message::system(SYSTEM_PROMPT),
            Message::user(&user_prompt),
        ];

        // Call the provider (no tools needed for title generation)
        let stream = match self.model.create_chat_completion_stream(messages, vec![]).await {
            Ok(s) => s,
            Err(e) => {
                error!(
                    session_id = session_id,
                    error = %e,
                    "Failed to create title generation stream"
                );
                return None;
            }
        };

        // Collect the response
        let mut title = String::new();
        let mut stream = std::pin::pin!(stream);

        while let Some(result) = stream.next().await {
            match result {
                Ok(response) => {
                    for choice in response.choices {
                        title.push_str(&choice.delta.content);
                    }
                }
                Err(e) => {
                    error!(
                        session_id = session_id,
                        error = %e,
                        "Error receiving from title stream"
                    );
                    return None;
                }
            }
        }

        let result = sanitize_title(&title);
        if result.is_empty() {
            return None;
        }

        debug!(
            session_id = session_id,
            title = result,
            "Generated session title"
        );

        Some(result)
    }
}

/// Sanitize a title by taking only the first non-empty line
/// and removing control characters
fn sanitize_title(title: &str) -> String {
    for line in title.lines() {
        let line = line.trim();
        if !line.is_empty() {
            // Remove carriage returns and other control characters
            return line.replace('\r', "").replace('\t', " ");
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_title_single_line() {
        assert_eq!(sanitize_title("Hello World"), "Hello World");
    }

    #[test]
    fn test_sanitize_title_with_whitespace() {
        assert_eq!(sanitize_title("  Hello World  "), "Hello World");
    }

    #[test]
    fn test_sanitize_title_multiline() {
        assert_eq!(
            sanitize_title("First Line\nSecond Line\nThird Line"),
            "First Line"
        );
    }

    #[test]
    fn test_sanitize_title_with_carriage_return() {
        assert_eq!(sanitize_title("Hello\r World"), "Hello World");
    }

    #[test]
    fn test_sanitize_title_empty_first_lines() {
        assert_eq!(
            sanitize_title("\n\n  \n  Actual Title\nExtra"),
            "Actual Title"
        );
    }

    #[test]
    fn test_sanitize_title_empty() {
        assert_eq!(sanitize_title(""), "");
        assert_eq!(sanitize_title("  \n  \n  "), "");
    }
}
