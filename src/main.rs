mod agent;
mod channels;
mod config;
mod llm;
mod mcp;
mod orchestration;
mod scheduler;
mod security;
mod server;
mod setup;
mod skills;
mod store;
mod terminal;
mod tool;
mod types;

use std::sync::Arc;

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

use crate::types::*;

// ---------------------------------------------------------------------------
// CLI definitions
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "marsclaw", version, about = "The fastest AI agent runtime. Written in Rust.")]
struct Cli {
    /// Config file path
    #[arg(short, long)]
    config: Option<String>,

    /// Override model
    #[arg(short, long)]
    model: Option<String>,

    /// Debug logging
    #[arg(short, long)]
    verbose: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Chat interactively
    Chat {
        /// Single prompt (non-interactive)
        prompt: Option<String>,
    },
    /// Start HTTP server + Web UI
    Serve {
        /// Listen address
        #[arg(long, default_value = ":8080")]
        addr: Option<String>,
    },
    /// Channel management
    Channels {
        #[command(subcommand)]
        action: ChannelAction,
    },
    /// Skill management
    Skills {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Interactive setup wizard
    Init,
}

#[derive(Subcommand)]
enum ChannelAction {
    /// Add a new channel interactively
    Add,
    /// List configured channels
    List,
    /// Remove a channel
    Remove {
        /// Channel ID to remove
        id: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillAction {
    /// List available and installed skills
    List,
    /// Install a skill by name or URL
    Install { source: String },
    /// Set the active skill
    Use { id: String },
}

// ---------------------------------------------------------------------------
// Default SOUL prompt
// ---------------------------------------------------------------------------

const DEFAULT_SOUL: &str = "You are MarsClaw, a fast and capable AI coding assistant.\n\n\
Rules:\n\
- Be concise and direct. Lead with the answer.\n\
- Use tools to read files before editing them.\n\
- Use edit_file for surgical changes, write_file for new files.\n\
- Run shell commands to verify your work (tests, build).\n\
- Never guess file contents -- always read first.\n\
- When you're done, say what you did in 1-2 sentences.";

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Setup logging.
    let filter = if cli.verbose { "debug" } else { "info,marsclaw=info" };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .init();

    // Load config.
    let mut cfg = config::Config::load(cli.config.as_deref())?;
    if let Some(ref model) = cli.model {
        match cfg.providers.default.as_str() {
            "anthropic" => cfg.providers.anthropic.default_model = model.clone(),
            "openai" => cfg.providers.openai.default_model = model.clone(),
            "gemini" => cfg.providers.gemini.default_model = model.clone(),
            "ollama" => cfg.providers.ollama.default_model = model.clone(),
            _ => {}
        }
    }

    // Dispatch subcommand.
    match cli.command {
        Some(Commands::Init) => {
            setup::run_wizard()?;
            Ok(())
        }
        Some(Commands::Channels { action }) => {
            let store = channels::ChannelStore::new();
            match action {
                ChannelAction::Add => channels::run_add(&store)?,
                ChannelAction::List => channels::run_list(&store)?,
                ChannelAction::Remove { id } => channels::run_remove(&store, id.as_deref())?,
            }
            Ok(())
        }
        Some(Commands::Skills { action }) => {
            match action {
                SkillAction::List => skills::run_list()?,
                SkillAction::Install { source } => skills::run_install(&source)?,
                SkillAction::Use { id } => skills::run_use(&id)?,
            }
            Ok(())
        }
        Some(Commands::Serve { addr }) => {
            let addr = normalize_addr(&addr.unwrap_or_else(|| ":8080".into()));
            run_serve(&cfg, &addr).await
        }
        Some(Commands::Chat { prompt: Some(p) }) => {
            run_single_prompt(&cfg, &p).await
        }
        // Default: interactive chat.
        None | Some(Commands::Chat { prompt: None }) => {
            run_interactive(&cfg).await
        }
    }
}

// ---------------------------------------------------------------------------
// Interactive chat
// ---------------------------------------------------------------------------

async fn run_interactive(cfg: &config::Config) -> anyhow::Result<()> {
    let provider = create_provider(cfg)?;
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let registry = tool::Registry::default_registry(&cwd);
    let cost = create_cost_tracker(cfg);
    let model = cfg.model().to_string();

    let (soul, _agents) = agent::discovery::discover_project_prompts(&cwd);
    let soul = skills::get_active_prompt().unwrap_or(if soul.is_empty() {
        DEFAULT_SOUL.to_string()
    } else {
        soul
    });

    terminal::run(provider, cfg.agent.clone(), registry, cost, &soul, &model).await
}

// ---------------------------------------------------------------------------
// Single prompt
// ---------------------------------------------------------------------------

async fn run_single_prompt(cfg: &config::Config, prompt: &str) -> anyhow::Result<()> {
    let provider = create_provider(cfg)?;
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let registry = tool::Registry::default_registry(&cwd);
    let cost = create_cost_tracker(cfg);
    let model = cfg.model().to_string();

    let (soul, agent_prompt) = agent::discovery::discover_project_prompts(&cwd);
    let soul = skills::get_active_prompt().unwrap_or(if soul.is_empty() {
        DEFAULT_SOUL.to_string()
    } else {
        soul
    });

    let agent = agent::Agent::new(
        provider,
        cfg.agent.clone(),
        registry.executors().clone(),
        registry.defs().to_vec(),
    )
    .with_stream_handler(|ev| {
        match ev {
            StreamEvent::Text { delta, .. } => {
                print!("{delta}");
                use std::io::Write;
                std::io::stdout().flush().ok();
            }
            StreamEvent::ToolStart { tool_call } => {
                eprintln!("\x1b[33m> {}\x1b[0m", tool_call.name);
            }
            StreamEvent::ToolDone { tool_call, .. } => {
                eprintln!("\x1b[32m\u{2713} {}\x1b[0m", tool_call.name);
            }
            StreamEvent::Error { message } => {
                eprintln!("\x1b[31m\u{2717} {message}\x1b[0m");
            }
        }
    })
    .with_cost_tracker(cost.clone());

    let parts = ContextParts {
        soul_prompt: soul,
        agent_prompt,
        history: vec![Message {
            role: Role::User,
            content: prompt.to_string(),
            ..Default::default()
        }],
        ..Default::default()
    };

    let cancel = CancellationToken::new();
    let result = agent.run(cancel, parts).await;
    println!();

    if cfg.cost.inline_display {
        let cost_line = cost.format_cost_line(&model, result.total_input, result.total_output);
        eprintln!("\x1b[2m{cost_line}\x1b[0m");
    }

    if let Some(ref err) = result.error {
        anyhow::bail!("{err}");
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// HTTP server
// ---------------------------------------------------------------------------

async fn run_serve(cfg: &config::Config, addr: &str) -> anyhow::Result<()> {
    let model = cfg.model().to_string();
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let (soul, _) = agent::discovery::discover_project_prompts(&cwd);
    let soul = skills::get_active_prompt().unwrap_or(if soul.is_empty() {
        DEFAULT_SOUL.to_string()
    } else {
        soul
    });

    let db: Arc<dyn store::Store> = Arc::new(store::SqliteStore::new()?);

    // Build task info for dashboard.
    let tasks: Vec<server::TaskInfo> = cfg
        .scheduler
        .tasks
        .iter()
        .map(|tc| server::TaskInfo {
            id: tc.id.clone(),
            name: tc.name.clone(),
            schedule: tc.schedule.clone(),
            channel: tc.channel.clone(),
            enabled: tc.enabled,
        })
        .collect();

    let server_config = server::ServerConfig {
        addr: addr.to_string(),
        model,
        soul,
        tasks,
    };

    eprintln!(
        "\x1b[1m\x1b[36mMarsClaw server running at http://localhost{addr}\x1b[0m"
    );

    server::run(server_config, db).await
}

// ---------------------------------------------------------------------------
// Provider factory
// ---------------------------------------------------------------------------

fn create_provider(cfg: &config::Config) -> anyhow::Result<Arc<dyn Provider>> {
    match cfg.providers.default.as_str() {
        "anthropic" => {
            let key = std::env::var(&cfg.providers.anthropic.api_key_env).map_err(|_| {
                anyhow::anyhow!(
                    "Set {} to use Anthropic",
                    cfg.providers.anthropic.api_key_env
                )
            })?;
            // Route through OpenAI-compatible endpoint is not available for Anthropic.
            // For now, we use the OpenAI provider as a placeholder.
            Ok(Arc::new(llm::openai::OpenAiProvider::new(
                &key,
                "https://api.anthropic.com/v1",
                &cfg.providers.anthropic.default_model,
            )))
        }
        "gemini" => {
            let key = std::env::var(&cfg.providers.gemini.api_key_env).map_err(|_| {
                anyhow::anyhow!("Set {} to use Gemini", cfg.providers.gemini.api_key_env)
            })?;
            Ok(Arc::new(llm::openai::OpenAiProvider::gemini(
                &key,
                &cfg.providers.gemini.default_model,
            )))
        }
        "openai" => {
            let key = std::env::var(&cfg.providers.openai.api_key_env).map_err(|_| {
                anyhow::anyhow!("Set {} to use OpenAI", cfg.providers.openai.api_key_env)
            })?;
            Ok(Arc::new(llm::openai::OpenAiProvider::new(
                &key,
                &cfg.providers.openai.base_url,
                &cfg.providers.openai.default_model,
            )))
        }
        "ollama" => Ok(Arc::new(llm::openai::OpenAiProvider::ollama(
            &cfg.providers.ollama.default_model,
        ))),
        other => anyhow::bail!("Unknown provider: {other}"),
    }
}

// ---------------------------------------------------------------------------
// Cost tracker factory
// ---------------------------------------------------------------------------

fn create_cost_tracker(cfg: &config::Config) -> Arc<dyn CostRecorder> {
    let tracker = llm::cost::CostTracker::new();
    if cfg.cost.daily_budget > 0.0 {
        tracker.set_daily_limit(cfg.cost.daily_budget);
    }
    if cfg.cost.monthly_budget > 0.0 {
        tracker.set_monthly_limit(cfg.cost.monthly_budget);
    }
    Arc::new(tracker)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Normalize ":8080" to "0.0.0.0:8080".
fn normalize_addr(addr: &str) -> String {
    if addr.starts_with(':') {
        format!("0.0.0.0{addr}")
    } else {
        addr.to_string()
    }
}
