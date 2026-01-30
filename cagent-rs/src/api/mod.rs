//! REST API server for cagent
//!
//! Provides HTTP endpoints for agent interaction, session management, and SSE streaming.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{sse::Event as SseEvent, IntoResponse, Sse},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer};
use tracing::{debug, error};

use crate::config::Config;
use crate::permissions::PermissionsConfig;
use crate::runtime::{Event, LocalRuntime, ResumeType, RuntimeConfig};
use crate::session::{Session, SessionMessage, SessionStore, SqliteSessionStore};

/// API server state
pub struct ApiState {
    /// Session store
    pub sessions: Arc<dyn SessionStore>,
    /// Agent configurations by name
    pub configs: HashMap<String, Config>,
    /// Active runtimes by session ID
    pub runtimes: RwLock<HashMap<String, Arc<LocalRuntime>>>,
    /// Runtime configuration
    pub runtime_config: RuntimeConfig,
}

impl ApiState {
    pub fn new(
        sessions: Arc<dyn SessionStore>,
        configs: HashMap<String, Config>,
        runtime_config: RuntimeConfig,
    ) -> Self {
        Self {
            sessions,
            configs,
            runtimes: RwLock::new(HashMap::new()),
            runtime_config,
        }
    }
}

// ============================================================================
// API Types
// ============================================================================

/// Agent info returned by GET /api/agents
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub multi: bool,
}

/// Session summary for GET /api/sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub num_messages: usize,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub working_dir: Option<String>,
}

/// Full session response for GET /api/sessions/:id
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    pub title: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub messages: Vec<SessionMessage>,
    pub tools_approved: bool,
    pub thinking: bool,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub working_dir: Option<String>,
    pub permissions: Option<PermissionsConfig>,
}

/// Request to create a new session
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CreateSessionRequest {
    #[serde(default)]
    pub title: String,
    pub working_dir: Option<String>,
    #[serde(default)]
    pub tools_approved: bool,
    #[serde(default)]
    pub thinking: bool,
    pub permissions: Option<PermissionsConfig>,
}

/// Request to run an agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunAgentRequest {
    pub messages: Vec<MessageInput>,
}

/// Message input for running agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageInput {
    pub role: String,
    pub content: String,
}

/// Request to resume a session (tool confirmation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResumeSessionRequest {
    pub confirmation: String,
    pub reason: Option<String>,
}

/// Request to update session permissions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePermissionsRequest {
    pub permissions: PermissionsConfig,
}

/// Request to update session title
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTitleRequest {
    pub title: String,
}

/// Response for title update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateTitleResponse {
    pub id: String,
    pub title: String,
}

/// Generic error response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

// ============================================================================
// Server Implementation
// ============================================================================

/// Create the API router
pub fn create_router(state: Arc<ApiState>) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    Router::new()
        // Agent endpoints
        .route("/api/agents", get(get_agents))
        .route("/api/agents/{id}", get(get_agent_config))
        // Session endpoints
        .route("/api/sessions", get(get_sessions))
        .route("/api/sessions", post(create_session))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}", delete(delete_session))
        .route("/api/sessions/{id}/resume", post(resume_session))
        .route("/api/sessions/{id}/tools/toggle", post(toggle_tools))
        .route("/api/sessions/{id}/thinking/toggle", post(toggle_thinking))
        .route("/api/sessions/{id}/permissions", patch(update_permissions))
        .route("/api/sessions/{id}/title", patch(update_title))
        // Agent execution
        .route("/api/sessions/{id}/agent/{agent}", post(run_agent))
        .route(
            "/api/sessions/{id}/agent/{agent}/{agent_name}",
            post(run_agent_named),
        )
        // Elicitation
        .route("/api/sessions/{id}/elicitation", post(elicitation))
        // Health check
        .route("/api/ping", get(ping))
        .layer(cors)
        .with_state(state)
}

/// Start the API server
pub async fn serve(state: Arc<ApiState>, addr: &str) -> anyhow::Result<()> {
    let router = create_router(state);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    println!("Listening on {}", local_addr);

    axum::serve(listener, router).await?;
    Ok(())
}

// ============================================================================
// Handlers
// ============================================================================

/// GET /api/ping - Health check
async fn ping() -> impl IntoResponse {
    Json(serde_json::json!({"status": "ok"}))
}

/// GET /api/agents - List all agents
async fn get_agents(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let agents: Vec<AgentInfo> = state
        .configs
        .iter()
        .map(|(name, config)| {
            let description = config
                .agents
                .values()
                .next()
                .and_then(|a| a.description.clone())
                .unwrap_or_default();
            let multi = config.agents.len() > 1;
            AgentInfo {
                name: name.clone(),
                description,
                multi,
            }
        })
        .collect();

    Json(agents)
}

/// GET /api/agents/:id - Get agent configuration
async fn get_agent_config(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.configs.get(&id) {
        Some(config) => Json(serde_json::to_value(config).unwrap()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(ErrorResponse { error: "Agent not found".to_string() })).into_response(),
    }
}

/// GET /api/sessions - List all sessions
async fn get_sessions(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    match state.sessions.list().await {
        Ok(sessions) => {
            let summaries: Vec<SessionSummary> = sessions
                .into_iter()
                .map(|s| SessionSummary {
                    id: s.id.clone(),
                    title: s.title.clone(),
                    created_at: s.created_at.to_rfc3339(),
                    num_messages: s.get_all_messages().len(),
                    input_tokens: s.input_tokens,
                    output_tokens: s.output_tokens,
                    working_dir: s.working_dir.clone(),
                })
                .collect();
            Json(summaries).into_response()
        }
        Err(e) => {
            error!("Failed to list sessions: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to list sessions: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// POST /api/sessions - Create a new session
async fn create_session(
    State(state): State<Arc<ApiState>>,
    Json(req): Json<CreateSessionRequest>,
) -> impl IntoResponse {
    let mut session = Session::new()
        .with_title(req.title)
        .with_tools_approved(req.tools_approved)
        .with_thinking(req.thinking);

    if let Some(wd) = req.working_dir {
        session = session.with_working_dir(wd);
    }

    if let Some(perms) = req.permissions {
        session = session.with_permissions(perms);
    }

    match state.sessions.save(&session).await {
        Ok(()) => Json(session).into_response(),
        Err(e) => {
            error!("Failed to create session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to create session: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// GET /api/sessions/:id - Get a session
async fn get_session(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.get(&id).await {
        Ok(Some(session)) => {
            let response = SessionResponse {
                id: session.id.clone(),
                title: session.title.clone(),
                created_at: session.created_at,
                messages: session.get_all_messages(),
                tools_approved: session.tools_approved,
                thinking: session.thinking,
                input_tokens: session.input_tokens,
                output_tokens: session.output_tokens,
                working_dir: session.working_dir.clone(),
                permissions: session.permissions.clone(),
            };
            Json(response).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => {
            error!("Failed to get session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get session: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// DELETE /api/sessions/:id - Delete a session
async fn delete_session(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.delete(&id).await {
        Ok(()) => Json(serde_json::json!({"message": "session deleted"})).into_response(),
        Err(e) => {
            error!("Failed to delete session: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to delete session: {}", e),
                }),
            )
                .into_response()
        }
    }
}

/// POST /api/sessions/:id/resume - Resume a session (tool confirmation)
async fn resume_session(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(req): Json<ResumeSessionRequest>,
) -> impl IntoResponse {
    let runtimes = state.runtimes.read().await;
    if let Some(runtime) = runtimes.get(&id) {
        let resume_type = match req.confirmation.as_str() {
            "approve" | "yes" | "y" => ResumeType::Approve,
            "approve_all" | "all" | "a" => ResumeType::ApproveSession,
            _ => ResumeType::Reject { reason: req.reason },
        };
        runtime.resume(resume_type).await;
        Json(serde_json::json!({"message": "session resumed"})).into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "No active runtime for session".to_string(),
            }),
        )
            .into_response()
    }
}

/// POST /api/sessions/:id/tools/toggle - Toggle tool approval mode
async fn toggle_tools(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.get(&id).await {
        Ok(Some(mut session)) => {
            session.tools_approved = !session.tools_approved;
            match state.sessions.save(&session).await {
                Ok(()) => Json(serde_json::json!(null)).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to save session: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to get session: {}", e),
            }),
        )
            .into_response(),
    }
}

/// POST /api/sessions/:id/thinking/toggle - Toggle thinking mode
async fn toggle_thinking(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.sessions.get(&id).await {
        Ok(Some(mut session)) => {
            session.thinking = !session.thinking;
            match state.sessions.save(&session).await {
                Ok(()) => Json(serde_json::json!(null)).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to save session: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to get session: {}", e),
            }),
        )
            .into_response(),
    }
}

/// PATCH /api/sessions/:id/permissions - Update session permissions
async fn update_permissions(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdatePermissionsRequest>,
) -> impl IntoResponse {
    match state.sessions.get(&id).await {
        Ok(Some(mut session)) => {
            session.permissions = Some(req.permissions);
            match state.sessions.save(&session).await {
                Ok(()) => {
                    Json(serde_json::json!({"message": "session permissions updated"})).into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to save session: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to get session: {}", e),
            }),
        )
            .into_response(),
    }
}

/// PATCH /api/sessions/:id/title - Update session title
async fn update_title(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(req): Json<UpdateTitleRequest>,
) -> impl IntoResponse {
    match state.sessions.get(&id).await {
        Ok(Some(mut session)) => {
            session.title = req.title.clone();
            match state.sessions.save(&session).await {
                Ok(()) => Json(UpdateTitleResponse {
                    id,
                    title: req.title,
                })
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to save session: {}", e),
                    }),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to get session: {}", e),
            }),
        )
            .into_response(),
    }
}

/// POST /api/sessions/:id/agent/:agent - Run an agent
async fn run_agent(
    State(state): State<Arc<ApiState>>,
    Path((session_id, agent_name)): Path<(String, String)>,
    Json(req): Json<RunAgentRequest>,
) -> impl IntoResponse {
    run_agent_impl(state, session_id, agent_name, "root".to_string(), req).await
}

/// POST /api/sessions/:id/agent/:agent/:agent_name - Run a specific agent
async fn run_agent_named(
    State(state): State<Arc<ApiState>>,
    Path((session_id, agent_name, current_agent)): Path<(String, String, String)>,
    Json(req): Json<RunAgentRequest>,
) -> impl IntoResponse {
    run_agent_impl(state, session_id, agent_name, current_agent, req).await
}

async fn run_agent_impl(
    state: Arc<ApiState>,
    session_id: String,
    agent_config_name: String,
    current_agent: String,
    req: RunAgentRequest,
) -> impl IntoResponse {
    debug!(
        "Running agent {} for session {}, current_agent={}",
        agent_config_name, session_id, current_agent
    );

    // Get or create session
    let mut session = match state.sessions.get(&session_id).await {
        Ok(Some(s)) => s,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Session not found".to_string(),
                }),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to get session: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Add messages to session
    for msg in req.messages {
        let session_msg = match msg.role.as_str() {
            "user" => SessionMessage::user(msg.content),
            "system" => SessionMessage::system(msg.content),
            _ => continue,
        };
        session.add_message(session_msg);
    }

    // Get agent config
    let config = match state.configs.get(&agent_config_name) {
        Some(c) => c.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "Agent configuration not found".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Build team and runtime
    let team = match crate::cli::build_team(&config, None) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to build team: {}", e),
                }),
            )
                .into_response();
        }
    };

    let runtime = match LocalRuntime::new(team, state.runtime_config.clone()) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to create runtime: {}", e),
                }),
            )
                .into_response();
        }
    };

    // Set current agent
    if let Err(e) = runtime.set_current_agent(&current_agent).await {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Failed to set agent: {}", e),
            }),
        )
            .into_response();
    }

    // Store runtime for resume/elicitation
    {
        let mut runtimes = state.runtimes.write().await;
        runtimes.insert(session_id.clone(), runtime.clone());
    }

    // Run agent and stream events
    let events = runtime.run_stream(&mut session).await;
    let state_clone = state.clone();
    let session_id_clone = session_id.clone();
    let session_clone = session.clone();

    let stream = async_stream::stream! {
        let mut rx = events;
        while let Some(event) = rx.recv().await {
            let event_json = serde_json::to_string(&event).unwrap_or_default();
            yield Ok::<_, std::convert::Infallible>(SseEvent::default().data(event_json));

            // Check if stream is done
            if matches!(event, Event::StreamStopped { .. }) {
                break;
            }
        }

        // Save session state after completion
        let _ = state_clone.sessions.save(&session_clone).await;

        // Remove runtime
        let mut runtimes = state_clone.runtimes.write().await;
        runtimes.remove(&session_id_clone);
    };

    Sse::new(stream).into_response()
}

/// POST /api/sessions/:id/elicitation - Handle elicitation response
async fn elicitation(
    State(state): State<Arc<ApiState>>,
    Path(id): Path<String>,
    Json(req): Json<ElicitationRequest>,
) -> impl IntoResponse {
    debug!("Elicitation for session {}: action={}", id, req.action);

    // For now, elicitation is not fully implemented in the runtime
    // This is a placeholder that acknowledges the request
    let _ = (state, req);

    Json(serde_json::json!(null)).into_response()
}

/// Elicitation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElicitationRequest {
    pub action: String,
    pub content: Option<String>,
}

// ============================================================================
// Server Builder
// ============================================================================

/// Builder for configuring and starting the API server
pub struct ApiServerBuilder {
    session_store: Option<Arc<dyn SessionStore>>,
    configs: HashMap<String, Config>,
    runtime_config: RuntimeConfig,
    addr: String,
}

impl Default for ApiServerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ApiServerBuilder {
    pub fn new() -> Self {
        Self {
            session_store: None,
            configs: HashMap::new(),
            runtime_config: RuntimeConfig::default(),
            addr: "127.0.0.1:8080".to_string(),
        }
    }

    pub fn with_session_store(mut self, store: Arc<dyn SessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    pub fn with_sqlite_store(self, path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let store = SqliteSessionStore::new(path)?;
        Ok(self.with_session_store(Arc::new(store)))
    }

    pub fn with_config(mut self, name: impl Into<String>, config: Config) -> Self {
        self.configs.insert(name.into(), config);
        self
    }

    pub fn with_configs(mut self, configs: HashMap<String, Config>) -> Self {
        self.configs = configs;
        self
    }

    pub fn with_runtime_config(mut self, config: RuntimeConfig) -> Self {
        self.runtime_config = config;
        self
    }

    pub fn with_addr(mut self, addr: impl Into<String>) -> Self {
        self.addr = addr.into();
        self
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        let session_store = self.session_store.unwrap_or_else(|| {
            Arc::new(crate::session::InMemorySessionStore::new())
        });

        let state = Arc::new(ApiState::new(
            session_store,
            self.configs,
            self.runtime_config,
        ));

        serve(state, &self.addr).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn create_test_state() -> Arc<ApiState> {
        Arc::new(ApiState::new(
            Arc::new(crate::session::InMemorySessionStore::new()),
            HashMap::new(),
            RuntimeConfig::default(),
        ))
    }

    #[tokio::test]
    async fn test_ping() {
        let state = create_test_state();
        let router = create_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/ping")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_sessions_empty() {
        let state = create_test_state();
        let router = create_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_agents_empty() {
        let state = create_test_state();
        let router = create_router(state);

        let response = router
            .oneshot(
                Request::builder()
                    .uri("/api/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
