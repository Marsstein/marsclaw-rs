//! Hook system for agent lifecycle events.
//!
//! Ported from Go: internal/hooks/hooks.go
//!
//! Hooks fire at key points in the agent loop: before/after tool calls,
//! before/after LLM calls, and on errors. All registered hooks for an
//! event run even if one fails; the first error is returned.

use crate::types::ToolCall;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Identifies when a hook fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    BeforeToolCall,
    AfterToolCall,
    BeforeLlmCall,
    AfterLlmCall,
    OnError,
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeToolCall => write!(f, "before_tool_call"),
            Self::AfterToolCall => write!(f, "after_tool_call"),
            Self::BeforeLlmCall => write!(f, "before_llm_call"),
            Self::AfterLlmCall => write!(f, "after_llm_call"),
            Self::OnError => write!(f, "on_error"),
        }
    }
}

/// Context passed to hook functions.
pub struct HookData<'a> {
    pub event: HookEvent,
    pub tool_call: Option<&'a ToolCall>,
    pub result: Option<&'a str>,
    pub error: Option<&'a str>,
    pub model: Option<&'a str>,
}

/// A hook callback. Receives the event and contextual data.
pub type HookFn = Box<dyn Fn(&HookData<'_>) -> Result<(), String> + Send + Sync>;

// ---------------------------------------------------------------------------
// Manager
// ---------------------------------------------------------------------------

/// Holds registered hooks and fires them at the appropriate points.
pub struct HookManager {
    hooks: Vec<(HookEvent, HookFn)>,
}

impl HookManager {
    pub fn new() -> Self {
        Self { hooks: Vec::new() }
    }

    /// Register a hook for the given event.
    pub fn register(&mut self, event: HookEvent, f: HookFn) {
        self.hooks.push((event, f));
    }

    /// Fire all hooks registered for `data.event`.
    ///
    /// All matching hooks run even if one fails. Returns the first error.
    pub fn fire(&self, data: &HookData<'_>) -> Result<(), String> {
        let mut first_err: Option<String> = None;

        for (evt, hook) in &self.hooks {
            if *evt != data.event {
                continue;
            }
            if let Err(e) = hook(data) {
                tracing::warn!(event = %data.event, error = %e, "hook error");
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        match first_err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Convenience: fire with just an event and a detail string.
    pub fn fire_simple(&self, event: HookEvent, detail: &str) {
        let data = HookData {
            event,
            tool_call: None,
            result: Some(detail),
            error: None,
            model: None,
        };
        let _ = self.fire(&data);
    }

    /// Check if any hooks are registered for an event.
    pub fn has_hooks(&self, event: HookEvent) -> bool {
        self.hooks.iter().any(|(e, _)| *e == event)
    }

    /// Returns a hook that logs tool calls and errors via `tracing`.
    pub fn logging_hook() -> HookFn {
        Box::new(|data: &HookData<'_>| {
            match data.event {
                HookEvent::BeforeToolCall => {
                    if let Some(tc) = data.tool_call {
                        tracing::info!(tool = %tc.name, "tool call");
                    }
                }
                HookEvent::AfterToolCall => {
                    if let Some(tc) = data.tool_call {
                        let len = data.result.map(|r| r.len()).unwrap_or(0);
                        tracing::info!(tool = %tc.name, result_len = len, "tool result");
                    }
                }
                HookEvent::OnError => {
                    if let Some(err) = data.error {
                        tracing::error!(error = %err, "agent error");
                    }
                }
                _ => {}
            }
            Ok(())
        })
    }
}

impl Default for HookManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    #[test]
    fn fire_calls_matching_hooks() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let mut mgr = HookManager::new();
        mgr.register(
            HookEvent::BeforeToolCall,
            Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        );

        let data = HookData {
            event: HookEvent::BeforeToolCall,
            tool_call: None,
            result: None,
            error: None,
            model: None,
        };
        mgr.fire(&data).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn fire_skips_non_matching_events() {
        let counter = Arc::new(AtomicU32::new(0));
        let c = counter.clone();

        let mut mgr = HookManager::new();
        mgr.register(
            HookEvent::OnError,
            Box::new(move |_| {
                c.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }),
        );

        let data = HookData {
            event: HookEvent::BeforeToolCall,
            tool_call: None,
            result: None,
            error: None,
            model: None,
        };
        mgr.fire(&data).unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn fire_returns_first_error_but_runs_all() {
        let counter = Arc::new(AtomicU32::new(0));
        let c1 = counter.clone();
        let c2 = counter.clone();

        let mut mgr = HookManager::new();
        mgr.register(
            HookEvent::OnError,
            Box::new(move |_| {
                c1.fetch_add(1, Ordering::SeqCst);
                Err("first".into())
            }),
        );
        mgr.register(
            HookEvent::OnError,
            Box::new(move |_| {
                c2.fetch_add(1, Ordering::SeqCst);
                Err("second".into())
            }),
        );

        let data = HookData {
            event: HookEvent::OnError,
            tool_call: None,
            result: None,
            error: Some("boom"),
            model: None,
        };
        let err = mgr.fire(&data).unwrap_err();
        assert_eq!(err, "first");
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn has_hooks_reflects_registration() {
        let mut mgr = HookManager::new();
        assert!(!mgr.has_hooks(HookEvent::BeforeLlmCall));

        mgr.register(HookEvent::BeforeLlmCall, Box::new(|_| Ok(())));
        assert!(mgr.has_hooks(HookEvent::BeforeLlmCall));
        assert!(!mgr.has_hooks(HookEvent::AfterLlmCall));
    }

    #[test]
    fn hook_event_display() {
        assert_eq!(HookEvent::BeforeToolCall.to_string(), "before_tool_call");
        assert_eq!(HookEvent::AfterToolCall.to_string(), "after_tool_call");
        assert_eq!(HookEvent::OnError.to_string(), "on_error");
    }
}
