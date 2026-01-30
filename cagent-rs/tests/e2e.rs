//! End-to-end tests for cagent
//!
//! These tests verify the complete behavior of cagent by running agents
//! against mock AI providers.
//!
//! # Running E2E tests
//!
//! ```sh
//! # Run all E2E tests
//! cargo test --test e2e
//!
//! # Run with VCR recording (to update cassettes)
//! VCR_MODE=record cargo test --test e2e
//! ```

use std::sync::Arc;

use cagent::agent::{Agent, Team};
use cagent::model::mock::MockProvider;
use cagent::model::Provider;
use cagent::runtime::{Event, LocalRuntime, RuntimeConfig};
use cagent::session::Session;
use cagent::tools::builtin::ThinkToolset;

/// Run an agent with a mock provider (for deterministic tests)
async fn run_agent_with_mock(mock_response: &str, input: &str) -> (String, Vec<Event>) {
    // Create a mock provider
    let provider = MockProvider::new().with_response(mock_response);

    // Create a simple agent with the mock provider
    let agent = Agent::new("test")
        .with_description("Test agent")
        .with_model(Arc::new(provider) as Arc<dyn Provider + Send + Sync>)
        .with_toolset(Arc::new(ThinkToolset));

    let team = Team::new(vec![agent], "test");

    // Create runtime
    let runtime =
        LocalRuntime::new(team, RuntimeConfig::default()).expect("Failed to create runtime");

    // Create session with input
    let mut session = Session::new()
        .with_tools_approved(true)
        .with_user_message(input);

    // Run and collect events
    let mut events = runtime.run_stream(&mut session).await;
    let mut collected_events = Vec::new();
    let mut output = String::new();

    while let Some(event) = events.recv().await {
        match &event {
            Event::AgentChoice { content, .. } => {
                output.push_str(content);
            }
            Event::Error { message } => {
                output.push_str(&format!("\nError: {}\n", message));
            }
            _ => {}
        }
        collected_events.push(event);
    }

    (output, collected_events)
}

// ============================================================================
// Tests with mock providers (deterministic)
// ============================================================================

#[tokio::test]
async fn test_basic_response() {
    let (output, events) = run_agent_with_mock("Hello! I'm an AI assistant.", "Hi there").await;

    assert!(output.contains("Hello"));
    assert!(!events.is_empty());

    // Should have stream started and stopped events
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::StreamStarted { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::StreamStopped { .. })));
}

#[tokio::test]
async fn test_empty_response() {
    let (output, events) = run_agent_with_mock("", "Test input").await;

    // Empty response should still complete without error
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::StreamStopped { .. })));
    // Output should be empty (or minimal)
    assert!(output.is_empty());
}

#[tokio::test]
async fn test_events_emitted() {
    let (_, events) = run_agent_with_mock("Test response", "Test input").await;

    // Verify event sequence
    let event_types: Vec<&str> = events
        .iter()
        .map(|e| match e {
            Event::StreamStarted { .. } => "StreamStarted",
            Event::StreamStopped { .. } => "StreamStopped",
            Event::AgentInfo { .. } => "AgentInfo",
            Event::AgentChoice { .. } => "AgentChoice",
            Event::MessageAdded { .. } => "MessageAdded",
            Event::TokenUsage { .. } => "TokenUsage",
            Event::Error { .. } => "Error",
            _ => "Other",
        })
        .collect();

    // Should have AgentInfo event
    assert!(event_types.contains(&"AgentInfo"));

    // Should have stream start/stop events
    assert!(event_types.contains(&"StreamStarted"));
    assert!(event_types.contains(&"StreamStopped"));
}

#[tokio::test]
async fn test_agent_info_event() {
    let (_, events) = run_agent_with_mock("Response", "Input").await;

    // Find AgentInfo event
    let agent_info = events.iter().find_map(|e| {
        if let Event::AgentInfo {
            name, description, ..
        } = e
        {
            Some((name.clone(), description.clone()))
        } else {
            None
        }
    });

    assert!(agent_info.is_some());
    let (name, description) = agent_info.unwrap();
    assert_eq!(name, "test");
    assert_eq!(description, "Test agent");
}

#[tokio::test]
async fn test_token_usage_event() {
    let (_, events) = run_agent_with_mock("Response", "Input").await;

    // Find TokenUsage event
    let usage = events.iter().find_map(|e| {
        if let Event::TokenUsage {
            input_tokens,
            output_tokens,
            ..
        } = e
        {
            Some((*input_tokens, *output_tokens))
        } else {
            None
        }
    });

    assert!(usage.is_some());
    let (input, output) = usage.unwrap();
    // MockProvider returns default usage (10, 20)
    assert_eq!(input, 10);
    assert_eq!(output, 20);
}

#[tokio::test]
async fn test_multiple_calls() {
    // First call
    let (output1, _) = run_agent_with_mock("First response", "First input").await;
    assert!(output1.contains("First"));

    // Second call (new runtime)
    let (output2, _) = run_agent_with_mock("Second response", "Second input").await;
    assert!(output2.contains("Second"));
}

// ============================================================================
// Test utilities
// ============================================================================

#[cfg(test)]
mod tests {
    use cagent::model::mock::mock_tool_call;

    #[test]
    fn test_mock_tool_call() {
        let tc = mock_tool_call("test_tool", r#"{"arg": "value"}"#);
        assert_eq!(tc.function.name, "test_tool");
        assert!(tc.id.starts_with("call_"));
    }
}
