//! Context builder: assembles the prompt sent to the LLM.
//!
//! Assembly order: SOUL.md -> AGENTS.md -> memory -> history.
//! Ported from Go: internal/agent/context.go

use crate::config::AgentConfig;
use crate::types::{Message, Provider, Role, ToolDef};
use std::sync::Arc;

/// Assembles the final message list that fits within the token budget.
pub struct ContextBuilder {
    config: AgentConfig,
    tool_defs: Vec<ToolDef>,
    provider: Arc<dyn Provider>,
}

impl ContextBuilder {
    pub fn new(
        provider: Arc<dyn Provider>,
        config: &AgentConfig,
        tool_defs: &[ToolDef],
    ) -> Self {
        Self {
            config: config.clone(),
            tool_defs: tool_defs.to_vec(),
            provider,
        }
    }

    /// Build the message list that fits within token budget.
    pub fn build(&self, parts: &crate::types::ContextParts, history: &[Message]) -> Vec<Message> {
        let max_input = self.config.max_input_tokens;
        let system_budget = (max_input as f64 * self.config.system_prompt_budget) as i32;
        let history_budget = (max_input as f64 * self.config.history_budget) as i32;

        let system_content = self.assemble_system_prompt(parts, system_budget);

        let mut messages = Vec::with_capacity(1 + history.len());

        if !system_content.is_empty() {
            messages.push(Message {
                role: Role::System,
                content: system_content,
                ..Default::default()
            });
        }

        let trimmed = self.fit_history(history, history_budget);
        messages.extend(trimmed);

        messages
    }

    /// Update tool definitions (used when tools are added dynamically).
    pub fn set_tool_defs(&mut self, defs: Vec<ToolDef>) {
        self.tool_defs = defs;
    }

    fn assemble_system_prompt(
        &self,
        parts: &crate::types::ContextParts,
        budget: i32,
    ) -> String {
        let sections = [&parts.soul_prompt, &parts.agent_prompt, &parts.memory];

        let mut result = String::new();
        for section in &sections {
            if section.is_empty() {
                continue;
            }
            if !result.is_empty() {
                result.push_str("\n\n");
            }
            result.push_str(section);
        }

        let tokens = self.count_tokens_for(
            &[Message {
                role: Role::System,
                content: result.clone(),
                ..Default::default()
            }],
        );

        if tokens <= budget {
            return result;
        }

        // Proportional truncation preserving UTF-8 boundaries.
        let ratio = budget as f64 / tokens as f64;
        let mut cut_len = (result.len() as f64 * ratio * 0.95) as usize;
        while cut_len > 0 && !result.is_char_boundary(cut_len) {
            cut_len -= 1;
        }

        let mut truncated = result[..cut_len].to_owned();
        truncated.push_str("\n\n[System prompt truncated to fit context window]");
        truncated
    }

    fn fit_history(&self, history: &[Message], budget: i32) -> Vec<Message> {
        if history.is_empty() {
            return Vec::new();
        }

        let total_tokens = self.count_tokens_for(history);
        if total_tokens <= budget {
            return history.to_vec();
        }

        // Preserve the first user message as anchor.
        let (anchor, remaining) = if history[0].role == Role::User {
            (vec![history[0].clone()], &history[1..])
        } else {
            (Vec::new(), history)
        };

        let anchor_tokens = self.count_tokens_for(&anchor);
        let placeholder = Message {
            role: Role::System,
            content: "[Earlier conversation omitted to fit context window]".into(),
            ..Default::default()
        };
        let placeholder_tokens = self.count_tokens_for(std::slice::from_ref(&placeholder));
        let available = budget - anchor_tokens - placeholder_tokens;

        if available <= 0 {
            return anchor;
        }

        // Keep as many recent messages as fit, scanning from the end.
        let mut kept: Vec<Message> = Vec::with_capacity(remaining.len());
        let mut used = 0;
        for msg in remaining.iter().rev() {
            let msg_tokens = self.count_tokens_for(std::slice::from_ref(msg));
            if used + msg_tokens > available {
                break;
            }
            kept.push(msg.clone());
            used += msg_tokens;
        }
        kept.reverse();

        let dropped = remaining.len() - kept.len();
        let mut result = Vec::with_capacity(anchor.len() + 1 + kept.len());
        result.extend(anchor);
        if dropped > 0 {
            result.push(placeholder);
        }
        result.extend(kept);
        result
    }

    /// Count tokens via the provider (accurate) with tool defs included.
    fn count_tokens_for(&self, messages: &[Message]) -> i32 {
        self.provider.count_tokens(messages, &self.tool_defs)
    }
}

/// Truncate a tool result string, keeping head + tail with an omission marker.
pub fn truncate_tool_result(content: &str, max_len: usize) -> String {
    if max_len == 0 || content.len() <= max_len {
        return content.to_owned();
    }

    let head_len = max_len * 7 / 10;
    let tail_len = max_len * 3 / 10;

    // Find valid char boundaries.
    let head_end = snap_to_char_boundary(content, head_len);
    let tail_start = snap_to_char_boundary_back(content, content.len() - tail_len);

    let omitted = content.len() - head_end - (content.len() - tail_start);

    format!(
        "{}\n\n[... {} characters omitted ...]\n\n{}",
        &content[..head_end],
        omitted,
        &content[tail_start..],
    )
}

/// Find the nearest char boundary at or before `pos`.
fn snap_to_char_boundary(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p > 0 && !s.is_char_boundary(p) {
        p -= 1;
    }
    p
}

/// Find the nearest char boundary at or after `pos`.
fn snap_to_char_boundary_back(s: &str, pos: usize) -> usize {
    let mut p = pos.min(s.len());
    while p < s.len() && !s.is_char_boundary(p) {
        p += 1;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_content_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_tool_result(s, 100), s);
    }

    #[test]
    fn truncate_zero_max_unchanged() {
        let s = "hello world";
        assert_eq!(truncate_tool_result(s, 0), s);
    }

    #[test]
    fn truncate_long_content_has_marker() {
        let s = "a".repeat(200);
        let result = truncate_tool_result(&s, 100);
        assert!(result.contains("[..."));
        assert!(result.contains("characters omitted"));
        assert!(result.len() < s.len() + 60);
    }
}
