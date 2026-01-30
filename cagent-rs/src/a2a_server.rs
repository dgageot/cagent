//! A2A (Agent-to-Agent) Server module
//!
//! This module provides A2A server functionality to expose cagent agents
//! via the Agent-to-Agent protocol for inter-agent communication.
//!
//! The A2A protocol enables agents to:
//! - Discover each other via agent cards
//! - Invoke tasks on remote agents
//! - Stream responses in real-time

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, error, info};

use crate::agent::Team;
use crate::runtime::{Event, LocalRuntime, RuntimeConfig};
use crate::session::Session;

/// A2A Agent Card - describes an agent's capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCard {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation_url: Option<String>,
    pub capabilities: AgentCapabilities,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<serde_json::Value>,
    pub default_input_modes: Vec<String>,
    pub default_output_modes: Vec<String>,
    pub skills: Vec<AgentSkill>,
}

/// Agent provider information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProvider {
    pub organization: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

/// Agent capabilities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCapabilities {
    pub streaming: bool,
    pub push_notifications: bool,
    pub state_transition_history: bool,
}

/// Agent skill definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSkill {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub examples: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_modes: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_modes: Option<Vec<String>>,
}

/// A2A Task state
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Canceled,
    Failed,
}

/// A2A Message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aMessage {
    pub role: String,
    pub parts: Vec<MessagePart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// A2A Message part
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessagePart {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        metadata: Option<serde_json::Value>,
    },
}

/// A2A Task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A2aTask {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub status: TaskState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<A2aMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifacts: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// JSON-RPC Request
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub method: String,
    pub params: serde_json::Value,
    pub id: serde_json::Value,
}

/// JSON-RPC Response
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
    pub id: serde_json::Value,
}

/// JSON-RPC Error
#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    fn success(id: serde_json::Value, result: impl Serialize) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: Some(serde_json::to_value(result).unwrap_or_default()),
            error: None,
            id,
        }
    }

    fn error(id: serde_json::Value, code: i32, message: &str) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
            id,
        }
    }
}

/// Send task request params
#[derive(Debug, Clone, Deserialize)]
pub struct SendTaskParams {
    pub id: String,
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<A2aMessage>,
}

/// Get task request params
#[derive(Debug, Clone, Deserialize)]
pub struct GetTaskParams {
    pub id: String,
    #[serde(rename = "historyLength")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub history_length: Option<u32>,
}

/// Cancel task request params
#[derive(Debug, Clone, Deserialize)]
pub struct CancelTaskParams {
    pub id: String,
}

/// A2A Server state
#[derive(Clone)]
struct A2aServerState {
    team: Arc<Team>,
    agent_name: String,
    tasks: Arc<Mutex<HashMap<String, A2aTask>>>,
    base_url: String,
}

impl A2aServerState {
    fn new(team: Arc<Team>, agent_name: String, base_url: String) -> Self {
        Self {
            team,
            agent_name,
            tasks: Arc::new(Mutex::new(HashMap::new())),
            base_url,
        }
    }

    fn agent_card(&self) -> AgentCard {
        let description = self
            .team
            .agent(&self.agent_name)
            .and_then(|a| a.description.clone())
            .unwrap_or_else(|| format!("cagent agent: {}", self.agent_name));

        AgentCard {
            name: self.agent_name.clone(),
            description: Some(description.clone()),
            url: self.base_url.clone(),
            provider: Some(AgentProvider {
                organization: "Docker".to_string(),
                url: Some("https://github.com/docker/cagent".to_string()),
            }),
            version: env!("CARGO_PKG_VERSION").to_string(),
            documentation_url: Some("https://github.com/docker/cagent".to_string()),
            capabilities: AgentCapabilities {
                streaming: true,
                push_notifications: false,
                state_transition_history: false,
            },
            authentication: None,
            default_input_modes: vec!["text".to_string()],
            default_output_modes: vec!["text".to_string()],
            skills: vec![AgentSkill {
                id: format!("{}_{}", self.agent_name, "invoke"),
                name: self.agent_name.clone(),
                description: Some(description),
                tags: vec!["llm".to_string(), "cagent".to_string()],
                examples: None,
                input_modes: None,
                output_modes: None,
            }],
        }
    }

    async fn execute_task(&self, task: &A2aTask) -> Result<A2aTask> {
        debug!(task_id = %task.id, agent = %self.agent_name, "Executing A2A task");

        // Extract message from task
        let message = task
            .history
            .as_ref()
            .and_then(|h| h.last())
            .and_then(|m| {
                m.parts.iter().find_map(|p| match p {
                    MessagePart::Text { text, .. } => Some(text.clone()),
                })
            })
            .unwrap_or_default();

        // Create a session for this request
        let mut session = Session::new()
            .with_tools_approved(true)
            .with_user_message(&message);

        // Create runtime and set current agent
        let rt = LocalRuntime::new((*self.team).clone(), RuntimeConfig::default())?;
        rt.set_current_agent(&self.agent_name).await?;

        // Run the agent
        let mut events = rt.run_stream(&mut session).await;

        // Collect response
        let mut response = String::new();
        while let Some(event) = events.recv().await {
            match event {
                Event::AgentChoice { content, .. } => {
                    response.push_str(&content);
                }
                Event::Error { message } => {
                    error!(task_id = %task.id, error = %message, "Task execution error");
                    return Ok(A2aTask {
                        id: task.id.clone(),
                        session_id: task.session_id.clone(),
                        status: TaskState::Failed,
                        history: task.history.clone(),
                        artifacts: None,
                        metadata: None,
                    });
                }
                _ => {}
            }
        }

        // Create response message
        let response_message = A2aMessage {
            role: "agent".to_string(),
            parts: vec![MessagePart::Text {
                text: response,
                metadata: None,
            }],
            metadata: None,
        };

        // Update task with response
        let mut history = task.history.clone().unwrap_or_default();
        history.push(response_message);

        Ok(A2aTask {
            id: task.id.clone(),
            session_id: task.session_id.clone(),
            status: TaskState::Completed,
            history: Some(history),
            artifacts: None,
            metadata: None,
        })
    }
}

/// Handler for agent card endpoint
async fn handle_agent_card(State(state): State<A2aServerState>) -> impl IntoResponse {
    Json(state.agent_card())
}

/// Handler for well-known agent card endpoint
async fn handle_well_known_agent(State(state): State<A2aServerState>) -> impl IntoResponse {
    Json(state.agent_card())
}

/// Handler for JSON-RPC endpoint
async fn handle_json_rpc(
    State(state): State<A2aServerState>,
    Json(request): Json<JsonRpcRequest>,
) -> impl IntoResponse {
    let response = match request.method.as_str() {
        "tasks/send" => {
            let params: SendTaskParams = match serde_json::from_value(request.params) {
                Ok(p) => p,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(JsonRpcResponse::error(
                            request.id,
                            -32602,
                            &format!("Invalid params: {}", e),
                        )),
                    )
                }
            };

            let task = A2aTask {
                id: params.id.clone(),
                session_id: params.session_id.clone(),
                status: TaskState::Working,
                history: params.message.map(|m| vec![m]),
                artifacts: None,
                metadata: None,
            };

            // Store task
            {
                let mut tasks = state.tasks.lock().await;
                tasks.insert(task.id.clone(), task.clone());
            }

            // Execute task
            match state.execute_task(&task).await {
                Ok(result) => {
                    let mut tasks = state.tasks.lock().await;
                    tasks.insert(result.id.clone(), result.clone());
                    JsonRpcResponse::success(request.id, result)
                }
                Err(e) => JsonRpcResponse::error(request.id, -32000, &e.to_string()),
            }
        }
        "tasks/get" => {
            let params: GetTaskParams = match serde_json::from_value(request.params) {
                Ok(p) => p,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(JsonRpcResponse::error(
                            request.id,
                            -32602,
                            &format!("Invalid params: {}", e),
                        )),
                    )
                }
            };

            let tasks = state.tasks.lock().await;
            match tasks.get(&params.id) {
                Some(task) => JsonRpcResponse::success(request.id, task.clone()),
                None => JsonRpcResponse::error(request.id, -32001, "Task not found"),
            }
        }
        "tasks/cancel" => {
            let params: CancelTaskParams = match serde_json::from_value(request.params) {
                Ok(p) => p,
                Err(e) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(JsonRpcResponse::error(
                            request.id,
                            -32602,
                            &format!("Invalid params: {}", e),
                        )),
                    )
                }
            };

            let mut tasks = state.tasks.lock().await;
            match tasks.get_mut(&params.id) {
                Some(task) => {
                    task.status = TaskState::Canceled;
                    JsonRpcResponse::success(request.id, task.clone())
                }
                None => JsonRpcResponse::error(request.id, -32001, "Task not found"),
            }
        }
        _ => JsonRpcResponse::error(request.id, -32601, "Method not found"),
    };

    (StatusCode::OK, Json(response))
}

/// Start an A2A server on the given port
///
/// # Arguments
/// * `team` - The team of agents
/// * `agent_name` - The agent to expose via A2A
/// * `port` - The port to listen on (0 for random available port)
pub async fn start_a2a_server(team: Arc<Team>, agent_name: String, port: u16) -> Result<()> {
    let actual_port = if port == 0 { 8080 } else { port };
    let address = format!("127.0.0.1:{}", actual_port);
    let base_url = format!("http://{}", address);

    info!(addr = %address, agent = %agent_name, "Starting A2A server");

    let state = A2aServerState::new(team, agent_name, base_url);

    let app = Router::new()
        .route("/", post(handle_json_rpc))
        .route("/agent-card", get(handle_agent_card))
        .route("/.well-known/agent.json", get(handle_well_known_agent))
        .with_state(state);

    println!("A2A server listening on http://{}", address);
    println!(
        "Agent card available at http://{}/.well-known/agent.json",
        address
    );

    let listener = tokio::net::TcpListener::bind(&address).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AgentBuilder, Team};

    #[test]
    fn test_agent_card_generation() {
        let agent = AgentBuilder::new("test")
            .with_description("A test agent")
            .build();
        let team = Team::new(vec![agent], "test");
        let state = A2aServerState::new(
            Arc::new(team),
            "test".to_string(),
            "http://localhost:8080".to_string(),
        );

        let card = state.agent_card();
        assert_eq!(card.name, "test");
        assert!(card.description.as_ref().unwrap().contains("A test agent"));
        assert_eq!(card.url, "http://localhost:8080");
        assert!(card.capabilities.streaming);
        assert_eq!(card.skills.len(), 1);
        assert_eq!(card.skills[0].name, "test");
    }

    #[test]
    fn test_task_serialization() {
        let task = A2aTask {
            id: "task-1".to_string(),
            session_id: "session-1".to_string(),
            status: TaskState::Completed,
            history: Some(vec![A2aMessage {
                role: "agent".to_string(),
                parts: vec![MessagePart::Text {
                    text: "Hello".to_string(),
                    metadata: None,
                }],
                metadata: None,
            }]),
            artifacts: None,
            metadata: None,
        };

        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("task-1"));
        assert!(json.contains("completed"));
        assert!(json.contains("Hello"));
    }

    #[test]
    fn test_json_rpc_response() {
        let response = JsonRpcResponse::success(
            serde_json::json!(1),
            serde_json::json!({"result": "ok"}),
        );
        assert!(response.result.is_some());
        assert!(response.error.is_none());

        let error_response = JsonRpcResponse::error(serde_json::json!(1), -32000, "Error");
        assert!(error_response.result.is_none());
        assert!(error_response.error.is_some());
    }
}
