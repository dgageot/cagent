use std::sync::Arc;

use futures::stream;

use crate::agent::{Agent, Team};
use crate::chat::{Choice, ChoiceDelta, FinishReason, Message, StreamResponse, Usage};
use crate::model::{MessageStream, Provider, ProviderError};
use crate::runtime::{Event, LocalRuntime, RuntimeConfig};
use crate::session::Session;
use crate::tools::Tool;

#[derive(Clone, Debug)]
struct FakeProvider {
    id: String,
}

#[async_trait::async_trait]
impl Provider for FakeProvider {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn context_limit(&self) -> Option<i64> {
        Some(128_000)
    }

    async fn create_chat_completion_stream(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<Tool>,
    ) -> Result<MessageStream, ProviderError> {
        // Emit a single chunk that does NOT mark the stream as stopped, to force
        // another iteration and let max_iterations terminate the loop.
        let response = StreamResponse {
            id: "resp_1".into(),
            model: self.id(),
            choices: vec![Choice {
                index: 0,
                delta: ChoiceDelta {
                    role: None,
                    content: "hi".into(),
                    reasoning_content: String::new(),
                    tool_calls: vec![],
                },
                finish_reason: Some(FinishReason::ToolCalls),
            }],
            usage: Some(Usage {
                input_tokens: 1,
                output_tokens: 1,
                cached_input_tokens: 0,
                cache_write_tokens: 0,
            }),
            rate_limit: None,
        };

        Ok(Box::pin(stream::iter(vec![Ok(response)])))
    }
}

#[tokio::test]
async fn runtime_emits_max_iterations_reached() {
    let model: Arc<dyn Provider> = Arc::new(FakeProvider { id: "fake".into() });

    let agent = Agent::new("root").with_model(model);
    let team = Team::new(vec![agent], "root");

    let runtime = LocalRuntime::new(team, RuntimeConfig::default()).unwrap();

    let mut session = Session::new().with_max_iterations(1);

    let events = runtime.run(&mut session).await.unwrap();

    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::MaxIterationsReached { max: 1 })),
        "expected MaxIterationsReached event; got: {events:?}"
    );
}
