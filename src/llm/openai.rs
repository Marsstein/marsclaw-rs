//! OpenAI-compatible provider (also serves Gemini and Ollama).

use std::collections::HashMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::types::{
    LlmResponse, Message, Provider, ProviderRequest, Role, StreamEvent, ToolCall, ToolDef,
};

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Talks to any OpenAI-compatible chat/completions endpoint.
pub struct OpenAiProvider {
    client: Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAiProvider {
    pub fn new(api_key: &str, base_url: &str, model: &str) -> Self {
        let base = if base_url.is_empty() {
            "https://api.openai.com/v1".to_owned()
        } else {
            base_url.trim_end_matches('/').to_owned()
        };

        Self {
            client: Client::new(),
            api_key: api_key.to_owned(),
            base_url: base,
            model: model.to_owned(),
        }
    }

    /// Convenience constructor for Google Gemini's OpenAI-compatible endpoint.
    pub fn gemini(api_key: &str, model: &str) -> Self {
        Self::new(
            api_key,
            "https://generativelanguage.googleapis.com/v1beta/openai",
            model,
        )
    }

    /// Convenience constructor for a local Ollama instance.
    pub fn ollama(model: &str) -> Self {
        Self::new("ollama", "http://localhost:11434/v1", model)
    }

    fn max_context_for_model(&self) -> i32 {
        if self.base_url.contains("generativelanguage.googleapis.com") {
            return 1_048_576;
        }
        if self.base_url.contains("localhost:11434") {
            return 32_000;
        }
        128_000
    }
}

// ---------------------------------------------------------------------------
// Provider trait
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    async fn call(&self, req: &ProviderRequest) -> anyhow::Result<LlmResponse> {
        let oai_req = build_request(&self.model, req, false);
        let body = do_request(&self.client, &self.api_key, &self.base_url, &oai_req).await?;
        let resp: OaiResponse = serde_json::from_str(&body)?;
        Ok(parse_response(&resp))
    }

    async fn stream(
        &self,
        req: &ProviderRequest,
        tx: mpsc::Sender<StreamEvent>,
    ) -> anyhow::Result<LlmResponse> {
        let oai_req = build_request(&self.model, req, true);
        let resp = send_request(&self.client, &self.api_key, &self.base_url, &oai_req).await?;

        let mut text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut active_tools: HashMap<usize, OaiToolCall> = HashMap::new();
        let mut model = String::new();
        let mut input_tokens: i32 = 0;
        let mut output_tokens: i32 = 0;

        let full_body = resp.text().await?;

        for line in full_body.lines() {
            let Some(data) = line.strip_prefix("data: ") else {
                continue;
            };
            if data == "[DONE]" {
                break;
            }

            let chunk: OaiResponse = match serde_json::from_str(data) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if !chunk.model.is_empty() {
                model.clone_from(&chunk.model);
            }
            if chunk.usage.prompt_tokens > 0 {
                input_tokens = chunk.usage.prompt_tokens;
                output_tokens = chunk.usage.completion_tokens;
            }
            if chunk.choices.is_empty() {
                continue;
            }

            let delta = &chunk.choices[0].delta;

            if !delta.content.is_empty() {
                text.push_str(&delta.content);
                let _ = tx
                    .send(StreamEvent::Text {
                        delta: delta.content.clone(),
                        done: false,
                    })
                    .await;
            }

            for (i, tc) in delta.tool_calls.iter().enumerate() {
                if !tc.id.is_empty() {
                    active_tools.insert(
                        i,
                        OaiToolCall {
                            id: tc.id.clone(),
                            r#type: "function".to_owned(),
                            function: OaiFunction {
                                name: tc.function.name.clone(),
                                arguments: String::new(),
                            },
                        },
                    );
                    let _ = tx
                        .send(StreamEvent::ToolStart {
                            tool_call: ToolCall {
                                id: tc.id.clone(),
                                name: tc.function.name.clone(),
                                arguments: serde_json::Value::Null,
                            },
                        })
                        .await;
                }
                if let Some(at) = active_tools.get_mut(&i) {
                    at.function.arguments.push_str(&tc.function.arguments);
                }
            }
        }

        for tc in active_tools.into_values() {
            let args_str = if tc.function.arguments.is_empty() {
                "{}".to_owned()
            } else {
                tc.function.arguments
            };
            let args: serde_json::Value =
                serde_json::from_str(&args_str).unwrap_or(serde_json::Value::Object(Default::default()));
            tool_calls.push(ToolCall {
                id: tc.id,
                name: tc.function.name,
                arguments: args,
            });
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
        self.max_context_for_model()
    }
}

// ---------------------------------------------------------------------------
// OpenAI wire types (private)
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct OaiRequest {
    model: String,
    messages: Vec<OaiMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OaiTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "is_false")]
    stream: bool,
}

fn is_false(v: &bool) -> bool {
    !v
}

#[derive(Serialize, Deserialize, Default)]
struct OaiMessage {
    role: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_calls: Vec<OaiToolCall>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    tool_call_id: String,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct OaiToolCall {
    #[serde(default)]
    id: String,
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    function: OaiFunction,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct OaiFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Serialize)]
struct OaiTool {
    r#type: String,
    function: OaiToolFunction,
}

#[derive(Serialize)]
struct OaiToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct OaiResponse {
    #[serde(default)]
    choices: Vec<OaiChoice>,
    #[serde(default)]
    usage: OaiUsage,
    #[serde(default)]
    model: String,
}

#[derive(Deserialize, Default)]
struct OaiChoice {
    #[serde(default)]
    message: OaiMessage,
    #[serde(default)]
    delta: OaiMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct OaiUsage {
    #[serde(default)]
    prompt_tokens: i32,
    #[serde(default)]
    completion_tokens: i32,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_request(default_model: &str, req: &ProviderRequest, stream: bool) -> OaiRequest {
    let model = if req.model.is_empty() {
        default_model.to_owned()
    } else {
        req.model.clone()
    };

    let messages: Vec<OaiMessage> = req
        .messages
        .iter()
        .filter_map(|m| match m.role {
            Role::System => Some(OaiMessage {
                role: "system".to_owned(),
                content: m.content.clone(),
                ..Default::default()
            }),
            Role::User => Some(OaiMessage {
                role: "user".to_owned(),
                content: m.content.clone(),
                ..Default::default()
            }),
            Role::Assistant => {
                let tool_calls: Vec<OaiToolCall> = m
                    .tool_calls
                    .iter()
                    .map(|tc| OaiToolCall {
                        id: tc.id.clone(),
                        r#type: "function".to_owned(),
                        function: OaiFunction {
                            name: tc.name.clone(),
                            arguments: tc.arguments.to_string(),
                        },
                    })
                    .collect();
                Some(OaiMessage {
                    role: "assistant".to_owned(),
                    content: m.content.clone(),
                    tool_calls,
                    ..Default::default()
                })
            }
            Role::Tool => m.tool_result.as_ref().map(|tr| OaiMessage {
                role: "tool".to_owned(),
                content: tr.content.clone(),
                tool_call_id: tr.call_id.clone(),
                ..Default::default()
            }),
        })
        .collect();

    let tools: Vec<OaiTool> = req
        .tools
        .iter()
        .map(|td| OaiTool {
            r#type: "function".to_owned(),
            function: OaiToolFunction {
                name: td.name.clone(),
                description: td.description.clone(),
                parameters: td.parameters.clone(),
            },
        })
        .collect();

    let temperature = if req.temperature > 0.0 {
        Some(req.temperature)
    } else {
        None
    };

    let max_tokens = if req.max_tokens > 0 {
        Some(req.max_tokens)
    } else {
        None
    };

    OaiRequest {
        model,
        messages,
        tools,
        max_tokens,
        temperature,
        stream,
    }
}

/// Send the HTTP request and return the raw `reqwest::Response` (for streaming).
async fn send_request(
    client: &Client,
    api_key: &str,
    base_url: &str,
    oai_req: &OaiRequest,
) -> anyhow::Result<reqwest::Response> {
    let url = format!("{base_url}/chat/completions");
    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(oai_req)
        .send()
        .await?;

    if resp.status().is_client_error() || resp.status().is_server_error() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        let msg = format!("OpenAI API error {status}: {body}");
        anyhow::bail!("{msg}");
    }

    Ok(resp)
}

/// Send the HTTP request, read the full body, and return it as a string (for non-streaming).
async fn do_request(
    client: &Client,
    api_key: &str,
    base_url: &str,
    oai_req: &OaiRequest,
) -> anyhow::Result<String> {
    let resp = send_request(client, api_key, base_url, oai_req).await?;
    let body = resp.text().await?;
    Ok(body)
}

fn parse_response(resp: &OaiResponse) -> LlmResponse {
    let mut result = LlmResponse {
        input_tokens: resp.usage.prompt_tokens,
        output_tokens: resp.usage.completion_tokens,
        model: resp.model.clone(),
        ..Default::default()
    };

    if let Some(choice) = resp.choices.first() {
        result.content.clone_from(&choice.message.content);
        for tc in &choice.message.tool_calls {
            let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            result.tool_calls.push(ToolCall {
                id: tc.id.clone(),
                name: tc.function.name.clone(),
                arguments: args,
            });
        }
    }

    result
}
