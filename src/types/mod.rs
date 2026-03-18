//! Core types shared across all marsclaw modules.
//!
//! Ported from Go: internal/types/types.go

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// Identifies who produced a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

// ---------------------------------------------------------------------------
// Tool invocation
// ---------------------------------------------------------------------------

/// An LLM's request to invoke a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Result fed back after executing a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// Message
// ---------------------------------------------------------------------------

/// A single entry in the conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolResult>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub token_count: i32,
    pub timestamp: DateTime<Utc>,
}

fn is_zero(v: &i32) -> bool {
    *v == 0
}

impl Default for Message {
    fn default() -> Self {
        Self {
            role: Role::User,
            content: String::new(),
            tool_calls: Vec::new(),
            tool_result: None,
            token_count: 0,
            timestamp: Utc::now(),
        }
    }
}

// ---------------------------------------------------------------------------
// Danger level
// ---------------------------------------------------------------------------

/// Controls whether human approval is required before executing a tool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum DangerLevel {
    /// Execute freely.
    None,
    /// Log but execute.
    Low,
    /// Ask if strict mode.
    Medium,
    /// Always ask.
    High,
}

impl Default for DangerLevel {
    fn default() -> Self {
        Self::None
    }
}

// ---------------------------------------------------------------------------
// Tool definition
// ---------------------------------------------------------------------------

/// Describes a tool the LLM can invoke.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    /// JSON Schema for the tool parameters.
    pub parameters: serde_json::Value,
    #[serde(default)]
    pub danger_level: DangerLevel,
    #[serde(default)]
    pub read_only: bool,
}

// ---------------------------------------------------------------------------
// LLM response
// ---------------------------------------------------------------------------

/// What comes back from a single LLM API call.
#[derive(Debug, Clone, Default)]
pub struct LlmResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub model: String,
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Emitted during streaming generation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Text {
        delta: String,
        done: bool,
    },
    ToolStart {
        tool_call: ToolCall,
    },
    ToolDone {
        tool_call: ToolCall,
        output: String,
    },
    Error {
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Stop reason
// ---------------------------------------------------------------------------

/// Why the agent loop terminated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    FinalResponse,
    MaxTurns,
    BudgetExceeded,
    Error(String),
    HumanDenied,
    ContextOverflow,
}

// ---------------------------------------------------------------------------
// Trace
// ---------------------------------------------------------------------------

/// Records one step in the agent loop for observability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    pub step: i32,
    pub phase: String,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: i64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub input_tokens: i32,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub output_tokens: i32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tool_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

// ---------------------------------------------------------------------------
// Run result
// ---------------------------------------------------------------------------

/// Final output of an agent run.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub response: String,
    pub stop_reason: StopReason,
    pub turn_count: i32,
    pub total_input: i32,
    pub total_output: i32,
    pub duration: std::time::Duration,
    pub error: Option<String>,
    pub trace: Vec<TraceEntry>,
    pub history: Vec<Message>,
}

impl Default for RunResult {
    fn default() -> Self {
        Self {
            response: String::new(),
            stop_reason: StopReason::FinalResponse,
            turn_count: 0,
            total_input: 0,
            total_output: 0,
            duration: std::time::Duration::ZERO,
            error: None,
            trace: Vec::new(),
            history: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Context assembly
// ---------------------------------------------------------------------------

/// Raw material before context assembly.
#[derive(Debug, Clone, Default)]
pub struct ContextParts {
    /// Core identity (SOUL.md).
    pub soul_prompt: String,
    /// Agent-specific instructions (AGENTS.md).
    pub agent_prompt: String,
    /// Injected memory / knowledge.
    pub memory: String,
    /// Conversation history.
    pub history: Vec<Message>,
}

// ---------------------------------------------------------------------------
// Provider request
// ---------------------------------------------------------------------------

/// What we send to the LLM.
#[derive(Debug, Clone)]
pub struct ProviderRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub max_tokens: i32,
    pub temperature: f64,
    pub stop: Vec<String>,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Interface any LLM backend must implement.
#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    async fn call(&self, req: &ProviderRequest) -> anyhow::Result<LlmResponse>;

    async fn stream(
        &self,
        req: &ProviderRequest,
        tx: tokio::sync::mpsc::Sender<StreamEvent>,
    ) -> anyhow::Result<LlmResponse>;

    fn count_tokens(&self, messages: &[Message], tools: &[ToolDef]) -> i32;

    fn max_context_window(&self) -> i32;
}

/// Runs a single tool call.
#[async_trait::async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String>;
}

/// Tracks token costs.
pub trait CostRecorder: Send + Sync {
    fn record(&self, model: &str, input_tokens: i32, output_tokens: i32) -> i64;
    fn format_cost_line(&self, model: &str, input_tokens: i32, output_tokens: i32) -> String;
    fn over_budget(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Approval callback
// ---------------------------------------------------------------------------

/// Called when human-in-the-loop approval is needed.
pub type ApprovalFn = Box<dyn Fn(&ToolCall, &str) -> bool + Send + Sync>;
