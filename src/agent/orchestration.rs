//! Multi-agent orchestration patterns.
//!
//! Provides higher-level coordination strategies for running multiple
//! agents together. Each pattern controls how agents are invoked and
//! how their outputs are combined.
//!
//! Patterns:
//! - **Pipeline**: sequential chain where each agent's output feeds the next.
//! - **Parallel**: run agents concurrently and merge results.
//! - **Debate**: agents argue opposing positions, a judge picks the best.
//! - **Supervisor**: a meta-agent delegates subtasks to specialist agents.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::agent::Agent;
use crate::types::*;

// ---------------------------------------------------------------------------
// Pipeline -- sequential agent chaining
// ---------------------------------------------------------------------------

/// One stage in a sequential pipeline.
pub struct PipelineStage {
    pub name: String,
    pub agent: Agent,
    pub parts: ContextParts,
}

/// Run agents sequentially: output of stage N becomes input of stage N+1.
pub async fn run_pipeline(stages: Vec<PipelineStage>, input: &str) -> Result<RunResult, String> {
    if stages.is_empty() {
        return Err("pipeline requires at least one stage".into());
    }

    let cancel = CancellationToken::new();
    let start = Instant::now();
    let mut current = input.to_string();
    let mut total_input: i32 = 0;
    let mut total_output: i32 = 0;
    let mut all_trace = Vec::new();

    for (i, stage) in stages.into_iter().enumerate() {
        let parts = ContextParts {
            soul_prompt: stage.parts.soul_prompt,
            agent_prompt: stage.parts.agent_prompt,
            memory: stage.parts.memory,
            history: vec![Message {
                role: Role::User,
                content: current.clone(),
                ..Default::default()
            }],
        };

        let result = stage.agent.run(cancel.clone(), parts).await;
        total_input += result.total_input;
        total_output += result.total_output;
        all_trace.extend(result.trace);

        if let Some(ref err) = result.error {
            return Err(format!(
                "pipeline stage {} ({}) failed: {}",
                i, stage.name, err
            ));
        }

        current = result.response;
    }

    Ok(RunResult {
        response: current,
        stop_reason: StopReason::FinalResponse,
        turn_count: all_trace
            .iter()
            .filter(|t| t.phase == "llm_call")
            .count() as i32,
        total_input,
        total_output,
        duration: start.elapsed(),
        trace: all_trace,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Parallel -- concurrent agent execution + optional aggregation
// ---------------------------------------------------------------------------

/// One agent in a fan-out group.
pub struct ParallelAgent {
    pub name: String,
    pub agent: Agent,
    pub parts: ContextParts,
}

/// Configuration for parallel fan-out execution.
pub struct ParallelConfig {
    pub agents: Vec<ParallelAgent>,
    /// Optional aggregator agent that synthesizes all results.
    pub aggregator: Option<(Agent, ContextParts)>,
}

/// Fan out a task to N agents concurrently, then optionally aggregate.
pub async fn run_parallel(config: ParallelConfig, task: &str) -> Result<RunResult, String> {
    if config.agents.is_empty() {
        return Err("parallel requires at least one agent".into());
    }

    let cancel = CancellationToken::new();
    let start = Instant::now();
    let agent_count = config.agents.len();

    let mut handles = Vec::with_capacity(agent_count);
    for pa in config.agents {
        let task_str = task.to_string();
        let cancel = cancel.clone();
        let name = pa.name.clone();

        handles.push(tokio::spawn(async move {
            let parts = ContextParts {
                soul_prompt: pa.parts.soul_prompt,
                agent_prompt: pa.parts.agent_prompt,
                memory: pa.parts.memory,
                history: vec![Message {
                    role: Role::User,
                    content: task_str,
                    ..Default::default()
                }],
            };
            (name, pa.agent.run(cancel, parts).await)
        }));
    }

    let mut total_input: i32 = 0;
    let mut total_output: i32 = 0;
    let mut all_trace = Vec::new();
    let mut combined = String::new();

    for handle in handles {
        match handle.await {
            Ok((name, result)) => {
                total_input += result.total_input;
                total_output += result.total_output;
                all_trace.extend(result.trace);

                if let Some(ref err) = result.error {
                    combined.push_str(&format!("## {name}\n[Error: {err}]\n\n"));
                } else {
                    combined.push_str(&format!("## {name}\n{}\n\n", result.response));
                }
            }
            Err(join_err) => {
                combined.push_str(&format!("## <unknown>\n[Error: {join_err}]\n\n"));
            }
        }
    }

    let Some((aggregator, agg_base_parts)) = config.aggregator else {
        return Ok(RunResult {
            response: combined,
            stop_reason: StopReason::FinalResponse,
            turn_count: 1,
            total_input,
            total_output,
            duration: start.elapsed(),
            trace: all_trace,
            ..Default::default()
        });
    };

    let agg_parts = ContextParts {
        soul_prompt: agg_base_parts.soul_prompt,
        agent_prompt: agg_base_parts.agent_prompt,
        memory: agg_base_parts.memory,
        history: vec![Message {
            role: Role::User,
            content: format!(
                "Task: {task}\n\nResponses from agents:\n\n{combined}\n\n\
                 Synthesize these into a single response."
            ),
            ..Default::default()
        }],
    };

    let agg_result = aggregator.run(cancel, agg_parts).await;
    total_input += agg_result.total_input;
    total_output += agg_result.total_output;
    all_trace.extend(agg_result.trace);

    Ok(RunResult {
        response: agg_result.response,
        stop_reason: agg_result.stop_reason,
        turn_count: (agent_count + 1) as i32,
        total_input,
        total_output,
        duration: start.elapsed(),
        error: agg_result.error,
        trace: all_trace,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Debate -- multi-round argumentation with a judge
// ---------------------------------------------------------------------------

/// One participant in a debate. Uses `Arc<Agent>` so the same agent
/// can be reused across multiple rounds.
pub struct Debater {
    pub name: String,
    /// Position directive, e.g. "argue for X" or "argue against X".
    pub position: String,
    pub agent: Arc<Agent>,
    pub parts: ContextParts,
}

/// Configuration for a multi-round debate.
pub struct DebateConfig {
    pub debaters: Vec<Debater>,
    pub judge: Agent,
    pub judge_parts: ContextParts,
    /// Number of debate rounds (defaults to 2 if zero).
    pub rounds: usize,
}

/// Run multiple debate rounds, then have a judge synthesize a verdict.
pub async fn run_debate(config: DebateConfig, topic: &str) -> Result<RunResult, String> {
    if config.debaters.len() < 2 {
        return Err("debate requires at least 2 debaters".into());
    }

    let rounds = if config.rounds == 0 { 2 } else { config.rounds };
    let cancel = CancellationToken::new();
    let start = Instant::now();
    let debater_count = config.debaters.len();

    let mut total_input: i32 = 0;
    let mut total_output: i32 = 0;
    let mut all_trace = Vec::new();
    let mut transcript = format!("# Debate: {topic}\n\n");
    let mut previous_args = String::new();

    for round in 1..=rounds {
        transcript.push_str(&format!("## Round {round}\n\n"));

        let mut handles = Vec::with_capacity(debater_count);
        for d in &config.debaters {
            let topic_str = topic.to_string();
            let prev = previous_args.clone();
            let cancel = cancel.clone();
            let name = d.name.clone();
            let position = d.position.clone();
            let agent = Arc::clone(&d.agent);
            let base_parts = d.parts.clone();

            handles.push(tokio::spawn(async move {
                let mut prompt = format!("Topic: {topic_str}\nYour position: {position}");
                if !prev.is_empty() {
                    prompt.push_str(&format!(
                        "\n\nPrevious round arguments:\n{prev}\n\n\
                         Respond to the other debaters' points."
                    ));
                }

                let parts = ContextParts {
                    soul_prompt: base_parts.soul_prompt,
                    agent_prompt: base_parts.agent_prompt,
                    memory: base_parts.memory,
                    history: vec![Message {
                        role: Role::User,
                        content: prompt,
                        ..Default::default()
                    }],
                };

                let result = agent.run(cancel, parts).await;
                (name, result)
            }));
        }

        let mut round_args = String::new();
        for handle in handles {
            match handle.await {
                Ok((name, result)) => {
                    total_input += result.total_input;
                    total_output += result.total_output;
                    all_trace.extend(result.trace);

                    if let Some(ref err) = result.error {
                        transcript.push_str(&format!("### {name}\n[Error: {err}]\n\n"));
                    } else {
                        transcript
                            .push_str(&format!("### {name}\n{}\n\n", result.response));
                        round_args
                            .push_str(&format!("**{name}**: {}\n\n", result.response));
                    }
                }
                Err(join_err) => {
                    transcript
                        .push_str(&format!("### <unknown>\n[Error: {join_err}]\n\n"));
                }
            }
        }

        previous_args = round_args;
    }

    // Judge synthesizes.
    let judge_parts = ContextParts {
        soul_prompt: config.judge_parts.soul_prompt,
        agent_prompt: format!(
            "{}\n\nYou are the judge. Evaluate the arguments from all debaters \
             and provide a balanced synthesis with your verdict.",
            config.judge_parts.agent_prompt
        ),
        memory: config.judge_parts.memory,
        history: vec![Message {
            role: Role::User,
            content: format!("{transcript}\n\nProvide your verdict and synthesis."),
            ..Default::default()
        }],
    };

    let judge_result = config.judge.run(cancel, judge_parts).await;
    total_input += judge_result.total_input;
    total_output += judge_result.total_output;
    all_trace.extend(judge_result.trace);

    Ok(RunResult {
        response: judge_result.response,
        stop_reason: judge_result.stop_reason,
        turn_count: (rounds * debater_count + 1) as i32,
        total_input,
        total_output,
        duration: start.elapsed(),
        error: judge_result.error,
        trace: all_trace,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Supervisor -- meta-agent delegates to specialists via tool calling
// ---------------------------------------------------------------------------

/// Definition of a specialist agent that can be invoked as a tool.
pub struct SubAgentDef {
    pub name: String,
    pub description: String,
    pub agent: Agent,
    pub parts: ContextParts,
}

/// Wraps a child agent as a [`ToolExecutor`] so the supervisor can call it.
struct SubAgentExecutor {
    agent: tokio::sync::Mutex<Agent>,
    parts: ContextParts,
}

#[async_trait::async_trait]
impl ToolExecutor for SubAgentExecutor {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: serde_json::Value = call.arguments.clone();
        let task = args
            .get("task")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'task' in sub-agent arguments"))?
            .to_string();

        let extra_context = args.get("context").cloned();

        let mut memory = self.parts.memory.clone();
        if let Some(ctx) = extra_context {
            memory.push_str("\n\n# Delegated Context\n");
            memory.push_str(&ctx.to_string());
        }

        let parts = ContextParts {
            soul_prompt: self.parts.soul_prompt.clone(),
            agent_prompt: self.parts.agent_prompt.clone(),
            memory,
            history: vec![Message {
                role: Role::User,
                content: task,
                ..Default::default()
            }],
        };

        let cancel = CancellationToken::new();
        let agent = self.agent.lock().await;
        let result = agent.run(cancel, parts).await;
        drop(agent);

        if let Some(err) = &result.error {
            return Err(anyhow::anyhow!(
                "sub-agent {:?} failed: {}",
                call.name,
                err
            ));
        }

        info!(
            agent = %call.name,
            stop_reason = ?result.stop_reason,
            turns = result.turn_count,
            "sub-agent complete"
        );

        Ok(result.response)
    }
}

/// Convert sub-agent definitions into tool definitions and executors.
fn register_sub_agents(
    defs: Vec<SubAgentDef>,
) -> (Vec<ToolDef>, HashMap<String, Arc<dyn ToolExecutor>>) {
    let schema = json!({
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
    });

    let mut tool_defs = Vec::with_capacity(defs.len());
    let mut executors: HashMap<String, Arc<dyn ToolExecutor>> =
        HashMap::with_capacity(defs.len());

    for def in defs {
        tool_defs.push(ToolDef {
            name: def.name.clone(),
            description: def.description,
            parameters: schema.clone(),
            danger_level: DangerLevel::None,
            read_only: false,
        });

        executors.insert(
            def.name,
            Arc::new(SubAgentExecutor {
                agent: tokio::sync::Mutex::new(def.agent),
                parts: def.parts,
            }),
        );
    }

    (tool_defs, executors)
}

/// Configuration for the supervisor pattern.
pub struct SupervisorConfig {
    pub coordinator: Agent,
    pub coord_parts: ContextParts,
    pub specialists: Vec<SubAgentDef>,
}

/// Run a coordinator that delegates tasks to specialist sub-agents via tool calling.
pub async fn run_supervisor(config: SupervisorConfig, task: &str) -> RunResult {
    let (tool_defs, executors) = register_sub_agents(config.specialists);

    let mut coordinator = config.coordinator;
    coordinator.add_tools(tool_defs, executors);

    let parts = ContextParts {
        soul_prompt: config.coord_parts.soul_prompt,
        agent_prompt: format!(
            "{}\n\nYou are a supervisor. Delegate tasks to your specialist agents. \
             Synthesize their outputs into a final response.",
            config.coord_parts.agent_prompt
        ),
        memory: config.coord_parts.memory,
        history: vec![Message {
            role: Role::User,
            content: task.to_string(),
            ..Default::default()
        }],
    };

    let cancel = CancellationToken::new();
    coordinator.run(cancel, parts).await
}
