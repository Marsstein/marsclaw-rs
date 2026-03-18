//! Interactive setup wizard that generates ~/.marsclaw/config.yaml.
//!
//! Ported from Go: internal/setup/wizard.go

use std::io::{self, BufRead, Write as IoWrite};
use std::path::PathBuf;

use crate::channels::{Channel, ChannelStore};

// ANSI escape codes.
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";

/// Run the 4-step interactive setup wizard.
pub fn run_wizard() -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home dir"))?;
    let dir = home.join(".marsclaw");
    let config_path = dir.join("config.yaml");

    if config_path.exists() {
        println!("Config already exists at {}", config_path.display());
        if !confirm("Overwrite?") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let reader = &mut io::stdin().lock();

    println!("\n  {BOLD}{CYAN}MarsClaw Setup{RESET}\n");

    // Step 1: Provider selection.
    let (provider_name, model, env_key) = step_provider(reader);

    // Step 2: Channel setup.
    step_channels(reader);

    // Step 3: MCP integrations.
    let mcp_configs = step_mcp(reader);

    // Step 4: Budget & Security.
    let (budget, strict) = step_security(reader);

    // Build config YAML.
    let yaml = build_config_yaml(&provider_name, &model, &env_key, &mcp_configs, &budget, strict);

    std::fs::create_dir_all(&dir)?;
    std::fs::write(&config_path, &yaml)?;

    // Summary.
    print_summary(&config_path, &env_key);

    Ok(())
}

// ---------------------------------------------------------------------------
// Step 1: Provider
// ---------------------------------------------------------------------------

fn step_provider(reader: &mut impl BufRead) -> (String, String, String) {
    println!("  {CYAN}Step 1/4 -- LLM Provider{RESET}");
    println!();
    println!("    1) Anthropic (Claude) -- best for coding");
    println!("    2) Google Gemini -- uses GCP credits");
    println!("    3) OpenAI (GPT-4o)");
    println!("    4) Ollama -- local, free, offline");
    println!();
    let choice = prompt(reader, "  Choice [1]: ", "1");

    match choice.as_str() {
        "2" => {
            let model = prompt(reader, "  Model [gemini-2.5-flash]: ", "gemini-2.5-flash");
            ("gemini".into(), model, "GEMINI_API_KEY".into())
        }
        "3" => {
            let model = prompt(reader, "  Model [gpt-4o]: ", "gpt-4o");
            ("openai".into(), model, "OPENAI_API_KEY".into())
        }
        "4" => {
            let model = prompt(reader, "  Model [llama3.1]: ", "llama3.1");
            println!();
            println!("  {YELLOW}Ollama Quick Start:{RESET}");
            println!("    1. Install: curl -fsSL https://ollama.ai/install.sh | sh");
            println!("    2. Pull:    ollama pull {model}");
            println!("    3. Run:     ollama serve  (runs on port 11434)");
            println!();

            if which_exists("ollama") {
                println!("  {GREEN}\u{2713} Ollama found on PATH{RESET}");
            } else {
                println!("  {YELLOW}\u{26a0} Ollama not found. Install it first.{RESET}");
            }
            println!();

            ("ollama".into(), model, String::new())
        }
        _ => {
            let model = prompt(
                reader,
                "  Model [claude-sonnet-4-20250514]: ",
                "claude-sonnet-4-20250514",
            );
            ("anthropic".into(), model, "ANTHROPIC_API_KEY".into())
        }
    }
}

// ---------------------------------------------------------------------------
// Step 2: Channels
// ---------------------------------------------------------------------------

fn step_channels(reader: &mut impl BufRead) {
    println!();
    println!("  {CYAN}Step 2/4 -- Channels{RESET}");
    println!();
    println!("    Connect MarsClaw to messaging platforms.");
    println!("    You can always add more later with: marsclaw channels add");
    println!();
    println!("    1) Telegram (Bot API)");
    println!("    2) Discord (Bot API)");
    println!("    3) Slack (Socket Mode)");
    println!("    4) WhatsApp (Cloud API)");
    println!("    5) Skip for now");
    println!();
    let choice = prompt(reader, "  Choice [5]: ", "5");

    if choice == "5" {
        return;
    }

    let store = ChannelStore::new();

    let ch = match choice.as_str() {
        "1" => {
            println!();
            println!("  {YELLOW}Get token from @BotFather on Telegram{RESET}");
            let token = prompt(reader, "  Enter Telegram bot token: ", "");
            let name = prompt(reader, "  Channel name [default]: ", "default");
            Channel {
                id: format!("telegram-{name}"),
                provider: "telegram".into(),
                name,
                token: Some(token),
                enabled: true,
                ..default_channel()
            }
        }
        "2" => {
            println!();
            println!("  {YELLOW}Get token from discord.com/developers/applications{RESET}");
            let token = prompt(reader, "  Enter Discord bot token: ", "");
            let name = prompt(reader, "  Channel name [default]: ", "default");
            Channel {
                id: format!("discord-{name}"),
                provider: "discord".into(),
                name,
                token: Some(token),
                enabled: true,
                ..default_channel()
            }
        }
        "3" => {
            println!();
            println!("  {YELLOW}Get tokens from api.slack.com/apps{RESET}");
            let bot_token = prompt(reader, "  Enter Slack bot token (xoxb-): ", "");
            let app_token = prompt(reader, "  Enter Slack app token (xapp-): ", "");
            let name = prompt(reader, "  Channel name [default]: ", "default");
            Channel {
                id: format!("slack-{name}"),
                provider: "slack".into(),
                name,
                bot_token: Some(bot_token),
                app_token: if app_token.is_empty() {
                    None
                } else {
                    Some(app_token)
                },
                enabled: true,
                ..default_channel()
            }
        }
        "4" => {
            println!();
            println!("  {YELLOW}Get credentials from developers.facebook.com{RESET}");
            let phone_id = prompt(reader, "  Enter Phone Number ID: ", "");
            let access_token = prompt(reader, "  Enter Access Token: ", "");
            let verify_token = prompt(
                reader,
                "  Enter Verify Token [marsclaw-verify]: ",
                "marsclaw-verify",
            );
            let name = prompt(reader, "  Channel name [default]: ", "default");
            Channel {
                id: format!("whatsapp-{name}"),
                provider: "whatsapp".into(),
                name,
                phone_number_id: Some(phone_id),
                access_token: Some(access_token),
                verify_token: Some(verify_token),
                enabled: true,
                ..default_channel()
            }
        }
        _ => return,
    };

    let has_credential = ch.token.as_ref().is_some_and(|t| !t.is_empty())
        || ch.bot_token.as_ref().is_some_and(|t| !t.is_empty())
        || ch
            .phone_number_id
            .as_ref()
            .is_some_and(|t| !t.is_empty());

    if has_credential {
        match store.add(ch.clone()) {
            Ok(()) => println!("  {GREEN}\u{2713} {} channel saved!{RESET}", ch.provider),
            Err(e) => eprintln!("  \x1b[31mFailed to save channel: {e}\x1b[0m"),
        }
    }
}

// ---------------------------------------------------------------------------
// Step 3: MCP
// ---------------------------------------------------------------------------

fn step_mcp(reader: &mut impl BufRead) -> Vec<String> {
    println!();
    println!("  {CYAN}Step 3/4 -- MCP Integrations{RESET}");
    println!();
    println!("    MCP servers let MarsClaw use external tools (Zapier, databases, etc.)");
    println!();

    let mut mcp_configs: Vec<String> = Vec::new();

    let zapier = prompt(
        reader,
        "  Add Zapier MCP? (connects 8,000+ apps) [y/N]: ",
        "n",
    );
    if yes(&zapier) {
        let url = prompt(reader, "  Zapier MCP server URL (from mcp.zapier.com): ", "");
        if !url.is_empty() {
            mcp_configs.push(format!(
                "  - name: zapier\n    command: npx\n    args: [\"-y\", \"@anthropic-ai/mcp-proxy\", \"{url}\"]"
            ));
        }
    }

    let n8n = prompt(
        reader,
        "  Add n8n MCP? (workflow automation, 400+ integrations) [y/N]: ",
        "n",
    );
    if yes(&n8n) {
        let url = prompt(
            reader,
            "  n8n webhook URL (e.g. http://localhost:5678): ",
            "http://localhost:5678",
        );
        if !url.is_empty() {
            mcp_configs.push(format!(
                "  - name: n8n\n    command: npx\n    args: [\"-y\", \"n8n-mcp-server\", \"--url\", \"{url}\"]"
            ));
        }
    }

    let custom = prompt(reader, "  Add a custom MCP server? [y/N]: ", "n");
    if yes(&custom) {
        let name = prompt(reader, "  MCP server name: ", "custom");
        let cmd = prompt(reader, "  Command (e.g. npx, uvx, node): ", "npx");
        let args_str = prompt(reader, "  Args (comma-separated): ", "");
        let args_quoted: Vec<String> = args_str
            .split(',')
            .map(|a| format!("\"{}\"", a.trim()))
            .collect();
        mcp_configs.push(format!(
            "  - name: {name}\n    command: {cmd}\n    args: [{}]",
            args_quoted.join(", ")
        ));
    }

    mcp_configs
}

// ---------------------------------------------------------------------------
// Step 4: Budget & Security
// ---------------------------------------------------------------------------

fn step_security(reader: &mut impl BufRead) -> (String, bool) {
    println!();
    println!("  {CYAN}Step 4/4 -- Budget & Security{RESET}");
    println!();
    let budget = prompt(reader, "  Daily budget in USD (0 = unlimited) [0]: ", "0");
    let strict = prompt(
        reader,
        "  Require approval for dangerous tools? [y/N]: ",
        "n",
    );
    (budget, yes(&strict))
}

// ---------------------------------------------------------------------------
// Config builder
// ---------------------------------------------------------------------------

fn build_config_yaml(
    provider_name: &str,
    model: &str,
    env_key: &str,
    mcp_configs: &[String],
    budget: &str,
    strict: bool,
) -> String {
    let mut b = String::new();
    b.push_str("# MarsClaw configuration\n");
    b.push_str("# Generated by: marsclaw init\n\n");
    b.push_str("providers:\n");
    b.push_str(&format!("  default: {provider_name}\n"));

    match provider_name {
        "anthropic" => {
            b.push_str("  anthropic:\n");
            b.push_str(&format!("    api_key_env: {env_key}\n"));
            b.push_str(&format!("    default_model: {model}\n"));
        }
        "gemini" => {
            b.push_str("  gemini:\n");
            b.push_str(&format!("    api_key_env: {env_key}\n"));
            b.push_str(&format!("    default_model: {model}\n"));
        }
        "openai" => {
            b.push_str("  openai:\n");
            b.push_str(&format!("    api_key_env: {env_key}\n"));
            b.push_str(&format!("    default_model: {model}\n"));
        }
        "ollama" => {
            b.push_str("  ollama:\n");
            b.push_str(&format!("    default_model: {model}\n"));
        }
        _ => {}
    }

    b.push_str("\nagent:\n");
    b.push_str("  max_turns: 25\n");
    b.push_str("  enable_streaming: true\n");

    b.push_str("\ncost:\n");
    b.push_str("  inline_display: true\n");
    b.push_str(&format!("  daily_budget: {budget}\n"));

    b.push_str("\nsecurity:\n");
    if strict {
        b.push_str("  strict_approval: true\n");
    } else {
        b.push_str("  strict_approval: false\n");
    }
    b.push_str("  scan_credentials: true\n");
    b.push_str("  path_traversal_guard: true\n");

    if !mcp_configs.is_empty() {
        b.push_str("\nmcp:\n");
        for mc in mcp_configs {
            b.push_str(mc);
            b.push('\n');
        }
    }

    b
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

fn print_summary(config_path: &PathBuf, env_key: &str) {
    println!();
    println!("  {BOLD}{GREEN}\u{2713} Setup complete!{RESET}");
    println!("  Config: {}", config_path.display());
    println!();

    if !env_key.is_empty() {
        if std::env::var(env_key).is_ok() {
            println!("  {GREEN}\u{2713}{RESET} {env_key} is set\n");
        } else {
            println!("  Set your API key:");
            println!("    {YELLOW}export {env_key}=\"your-key-here\"{RESET}\n");
        }
    }

    println!("  {BOLD}Next steps:{RESET}");
    println!("    marsclaw              -- start chatting");
    println!("    marsclaw serve        -- start Web UI + API");
    println!("    marsclaw channels add -- connect Telegram, Discord, etc.");
    println!("    marsclaw telegram     -- run Telegram bot");
    println!();
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn prompt(reader: &mut impl BufRead, label: &str, default: &str) -> String {
    print!("{label}");
    io::stdout().flush().ok();

    let mut line = String::new();
    reader.read_line(&mut line).ok();
    let trimmed = line.trim();
    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn confirm(question: &str) -> bool {
    print!("{question} [y/N] ");
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok();
    let trimmed = line.trim().to_lowercase();
    trimmed == "y" || trimmed == "yes"
}

fn yes(s: &str) -> bool {
    let lower = s.trim().to_lowercase();
    lower == "y" || lower == "yes"
}

fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn default_channel() -> Channel {
    Channel {
        id: String::new(),
        provider: String::new(),
        name: String::new(),
        token: None,
        bot_token: None,
        app_token: None,
        phone_number_id: None,
        access_token: None,
        verify_token: None,
        page_id: None,
        enabled: false,
    }
}
