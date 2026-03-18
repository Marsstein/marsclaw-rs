//! HTTP server with Axum, serving the API and embedded Web UI.
//!
//! Ported from Go: internal/server/server.go

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
use crate::store::{Session, Store};
use crate::types::Message;

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
    config: ServerConfig,
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
    let _history = state.store.get_messages(&id).await.unwrap_or_default();

    let user_msg = Message {
        role: crate::types::Role::User,
        content: body.message.clone(),
        timestamp: Utc::now(),
        ..Default::default()
    };

    // Persist the user message.
    let _ = state.store.append_messages(&id, &[user_msg]).await;

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

    // Return SSE stream. The agent integration will be wired later;
    // for now we emit a placeholder response and [DONE].
    let stream = futures::stream::iter(vec![
        Ok::<_, Infallible>(
            Event::default()
                .data(serde_json::to_string(&serde_json::json!({"type": "text", "delta": "Agent not yet connected.", "done": true})).unwrap()),
        ),
        Ok(Event::default().data("[DONE]")),
    ]);

    Sse::new(stream).into_response()
}

async fn handle_get_config(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "model": state.config.model,
        "tasks": state.config.tasks.len(),
    }))
}

async fn handle_list_tasks(State(state): State<Arc<AppState>>) -> Json<Vec<TaskInfo>> {
    Json(state.config.tasks.clone())
}

async fn handle_list_channels() -> Response {
    let store = crate::channels::ChannelStore::new();

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
        "providers": crate::channels::supported_providers(),
    }))
    .into_response()
}

async fn handle_delete_channel(Path(id): Path<String>) -> Response {
    let store = crate::channels::ChannelStore::new();

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
    let state = Arc::new(AppState { store, config });

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
