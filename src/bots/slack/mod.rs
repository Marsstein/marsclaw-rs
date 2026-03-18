//! Slack bot module using Socket Mode (WebSocket) + Web API.
//!
//! Ported from Go: internal/slack/bot.go

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::connect_async;
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, SafetyCheck};
use crate::config::AgentConfig;
use crate::store::{self, Store};
use crate::tool::Registry;
use crate::types::*;

const SLACK_API: &str = "https://slack.com/api";

/// Slack message chunk limit (practical, not hard).
const MAX_MESSAGE_LEN: usize = 4000;

/// Configuration for creating a Slack bot.
pub struct SlackBotConfig {
    pub bot_token: String,
    pub app_token: String,
    pub provider: Arc<dyn Provider>,
    pub agent_cfg: AgentConfig,
    pub registry: Registry,
    pub safety: Option<Arc<dyn SafetyCheck>>,
    pub cost: Arc<dyn CostRecorder>,
    pub store: Arc<dyn Store>,
    pub soul: String,
}

/// Runs a Slack bot via Socket Mode WebSocket.
pub struct SlackBot {
    bot_token: String,
    app_token: String,
    client: reqwest::Client,
    provider: Arc<dyn Provider>,
    agent_cfg: AgentConfig,
    registry: Registry,
    safety: Option<Arc<dyn SafetyCheck>>,
    cost: Arc<dyn CostRecorder>,
    store: Arc<dyn Store>,
    soul: String,
    bot_id: Mutex<String>,
    sessions: Mutex<HashMap<String, String>>,
}

impl SlackBot {
    pub fn new(cfg: SlackBotConfig) -> Self {
        Self {
            bot_token: cfg.bot_token,
            app_token: cfg.app_token,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            provider: cfg.provider,
            agent_cfg: cfg.agent_cfg,
            registry: cfg.registry,
            safety: cfg.safety,
            cost: cfg.cost,
            store: cfg.store,
            soul: cfg.soul,
            bot_id: Mutex::new(String::new()),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Start the bot: authenticate, open Socket Mode connection, process events.
    pub async fn run(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        tracing::info!("slack bot starting");

        // Verify identity via auth.test.
        let auth = self.slack_api("auth.test", &json!({})).await?;
        let user_id = auth["user_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("auth.test: missing user_id"))?;
        let user = auth["user"].as_str().unwrap_or("unknown");
        *self.bot_id.lock().await = user_id.to_string();
        tracing::info!(user = user, "slack bot ready");

        // Open Socket Mode connection.
        self.socket_mode_loop(cancel).await
    }

    /// Connect to Socket Mode and process envelope events.
    async fn socket_mode_loop(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        // Request a WebSocket URL via apps.connections.open.
        let ws_url = self.get_socket_url().await?;

        let (ws, _) = connect_async(&ws_url).await?;
        let (mut writer, mut reader) = ws.split();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("slack bot shutting down");
                    return Ok(());
                }
                msg = reader.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "socket mode read error");
                            return Err(e.into());
                        }
                        None => {
                            tracing::warn!("socket mode connection closed");
                            return Ok(());
                        }
                    };

                    let text = match msg {
                        WsMessage::Text(t) => t.to_string(),
                        WsMessage::Ping(data) => {
                            writer.send(WsMessage::Pong(data)).await.ok();
                            continue;
                        }
                        WsMessage::Close(_) => {
                            tracing::info!("socket mode sent close frame");
                            return Ok(());
                        }
                        _ => continue,
                    };

                    let envelope: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    // Acknowledge the envelope immediately.
                    if let Some(envelope_id) = envelope["envelope_id"].as_str() {
                        let ack = json!({ "envelope_id": envelope_id });
                        writer.send(WsMessage::Text(ack.to_string().into())).await.ok();
                    }

                    // Handle events_api type envelopes.
                    let envelope_type = envelope["type"].as_str().unwrap_or("");
                    if envelope_type == "events_api" {
                        self.handle_events_api(&envelope["payload"]).await;
                    } else if envelope_type == "hello" {
                        tracing::debug!("socket mode hello received");
                    }
                }
            }
        }
    }

    /// Parse an events_api payload and dispatch message events.
    async fn handle_events_api(&self, payload: &Value) {
        let event = &payload["event"];
        let event_type = event["type"].as_str().unwrap_or("");

        if event_type != "message" && event_type != "app_mention" {
            return;
        }

        // Skip bot messages, subtypes (edits, joins, etc.).
        if event["subtype"].is_string() {
            return;
        }

        let channel = event["channel"].as_str().unwrap_or("").to_string();
        let text = event["text"].as_str().unwrap_or("").to_string();
        let user = event["user"].as_str().unwrap_or("").to_string();

        if text.is_empty() {
            return;
        }

        self.handle_message(&channel, &text, &user).await;
    }

    /// Process a single incoming message.
    async fn handle_message(&self, channel_id: &str, text: &str, user_id: &str) {
        let bot_id = self.bot_id.lock().await.clone();
        if user_id == bot_id {
            return;
        }

        let session_id = self.get_or_create_session(channel_id).await;
        let history = self
            .store
            .get_messages(&session_id)
            .await
            .unwrap_or_default();

        let user_msg = Message {
            role: Role::User,
            content: text.to_string(),
            timestamp: Utc::now(),
            ..Default::default()
        };

        let mut full_history = history;
        full_history.push(user_msg.clone());
        let history_len = full_history.len();

        let mut agent_cfg = self.agent_cfg.clone();
        agent_cfg.enable_streaming = false;

        let mut agent = Agent::new(
            self.provider.clone(),
            agent_cfg,
            self.registry.executors().clone(),
            self.registry.defs().to_vec(),
        )
        .with_cost_tracker(self.cost.clone());

        if let Some(ref safety) = self.safety {
            agent = agent.with_safety(safety.clone());
        }

        let parts = ContextParts {
            soul_prompt: self.soul.clone(),
            history: full_history,
            ..Default::default()
        };

        let cancel = CancellationToken::new();
        let result = agent.run(cancel, parts).await;

        let response = if result.response.is_empty() {
            "I couldn't generate a response.".to_string()
        } else {
            result.response
        };

        self.send_message(channel_id, &response).await.ok();

        // Persist new messages.
        let mut new_msgs = vec![user_msg];
        if result.history.len() > history_len {
            let additional = &result.history[history_len - 1..];
            new_msgs.extend_from_slice(additional);
        }
        self.store
            .append_messages(&session_id, &new_msgs)
            .await
            .ok();
    }

    /// Send a message to a Slack channel, chunking at newline boundaries.
    pub async fn send_message(&self, channel: &str, text: &str) -> anyhow::Result<()> {
        let mut remaining = text;
        while !remaining.is_empty() {
            let chunk;
            if remaining.len() > MAX_MESSAGE_LEN {
                let mut cut = MAX_MESSAGE_LEN;
                for i in (MAX_MESSAGE_LEN / 2..MAX_MESSAGE_LEN).rev() {
                    if remaining.as_bytes()[i] == b'\n' {
                        cut = i + 1;
                        break;
                    }
                }
                chunk = &remaining[..cut];
                remaining = &remaining[cut..];
            } else {
                chunk = remaining;
                remaining = "";
            }

            self.slack_api(
                "chat.postMessage",
                &json!({
                    "channel": channel,
                    "text": chunk,
                }),
            )
            .await?;
        }
        Ok(())
    }

    async fn get_or_create_session(&self, channel_id: &str) -> String {
        let mut sessions = self.sessions.lock().await;
        if let Some(id) = sessions.get(channel_id) {
            return id.clone();
        }

        let id = format!("slack_{}_{}", channel_id, Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let session = store::Session {
            id: id.clone(),
            title: format!("Slack channel {channel_id}"),
            source: "slack".to_string(),
            metadata: Some(json!({ "channel_id": channel_id })),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.store.create_session(&session).await.ok();
        sessions.insert(channel_id.to_string(), id.clone());
        id
    }

    /// Request a Socket Mode WebSocket URL from Slack.
    async fn get_socket_url(&self) -> anyhow::Result<String> {
        let resp = self
            .client
            .post(format!("{SLACK_API}/apps.connections.open"))
            .header("Authorization", format!("Bearer {}", self.app_token))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .send()
            .await?;
        let body = resp.json::<Value>().await?;

        if !body["ok"].as_bool().unwrap_or(false) {
            let err = body["error"].as_str().unwrap_or("unknown error");
            anyhow::bail!("apps.connections.open: {err}");
        }

        let url = body["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("apps.connections.open: missing url"))?;
        Ok(url.to_string())
    }

    // -----------------------------------------------------------------------
    // Slack Web API helper
    // -----------------------------------------------------------------------

    async fn slack_api(&self, method: &str, body: &Value) -> anyhow::Result<Value> {
        let resp = self
            .client
            .post(format!("{SLACK_API}/{method}"))
            .header("Authorization", format!("Bearer {}", self.bot_token))
            .json(body)
            .send()
            .await?;
        let result = resp.json::<Value>().await?;

        if !result["ok"].as_bool().unwrap_or(false) {
            let err_msg = result["error"].as_str().unwrap_or("unknown");
            anyhow::bail!("slack {method}: {err_msg}");
        }

        Ok(result)
    }
}
