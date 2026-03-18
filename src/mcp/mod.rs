//! MCP (Model Context Protocol) client.
//!
//! Implements JSON-RPC 2.0 over stdio to communicate with MCP servers.
//! Each MCP server exposes tools that get registered into the agent's tool registry.
//!
//! To be implemented: full JSON-RPC 2.0 transport, tool listing, and tool execution.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::McpServerConfig;
use crate::types::{ToolCall, ToolDef, ToolExecutor};

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Connection to a single MCP server process.
pub struct McpClient {
    name: String,
}

impl McpClient {
    /// Shut down the MCP server process.
    pub fn close(&self) {
        tracing::debug!(server = %self.name, "mcp client closed");
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Connect to all configured MCP servers, list their tools, and return
/// tool definitions + executors ready for registry insertion.
///
/// Current implementation is a no-op placeholder. When the full MCP protocol
/// is implemented, this will:
/// 1. Spawn each server process.
/// 2. Perform the initialize handshake.
/// 3. Call `tools/list` to discover available tools.
/// 4. Return tool defs and executor wrappers that call `tools/call`.
pub fn register_mcp_servers(
    _configs: &[McpServerConfig],
) -> anyhow::Result<(Vec<ToolDef>, HashMap<String, Arc<dyn ToolExecutor>>, Vec<McpClient>)> {
    // Placeholder: no MCP servers connected yet.
    Ok((Vec::new(), HashMap::new(), Vec::new()))
}

// ---------------------------------------------------------------------------
// MCP Tool Executor (placeholder)
// ---------------------------------------------------------------------------

/// Executor that forwards tool calls to an MCP server via JSON-RPC.
struct McpToolExecutor {
    _server_name: String,
    _tool_name: String,
}

#[async_trait::async_trait]
impl ToolExecutor for McpToolExecutor {
    async fn execute(&self, _call: &ToolCall) -> anyhow::Result<String> {
        anyhow::bail!("MCP tool execution not yet implemented")
    }
}
