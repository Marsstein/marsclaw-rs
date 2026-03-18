//! Discord bot module using REST API + Gateway WebSocket.
//!
//! Ported from Go: internal/discord/bot.go

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream};
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, SafetyCheck};
use crate::config::AgentConfig;
use crate::store::{self, Store};
use crate::tool::Registry;
use crate::types::*;

const API_BASE: &str = "https://discord.com/api/v10";
const GATEWAY_URL: &str = "wss://gateway.discord.gg/?v=10&encoding=json";

/// Discord message length limit.
const MAX_MESSAGE_LEN: usize = 2000;

/// Gateway opcodes.
const OP_DISPATCH: u64 = 0;
const OP_HEARTBEAT: u64 = 1;
const OP_IDENTIFY: u64 = 2;
const OP_HELLO: u64 = 10;
const OP_HEARTBEAT_ACK: u64 = 11;

/// Intents: GUILDS | GUILD_MESSAGES | MESSAGE_CONTENT | DIRECT_MESSAGES.
const INTENTS: u64 = (1 << 0) | (1 << 9) | (1 << 15) | (1 << 12);

type WsWriter =
    SplitSink<tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, WsMessage>;
type WsReader =
    SplitStream<tokio_tungstenite::WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>;

/// Configuration for creating a Discord bot.
pub struct DiscordBotConfig {
    pub token: String,
    pub provider: Arc<dyn Provider>,
    pub agent_cfg: AgentConfig,
    pub registry: Registry,
    pub safety: Option<Arc<dyn SafetyCheck>>,
    pub cost: Arc<dyn CostRecorder>,
    pub store: Arc<dyn Store>,
    pub soul: String,
}

/// Runs a Discord bot via the Gateway WebSocket.
pub struct DiscordBot {
    token: String,
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

impl DiscordBot {
    pub fn new(cfg: DiscordBotConfig) -> Self {
        Self {
            token: cfg.token,
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

    /// Start the bot: authenticate, connect to Gateway, and process events.
    pub async fn run(&self, cancel: CancellationToken) -> anyhow::Result<()> {
        tracing::info!("discord bot starting");

        // Get bot identity via REST.
        let me = self.api_get("/users/@me").await?;
        let id = me["id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing bot id from /users/@me"))?;
        let username = me["username"].as_str().unwrap_or("unknown");
        *self.bot_id.lock().await = id.to_string();
        tracing::info!(user = username, "discord bot ready");

        // Connect to Gateway WebSocket.
        let (ws, _) = connect_async(GATEWAY_URL).await?;
        let (writer, reader) = ws.split();
        let writer = Arc::new(Mutex::new(writer));

        self.gateway_loop(writer, reader, cancel).await
    }

    /// Main Gateway event loop: identify, heartbeat, and dispatch.
    async fn gateway_loop(
        &self,
        writer: Arc<Mutex<WsWriter>>,
        mut reader: WsReader,
        cancel: CancellationToken,
    ) -> anyhow::Result<()> {
        let mut heartbeat_interval: Option<tokio::time::Interval> = None;
        let mut sequence: Option<u64> = None;

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    tracing::info!("discord bot shutting down");
                    return Ok(());
                }
                _ = async {
                    if let Some(ref mut iv) = heartbeat_interval {
                        iv.tick().await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    let payload = json!({ "op": OP_HEARTBEAT, "d": sequence });
                    let mut w = writer.lock().await;
                    w.send(WsMessage::Text(payload.to_string().into())).await.ok();
                }
                msg = reader.next() => {
                    let msg = match msg {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            tracing::error!(error = %e, "gateway read error");
                            return Err(e.into());
                        }
                        None => {
                            tracing::warn!("gateway connection closed");
                            return Ok(());
                        }
                    };

                    let text = match msg {
                        WsMessage::Text(t) => t.to_string(),
                        WsMessage::Close(_) => {
                            tracing::info!("gateway sent close frame");
                            return Ok(());
                        }
                        _ => continue,
                    };

                    let event: Value = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    let op = event["op"].as_u64().unwrap_or(u64::MAX);

                    // Track sequence number for heartbeats.
                    if let Some(s) = event["s"].as_u64() {
                        sequence = Some(s);
                    }

                    match op {
                        OP_HELLO => {
                            let interval_ms = event["d"]["heartbeat_interval"]
                                .as_u64()
                                .unwrap_or(41250);
                            heartbeat_interval = Some(tokio::time::interval(
                                Duration::from_millis(interval_ms),
                            ));

                            // Send IDENTIFY.
                            let identify = json!({
                                "op": OP_IDENTIFY,
                                "d": {
                                    "token": self.token,
                                    "intents": INTENTS,
                                    "properties": {
                                        "os": "linux",
                                        "browser": "marsclaw",
                                        "device": "marsclaw",
                                    }
                                }
                            });
                            let mut w = writer.lock().await;
                            w.send(WsMessage::Text(identify.to_string().into())).await?;
                        }
                        OP_HEARTBEAT_ACK => {}
                        OP_DISPATCH => {
                            let event_name = event["t"].as_str().unwrap_or("");
                            if event_name == "MESSAGE_CREATE" {
                                let d = &event["d"];
                                let channel_id = d["channel_id"].as_str().unwrap_or("").to_string();
                                let content = d["content"].as_str().unwrap_or("").to_string();
                                let author_id = d["author"]["id"].as_str().unwrap_or("").to_string();

                                if !content.is_empty() {
                                    self.handle_message(&channel_id, &content, &author_id).await;
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Process a single incoming message.
    async fn handle_message(&self, channel_id: &str, content: &str, author_id: &str) {
        let bot_id = self.bot_id.lock().await.clone();
        if author_id == bot_id {
            return;
        }

        // Show typing indicator.
        self.send_typing(channel_id).await.ok();

        let session_id = self.get_or_create_session(channel_id).await;
        let history = self
            .store
            .get_messages(&session_id)
            .await
            .unwrap_or_default();

        let user_msg = Message {
            role: Role::User,
            content: content.to_string(),
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

        self.send_chunked(channel_id, &response).await.ok();

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

    /// Send a message, splitting at newline boundaries to stay under the limit.
    pub async fn send_message(&self, channel_id: &str, text: &str) -> anyhow::Result<()> {
        self.send_chunked(channel_id, text).await
    }

    async fn send_chunked(&self, channel_id: &str, text: &str) -> anyhow::Result<()> {
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

            self.api_post(
                &format!("/channels/{channel_id}/messages"),
                &json!({ "content": chunk }),
            )
            .await?;
        }
        Ok(())
    }

    async fn send_typing(&self, channel_id: &str) -> anyhow::Result<()> {
        self.api_post(&format!("/channels/{channel_id}/typing"), &json!({}))
            .await?;
        Ok(())
    }

    async fn get_or_create_session(&self, channel_id: &str) -> String {
        let mut sessions = self.sessions.lock().await;
        if let Some(id) = sessions.get(channel_id) {
            return id.clone();
        }

        let id = format!("discord_{}_{}", channel_id, Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let session = store::Session {
            id: id.clone(),
            title: format!("Discord channel {channel_id}"),
            source: "discord".to_string(),
            metadata: Some(json!({ "channel_id": channel_id })),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        self.store.create_session(&session).await.ok();
        sessions.insert(channel_id.to_string(), id.clone());
        id
    }

    // -----------------------------------------------------------------------
    // HTTP helpers
    // -----------------------------------------------------------------------

    async fn api_get(&self, path: &str) -> anyhow::Result<Value> {
        let resp = self
            .client
            .get(format!("{API_BASE}{path}"))
            .header("Authorization", format!("Bot {}", self.token))
            .send()
            .await?;
        let body = resp.json::<Value>().await?;
        Ok(body)
    }

    async fn api_post(&self, path: &str, body: &Value) -> anyhow::Result<Value> {
        let resp = self
            .client
            .post(format!("{API_BASE}{path}"))
            .header("Authorization", format!("Bot {}", self.token))
            .json(body)
            .send()
            .await?;
        let result = resp.json::<Value>().await?;
        Ok(result)
    }
}
