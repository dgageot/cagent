//! Save sessions as evaluation files

use std::path::Path;

use crate::chat::MessageRole;
use crate::session::{Session, SessionItem as SessionSessionItem, SessionMessage as SessionSessionMessage};

use super::types::{
    EvalCriteria, EvalSession, EvalSessionInner, SessionItem, SessionMessage, ToolCallFunction,
    ToolCallItem,
};

/// Convert a crate::session::Session to an EvalSession for saving
impl From<&Session> for EvalSession {
    fn from(session: &Session) -> Self {
        EvalSession {
            title: session.title.clone(),
            evals: EvalCriteria::default(),
            messages: convert_session_items(&session.messages),
            source_path: String::new(),
        }
    }
}

fn convert_session_items(items: &[SessionSessionItem]) -> Vec<SessionItem> {
    items
        .iter()
        .filter_map(|item| match item {
            SessionSessionItem::Message { message } => {
                Some(SessionItem::Message {
                    message: convert_message(message),
                })
            }
            SessionSessionItem::SubSession { sub_session } => {
                Some(SessionItem::SubSession {
                    sub_session: Box::new(EvalSessionInner {
                        messages: convert_session_items(&sub_session.messages),
                    }),
                })
            }
            SessionSessionItem::Summary { .. } => None, // Skip summaries in eval output
        })
        .collect()
}

fn convert_message(msg: &SessionSessionMessage) -> SessionMessage {
    let role = match msg.message.role {
        MessageRole::System => "system",
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
    };

    let tool_calls: Vec<ToolCallItem> = msg
        .message
        .tool_calls
        .iter()
        .map(|tc| ToolCallItem {
            function: ToolCallFunction {
                name: tc.function.name.clone(),
            },
        })
        .collect();

    SessionMessage {
        role: role.to_string(),
        content: msg.message.content.clone(),
        tool_calls,
    }
}

/// Save a session as an evaluation JSON file
///
/// Returns the path to the saved file
pub async fn save_eval(session: &Session, dir: impl AsRef<Path>) -> anyhow::Result<String> {
    let dir = dir.as_ref();

    // Create evals directory if it doesn't exist
    tokio::fs::create_dir_all(dir).await?;

    // Generate a unique filename based on title and timestamp
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let title_slug = slugify(&session.title);
    let filename = if title_slug.is_empty() {
        format!("eval_{}.json", timestamp)
    } else {
        format!("{}_{}.json", title_slug, timestamp)
    };

    let path = dir.join(&filename);

    // Convert and save
    let eval_session = EvalSession::from(session);
    let json = serde_json::to_string_pretty(&eval_session)?;
    tokio::fs::write(&path, json).await?;

    Ok(path.display().to_string())
}

/// Convert a string to a URL-safe slug
fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
        .chars()
        .take(50) // Limit length
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Message;
    use crate::session::SessionMessage as SM;
    use crate::tools::{FunctionCall, ToolCall};

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Hello World!"), "hello_world");
        assert_eq!(slugify("Test   Session"), "test_session");
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("123-abc"), "123_abc");
    }

    #[test]
    fn test_convert_session() {
        let mut session = Session::new();
        session.title = "Test Session".to_string();

        // Add a user message
        session.add_message(SM::user("Hello"));

        // Add an assistant message with tool calls
        let mut assistant_msg = Message::assistant("Let me help you");
        assistant_msg.tool_calls = vec![ToolCall {
            id: "call_123".to_string(),
            call_type: Default::default(),
            function: FunctionCall {
                name: "read_file".to_string(),
                arguments: "{}".to_string(),
            },
        }];
        session.add_message(SM::new(None, assistant_msg));

        let eval: EvalSession = (&session).into();

        assert_eq!(eval.title, "Test Session");
        assert_eq!(eval.messages.len(), 2);

        // Check first message
        if let SessionItem::Message { message } = &eval.messages[0] {
            assert_eq!(message.role, "user");
            assert_eq!(message.content, "Hello");
        } else {
            panic!("Expected message");
        }

        // Check second message with tool call
        if let SessionItem::Message { message } = &eval.messages[1] {
            assert_eq!(message.role, "assistant");
            assert_eq!(message.tool_calls.len(), 1);
            assert_eq!(message.tool_calls[0].function.name, "read_file");
        } else {
            panic!("Expected message");
        }
    }

    #[tokio::test]
    async fn test_save_eval() {
        let tmp = tempfile::tempdir().unwrap();

        let mut session = Session::new();
        session.title = "My Test".to_string();
        session.add_message(SM::user("Hello"));

        let path = save_eval(&session, tmp.path()).await.unwrap();
        assert!(path.contains("my_test_"));
        assert!(path.ends_with(".json"));

        // Verify file contents
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let loaded: EvalSession = serde_json::from_str(&content).unwrap();
        assert_eq!(loaded.title, "My Test");
    }
}
