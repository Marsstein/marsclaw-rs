//! Interactive terminal REPL for MarsClaw.
//!
//! Ported from Go: internal/terminal/terminal.go

use std::io::{self, BufRead, Write as IoWrite};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, SafetyCheck};
use crate::config::AgentConfig;
use crate::tool::Registry;
use crate::types::*;

// ANSI escape codes.
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

/// Run an interactive terminal session.
pub async fn run(
    provider: Arc<dyn Provider>,
    config: AgentConfig,
    registry: Registry,
    cost: Arc<dyn CostRecorder>,
    safety: Option<Arc<dyn SafetyCheck>>,
    soul: &str,
    model: &str,
) -> anyhow::Result<()> {
    print_banner(model);

    let mut history: Vec<Message> = Vec::with_capacity(64);
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    loop {
        print!("\n{BOLD}{CYAN}> {RESET}");
        io::stderr().flush().ok();
        io::stdout().flush().ok();

        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            // EOF
            println!();
            return Ok(());
        }

        let input = line.trim();
        if input.is_empty() {
            continue;
        }

        match input {
            "/quit" | "/exit" | "/q" => {
                println!("{DIM}Bye!{RESET}");
                return Ok(());
            }
            "/clear" => {
                history.clear();
                println!("{DIM}Conversation cleared.{RESET}");
                continue;
            }
            "/history" => {
                print_history(&history);
                continue;
            }
            "/help" => {
                print_help();
                continue;
            }
            _ => {}
        }

        history.push(Message {
            role: Role::User,
            content: input.to_string(),
            ..Default::default()
        });

        let parts = ContextParts {
            soul_prompt: soul.to_string(),
            history: history.clone(),
            ..Default::default()
        };

        println!();

        let mut agent = Agent::new(
            provider.clone(),
            config.clone(),
            registry.executors().clone(),
            registry.defs().to_vec(),
        )
        .with_stream_handler(stream_handler)
        .with_cost_tracker(cost.clone());

        if let Some(ref s) = safety {
            agent = agent.with_safety(s.clone());
        }

        let cancel = CancellationToken::new();
        let result = agent.run(cancel, parts).await;

        // Update history from agent result.
        if !result.history.is_empty() {
            history = result.history;
        } else {
            history.push(Message {
                role: Role::Assistant,
                content: result.response.clone(),
                ..Default::default()
            });
        }

        if result.stop_reason == StopReason::Error("cancelled".into()) {
            eprintln!("{RED}{BOLD}Error: {}{RESET}", result.error.as_deref().unwrap_or("unknown"));
        }

        println!();

        // Cost line.
        let cost_line = cost.format_cost_line(model, result.total_input, result.total_output);
        println!("{DIM}{cost_line}{RESET}");

        if result.stop_reason == StopReason::MaxTurns {
            println!("{YELLOW}Agent stopped: max turns reached{RESET}");
        }
    }
}

fn stream_handler(ev: StreamEvent) {
    match ev {
        StreamEvent::Text { delta, .. } => {
            print!("{delta}");
            io::stdout().flush().ok();
        }
        StreamEvent::ToolStart { tool_call } => {
            eprintln!("{YELLOW}> {}{RESET}", tool_call.name);
        }
        StreamEvent::ToolDone { tool_call, .. } => {
            eprintln!("{GREEN}\u{2713} {}{RESET}", tool_call.name);
        }
        StreamEvent::Error { message } => {
            eprintln!("{RED}\u{2717} {message}{RESET}");
        }
    }
}

fn print_banner(model: &str) {
    println!(
        r#"
{BOLD}{CYAN}  _     _ _        ____ _
 | |   (_) |_ ___ / ___| | __ ___      __
 | |   | | __/ _ \ |   | |/ _` \ \ /\ / /
 | |___| | ||  __/ |___| | (_| |\ V  V /
 |_____|_|\__\___|\____|_|\__,_| \_/\_/  {RESET}

{DIM}  Lightweight, secure, multi-agent AI runtime{RESET}
{DIM}  Model: {model} | Type /help for commands{RESET}
"#
    );
}

fn print_help() {
    println!(
        r#"
{BOLD}Commands:{RESET}
  /help      Show this help
  /clear     Clear conversation history
  /history   Show message history
  /quit      Exit MarsClaw

{BOLD}Available tools:{RESET} read_file, write_file, edit_file, shell, list_files, search
"#
    );
}

fn print_history(history: &[Message]) {
    if history.is_empty() {
        println!("{DIM}No messages yet.{RESET}");
        return;
    }

    for msg in history {
        let (role_str, color) = match msg.role {
            Role::User => ("user", CYAN),
            Role::Assistant => ("assistant", GREEN),
            Role::Tool => ("tool", YELLOW),
            Role::System => ("system", DIM),
        };

        let content = if msg.content.chars().count() > 100 {
            let truncated: String = msg.content.chars().take(100).collect();
            format!("{truncated}...")
        } else {
            msg.content.clone()
        };

        println!("{color}[{role_str}]{RESET} {content}");
    }
}
