//! Core agent loop: the heart of MarsClaw.
//!
//! Ported from Go: internal/agent/agent.go

pub mod context;
pub mod discovery;
pub mod orchestrator;

use crate::config::AgentConfig;
use crate::llm::retry::{self, RetryConfig};
use crate::types::*;

use context::{truncate_tool_result, ContextBuilder};

use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Safety trait
// ---------------------------------------------------------------------------

/// Validates tool calls and redacts credentials from output.
pub trait SafetyCheck: Send + Sync {
    /// Return `Ok(())` to allow, `Err(reason)` to block.
    /// If the error string starts with "DENIED:" the agent loop will stop
    /// with `StopReason::HumanDenied`.
    fn validate(&self, call: &ToolCall) -> Result<(), String>;

    /// Scan `content` for credentials and return a redacted copy.
    /// The bool indicates whether any credentials were found.
    fn scan_credentials(&self, content: &str) -> (String, bool);
}

// ---------------------------------------------------------------------------
// Agent
// ---------------------------------------------------------------------------

/// One Agent per conversation. Drives the LLM + tool-call loop.
pub struct Agent {
    provider: Arc<dyn Provider>,
    config: AgentConfig,
    tools: HashMap<String, Arc<dyn ToolExecutor>>,
    tool_defs: Vec<ToolDef>,
    context_builder: ContextBuilder,
    safety: Option<Arc<dyn SafetyCheck>>,
    cost: Option<Arc<dyn CostRecorder>>,
    on_stream: Option<Box<dyn Fn(StreamEvent) + Send + Sync>>,
}

impl Agent {
    /// Create a new agent with the given provider, tools, and config.
    pub fn new(
        provider: Arc<dyn Provider>,
        config: AgentConfig,
        tools: HashMap<String, Arc<dyn ToolExecutor>>,
        tool_defs: Vec<ToolDef>,
    ) -> Self {
        let context_builder = ContextBuilder::new(provider.clone(), &config, &tool_defs);
        Self {
            provider,
            config,
            tools,
            tool_defs,
            context_builder,
            safety: None,
            cost: None,
            on_stream: None,
        }
    }

    /// Set a streaming event callback.
    pub fn with_stream_handler(
        mut self,
        f: impl Fn(StreamEvent) + Send + Sync + 'static,
    ) -> Self {
        self.on_stream = Some(Box::new(f));
        self
    }

    /// Attach a cost tracker.
    pub fn with_cost_tracker(mut self, cost: Arc<dyn CostRecorder>) -> Self {
        self.cost = Some(cost);
        self
    }

    /// Attach a safety checker.
    pub fn with_safety(mut self, safety: Arc<dyn SafetyCheck>) -> Self {
        self.safety = Some(safety);
        self
    }

    /// Dynamically add tools (used by supervisor pattern).
    pub fn add_tools(
        &mut self,
        defs: Vec<ToolDef>,
        executors: HashMap<String, Arc<dyn ToolExecutor>>,
    ) {
        self.tool_defs.extend(defs);
        self.tools.extend(executors);
        self.context_builder.set_tool_defs(self.tool_defs.clone());
    }

    // -----------------------------------------------------------------------
    // Main agent loop
    // -----------------------------------------------------------------------

    /// Execute the full agent loop for a single user turn.
    pub async fn run(&self, cancel: CancellationToken, parts: ContextParts) -> RunResult {
        let start = Instant::now();
        let mut history = parts.history.clone();
        let mut total_input: i32 = 0;
        let mut total_output: i32 = 0;
        let mut trace: Vec<TraceEntry> = Vec::with_capacity(16);
        let mut stop_reason = StopReason::MaxTurns;
        let mut final_response = String::new();

        for turn in 0..self.config.max_turns {
            if cancel.is_cancelled() {
                stop_reason = StopReason::Error("cancelled".into());
                break;
            }

            info!(turn = turn + 1, history_len = history.len(), "agent turn");

            // Phase 1: Build context (system + history, fit to budget).
            let messages = self.context_builder.build(&parts, &history);

            // Phase 2: Budget guard.
            if let Some(cost) = &self.cost
                && cost.over_budget()
            {
                final_response =
                    "Daily cost budget exceeded. Please try again tomorrow.".into();
                stop_reason = StopReason::BudgetExceeded;
                break;
            }

            // Phase 3: Call LLM (with retry + optional streaming).
            let llm_start = Instant::now();
            let llm_result = self.call_llm(&messages, &cancel).await;

            let resp = match llm_result {
                Ok(r) => {
                    self.record_llm_trace(&mut trace, llm_start, &r, None);
                    r
                }
                Err(e) => {
                    self.record_llm_trace(&mut trace, llm_start, &LlmResponse::default(), Some(&e));
                    stop_reason = StopReason::Error(e.to_string());
                    break;
                }
            };

            // Track tokens.
            total_input += resp.input_tokens;
            total_output += resp.output_tokens;

            if let Some(cost) = &self.cost {
                cost.record(&resp.model, resp.input_tokens, resp.output_tokens);
            }

            // Phase 4: Check token budget.
            if total_input > self.config.max_input_tokens {
                final_response = if resp.content.is_empty() {
                    "I've reached the context limit for this conversation.".into()
                } else {
                    resp.content.clone()
                };
                stop_reason = StopReason::BudgetExceeded;
                break;
            }

            // Phase 5: No tool calls = final response.
            if resp.tool_calls.is_empty() {
                history.push(Message {
                    role: Role::Assistant,
                    content: resp.content.clone(),
                    ..Default::default()
                });
                final_response = resp.content;
                stop_reason = StopReason::FinalResponse;
                break;
            }

            // Phase 6: Has tool calls -- add assistant message, execute them.
            history.push(Message {
                role: Role::Assistant,
                content: resp.content.clone(),
                tool_calls: resp.tool_calls.clone(),
                ..Default::default()
            });

            // Check consecutive tool turns *before* executing.
            if self.count_consecutive_tool_turns(&history) >= self.config.max_consecutive_tool_calls
            {
                history.push(Message {
                    role: Role::System,
                    content: "You have made many consecutive tool calls. \
                              Please synthesize what you've learned and respond to the user now."
                        .into(),
                    ..Default::default()
                });
                continue;
            }

            let tool_stop =
                self.execute_tool_calls(&resp.tool_calls, &mut history, &mut trace, &cancel)
                    .await;

            if let Some(reason) = tool_stop {
                stop_reason = reason;
                break;
            }
        }

        if stop_reason == StopReason::MaxTurns && final_response.is_empty() {
            final_response =
                "I've reached the maximum number of turns for this conversation.".into();
        }

        let duration = start.elapsed();
        info!(
            stop_reason = ?stop_reason,
            turns = trace.len(),
            duration_ms = duration.as_millis() as i64,
            "agent run complete",
        );

        RunResult {
            response: final_response,
            stop_reason,
            turn_count: trace
                .iter()
                .filter(|t| t.phase == "llm_call")
                .count() as i32,
            total_input,
            total_output,
            duration,
            error: None,
            trace,
            history,
        }
    }

    // -----------------------------------------------------------------------
    // LLM call (retry + streaming)
    // -----------------------------------------------------------------------

    async fn call_llm(
        &self,
        messages: &[Message],
        _cancel: &CancellationToken,
    ) -> anyhow::Result<LlmResponse> {
        let req = ProviderRequest {
            model: String::new(), // provider uses its configured model
            messages: messages.to_vec(),
            tools: self.tool_defs.clone(),
            max_tokens: self.config.max_output_tokens,
            temperature: self.config.temperature,
            stop: Vec::new(),
        };

        let streaming = self.config.enable_streaming && self.on_stream.is_some();
        if streaming {
            return self.call_streaming(&req).await;
        }

        let rc = RetryConfig {
            max_retries: self.config.max_retries,
            base_delay: Duration::from_secs(1),
        };
        let provider = self.provider.clone();

        retry::with_retry(&rc, || {
            let req = req.clone();
            let provider = provider.clone();
            async move { provider.call(&req).await }
        })
        .await
    }

    async fn call_streaming(&self, req: &ProviderRequest) -> anyhow::Result<LlmResponse> {
        let (tx, mut rx) = mpsc::channel::<StreamEvent>(64);
        let provider = self.provider.clone();
        let req = req.clone();

        let handle = tokio::spawn(async move { provider.stream(&req, tx).await });

        while let Some(ev) = rx.recv().await {
            if let Some(f) = &self.on_stream {
                f(ev);
            }
        }

        handle
            .await
            .map_err(|e| anyhow::anyhow!("stream task panicked: {e}"))?
    }

    // -----------------------------------------------------------------------
    // Tool execution
    // -----------------------------------------------------------------------

    async fn execute_tool_calls(
        &self,
        calls: &[ToolCall],
        history: &mut Vec<Message>,
        trace: &mut Vec<TraceEntry>,
        cancel: &CancellationToken,
    ) -> Option<StopReason> {
        for tc in calls {
            if cancel.is_cancelled() {
                return Some(StopReason::Error("cancelled".into()));
            }

            if let Some(f) = &self.on_stream {
                f(StreamEvent::ToolStart {
                    tool_call: tc.clone(),
                });
            }

            let tool_start = Instant::now();

            // Safety validation.
            if let Some(safety) = &self.safety
                && let Err(reason) = safety.validate(tc)
            {
                let is_denied = reason.starts_with("DENIED:");
                self.append_tool_result(history, &tc.id, &reason, true);
                self.record_tool_trace(
                    trace,
                    tc,
                    tool_start,
                    Some(&anyhow::anyhow!("{}", reason)),
                );
                if is_denied {
                    return Some(StopReason::HumanDenied);
                }
                continue;
            }

            // Find executor.
            let executor = match self.tools.get(&tc.name) {
                Some(e) => e,
                None => {
                    let msg = format!("Error: tool {:?} has no registered executor", tc.name);
                    self.append_tool_result(history, &tc.id, &msg, true);
                    self.record_tool_trace(
                        trace,
                        tc,
                        tool_start,
                        Some(&anyhow::anyhow!("no executor for {:?}", tc.name)),
                    );
                    continue;
                }
            };

            // Execute with timeout.
            let timeout = Duration::from_secs(self.config.tool_timeout_secs);
            let output = match tokio::time::timeout(timeout, executor.execute(tc)).await {
                Ok(Ok(out)) => out,
                Ok(Err(e)) => {
                    let msg = format!("Error: tool {:?} failed: {e}", tc.name);
                    self.append_tool_result(history, &tc.id, &msg, true);
                    self.record_tool_trace(trace, tc, tool_start, Some(&e));
                    if let Some(f) = &self.on_stream {
                        f(StreamEvent::ToolDone {
                            tool_call: tc.clone(),
                            output: msg,
                        });
                    }
                    continue;
                }
                Err(_) => {
                    let msg = format!(
                        "Error: tool {:?} timed out after {}s",
                        tc.name, self.config.tool_timeout_secs
                    );
                    self.append_tool_result(history, &tc.id, &msg, true);
                    self.record_tool_trace(
                        trace,
                        tc,
                        tool_start,
                        Some(&anyhow::anyhow!("timeout")),
                    );
                    if let Some(f) = &self.on_stream {
                        f(StreamEvent::ToolDone {
                            tool_call: tc.clone(),
                            output: msg,
                        });
                    }
                    continue;
                }
            };

            // Credential scanning.
            let output = if let Some(safety) = &self.safety {
                let (redacted, found) = safety.scan_credentials(&output);
                if found {
                    warn!(tool = %tc.name, "credentials redacted from tool output");
                }
                redacted
            } else {
                output
            };

            // Truncate.
            let output = truncate_tool_result(&output, self.config.max_tool_result_len);

            self.append_tool_result(history, &tc.id, &output, false);
            self.record_tool_trace(trace, tc, tool_start, None);

            if let Some(f) = &self.on_stream {
                f(StreamEvent::ToolDone {
                    tool_call: tc.clone(),
                    output,
                });
            }
        }

        None
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn append_tool_result(
        &self,
        history: &mut Vec<Message>,
        call_id: &str,
        content: &str,
        is_error: bool,
    ) {
        history.push(Message {
            role: Role::Tool,
            tool_result: Some(ToolResult {
                call_id: call_id.to_owned(),
                content: content.to_owned(),
                is_error,
            }),
            ..Default::default()
        });
    }

    fn record_llm_trace(
        &self,
        trace: &mut Vec<TraceEntry>,
        start: Instant,
        resp: &LlmResponse,
        err: Option<&anyhow::Error>,
    ) {
        trace.push(TraceEntry {
            step: trace.len() as i32,
            phase: "llm_call".into(),
            timestamp: Utc::now(),
            duration_ms: start.elapsed().as_millis() as i64,
            input_tokens: resp.input_tokens,
            output_tokens: resp.output_tokens,
            tool_name: String::new(),
            error: err.map(|e| e.to_string()).unwrap_or_default(),
        });
    }

    fn record_tool_trace(
        &self,
        trace: &mut Vec<TraceEntry>,
        call: &ToolCall,
        start: Instant,
        err: Option<&anyhow::Error>,
    ) {
        trace.push(TraceEntry {
            step: trace.len() as i32,
            phase: "tool_call".into(),
            timestamp: Utc::now(),
            duration_ms: start.elapsed().as_millis() as i64,
            input_tokens: 0,
            output_tokens: 0,
            tool_name: call.name.clone(),
            error: err.map(|e| e.to_string()).unwrap_or_default(),
        });
    }

    /// Count how many consecutive assistant turns (with tool calls) are at
    /// the end of history, skipping over interleaved tool-result messages.
    fn count_consecutive_tool_turns(&self, history: &[Message]) -> i32 {
        let mut count = 0;
        for msg in history.iter().rev() {
            match msg.role {
                Role::Assistant if !msg.tool_calls.is_empty() => count += 1,
                Role::Tool => continue,
                _ => break,
            }
        }
        count
    }
}
