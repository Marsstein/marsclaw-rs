//! HTTP server with Axum, serving the API and embedded Web UI.
//!
//! Ported from Go: internal/server/server.go

pub mod terminal;

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, SafetyCheck};
use crate::config::AgentConfig;
use crate::store::{Session, Store};
use crate::tool::Registry;
use crate::types::*;

// ---------------------------------------------------------------------------
// Embedded UI
// ---------------------------------------------------------------------------

static UI_HTML: &str = include_str!("ui.html");

// ---------------------------------------------------------------------------
// Config + types
// ---------------------------------------------------------------------------

pub struct ServerConfig {
    pub addr: String,
    pub model: String,
    pub soul: String,
    pub tasks: Vec<TaskInfo>,
    pub provider: Arc<dyn Provider>,
    pub agent_cfg: AgentConfig,
    pub registry: Registry,
    pub cost: Arc<dyn CostRecorder>,
    pub safety: Option<Arc<dyn SafetyCheck>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskInfo {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub channel: String,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct AppState {
    store: Arc<dyn Store>,
    model: String,
    soul: String,
    tasks: Vec<TaskInfo>,
    provider: Arc<dyn Provider>,
    agent_cfg: AgentConfig,
    registry: Registry,
    cost: Arc<dyn CostRecorder>,
    safety: Option<Arc<dyn SafetyCheck>>,
}

// ---------------------------------------------------------------------------
// Error response helper
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

fn error_response(status: StatusCode, msg: impl Into<String>) -> Response {
    let body = ErrorBody {
        error: msg.into(),
    };
    (status, Json(body)).into_response()
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_ui() -> Html<&'static str> {
    Html(UI_HTML)
}

async fn handle_create_session(State(state): State<Arc<AppState>>) -> Response {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let id = format!("sess_{nanos}");
    let now = Utc::now();

    let session = Session {
        id,
        title: "New conversation".to_owned(),
        source: "server".to_owned(),
        metadata: None,
        created_at: now,
        updated_at: now,
    };

    if let Err(e) = state.store.create_session(&session).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }

    (StatusCode::CREATED, Json(session)).into_response()
}

async fn handle_list_sessions(State(state): State<Arc<AppState>>) -> Response {
    match state.store.list_sessions().await {
        Ok(sessions) => Json(sessions).into_response(),
        Err(e) => error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn handle_get_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    let session = match state.store.get_session(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let messages = match state.store.get_messages(&id).await {
        Ok(m) => m,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    Json(serde_json::json!({
        "session": session,
        "messages": messages,
    }))
    .into_response()
}

async fn handle_delete_session(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Response {
    if let Err(e) = state.store.delete_session(&id).await {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    StatusCode::NO_CONTENT.into_response()
}

#[derive(Deserialize)]
struct SendMessageBody {
    message: String,
}

async fn handle_send_message(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<SendMessageBody>,
) -> Response {
    // Verify session exists.
    let session = match state.store.get_session(&id).await {
        Ok(Some(s)) => s,
        Ok(None) => return error_response(StatusCode::NOT_FOUND, "session not found"),
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    // Load history.
    let history = state.store.get_messages(&id).await.unwrap_or_default();

    let user_msg = Message {
        role: Role::User,
        content: body.message.clone(),
        timestamp: Utc::now(),
        ..Default::default()
    };

    // Persist the user message.
    let _ = state.store.append_messages(&id, &[user_msg.clone()]).await;

    // Auto-title on first message.
    if session.title == "New conversation" && !body.message.is_empty() {
        let title = if body.message.chars().count() > 60 {
            let truncated: String = body.message.chars().take(60).collect();
            format!("{truncated}...")
        } else {
            body.message.clone()
        };
        let _ = state.store.update_title(&id, &title).await;
    }

    // Build the agent and run it, streaming results via SSE.
    let provider = state.provider.clone();
    let agent_cfg = state.agent_cfg.clone();
    let registry = state.registry.clone();
    let cost = state.cost.clone();
    let safety = state.safety.clone();
    let soul = state.soul.clone();
    let store = state.store.clone();
    let session_id = id.clone();

    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);

    tokio::spawn(async move {
        // Build history including the new user message.
        let mut full_history = history;
        full_history.push(user_msg);

        let (stream_tx, mut stream_rx) = tokio::sync::mpsc::channel::<StreamEvent>(64);

        let mut agent = Agent::new(
            provider,
            agent_cfg,
            registry.executors().clone(),
            registry.defs().to_vec(),
        )
        .with_cost_tracker(cost);

        if let Some(safety) = safety {
            agent = agent.with_safety(safety);
        }

        // Forward stream events to SSE in a background task.
        let tx_clone = tx.clone();
        let forward_handle = tokio::spawn(async move {
            while let Some(ev) = stream_rx.recv().await {
                let data = match &ev {
                    StreamEvent::Text { delta, done } => {
                        serde_json::json!({"type": "text", "delta": delta, "done": done})
                    }
                    StreamEvent::ToolStart { tool_call } => {
                        serde_json::json!({"type": "tool_start", "tool": tool_call.name})
                    }
                    StreamEvent::ToolDone { tool_call, output } => {
                        serde_json::json!({"type": "tool_done", "tool": tool_call.name, "output": &output[..output.len().min(500)]})
                    }
                    StreamEvent::Error { message } => {
                        serde_json::json!({"type": "error", "message": message})
                    }
                };
                let event = Event::default().data(serde_json::to_string(&data).unwrap());
                if tx_clone.send(Ok(event)).await.is_err() {
                    break;
                }
            }
        });

        // Set up streaming via channel-based handler.
        let agent = agent.with_stream_handler(move |ev| {
            let _ = stream_tx.try_send(ev);
        });

        let parts = ContextParts {
            soul_prompt: soul,
            history: full_history,
            ..Default::default()
        };

        let cancel = CancellationToken::new();
        let result = agent.run(cancel, parts).await;

        // Wait for forwarding to finish.
        forward_handle.abort();

        // Send final text if the agent produced a response.
        if !result.response.is_empty() {
            let final_event = Event::default().data(
                serde_json::to_string(&serde_json::json!({
                    "type": "text",
                    "delta": result.response,
                    "done": true,
                })).unwrap(),
            );
            let _ = tx.send(Ok(final_event)).await;
        }

        // Persist assistant response.
        let assistant_msg = Message {
            role: Role::Assistant,
            content: result.response,
            timestamp: Utc::now(),
            ..Default::default()
        };
        let _ = store.append_messages(&session_id, &[assistant_msg]).await;

        // Send [DONE] sentinel.
        let _ = tx.send(Ok(Event::default().data("[DONE]"))).await;
    });

    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    Sse::new(stream).into_response()
}

async fn handle_get_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "model": state.model,
        "tasks": state.tasks.len(),
    }))
}

async fn handle_list_tasks(State(state): State<Arc<AppState>>) -> Json<Vec<TaskInfo>> {
    Json(state.tasks.clone())
}

async fn handle_list_channels() -> Response {
    let store = crate::bots::channels::ChannelStore::new();

    let channels = match store.list() {
        Ok(c) => c,
        Err(e) => return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    // Mask tokens for security.
    let safe: Vec<serde_json::Value> = channels
        .iter()
        .map(|ch| {
            serde_json::json!({
                "id": ch.id,
                "provider": ch.provider,
                "name": ch.name,
                "enabled": ch.enabled,
            })
        })
        .collect();

    Json(serde_json::json!({
        "channels": safe,
        "providers": crate::bots::channels::supported_providers(),
    }))
    .into_response()
}

async fn handle_delete_channel(Path(id): Path<String>) -> Response {
    let store = crate::bots::channels::ChannelStore::new();

    if let Err(e) = store.remove(&id) {
        return error_response(StatusCode::INTERNAL_SERVER_ERROR, e.to_string());
    }
    StatusCode::NO_CONTENT.into_response()
}

// ---------------------------------------------------------------------------
// Server entry point
// ---------------------------------------------------------------------------

pub async fn run(
    config: ServerConfig,
    store: Arc<dyn Store>,
    extra: Option<Router>,
) -> anyhow::Result<()> {
    let addr = config.addr.clone();
    let state = Arc::new(AppState {
        store,
        model: config.model,
        soul: config.soul,
        tasks: config.tasks,
        provider: config.provider,
        agent_cfg: config.agent_cfg,
        registry: config.registry,
        cost: config.cost,
        safety: config.safety,
    });

    let mut app = Router::new()
        .route("/", get(handle_ui))
        .route("/app", get(handle_ui))
        .route("/api/sessions", post(handle_create_session))
        .route("/api/sessions", get(handle_list_sessions))
        .route("/api/sessions/{id}", get(handle_get_session))
        .route("/api/sessions/{id}", delete(handle_delete_session))
        .route("/api/sessions/{id}/messages", post(handle_send_message))
        .route("/api/config", get(handle_get_config))
        .route("/api/scheduler/tasks", get(handle_list_tasks))
        .route("/api/channels", get(handle_list_channels))
        .route("/api/channels/{id}", delete(handle_delete_channel))
        .with_state(state);

    if let Some(extra) = extra {
        app = app.merge(extra);
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("server listening on {}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
