//! Runtime execution engine for the agent system.
//!
//! The runtime is responsible for:
//! - Managing the conversation loop between user, agent, and tools
//! - Streaming responses from AI providers
//! - Executing tool calls and collecting results
//! - Handling agent delegation via `transfer_task`
//! - Emitting events for UI/logging consumers
//!
//! # Architecture
//!
//! The runtime uses an event-driven architecture with a streaming execution loop:
//!
//! 1. User message is added to the session
//! 2. Session messages are sent to the AI provider
//! 3. Provider streams back response chunks (text + tool calls)
//! 4. Tool calls are executed (with optional user confirmation)
//! 5. Tool results are added back to session
//! 6. Loop continues until agent stops (no more tool calls) or max iterations reached
//!
//! # Events
//!
//! The runtime emits [`Event`]s that can be consumed by:
//! - TUI for rendering
//! - CLI for output
//! - API servers for streaming to clients
//!
//! # Example
//!
//! ```ignore
//! use cagent::runtime::Runtime;
//!
//! let runtime = Runtime::new(team);
//! let events = runtime.run_stream(ctx, session, resume_rx).await;
//!
//! while let Some(event) = events.recv().await {
//!     match event {
//!         Event::AgentChoice { content, .. } => println!("{}", content),
//!         Event::Error { message } => eprintln!("Error: {}", message),
//!         _ => {}
//!     }
//! }
//! ```

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::{mpsc, RwLock};
use tracing::debug;

use crate::agent::{Agent, Team};
use crate::chat::{FinishReason, Message, StreamResponse, Usage};
use crate::hooks::{Executor as HookExecutor, HookInput};
use crate::model::ProviderError;
use crate::permissions::{Checker as PermissionsChecker, Decision as PermissionDecision};
use crate::session::{FileSessionStore, Session, SessionMessage, SessionStore};
use crate::tools::{Tool, ToolCall, ToolCallResult};

// Re-export elicitation types
pub use crate::tools::mcp::{ElicitationRequest, ElicitationResponse, ElicitationAction};

// ============================================================================
// Events
// ============================================================================

#[derive(Debug, Clone, serde::Serialize)]
pub enum Event {
    StreamStarted {
        session_id: String,
        agent: String,
    },
    StreamStopped {
        session_id: String,
        agent: String,
    },
    AgentInfo {
        name: String,
        model: String,
        description: String,
        welcome_message: Option<String>,
    },
    AgentChoice {
        agent: String,
        content: String,
    },
    AgentReasoning {
        agent: String,
        content: String,
    },
    ToolCall {
        agent: String,
        tool_call: ToolCall,
        tool: Tool,
    },
    ToolCallResponse {
        agent: String,
        tool_call: ToolCall,
        result: ToolCallResult,
    },
    ToolCallConfirmation {
        agent: String,
        tool_call: ToolCall,
        tool: Tool,
    },
    /// Streaming partial tool call (for incremental UI updates)
    PartialToolCall {
        agent: String,
        tool_call: ToolCall,
    },
    TokenUsage {
        session_id: String,
        agent: String,
        input_tokens: i64,
        output_tokens: i64,
        total_tokens: i64,
        cost: f64,
        context_length: Option<i64>,
        context_limit: Option<i64>,
    },
    UserMessage {
        session_id: String,
        content: String,
    },
    MessageAdded {
        session_id: String,
        message: SessionMessage,
        agent: String,
    },
    AgentSwitching {
        active: bool,
        from_agent: String,
        to_agent: String,
    },
    SubSessionCompleted {
        session_id: String,
        sub_session: Session,
        agent: String,
    },
    /// Auto-generated session title
    SessionTitle {
        session_id: String,
        title: String,
    },
    /// Session summary (from compaction)
    SessionSummary {
        session_id: String,
        summary: String,
    },
    /// Session compaction completed
    SessionCompaction {
        session_id: String,
        items_before: usize,
        items_after: usize,
    },
    MaxIterationsReached {
        max: usize,
    },
    /// MCP server initialization started
    McpInitStarted {
        toolset: String,
    },
    /// MCP server initialization finished
    McpInitFinished {
        toolset: String,
        success: bool,
        error: Option<String>,
    },
    /// Team/multi-agent info
    TeamInfo {
        agents: Vec<String>,
        default_agent: String,
    },
    /// Available tools info
    ToolsetInfo {
        agent: String,
        tools: Vec<Tool>,
    },
    /// Hook blocked a tool execution
    HookBlocked {
        agent: String,
        tool_name: String,
        hook_type: String,
        reason: Option<String>,
    },
    Error {
        message: String,
    },
    Warning {
        message: String,
        agent: String,
    },
}

#[derive(Debug, Clone)]
pub enum ResumeType {
    Approve,
    ApproveSession,
    Reject { reason: Option<String> },
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeConfig {
    pub session_compaction: bool,
}

// ============================================================================
// Runtime
// ============================================================================

pub struct LocalRuntime {
    team: Team,
    current_agent: RwLock<String>,
    session_store: Arc<dyn SessionStore>,
    resume_tx: mpsc::Sender<ResumeType>,
    resume_rx: RwLock<Option<mpsc::Receiver<ResumeType>>>,
    #[allow(dead_code)]
    config: RuntimeConfig,
}

impl LocalRuntime {
    pub fn new(team: Team, config: RuntimeConfig) -> anyhow::Result<Self> {
        let default_agent = team
            .default_agent()
            .ok_or_else(|| anyhow::anyhow!("No default agent found"))?;
        let (resume_tx, resume_rx) = mpsc::channel(1);

        Ok(Self {
            current_agent: RwLock::new(default_agent.name.clone()),
            team,
            session_store: Arc::new(FileSessionStore::new_default()),
            resume_tx,
            resume_rx: RwLock::new(Some(resume_rx)),
            config,
        })
    }

    pub fn with_session_store(
        mut self,
        session_store: Arc<dyn SessionStore>,
    ) -> anyhow::Result<Self> {
        self.session_store = session_store;
        Ok(self)
    }

    pub fn session_store(&self) -> Arc<dyn SessionStore> {
        Arc::clone(&self.session_store)
    }

    /// Get the team
    pub fn team(&self) -> &Team {
        &self.team
    }

    /// Get all agent names
    pub fn agent_names(&self) -> Vec<String> {
        self.team.agent_names()
    }

    pub async fn set_current_agent(&self, name: &str) -> anyhow::Result<()> {
        if self.team.agent(name).is_none() {
            anyhow::bail!("Agent '{}' not found", name);
        }
        *self.current_agent.write().await = name.to_string();
        Ok(())
    }

    pub async fn current_agent(&self) -> Option<Agent> {
        self.team.agent(&self.current_agent.read().await).cloned()
    }

    pub async fn resume(&self, resume_type: ResumeType) {
        let _ = self.resume_tx.send(resume_type).await;
    }

    pub async fn run_stream(&self, session: &mut Session) -> mpsc::Receiver<Event> {
        let (event_tx, event_rx) = mpsc::channel(128);

        let Some(agent) = self.current_agent().await else {
            let _ = event_tx
                .send(Event::Error {
                    message: "No current agent".into(),
                })
                .await;
            return event_rx;
        };

        // Emit initial events
        let _ = event_tx
            .send(Event::TeamInfo {
                agents: self.team.agent_names(),
                default_agent: self.team.default_agent_name().to_string(),
            })
            .await;

        let _ = event_tx
            .send(Event::AgentInfo {
                name: agent.name.clone(),
                model: agent.get_model().map(|m| m.id()).unwrap_or_default(),
                description: agent.description.clone().unwrap_or_default(),
                welcome_message: agent.welcome_message.clone(),
            })
            .await;

        let _ = event_tx
            .send(Event::StreamStarted {
                session_id: session.id.clone(),
                agent: agent.name.clone(),
            })
            .await;

        // Spawn agent loop
        let session_id = session.id.clone();
        let mut session = session.clone();
        let team = self.team.clone();
        let agent_name = self.current_agent.read().await.clone();
        let tools_approved = session.tools_approved;
        let max_iterations = session.max_iterations;
        let resume_rx = self.resume_rx.write().await.take();

        tokio::spawn(async move {
            // Execute session start hooks if configured
            if let Some(agent) = team.agent(&agent_name) {
                if let Some(ref hooks_config) = agent.hooks {
                    if hooks_config.has_session_start_hooks() {
                        let executor = HookExecutor::new(
                            hooks_config.clone(),
                            std::env::current_dir()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        );

                        let mut hook_input = HookInput {
                            session_id: session.id.clone(),
                            cwd: std::env::current_dir()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                            hook_event_name: crate::hooks::EventType::SessionStart,
                            tool_name: None,
                            tool_use_id: None,
                            tool_input: None,
                            tool_response: None,
                            source: Some(agent.name.clone()),
                            reason: None,
                        };

                        let _ = executor.execute_session_start(&mut hook_input).await;
                    }
                }
            }

            let result = AgentLoop::new(
                &team,
                &agent_name,
                &mut session,
                event_tx.clone(),
                resume_rx,
            )
            .run(tools_approved, max_iterations)
            .await;

            if let Err(e) = result {
                let _ = event_tx
                    .send(Event::Error {
                        message: e.to_string(),
                    })
                    .await;
            }

            // Execute session end hooks if configured
            if let Some(agent) = team.agent(&agent_name) {
                if let Some(ref hooks_config) = agent.hooks {
                    if hooks_config.has_session_end_hooks() {
                        let executor = HookExecutor::new(
                            hooks_config.clone(),
                            std::env::current_dir()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                        );

                        let mut hook_input = HookInput {
                            session_id: session.id.clone(),
                            cwd: std::env::current_dir()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string(),
                            hook_event_name: crate::hooks::EventType::SessionEnd,
                            tool_name: None,
                            tool_use_id: None,
                            tool_input: None,
                            tool_response: None,
                            source: Some(agent.name.clone()),
                            reason: None,
                        };

                        let _ = executor.execute_session_end(&mut hook_input).await;
                    }
                }
            }

            let _ = event_tx
                .send(Event::StreamStopped {
                    session_id,
                    agent: agent_name,
                })
                .await;
        });

        event_rx
    }

    pub async fn run(&self, session: &mut Session) -> anyhow::Result<Vec<Event>> {
        let mut events = Vec::new();
        let mut rx = self.run_stream(session).await;
        while let Some(event) = rx.recv().await {
            let is_error = matches!(&event, Event::Error { .. });
            events.push(event);
            if is_error {
                break;
            }
        }
        Ok(events)
    }
}

// ============================================================================
// Agent Loop (extracted for clarity)
// ============================================================================

struct AgentLoop<'a> {
    team: &'a Team,
    agent_name: &'a str,
    session: &'a mut Session,
    event_tx: mpsc::Sender<Event>,
    resume_rx: Option<mpsc::Receiver<ResumeType>>,
}

impl<'a> AgentLoop<'a> {
    fn new(
        team: &'a Team,
        agent_name: &'a str,
        session: &'a mut Session,
        event_tx: mpsc::Sender<Event>,
        resume_rx: Option<mpsc::Receiver<ResumeType>>,
    ) -> Self {
        Self {
            team,
            agent_name,
            session,
            event_tx,
            resume_rx,
        }
    }

    fn run(
        mut self,
        mut tools_approved: bool,
        max_iterations: usize,
    ) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            let agent = self
                .team
                .agent(self.agent_name)
                .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", self.agent_name))?;
            let model = agent
                .get_model()
                .ok_or_else(|| anyhow::anyhow!("Agent '{}' has no model", self.agent_name))?;

            for iteration in 0.. {
                if max_iterations > 0 && iteration >= max_iterations {
                    let _ = self
                        .event_tx
                        .send(Event::MaxIterationsReached {
                            max: max_iterations,
                        })
                        .await;
                    break;
                }

                let tools = agent.tools().await?;
                debug!("Agent {} has {} tools", agent.name, tools.len());

                let messages = self.session.get_messages(agent);
                let mut stream = model
                    .create_chat_completion_stream(messages, tools.clone())
                    .await?;

                // Process stream
                let StreamResult {
                    content,
                    reasoning,
                    tool_calls,
                    usage,
                    stopped,
                } = self.process_stream(&mut stream, &agent.name).await?;

                // Add assistant message
                if !content.trim().is_empty() || !tool_calls.is_empty() {
                    let mut msg = Message::assistant(&content);
                    msg.tool_calls = tool_calls.clone();
                    if !reasoning.is_empty() {
                        msg.reasoning_content = Some(reasoning);
                    }
                    msg.usage = usage.clone();

                    let session_msg = SessionMessage::new(Some(agent), msg);
                    self.session.add_message(session_msg.clone());
                    let _ = self
                        .event_tx
                        .send(Event::MessageAdded {
                            session_id: self.session.id.clone(),
                            message: session_msg,
                            agent: agent.name.clone(),
                        })
                        .await;
                }

                // Emit token usage
                if let Some(ref u) = usage {
                    // Calculate context length (approximation: input tokens represent current context)
                    let context_length = self.session.input_tokens;

                    // Get context limit from model (default to common limits based on model name)
                    let context_limit = model.context_limit().unwrap_or_else(|| {
                        // Default context limits for common models
                        let model_id = model.id().to_lowercase();
                        if model_id.contains("gpt-4o")
                            || model_id.contains("claude-3")
                            || model_id.contains("gpt-4-turbo")
                            || model_id.contains("claude-2")
                        {
                            128_000
                        } else if model_id.contains("gpt-4") {
                            8_192
                        } else if model_id.contains("gpt-3.5") {
                            16_385
                        } else if model_id.contains("gemini") {
                            1_000_000
                        } else {
                            128_000 // Default fallback
                        }
                    });

                    let _ = self
                        .event_tx
                        .send(Event::TokenUsage {
                            session_id: self.session.id.clone(),
                            agent: agent.name.clone(),
                            input_tokens: u.input_tokens,
                            output_tokens: u.output_tokens,
                            total_tokens: u.input_tokens + u.output_tokens,
                            cost: self.session.cost,
                            context_length: Some(context_length),
                            context_limit: Some(context_limit),
                        })
                        .await;
                }

                // Process tool calls
                for tc in &tool_calls {
                    self.handle_tool_call(agent, &tools, tc, &mut tools_approved)
                        .await?;
                }

                if stopped && tool_calls.is_empty() {
                    break;
                }
            }

            Ok(())
        })
    }

    async fn process_stream(
        &mut self,
        stream: &mut (impl futures::Stream<Item = Result<StreamResponse, ProviderError>> + Unpin),
        agent_name: &str,
    ) -> anyhow::Result<StreamResult> {
        let mut result = StreamResult::default();
        let mut tool_call_map: HashMap<String, usize> = HashMap::new();

        while let Some(response) = stream.next().await {
            let response = response?;

            if let Some(u) = response.usage {
                self.session.input_tokens = u.input_tokens;
                self.session.output_tokens = u.output_tokens;
                result.usage = Some(u);
            }

            for choice in response.choices {
                if let Some(reason) = choice.finish_reason {
                    if matches!(reason, FinishReason::Stop | FinishReason::Length) {
                        result.stopped = true;
                    }
                }

                if !choice.delta.content.is_empty() {
                    let _ = self
                        .event_tx
                        .send(Event::AgentChoice {
                            agent: agent_name.to_string(),
                            content: choice.delta.content.clone(),
                        })
                        .await;
                    result.content.push_str(&choice.delta.content);
                }

                if !choice.delta.reasoning_content.is_empty() {
                    let _ = self
                        .event_tx
                        .send(Event::AgentReasoning {
                            agent: agent_name.to_string(),
                            content: choice.delta.reasoning_content.clone(),
                        })
                        .await;
                    result.reasoning.push_str(&choice.delta.reasoning_content);
                }

                for tc in choice.delta.tool_calls {
                    if let Some(&idx) = tool_call_map.get(&tc.id) {
                        if !tc.function.name.is_empty() {
                            result.tool_calls[idx].function.name = tc.function.name;
                        }
                        result.tool_calls[idx]
                            .function
                            .arguments
                            .push_str(&tc.function.arguments);
                    } else if !tc.id.is_empty() {
                        tool_call_map.insert(tc.id.clone(), result.tool_calls.len());
                        result.tool_calls.push(tc);
                    }
                }
            }
        }

        Ok(result)
    }

    async fn handle_tool_call(
        &mut self,
        agent: &Agent,
        tools: &[Tool],
        tc: &ToolCall,
        tools_approved: &mut bool,
    ) -> anyhow::Result<()> {
        // Handle transfer_task specially
        if tc.function.name == "transfer_task" {
            let result = self.handle_transfer_task(tc, *tools_approved).await;
            let content = result
                .as_ref()
                .map(|r| r.output.clone())
                .unwrap_or_else(|e| format!("Error: {}", e));
            self.session.add_message(SessionMessage::new(
                Some(agent),
                Message::tool(&tc.id, &content),
            ));
            return Ok(());
        }

        // Handle handoff specially
        if tc.function.name == "handoff" {
            let result = self.handle_handoff(agent, tc, *tools_approved).await;
            let content = result
                .as_ref()
                .map(|r| r.output.clone())
                .unwrap_or_else(|e| format!("Error: {}", e));
            self.session.add_message(SessionMessage::new(
                Some(agent),
                Message::tool(&tc.id, &content),
            ));
            return Ok(());
        }

        let Some(tool) = tools.iter().find(|t| t.name == tc.function.name) else {
            self.session.add_message(SessionMessage::new(
                Some(agent),
                Message::tool(&tc.id, format!("Unknown tool: {}", tc.function.name)),
            ));
            return Ok(());
        };

        // Parse tool arguments for permission checking and hooks
        let tool_args: Option<HashMap<String, serde_json::Value>> =
            serde_json::from_str(&tc.function.arguments).ok();

        // Execute pre-tool-use hooks if configured
        if let Some(ref hooks_config) = agent.hooks {
            if !hooks_config.is_empty() {
                let executor = HookExecutor::new(
                    hooks_config.clone(),
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                );

                let mut hook_input = HookInput {
                    session_id: self.session.id.clone(),
                    cwd: std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    hook_event_name: crate::hooks::EventType::PreToolUse,
                    tool_name: Some(tc.function.name.clone()),
                    tool_use_id: Some(tc.id.clone()),
                    tool_input: tool_args.clone(),
                    tool_response: None,
                    source: None,
                    reason: None,
                };

                let result = executor.execute_pre_tool_use(&mut hook_input).await;
                if !result.allowed {
                    // Hook blocked the tool call
                    let _ = self
                        .event_tx
                        .send(Event::HookBlocked {
                            agent: agent.name.clone(),
                            tool_name: tc.function.name.clone(),
                            hook_type: "pre_tool_use".to_string(),
                            reason: result.message.clone(),
                        })
                        .await;

                    let msg = result
                        .message
                        .unwrap_or_else(|| "Blocked by pre-tool-use hook".to_string());
                    self.session.add_message(SessionMessage::new(
                        Some(agent),
                        Message::tool(&tc.id, &msg),
                    ));
                    return Ok(());
                }
            }
        }

        // Check permissions using the session's permission config
        let permission_decision = if let Some(ref perms) = self.session.permissions {
            let checker = PermissionsChecker::from_config(perms);
            checker.check_with_args(&tc.function.name, tool_args.as_ref())
        } else {
            PermissionDecision::Ask
        };

        // Handle permission decision
        match permission_decision {
            PermissionDecision::Deny => {
                let msg = format!(
                    "Tool '{}' is denied by permissions policy",
                    tc.function.name
                );
                self.session.add_message(SessionMessage::new(
                    Some(agent),
                    Message::tool(&tc.id, &msg),
                ));
                return Ok(());
            }
            PermissionDecision::Allow => {
                // Skip confirmation, tool is auto-approved
            }
            PermissionDecision::Ask => {
                // Check if we need confirmation (not already approved and not read-only)
                if !*tools_approved && !tool.annotations.read_only_hint {
                    let _ = self
                        .event_tx
                        .send(Event::ToolCallConfirmation {
                            agent: agent.name.clone(),
                            tool_call: tc.clone(),
                            tool: tool.clone(),
                        })
                        .await;

                    if let Some(ref mut rx) = self.resume_rx {
                        if let Some(resume) = rx.recv().await {
                            match resume {
                                ResumeType::Approve => {}
                                ResumeType::ApproveSession => *tools_approved = true,
                                ResumeType::Reject { reason } => {
                                    let msg = format!(
                                        "Rejected{}",
                                        reason.map(|r| format!(": {}", r)).unwrap_or_default()
                                    );
                                    self.session.add_message(SessionMessage::new(
                                        Some(agent),
                                        Message::tool(&tc.id, &msg),
                                    ));
                                    return Ok(());
                                }
                            }
                        }
                    }
                }
            }
        }

        // Execute tool
        let _ = self
            .event_tx
            .send(Event::ToolCall {
                agent: agent.name.clone(),
                tool_call: tc.clone(),
                tool: tool.clone(),
            })
            .await;

        let result = execute_tool(agent, tc).await;

        // Execute post-tool-use hooks if configured
        if let Some(ref hooks_config) = agent.hooks {
            if !hooks_config.is_empty() {
                let executor = HookExecutor::new(
                    hooks_config.clone(),
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                );

                let mut hook_input = HookInput {
                    session_id: self.session.id.clone(),
                    cwd: std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    hook_event_name: crate::hooks::EventType::PostToolUse,
                    tool_name: Some(tc.function.name.clone()),
                    tool_use_id: Some(tc.id.clone()),
                    tool_input: tool_args,
                    tool_response: Some(serde_json::Value::String(result.output.clone())),
                    source: None,
                    reason: None,
                };

                let _ = executor.execute_post_tool_use(&mut hook_input).await;
                // Post-tool hooks don't block, they just provide additional context
            }
        }

        let _ = self
            .event_tx
            .send(Event::ToolCallResponse {
                agent: agent.name.clone(),
                tool_call: tc.clone(),
                result: result.clone(),
            })
            .await;

        let content = if result.output.trim().is_empty() {
            "(no output)"
        } else {
            &result.output
        };
        self.session.add_message(SessionMessage::new(
            Some(agent),
            Message::tool(&tc.id, content),
        ));

        Ok(())
    }

    async fn handle_transfer_task(
        &mut self,
        tc: &ToolCall,
        tools_approved: bool,
    ) -> anyhow::Result<ToolCallResult> {
        #[derive(Deserialize)]
        struct Args {
            agent: String,
            task: String,
            #[serde(default)]
            expected_output: Option<String>,
        }

        let args: Args = serde_json::from_str(&tc.function.arguments)?;
        let target = self
            .team
            .agent(&args.agent)
            .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.agent))?;

        let _ = self
            .event_tx
            .send(Event::AgentSwitching {
                active: true,
                from_agent: self.session.id.clone(),
                to_agent: args.agent.clone(),
            })
            .await;

        let mut sub_session = Session::new()
            .with_parent_id(&self.session.id)
            .with_max_iterations(target.max_iterations)
            .with_tools_approved(tools_approved)
            .with_system_message(format!(
                "Your task:\n\n<task>\n{}\n</task>{}",
                args.task,
                args.expected_output
                    .map(|o| format!("\n\n<expected>\n{}\n</expected>", o))
                    .unwrap_or_default()
            ))
            .with_user_message("Proceed.");

        let result = AgentLoop::new(
            self.team,
            &args.agent,
            &mut sub_session,
            self.event_tx.clone(),
            self.resume_rx.take(),
        )
        .run(tools_approved, target.max_iterations)
        .await;

        let _ = self
            .event_tx
            .send(Event::AgentSwitching {
                active: false,
                from_agent: args.agent.clone(),
                to_agent: self.session.id.clone(),
            })
            .await;

        let _ = self
            .event_tx
            .send(Event::SubSessionCompleted {
                session_id: self.session.id.clone(),
                sub_session: sub_session.clone(),
                agent: args.agent,
            })
            .await;

        self.session.add_sub_session(sub_session.clone());

        match result {
            Ok(_) => Ok(ToolCallResult::success(
                sub_session
                    .get_last_assistant_content()
                    .unwrap_or_else(|| "Done.".to_string()),
            )),
            Err(e) => Ok(ToolCallResult::error(format!("Failed: {}", e))),
        }
    }

    async fn handle_handoff(
        &mut self,
        current_agent: &Agent,
        tc: &ToolCall,
        tools_approved: bool,
    ) -> anyhow::Result<ToolCallResult> {
        #[derive(Deserialize)]
        struct Args {
            agent: String,
        }

        let args: Args = serde_json::from_str(&tc.function.arguments)?;

        // Validate that target agent is in handoffs list
        if !current_agent.handoffs.iter().any(|h| h.name == args.agent) {
            let handoff_names: Vec<_> = current_agent
                .handoffs
                .iter()
                .map(|h| h.name.as_str())
                .collect();
            if handoff_names.is_empty() {
                return Ok(ToolCallResult::error(format!(
                    "Agent {} cannot hand off to {}: target agent not in handoffs list. This agent has no handoff agents configured.",
                    current_agent.name, args.agent
                )));
            } else {
                return Ok(ToolCallResult::error(format!(
                    "Agent {} cannot hand off to {}: target agent not in handoffs list. Available handoff agent IDs are: {}",
                    current_agent.name, args.agent, handoff_names.join(", ")
                )));
            }
        }

        let target = self
            .team
            .agent(&args.agent)
            .ok_or_else(|| anyhow::anyhow!("Agent '{}' not found", args.agent))?;

        let _ = self
            .event_tx
            .send(Event::AgentSwitching {
                active: true,
                from_agent: current_agent.name.clone(),
                to_agent: args.agent.clone(),
            })
            .await;

        // For handoff, we continue with the same session but switch agents
        // The handoff message explains the context to the new agent
        let handoff_message = format!(
            "The agent {} handed off the conversation to you. \
            Continue from where it left off, using your specialized skills. \
            Your available handoff agents and tools are specified in the system messages that follow. \
            Review the conversation history to understand the context, then provide assistance.",
            current_agent.name
        );

        // Add handoff context as a system message
        self.session.add_message(SessionMessage::new(
            Some(target),
            Message::system(&handoff_message),
        ));

        // Run the new agent with the current session
        let result = AgentLoop::new(
            self.team,
            &args.agent,
            self.session,
            self.event_tx.clone(),
            self.resume_rx.take(),
        )
        .run(tools_approved, target.max_iterations)
        .await;

        let _ = self
            .event_tx
            .send(Event::AgentSwitching {
                active: false,
                from_agent: args.agent.clone(),
                to_agent: current_agent.name.clone(),
            })
            .await;

        match result {
            Ok(_) => Ok(ToolCallResult::success(handoff_message)),
            Err(e) => Ok(ToolCallResult::error(format!("Handoff failed: {}", e))),
        }
    }
}

#[derive(Default)]
struct StreamResult {
    content: String,
    reasoning: String,
    tool_calls: Vec<ToolCall>,
    usage: Option<Usage>,
    stopped: bool,
}

async fn execute_tool(agent: &Agent, tool_call: &ToolCall) -> ToolCallResult {
    for toolset in &agent.toolsets {
        let Ok(tools) = toolset.tools().await else {
            continue;
        };
        if tools.iter().any(|t| t.name == tool_call.function.name) {
            return match toolset.execute(tool_call).await {
                Ok(r) => r,
                Err(e) => ToolCallResult::error(format!("Error: {}", e)),
            };
        }
    }
    ToolCallResult::error(format!("Tool '{}' not found", tool_call.function.name))
}
