//! Telegram bot integration via long-polling getUpdates API.
//!
//! Ported from Go: internal/telegram/bot.go + api.go

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::store::{self, Store};
use crate::tool::Registry;
use crate::types::*;

// ---------------------------------------------------------------------------
// Telegram API types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    ok: bool,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: i64,
    message: Option<TgMessage>,
}

#[derive(Debug, Deserialize)]
struct TgMessage {
    #[allow(dead_code)]
    message_id: i64,
    chat: TgChat,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TgChat {
    id: i64,
}

// ---------------------------------------------------------------------------
// Bot
// ---------------------------------------------------------------------------

/// Telegram bot that forwards messages to the MarsClaw agent.
pub struct TelegramBot {
    client: reqwest::Client,
    api_url: String,
    provider: Arc<dyn Provider>,
    config: AgentConfig,
    registry: Registry,
    cost: Arc<dyn CostRecorder>,
    store: Arc<dyn Store>,
    soul: String,
    model: String,
    sessions: Mutex<HashMap<i64, String>>,
}

impl TelegramBot {
    pub fn new(
        token: &str,
        provider: Arc<dyn Provider>,
        config: AgentConfig,
        registry: Registry,
        cost: Arc<dyn CostRecorder>,
        store: Arc<dyn Store>,
        soul: &str,
        model: &str,
    ) -> Self {
        Self {
            api_url: format!("https://api.telegram.org/bot{token}"),
            client: reqwest::Client::new(),
            provider,
            config,
            registry,
            cost,
            store,
            soul: soul.to_owned(),
            model: model.to_owned(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Start the long-polling loop. Blocks until cancelled or fatal error.
    pub async fn run(&self) -> anyhow::Result<()> {
        info!("telegram bot starting");

        let mut offset: i64 = 0;
        loop {
            let updates = match self.get_updates(offset).await {
                Ok(u) => u,
                Err(e) => {
                    error!(error = %e, "get updates failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };

            for update in updates {
                offset = update.update_id + 1;
                let msg = match update.message {
                    Some(ref m) if m.text.is_some() => m,
                    _ => continue,
                };
                self.handle_message(msg).await;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Telegram API calls
    // -----------------------------------------------------------------------

    async fn get_updates(&self, offset: i64) -> anyhow::Result<Vec<Update>> {
        let body = serde_json::json!({
            "offset": offset,
            "timeout": 30,
            "allowed_updates": ["message"],
        });

        let resp = self
            .client
            .post(format!("{}/getUpdates", self.api_url))
            .json(&body)
            .timeout(Duration::from_secs(35))
            .send()
            .await?;

        let api: ApiResponse<Vec<Update>> = resp.json().await?;
        if !api.ok {
            anyhow::bail!("telegram API error on getUpdates");
        }
        Ok(api.result.unwrap_or_default())
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        self.send_chunked(chat_id, text).await
    }

    async fn send_typing(&self, chat_id: i64) -> anyhow::Result<()> {
        let body = serde_json::json!({
            "chat_id": chat_id,
            "action": "typing",
        });
        self.post("/sendChatAction", &body).await
    }

    async fn send_chunked(&self, chat_id: i64, text: &str) -> anyhow::Result<()> {
        const MAX_LEN: usize = 4096;
        let mut remaining = text;

        while !remaining.is_empty() {
            let chunk = if remaining.len() <= MAX_LEN {
                let c = remaining;
                remaining = "";
                c
            } else {
                let cut = find_split_point(remaining, MAX_LEN);
                let c = &remaining[..cut];
                remaining = &remaining[cut..];
                c
            };

            let body = serde_json::json!({
                "chat_id": chat_id,
                "text": chunk,
                "parse_mode": "Markdown",
            });
            self.post("/sendMessage", &body).await?;
        }
        Ok(())
    }

    async fn post(&self, method: &str, body: &serde_json::Value) -> anyhow::Result<()> {
        let resp = self
            .client
            .post(format!("{}{method}", self.api_url))
            .json(body)
            .send()
            .await?;

        if resp.status().as_u16() >= 400 {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            anyhow::bail!("telegram {method} error {status}: {err_body}");
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Message handling
    // -----------------------------------------------------------------------

    async fn handle_message(&self, msg: &TgMessage) {
        let chat_id = msg.chat.id;
        let text = msg.text.as_deref().unwrap_or("");

        match text {
            "/start" => {
                self.send_message(chat_id, "Welcome to MarsClaw! Send me any message and I'll help you.")
                    .await
                    .ok();
            }
            "/clear" => {
                self.sessions.lock().await.remove(&chat_id);
                self.send_message(chat_id, "Conversation cleared.")
                    .await
                    .ok();
            }
            "/help" => {
                self.send_message(
                    chat_id,
                    "/start - Welcome\n/clear - New conversation\n/help - This message\n\nJust type anything to chat!",
                )
                .await
                .ok();
            }
            _ => {
                self.process_message(chat_id, text).await;
            }
        }
    }

    async fn process_message(&self, chat_id: i64, text: &str) {
        self.send_typing(chat_id).await.ok();

        let session_id = self.get_or_create_session(chat_id).await;

        let history = self
            .store
            .get_messages(&session_id)
            .await
            .unwrap_or_default();

        let user_msg = Message {
            role: Role::User,
            content: text.to_owned(),
            ..Default::default()
        };
        let mut full_history = history.clone();
        full_history.push(user_msg.clone());

        // Disable streaming for Telegram (no real-time display).
        let mut agent_cfg = self.config.clone();
        agent_cfg.enable_streaming = false;

        let agent = Agent::new(
            self.provider.clone(),
            agent_cfg,
            self.registry.executors().clone(),
            self.registry.defs().to_vec(),
        )
        .with_cost_tracker(self.cost.clone());

        let parts = ContextParts {
            soul_prompt: self.soul.clone(),
            history: full_history.clone(),
            ..Default::default()
        };

        // Keep typing indicator alive while agent runs.
        let typing_cancel = CancellationToken::new();
        let typing_token = typing_cancel.clone();
        let typing_client = self.client.clone();
        let typing_url = format!("{}/sendChatAction", self.api_url);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(4));
            loop {
                interval.tick().await;
                if typing_token.is_cancelled() {
                    return;
                }
                let body = serde_json::json!({
                    "chat_id": chat_id,
                    "action": "typing",
                });
                typing_client.post(&typing_url).json(&body).send().await.ok();
            }
        });

        let cancel = CancellationToken::new();
        let result = agent.run(cancel, parts).await;
        typing_cancel.cancel();

        // Build response.
        let mut response = result.response.clone();
        if response.is_empty() {
            response = "I couldn't generate a response.".into();
        }

        let cost_line = self
            .cost
            .format_cost_line(&self.model, result.total_input, result.total_output);
        response.push_str("\n\n`");
        response.push_str(&cost_line);
        response.push('`');

        if let Err(e) = self.send_message(chat_id, &response).await {
            error!(chat_id, error = %e, "send message failed");
        }

        // Persist new messages to the store.
        let mut new_msgs = vec![user_msg];
        if result.history.len() > full_history.len() {
            new_msgs.extend_from_slice(&result.history[full_history.len() - 1..]);
        }
        if let Err(e) = self.store.append_messages(&session_id, &new_msgs).await {
            warn!(session_id, error = %e, "failed to persist messages");
        }
    }

    async fn get_or_create_session(&self, chat_id: i64) -> String {
        let mut sessions = self.sessions.lock().await;

        if let Some(id) = sessions.get(&chat_id) {
            return id.clone();
        }

        let now = chrono::Utc::now();
        let id = format!("tg_{}_{}", chat_id, now.timestamp_nanos_opt().unwrap_or(0));

        let session = store::Session {
            id: id.clone(),
            title: format!("Telegram chat {chat_id}"),
            source: "telegram".into(),
            metadata: Some(serde_json::json!({"chat_id": chat_id.to_string()})),
            created_at: now,
            updated_at: now,
        };

        if let Err(e) = self.store.create_session(&session).await {
            warn!(error = %e, "failed to create session for chat {chat_id}");
        }

        sessions.insert(chat_id, id.clone());
        id
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find the best split point for chunking text at Telegram's 4096 char limit.
/// Tries to break at a newline in the second half of the chunk.
fn find_split_point(text: &str, max_len: usize) -> usize {
    let half = max_len / 2;
    for i in (half..max_len).rev() {
        if text.as_bytes().get(i) == Some(&b'\n') {
            return i + 1;
        }
    }
    max_len
}
