mod agent;
mod bots;
mod config;
mod llm;
mod platform;
mod server;
mod store;
mod tool;
mod types;

use bots::{channels, discord, slack, telegram, whatsapp};
use platform::{scheduler, security, skills};

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
    /// Run Telegram bot
    Telegram {
        /// Channel ID from channels.json
        #[arg(long)]
        channel: Option<String>,
    },
    /// Run Discord bot
    Discord {
        /// Channel ID from channels.json
        #[arg(long)]
        channel: Option<String>,
    },
    /// Run Slack bot
    Slack {
        /// Channel ID from channels.json
        #[arg(long)]
        channel: Option<String>,
    },
    /// Run WhatsApp webhook bot (mounted on serve)
    WhatsApp {
        /// Channel ID from channels.json
        #[arg(long)]
        channel: Option<String>,
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
        Some(Commands::Telegram { channel }) => {
            run_telegram(&cfg, channel.as_deref()).await
        }
        Some(Commands::Discord { channel }) => {
            run_discord(&cfg, channel.as_deref()).await
        }
        Some(Commands::Slack { channel }) => {
            run_slack(&cfg, channel.as_deref()).await
        }
        Some(Commands::WhatsApp { channel }) => {
            run_whatsapp(&cfg, channel.as_deref()).await
        }
        Some(Commands::Init) => {
            config::setup::run_wizard()?;
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
// Shared runtime setup: provider + registry + MCP + safety + memory
// ---------------------------------------------------------------------------

/// Everything the agent needs to run, built from config.
struct RuntimeStack {
    provider: Arc<dyn Provider>,
    registry: tool::Registry,
    cost: Arc<dyn CostRecorder>,
    safety: Option<Arc<dyn agent::SafetyCheck>>,
    soul: String,
    agent_prompt: String,
    memory_text: String,
    model: String,
}

async fn setup_runtime(cfg: &config::Config) -> anyhow::Result<RuntimeStack> {
    let provider = create_provider(cfg)?;
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let mut registry = tool::Registry::default_registry(&cwd);
    let cost = create_cost_tracker(cfg);
    let model = cfg.model().to_string();

    // Discover project prompts.
    let (discovered_soul, agent_prompt) = agent::discovery::discover_project_prompts(&cwd);
    let soul = skills::get_active_prompt().unwrap_or(if discovered_soul.is_empty() {
        DEFAULT_SOUL.to_string()
    } else {
        discovered_soul
    });

    // Wire MCP tools into the registry.
    if !cfg.mcp.is_empty() {
        match platform::mcp::register_mcp_servers(&cfg.mcp).await {
            Ok((defs, executors, _clients)) => {
                tracing::info!(count = defs.len(), "MCP tools registered");
                registry.merge(defs, executors);
            }
            Err(e) => {
                tracing::warn!("MCP registration failed (continuing without): {e}");
            }
        }
    }

    // Build safety checker from config.
    let safety: Option<Arc<dyn agent::SafetyCheck>> = {
        let checker = security::SafetyChecker::new(
            security::SafetyConfig {
                strict_approval: cfg.security.strict_approval,
                scan_credentials: cfg.security.scan_credentials,
                path_traversal_guard: cfg.security.path_traversal_guard,
                allowed_dirs: cfg.security.allowed_dirs.clone(),
            },
            registry.defs(),
            None,
        );
        Some(Arc::new(checker))
    };

    // Load persistent memory with config budgets.
    let memory_text = match platform::memory::MemoryManager::with_budgets(
        cfg.memory.episodic_max_chars,
        cfg.memory.semantic_max_chars,
        cfg.memory.procedural_max_chars,
    ) {
        Ok(mm) => mm.inject(&soul),
        Err(e) => {
            tracing::warn!("memory init failed (continuing without): {e}");
            String::new()
        }
    };

    Ok(RuntimeStack {
        provider,
        registry,
        cost,
        safety,
        soul,
        agent_prompt,
        memory_text,
        model,
    })
}

// ---------------------------------------------------------------------------
// Interactive chat
// ---------------------------------------------------------------------------

async fn run_interactive(cfg: &config::Config) -> anyhow::Result<()> {
    let rt = setup_runtime(cfg).await?;
    server::terminal::run(
        rt.provider, cfg.agent.clone(), rt.registry, rt.cost, rt.safety,
        &rt.soul, &rt.model,
    ).await
}

// ---------------------------------------------------------------------------
// Single prompt
// ---------------------------------------------------------------------------

async fn run_single_prompt(cfg: &config::Config, prompt: &str) -> anyhow::Result<()> {
    let rt = setup_runtime(cfg).await?;

    let mut agent = agent::Agent::new(
        rt.provider,
        cfg.agent.clone(),
        rt.registry.executors().clone(),
        rt.registry.defs().to_vec(),
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
    .with_cost_tracker(rt.cost.clone());

    if let Some(safety) = rt.safety {
        agent = agent.with_safety(safety);
    }

    let parts = ContextParts {
        soul_prompt: rt.soul,
        agent_prompt: rt.agent_prompt,
        memory: rt.memory_text,
        history: vec![Message {
            role: Role::User,
            content: prompt.to_string(),
            ..Default::default()
        }],
    };

    let cancel = CancellationToken::new();
    let result = agent.run(cancel, parts).await;
    println!();

    if cfg.cost.inline_display {
        let cost_line = rt.cost.format_cost_line(&rt.model, result.total_input, result.total_output);
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
    let rt = setup_runtime(cfg).await?;
    let db: Arc<dyn store::Store> = Arc::new(store::SqliteStore::new()?);

    // Mount WhatsApp webhook if configured.
    let extra = if let Some(ref wa_cfg) = cfg.whatsapp {
        if !wa_cfg.phone_number_id.is_empty() {
            let bot = Arc::new(whatsapp::WhatsAppBot::new(
                &wa_cfg.phone_number_id,
                &wa_cfg.access_token,
                &wa_cfg.verify_token,
                rt.provider.clone(),
                cfg.agent.clone(),
                rt.registry.clone(),
                rt.cost.clone(),
                db.clone(),
                &rt.soul,
                &rt.model,
            ));
            tracing::info!("whatsapp webhook mounted at /webhook/whatsapp");
            Some(bot.router())
        } else {
            None
        }
    } else {
        None
    };

    // Start scheduler in background if tasks are configured.
    let enabled_tasks: Vec<_> = cfg.scheduler.tasks.iter()
        .filter(|t| t.enabled)
        .map(|t| scheduler::Task {
            id: t.id.clone(),
            name: t.name.clone(),
            schedule: t.schedule.clone(),
            prompt: t.prompt.clone(),
            channel: t.channel.clone(),
            enabled: true,
        })
        .collect();

    let cancel = CancellationToken::new();
    if !enabled_tasks.is_empty() {
        let sched = scheduler::Scheduler::new(
            enabled_tasks,
            rt.provider.clone(),
            cfg.agent.clone(),
            rt.registry.clone(),
            rt.soul.clone(),
            Arc::new(|channel: &str, msg: &str| {
                tracing::info!(channel = channel, "scheduler output: {}", &msg[..msg.len().min(200)]);
            }),
        );
        let sched_cancel = cancel.clone();
        tokio::spawn(async move {
            sched.run(sched_cancel).await;
        });
        tracing::info!(count = cfg.scheduler.tasks.len(), "scheduler started");
    }

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
        model: rt.model,
        soul: rt.soul,
        tasks,
        provider: rt.provider,
        agent_cfg: cfg.agent.clone(),
        registry: rt.registry,
        cost: rt.cost,
        safety: rt.safety,
    };

    eprintln!(
        "\x1b[1m\x1b[36mMarsClaw server running at http://localhost{addr}\x1b[0m"
    );

    let result = server::run(server_config, db, extra).await;
    cancel.cancel();
    result
}

// ---------------------------------------------------------------------------
// Telegram bot
// ---------------------------------------------------------------------------

async fn run_telegram(cfg: &config::Config, channel_id: Option<&str>) -> anyhow::Result<()> {
    let token = resolve_telegram_token(channel_id)?;
    let rt = setup_runtime(cfg).await?;
    let db: Arc<dyn store::Store> = Arc::new(store::SqliteStore::new()?);

    let bot = telegram::TelegramBot::new(
        &token, rt.provider, cfg.agent.clone(), rt.registry, rt.cost, db, &rt.soul, &rt.model,
    );

    bot.run().await
}

fn resolve_telegram_token(channel_id: Option<&str>) -> anyhow::Result<String> {
    // 1. Environment variable takes priority.
    if let Ok(token) = std::env::var("TELEGRAM_BOT_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    // 2. Look up from channels.json.
    let store = channels::ChannelStore::new();
    let channels = store.list().unwrap_or_default();

    let tg_channels: Vec<_> = channels
        .iter()
        .filter(|c| c.provider == "telegram")
        .collect();

    let channel = match channel_id {
        Some(id) => tg_channels.iter().find(|c| c.id == id).copied(),
        None => tg_channels.first().copied(),
    };

    if let Some(ch) = channel {
        if let Some(ref token) = ch.token {
            if !token.is_empty() {
                return Ok(token.clone());
            }
        }
    }

    anyhow::bail!(
        "No Telegram bot token found.\n\
         Set TELEGRAM_BOT_TOKEN env var or run: marsclaw channels add"
    )
}

// ---------------------------------------------------------------------------
// Discord bot
// ---------------------------------------------------------------------------

async fn run_discord(cfg: &config::Config, channel_id: Option<&str>) -> anyhow::Result<()> {
    let token = resolve_discord_token(channel_id)?;
    let rt = setup_runtime(cfg).await?;
    let db: Arc<dyn store::Store> = Arc::new(store::SqliteStore::new()?);

    let bot = discord::DiscordBot::new(discord::DiscordBotConfig {
        token,
        provider: rt.provider,
        agent_cfg: cfg.agent.clone(),
        registry: rt.registry,
        safety: rt.safety,
        cost: rt.cost,
        store: db,
        soul: rt.soul,
    });

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    bot.run(cancel).await
}

fn resolve_discord_token(channel_id: Option<&str>) -> anyhow::Result<String> {
    if let Ok(token) = std::env::var("DISCORD_BOT_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let ch_store = channels::ChannelStore::new();
    let all_channels = ch_store.list().unwrap_or_default();

    let dc_channels: Vec<_> = all_channels
        .iter()
        .filter(|c| c.provider == "discord")
        .collect();

    let channel = match channel_id {
        Some(id) => dc_channels.iter().find(|c| c.id == id).copied(),
        None => dc_channels.first().copied(),
    };

    if let Some(ch) = channel {
        if let Some(ref token) = ch.token {
            if !token.is_empty() {
                return Ok(token.clone());
            }
        }
    }

    anyhow::bail!(
        "No Discord bot token found.\n\
         Set DISCORD_BOT_TOKEN env var or run: marsclaw channels add"
    )
}

// ---------------------------------------------------------------------------
// Slack bot
// ---------------------------------------------------------------------------

async fn run_slack(cfg: &config::Config, channel_id: Option<&str>) -> anyhow::Result<()> {
    let (bot_token, app_token) = resolve_slack_tokens(channel_id)?;
    let rt = setup_runtime(cfg).await?;
    let db: Arc<dyn store::Store> = Arc::new(store::SqliteStore::new()?);

    let bot = slack::SlackBot::new(slack::SlackBotConfig {
        bot_token,
        app_token,
        provider: rt.provider,
        agent_cfg: cfg.agent.clone(),
        registry: rt.registry,
        safety: rt.safety,
        cost: rt.cost,
        store: db,
        soul: rt.soul,
    });

    let cancel = CancellationToken::new();
    let cancel_clone = cancel.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        cancel_clone.cancel();
    });

    bot.run(cancel).await
}

fn resolve_slack_tokens(channel_id: Option<&str>) -> anyhow::Result<(String, String)> {
    let bot_token_env = std::env::var("SLACK_BOT_TOKEN").unwrap_or_default();
    let app_token_env = std::env::var("SLACK_APP_TOKEN").unwrap_or_default();

    if !bot_token_env.is_empty() && !app_token_env.is_empty() {
        return Ok((bot_token_env, app_token_env));
    }

    let ch_store = channels::ChannelStore::new();
    let all_channels = ch_store.list().unwrap_or_default();

    let sl_channels: Vec<_> = all_channels
        .iter()
        .filter(|c| c.provider == "slack")
        .collect();

    let channel = match channel_id {
        Some(id) => sl_channels.iter().find(|c| c.id == id).copied(),
        None => sl_channels.first().copied(),
    };

    if let Some(ch) = channel {
        let bt = if bot_token_env.is_empty() {
            ch.bot_token.clone().unwrap_or_default()
        } else {
            bot_token_env
        };
        let at = if app_token_env.is_empty() {
            ch.app_token.clone().unwrap_or_default()
        } else {
            app_token_env
        };

        if !bt.is_empty() && !at.is_empty() {
            return Ok((bt, at));
        }
    }

    anyhow::bail!(
        "No Slack tokens found.\n\
         Set SLACK_BOT_TOKEN and SLACK_APP_TOKEN env vars, or run: marsclaw channels add"
    )
}

// ---------------------------------------------------------------------------
// WhatsApp bot (runs as serve sub-mount or standalone)
// ---------------------------------------------------------------------------

async fn run_whatsapp(cfg: &config::Config, _channel_id: Option<&str>) -> anyhow::Result<()> {
    let wa_cfg = cfg
        .whatsapp
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No WhatsApp config found. Add [whatsapp] section to config.yaml"))?;

    let rt = setup_runtime(cfg).await?;
    let db: Arc<dyn store::Store> = Arc::new(store::SqliteStore::new()?);

    let bot = Arc::new(whatsapp::WhatsAppBot::new(
        &wa_cfg.phone_number_id,
        &wa_cfg.access_token,
        &wa_cfg.verify_token,
        rt.provider,
        cfg.agent.clone(),
        rt.registry,
        rt.cost,
        db,
        &rt.soul,
        &rt.model,
    ));

    let app = bot.router();
    let addr = normalize_addr(":8080");
    eprintln!("\x1b[1m\x1b[36mWhatsApp webhook listening at http://localhost{}\x1b[0m", &addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
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
            Ok(Arc::new(llm::anthropic::AnthropicProvider::new(
                &key,
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
        "" => anyhow::bail!(
            "No provider configured. Run `marsclaw init` to set up your LLM provider."
        ),
        other => anyhow::bail!("Unknown provider: {other}. Run `marsclaw init` to configure."),
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
