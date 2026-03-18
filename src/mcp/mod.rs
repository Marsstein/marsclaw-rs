//! MCP (Model Context Protocol) client — full JSON-RPC 2.0 over stdio.
//!
//! Ported from Go: internal/mcp/client.go + internal/mcp/executor.go
//!
//! Each MCP server is a child process communicating via newline-delimited
//! JSON-RPC 2.0 on stdin/stdout. On connect we:
//! 1. Send `initialize` with protocol version + client info.
//! 2. Send `notifications/initialized`.
//! 3. Call `tools/list` to discover available tools.
//!
//! Tool names are prefixed with the server name (`servername_toolname`) to
//! avoid collisions when multiple servers are connected.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::config::McpServerConfig;
use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 wire types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: i64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: String,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<i64>,
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[derive(Debug, Deserialize)]
struct McpToolInfo {
    name: String,
    description: String,
    #[serde(rename = "inputSchema")]
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
struct McpToolsResponse {
    tools: Vec<McpToolInfo>,
}

#[derive(Debug, Deserialize)]
struct McpToolResult {
    content: Vec<McpContent>,
    #[serde(rename = "isError", default)]
    is_error: bool,
}

#[derive(Debug, Deserialize)]
struct McpContent {
    #[serde(rename = "type")]
    content_type: String,
    #[serde(default)]
    text: String,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Connection to a single MCP server process via stdio JSON-RPC 2.0.
pub struct McpClient {
    name: String,
    child: Child,
    stdin: tokio::process::ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: AtomicI64,
    tools: Vec<ToolDef>,
}

impl McpClient {
    /// Spawn an MCP server, perform the initialize handshake, and discover tools.
    pub async fn new(config: &McpServerConfig) -> anyhow::Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        for env_pair in &config.env {
            if let Some((key, val)) = env_pair.split_once('=') {
                cmd.env(key, val);
            }
        }

        let mut child = cmd.spawn().map_err(|e| {
            anyhow::anyhow!("mcp start {:?}: {e}", config.command)
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            anyhow::anyhow!("mcp stdin pipe unavailable")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            anyhow::anyhow!("mcp stdout pipe unavailable")
        })?;

        let mut client = Self {
            name: config.name.clone(),
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: AtomicI64::new(0),
            tools: Vec::new(),
        };

        if let Err(e) = client.initialize().await {
            client.close().await;
            return Err(anyhow::anyhow!("mcp initialize: {e}"));
        }

        if let Err(e) = client.discover_tools().await {
            client.close().await;
            return Err(anyhow::anyhow!("mcp discover tools: {e}"));
        }

        Ok(client)
    }

    /// Discovered tool definitions (names prefixed with server name).
    pub fn tools(&self) -> &[ToolDef] {
        &self.tools
    }

    /// Invoke a tool by its original (unprefixed) name.
    pub async fn call_tool(&mut self, name: &str, args: Value) -> anyhow::Result<String> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args,
        });
        let result = self.send_request("tools/call", Some(params)).await?;

        let tool_result: McpToolResult = match serde_json::from_value(result.clone()) {
            Ok(tr) => tr,
            Err(_) => return Ok(result.to_string()),
        };

        let mut output = String::new();
        for c in &tool_result.content {
            if c.content_type == "text" {
                output.push_str(&c.text);
            }
        }

        if tool_result.is_error {
            return Err(anyhow::anyhow!("mcp tool error: {output}"));
        }

        Ok(output)
    }

    /// Shut down the MCP server process.
    pub async fn close(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }

    // -- Private protocol methods --

    async fn initialize(&mut self) -> anyhow::Result<()> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "marsclaw",
                "version": "1.0.0",
            },
        });
        self.send_request("initialize", Some(params)).await?;

        // Send initialized notification (no response expected).
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized".to_string(),
        };
        let mut data = serde_json::to_vec(&notif)?;
        data.push(b'\n');
        self.stdin.write_all(&data).await?;

        Ok(())
    }

    async fn discover_tools(&mut self) -> anyhow::Result<()> {
        let result = self.send_request("tools/list", None).await?;
        let resp: McpToolsResponse = serde_json::from_value(result)
            .map_err(|e| anyhow::anyhow!("parse tools: {e}"))?;

        self.tools = resp
            .tools
            .into_iter()
            .map(|t| ToolDef {
                name: format!("{}_{}", self.name, t.name),
                description: format!("[{}] {}", self.name, t.description),
                parameters: t.input_schema,
                danger_level: DangerLevel::Medium,
                read_only: false,
            })
            .collect();

        Ok(())
    }

    async fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> anyhow::Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut data = serde_json::to_vec(&req)?;
        data.push(b'\n');
        self.stdin.write_all(&data).await.map_err(|e| {
            anyhow::anyhow!("write request: {e}")
        })?;
        self.stdin.flush().await?;

        // Read lines until we find a response matching our ID.
        let mut line = String::new();
        loop {
            line.clear();
            let bytes_read = self.reader.read_line(&mut line).await.map_err(|e| {
                anyhow::anyhow!("read response: {e}")
            })?;

            if bytes_read == 0 {
                return Err(anyhow::anyhow!("mcp server closed stdout"));
            }

            let resp: JsonRpcResponse = match serde_json::from_str(line.trim()) {
                Ok(r) => r,
                Err(_) => continue, // skip non-JSON lines (notifications, logs)
            };

            if resp.id != Some(id) {
                continue; // skip notifications or mismatched IDs
            }

            if let Some(err) = resp.error {
                return Err(anyhow::anyhow!("rpc error {}: {}", err.code, err.message));
            }

            return Ok(resp.result.unwrap_or(Value::Null));
        }
    }
}

// ---------------------------------------------------------------------------
// Executor
// ---------------------------------------------------------------------------

/// Adapter that implements `ToolExecutor` by forwarding calls to an `McpClient`.
pub struct McpToolExecutor {
    client: Arc<Mutex<McpClient>>,
    server_name: String,
}

#[async_trait::async_trait]
impl ToolExecutor for McpToolExecutor {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let prefix = format!("{}_", self.server_name);
        let original_name = call.name.strip_prefix(&prefix).unwrap_or(&call.name);
        let mut client = self.client.lock().await;
        client.call_tool(original_name, call.arguments.clone()).await
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Connect to all configured MCP servers, discover their tools, and return
/// tool definitions + executor wrappers ready for registry insertion.
pub async fn register_mcp_servers(
    configs: &[McpServerConfig],
) -> anyhow::Result<(Vec<ToolDef>, HashMap<String, Arc<dyn ToolExecutor>>, Vec<Arc<Mutex<McpClient>>>)> {
    let mut all_defs = Vec::new();
    let mut executors: HashMap<String, Arc<dyn ToolExecutor>> = HashMap::new();
    let mut clients: Vec<Arc<Mutex<McpClient>>> = Vec::new();

    for cfg in configs {
        let client = match McpClient::new(cfg).await {
            Ok(c) => c,
            Err(e) => {
                // Close already-opened clients on error.
                for c in &clients {
                    let mut locked: tokio::sync::MutexGuard<'_, McpClient> = c.lock().await;
                    locked.close().await;
                }
                return Err(anyhow::anyhow!("mcp server {:?}: {e}", cfg.name));
            }
        };

        let server_name = client.name.clone();
        let tool_defs: Vec<ToolDef> = client.tools().to_vec();
        let shared = Arc::new(Mutex::new(client));
        clients.push(shared.clone());

        let executor: Arc<dyn ToolExecutor> = Arc::new(McpToolExecutor {
            client: shared,
            server_name: server_name.clone(),
        });

        for def in &tool_defs {
            executors.insert(def.name.clone(), executor.clone());
        }
        all_defs.extend(tool_defs);
    }

    Ok((all_defs, executors, clients))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rpc_request_serializes_correctly() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method: "tools/list".to_string(),
            params: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"tools/list\""));
        assert!(!json.contains("params"));
    }

    #[test]
    fn json_rpc_request_with_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 2,
            method: "initialize".to_string(),
            params: Some(serde_json::json!({"protocolVersion": "2024-11-05"})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"params\""));
        assert!(json.contains("2024-11-05"));
    }

    #[test]
    fn json_rpc_response_deserializes_result() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn json_rpc_response_deserializes_error() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid request"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(raw).unwrap();
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid request");
    }

    #[test]
    fn mcp_tool_result_parses_text_content() {
        let raw = r#"{"content":[{"type":"text","text":"hello"},{"type":"text","text":" world"}],"isError":false}"#;
        let result: McpToolResult = serde_json::from_str(raw).unwrap();
        assert!(!result.is_error);
        let output: String = result.content.iter()
            .filter(|c| c.content_type == "text")
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(output, "hello world");
    }

    #[test]
    fn mcp_tool_result_error_flag() {
        let raw = r#"{"content":[{"type":"text","text":"not found"}],"isError":true}"#;
        let result: McpToolResult = serde_json::from_str(raw).unwrap();
        assert!(result.is_error);
    }

    #[test]
    fn notification_serializes_without_id() {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: "notifications/initialized".to_string(),
        };
        let json = serde_json::to_string(&notif).unwrap();
        assert!(!json.contains("\"id\""));
        assert!(json.contains("notifications/initialized"));
    }

    #[test]
    fn strip_prefix_works_for_executor() {
        let server_name = "filesystem";
        let prefixed = format!("{server_name}_read_file");
        let prefix = format!("{server_name}_");
        let original = prefixed.strip_prefix(&prefix).unwrap_or(&prefixed);
        assert_eq!(original, "read_file");
    }
}
