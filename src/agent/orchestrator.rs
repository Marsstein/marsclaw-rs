//! Sub-agent orchestration: delegate tasks to child agents as tool calls.
//!
//! Ported from Go: internal/agent/orchestrator.go

use crate::types::*;

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::info;

use super::Agent;

// ---------------------------------------------------------------------------
// SubAgentDef
// ---------------------------------------------------------------------------

/// Defines a child agent that can be invoked as a tool.
pub struct SubAgentDef {
    pub name: String,
    pub description: String,
    pub agent: Arc<Agent>,
    pub parts: ContextParts,
}

// ---------------------------------------------------------------------------
// SubAgentExecutor
// ---------------------------------------------------------------------------

/// Wraps a child agent as a [`ToolExecutor`].
pub struct SubAgentExecutor {
    child: Arc<Agent>,
    parts: ContextParts,
    cancel: CancellationToken,
}

impl SubAgentExecutor {
    pub fn new(child: Arc<Agent>, parts: ContextParts, cancel: CancellationToken) -> Self {
        Self {
            child,
            parts,
            cancel,
        }
    }
}

/// Arguments parsed from the tool call JSON.
#[derive(Deserialize)]
struct SubAgentArgs {
    task: String,
    #[serde(default)]
    context: Option<serde_json::Value>,
}

#[async_trait::async_trait]
impl ToolExecutor for SubAgentExecutor {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: SubAgentArgs = serde_json::from_value(call.arguments.clone())
            .map_err(|e| anyhow::anyhow!("invalid sub-agent arguments: {e}"))?;

        let mut child_parts = ContextParts {
            soul_prompt: self.parts.soul_prompt.clone(),
            agent_prompt: self.parts.agent_prompt.clone(),
            memory: self.parts.memory.clone(),
            history: vec![Message {
                role: Role::User,
                content: args.task,
                timestamp: Utc::now(),
                ..Default::default()
            }],
        };

        if let Some(ctx) = args.context {
            child_parts.memory.push_str("\n\n# Delegated Context\n");
            child_parts.memory.push_str(&ctx.to_string());
        }

        let result = self.child.run(self.cancel.child_token(), child_parts).await;

        if let Some(err) = &result.error {
            return Err(anyhow::anyhow!("sub-agent {:?} failed: {err}", call.name));
        }

        info!(
            agent = %call.name,
            stop_reason = ?result.stop_reason,
            turns = result.turn_count,
            "sub-agent complete",
        );

        Ok(result.response)
    }
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Schema shared by all sub-agent tool definitions.
fn sub_agent_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "task": {
                "type": "string",
                "description": "The task or question to delegate to this agent"
            },
            "context": {
                "type": "object",
                "description": "Optional additional context for the agent"
            }
        },
        "required": ["task"]
    })
}

/// Converts [`SubAgentDef`]s into tool definitions and executors that can be
/// added to a parent agent via [`Agent::add_tools`].
pub fn register_sub_agents(
    defs: Vec<SubAgentDef>,
    cancel: CancellationToken,
) -> (Vec<ToolDef>, HashMap<String, Arc<dyn ToolExecutor>>) {
    let schema = sub_agent_schema();
    let mut tool_defs = Vec::with_capacity(defs.len());
    let mut executors: HashMap<String, Arc<dyn ToolExecutor>> = HashMap::with_capacity(defs.len());

    for def in defs {
        tool_defs.push(ToolDef {
            name: def.name.clone(),
            description: def.description,
            parameters: schema.clone(),
            danger_level: DangerLevel::None,
            read_only: false,
        });

        let executor = SubAgentExecutor::new(def.agent, def.parts, cancel.clone());
        executors.insert(def.name, Arc::new(executor));
    }

    (tool_defs, executors)
}
