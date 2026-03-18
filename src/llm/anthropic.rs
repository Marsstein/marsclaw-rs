//! Anthropic Messages API provider (raw HTTP, no SDK).

use std::collections::HashMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::types::{
    LlmResponse, Message, Provider, ProviderRequest, Role, StreamEvent, ToolCall, ToolDef,
};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const MAX_CONTEXT: i32 = 200_000;

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Talks to the Anthropic Messages API via raw HTTP.
pub struct AnthropicProvider {
    client: Client,
    api_key: String,
    model: String,
}

impl AnthropicProvider {
    pub fn new(api_key: &str, model: &str) -> Self {
        Self {
            client: Client::new(),
            api_key: api_key.to_owned(),
            model: model.to_owned(),
        }
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Provider for AnthropicProvider {
    async fn call(&self, req: &ProviderRequest) -> anyhow::Result<LlmResponse> {
        let api_req = build_request(&self.model, req, false);
        let body = do_request(&self.client, &self.api_key, &api_req).await?;
        let resp: ApiResponse = serde_json::from_str(&body).map_err(|e| {
            anyhow::anyhow!("Anthropic response parse error: {e}\nbody: {body}")
        })?;
        Ok(parse_response(&resp))
    }

    async fn stream(
        &self,
        req: &ProviderRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> anyhow::Result<LlmResponse> {
        let api_req = build_request(&self.model, req, true);
        let resp = send_request(&self.client, &self.api_key, &api_req).await?;

        let mut text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut active_tools: HashMap<i64, ToolBuilder> = HashMap::new();
        let mut model = String::new();
        let mut input_tokens: i32 = 0;
        let mut output_tokens: i32 = 0;

        let full_body = resp.text().await?;

        for line in full_body.lines() {
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };

            let event: SseEvent = match serde_json::from_str(data) {
                Ok(e) => e,
                Err(_) => continue,
            };

            match event.r#type.as_str() {
                "message_start" => {
                    if let Some(ref msg) = event.message {
                        input_tokens = msg.usage.input_tokens;
                        model.clone_from(&msg.model);
                    }
                }
                "content_block_start" => {
                    if let Some(ref cb) = event.content_block {
                        if cb.r#type == "tool_use" {
                            active_tools.insert(
                                event.index,
                                ToolBuilder {
                                    id: cb.id.clone().unwrap_or_default(),
                                    name: cb.name.clone().unwrap_or_default(),
                                    json: String::new(),
                                },
                            );
                            let _ = tx
                                .send(StreamEvent::ToolStart {
                                    tool_call: ToolCall {
                                        id: cb.id.clone().unwrap_or_default(),
                                        name: cb.name.clone().unwrap_or_default(),
                                        arguments: serde_json::Value::Null,
                                    },
                                })
                                .await;
                        }
                    }
                }
                "content_block_delta" => {
                    if let Some(ref delta) = event.delta {
                        match delta.r#type.as_str() {
                            "text_delta" => {
                                let t = delta.text.as_deref().unwrap_or_default();
                                text.push_str(t);
                                let _ = tx
                                    .send(StreamEvent::Text {
                                        delta: t.to_owned(),
                                        done: false,
                                    })
                                    .await;
                            }
                            "input_json_delta" => {
                                if let Some(ref partial) = delta.partial_json {
                                    if let Some(tb) = active_tools.get_mut(&event.index) {
                                        tb.json.push_str(partial);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "content_block_stop" => {
                    if let Some(tb) = active_tools.remove(&event.index) {
                        let raw = if tb.json.is_empty() {
                            "{}".to_owned()
                        } else {
                            tb.json
                        };
                        let args: serde_json::Value = serde_json::from_str(&raw)
                            .unwrap_or(serde_json::Value::Object(Default::default()));
                        tool_calls.push(ToolCall {
                            id: tb.id,
                            name: tb.name,
                            arguments: args,
                        });
                    }
                }
                "message_delta" => {
                    if let Some(ref usage) = event.usage {
                        output_tokens = usage.output_tokens;
                    }
                }
                _ => {}
            }
        }

        Ok(LlmResponse {
            content: text,
            tool_calls,
            input_tokens,
            output_tokens,
            model,
        })
    }

    fn count_tokens(&self, messages: &[Message], tools: &[ToolDef]) -> i32 {
        let mut total: i32 = 0;
        for m in messages {
            total += m.content.len() as i32 / 4;
            for tc in &m.tool_calls {
                total += tc.arguments.to_string().len() as i32 / 4;
            }
            if let Some(ref tr) = m.tool_result {
                total += tr.content.len() as i32 / 4;
            }
        }
        for tool in tools {
            total += tool.description.len() as i32 / 4;
            total += tool.parameters.to_string().len() as i32 / 4;
        }
        total
    }

    fn max_context_window(&self) -> i32 {
        MAX_CONTEXT
    }
}

// ---------------------------------------------------------------------------
// Anthropic wire types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    messages: Vec<ApiMessage>,
    max_tokens: i32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    system: Vec<ApiTextBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    stop_sequences: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<ApiTool>,
    #[serde(skip_serializing_if = "is_false")]
    stream: bool,
}

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Serialize)]
struct ApiTextBlock {
    r#type: String,
    text: String,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: serde_json::Value,
}

#[derive(Serialize)]
struct ApiTool {
    name: String,
    description: String,
    input_schema: serde_json::Value,
}

// --- Response types ---

#[derive(Deserialize, Default)]
struct ApiResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    #[serde(default)]
    usage: ApiUsage,
    #[serde(default)]
    model: String,
}

#[derive(Deserialize, Default)]
struct ContentBlock {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[derive(Deserialize, Default)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: i32,
    #[serde(default)]
    output_tokens: i32,
}

// --- SSE types ---

#[derive(Deserialize, Default)]
struct SseEvent {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    index: i64,
    #[serde(default)]
    message: Option<SseMessage>,
    #[serde(default)]
    content_block: Option<ContentBlock>,
    #[serde(default)]
    delta: Option<SseDelta>,
    #[serde(default)]
    usage: Option<ApiUsage>,
}

#[derive(Deserialize, Default)]
struct SseMessage {
    #[serde(default)]
    model: String,
    #[serde(default)]
    usage: ApiUsage,
}

#[derive(Deserialize, Default)]
struct SseDelta {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    partial_json: Option<String>,
}

struct ToolBuilder {
    id: String,
    name: String,
    json: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_request(default_model: &str, req: &ProviderRequest, stream: bool) -> ApiRequest {
    let model = if req.model.is_empty() {
        default_model.to_owned()
    } else {
        req.model.clone()
    };

    let mut system: Vec<ApiTextBlock> = Vec::new();
    let mut messages: Vec<ApiMessage> = Vec::new();

    for m in &req.messages {
        match m.role {
            Role::System => {
                system.push(ApiTextBlock {
                    r#type: "text".to_owned(),
                    text: m.content.clone(),
                });
            }
            Role::User => {
                messages.push(ApiMessage {
                    role: "user".to_owned(),
                    content: serde_json::Value::String(m.content.clone()),
                });
            }
            Role::Assistant => {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(serde_json::json!({
                        "type": "text",
                        "text": m.content,
                    }));
                }
                for tc in &m.tool_calls {
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": tc.arguments,
                    }));
                }
                if !blocks.is_empty() {
                    messages.push(ApiMessage {
                        role: "assistant".to_owned(),
                        content: serde_json::Value::Array(blocks),
                    });
                }
            }
            Role::Tool => {
                if let Some(ref tr) = m.tool_result {
                    let block = serde_json::json!([{
                        "type": "tool_result",
                        "tool_use_id": tr.call_id,
                        "content": tr.content,
                        "is_error": tr.is_error,
                    }]);
                    messages.push(ApiMessage {
                        role: "user".to_owned(),
                        content: block,
                    });
                }
            }
        }
    }

    let max_tokens = if req.max_tokens > 0 {
        req.max_tokens
    } else {
        4096
    };

    let temperature = if req.temperature > 0.0 {
        Some(req.temperature)
    } else {
        None
    };

    ApiRequest {
        model,
        messages,
        max_tokens,
        system,
        temperature,
        stop_sequences: req.stop.clone(),
        tools: req
            .tools
            .iter()
            .map(|td| ApiTool {
                name: td.name.clone(),
                description: td.description.clone(),
                input_schema: td.parameters.clone(),
            })
            .collect(),
        stream,
    }
}

/// Send the HTTP request and return the raw `reqwest::Response` (for streaming).
async fn send_request(
    client: &Client,
    api_key: &str,
    api_req: &ApiRequest,
) -> anyhow::Result<reqwest::Response> {
    let resp = client
        .post(API_URL)
        .header("Content-Type", "application/json")
        .header("x-api-key", api_key)
        .header("anthropic-version", API_VERSION)
        .json(api_req)
        .send()
        .await?;

    if resp.status().is_client_error() || resp.status().is_server_error() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("Anthropic API error {status}: {body}");
    }

    Ok(resp)
}

/// Send the HTTP request and return the full body as a string (non-streaming).
async fn do_request(
    client: &Client,
    api_key: &str,
    api_req: &ApiRequest,
) -> anyhow::Result<String> {
    let resp = send_request(client, api_key, api_req).await?;
    let body = resp.text().await?;
    Ok(body)
}

fn parse_response(resp: &ApiResponse) -> LlmResponse {
    let mut result = LlmResponse {
        input_tokens: resp.usage.input_tokens,
        output_tokens: resp.usage.output_tokens,
        model: resp.model.clone(),
        ..Default::default()
    };

    for block in &resp.content {
        match block.r#type.as_str() {
            "text" => {
                if let Some(ref t) = block.text {
                    result.content.push_str(t);
                }
            }
            "tool_use" => {
                result.tool_calls.push(ToolCall {
                    id: block.id.clone().unwrap_or_default(),
                    name: block.name.clone().unwrap_or_default(),
                    arguments: block
                        .input
                        .clone()
                        .unwrap_or(serde_json::Value::Object(Default::default())),
                });
            }
            _ => {}
        }
    }

    result
}
