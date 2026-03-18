//! WhatsApp Cloud API webhook bot.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::store::{Session, Store};
use crate::tool::Registry;
use crate::types::*;

const GRAPH_API: &str = "https://graph.facebook.com/v21.0";

pub struct WhatsAppBot {
    phone_number_id: String,
    access_token: String,
    verify_token: String,
    provider: Arc<dyn Provider>,
    config: AgentConfig,
    registry: Registry,
    cost: Arc<dyn CostRecorder>,
    store: Arc<dyn Store>,
    soul: String,
    model: String,
    client: reqwest::Client,
    sessions: Mutex<HashMap<String, String>>,
}

impl WhatsAppBot {
    pub fn new(
        phone_number_id: &str,
        access_token: &str,
        verify_token: &str,
        provider: Arc<dyn Provider>,
        config: AgentConfig,
        registry: Registry,
        cost: Arc<dyn CostRecorder>,
        store: Arc<dyn Store>,
        soul: &str,
        model: &str,
    ) -> Self {
        Self {
            phone_number_id: phone_number_id.to_string(),
            access_token: access_token.to_string(),
            verify_token: verify_token.to_string(),
            provider,
            config,
            registry,
            cost,
            store,
            soul: soul.to_string(),
            model: model.to_string(),
            client: reqwest::Client::new(),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// Returns an Axum router for the WhatsApp webhook.
    pub fn router(self: Arc<Self>) -> Router {
        Router::new()
            .route("/webhook/whatsapp", get(handle_verify))
            .route("/webhook/whatsapp", post(handle_webhook))
            .with_state(self)
    }

    async fn handle_message(self: &Arc<Self>, from: &str, text: &str) {
        tracing::info!(from = from, "whatsapp message");

        let session_id = self.get_or_create_session(from).await;
        let history = self
            .store
            .get_messages(&session_id)
            .await
            .unwrap_or_default();

        let user_msg = Message {
            role: Role::User,
            content: text.to_string(),
            timestamp: chrono::Utc::now(),
            ..Default::default()
        };

        let mut full_history = history.clone();
        full_history.push(user_msg.clone());

        let mut config = self.config.clone();
        config.enable_streaming = false;

        let agent = Agent::new(
            self.provider.clone(),
            config,
            self.registry.executors().clone(),
            self.registry.defs().to_vec(),
        )
        .with_cost_tracker(self.cost.clone());

        let parts = ContextParts {
            soul_prompt: self.soul.clone(),
            history: full_history.clone(),
            ..Default::default()
        };

        let cancel = tokio_util::sync::CancellationToken::new();
        let result = agent.run(cancel, parts).await;

        let response = if result.response.is_empty() {
            "I couldn't generate a response.".to_string()
        } else {
            result.response
        };

        self.send_chunked(from, &response, 4096).await;

        // Persist messages.
        let mut new_msgs = vec![user_msg];
        if result.history.len() > full_history.len() {
            new_msgs.extend_from_slice(&result.history[full_history.len() - 1..]);
        }
        let _ = self.store.append_messages(&session_id, &new_msgs).await;
    }

    async fn send_chunked(&self, to: &str, text: &str, max_len: usize) {
        let mut remaining = text;
        while !remaining.is_empty() {
            let chunk = if remaining.len() > max_len {
                let mut cut = max_len;
                for i in (max_len / 2..max_len).rev() {
                    if remaining.as_bytes().get(i) == Some(&b'\n') {
                        cut = i + 1;
                        break;
                    }
                }
                let c = &remaining[..cut];
                remaining = &remaining[cut..];
                c
            } else {
                let c = remaining;
                remaining = "";
                c
            };

            if let Err(e) = self.send_text(to, chunk).await {
                tracing::error!(error = %e, "whatsapp send failed");
                break;
            }
        }
    }

    async fn send_text(&self, to: &str, text: &str) -> anyhow::Result<()> {
        let url = format!("{}/{}/messages", GRAPH_API, self.phone_number_id);
        let payload = serde_json::json!({
            "messaging_product": "whatsapp",
            "to": to,
            "type": "text",
            "text": { "body": text },
        });

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.access_token)
            .json(&payload)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("whatsapp API {}: {}", status, body);
        }
        Ok(())
    }

    async fn get_or_create_session(&self, phone: &str) -> String {
        let mut sessions = self.sessions.lock().await;
        if let Some(id) = sessions.get(phone) {
            return id.clone();
        }

        let now = chrono::Utc::now();
        let id = format!(
            "wa_{}_{}",
            phone,
            now.timestamp_nanos_opt().unwrap_or_default()
        );

        let session = Session {
            id: id.clone(),
            title: format!("WhatsApp {}", phone),
            source: "whatsapp".to_string(),
            metadata: Some(serde_json::json!({"phone": phone})),
            created_at: now,
            updated_at: now,
        };
        let _ = self.store.create_session(&session).await;
        sessions.insert(phone.to_string(), id.clone());
        id
    }
}

// --- Webhook handlers ---

#[derive(Deserialize)]
struct VerifyQuery {
    #[serde(rename = "hub.mode")]
    mode: Option<String>,
    #[serde(rename = "hub.verify_token")]
    verify_token: Option<String>,
    #[serde(rename = "hub.challenge")]
    challenge: Option<String>,
}

async fn handle_verify(
    State(bot): State<Arc<WhatsAppBot>>,
    Query(q): Query<VerifyQuery>,
) -> impl IntoResponse {
    if q.mode.as_deref() == Some("subscribe")
        && q.verify_token.as_deref() == Some(&bot.verify_token)
    {
        (StatusCode::OK, q.challenge.unwrap_or_default())
    } else {
        (StatusCode::FORBIDDEN, "forbidden".to_string())
    }
}

#[derive(Deserialize)]
struct WebhookPayload {
    entry: Vec<WebhookEntry>,
}

#[derive(Deserialize)]
struct WebhookEntry {
    changes: Vec<WebhookChange>,
}

#[derive(Deserialize)]
struct WebhookChange {
    field: String,
    value: WebhookValue,
}

#[derive(Deserialize)]
struct WebhookValue {
    #[serde(default)]
    messages: Vec<WebhookMessage>,
}

#[derive(Deserialize)]
struct WebhookMessage {
    from: String,
    #[serde(rename = "type")]
    msg_type: String,
    #[serde(default)]
    text: WebhookText,
}

#[derive(Deserialize, Default)]
struct WebhookText {
    #[serde(default)]
    body: String,
}

async fn handle_webhook(
    State(bot): State<Arc<WhatsAppBot>>,
    Json(payload): Json<WebhookPayload>,
) -> StatusCode {
    for entry in &payload.entry {
        for change in &entry.changes {
            if change.field != "messages" {
                continue;
            }
            for msg in &change.value.messages {
                if msg.msg_type != "text" {
                    continue;
                }
                let bot = bot.clone();
                let from = msg.from.clone();
                let body = msg.text.body.clone();
                tokio::spawn(async move {
                    bot.handle_message(&from, &body).await;
                });
            }
        }
    }
    StatusCode::OK
}
