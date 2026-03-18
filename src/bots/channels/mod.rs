//! Channel management — configure messaging provider connections.
//!
//! Ported from Go: internal/channels/channels.go + cli.go

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, BufRead, Write as IoWrite};
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

// ANSI escape codes.
const BOLD: &str = "\x1b[1m";
const CYAN: &str = "\x1b[36m";
const YELLOW: &str = "\x1b[33m";
const GREEN: &str = "\x1b[32m";
const DIM: &str = "\x1b[90m";
const RESET: &str = "\x1b[0m";

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub provider: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phone_number_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub page_id: Option<String>,
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub method: String,
    pub token_label: String,
    pub token_env: String,
}

/// All supported channel providers.
pub fn supported_providers() -> Vec<ProviderInfo> {
    vec![
        ProviderInfo {
            id: "telegram".into(),
            name: "Telegram".into(),
            method: "Bot API".into(),
            token_label: "Bot token (from @BotFather)".into(),
            token_env: "TELEGRAM_BOT_TOKEN".into(),
        },
        ProviderInfo {
            id: "discord".into(),
            name: "Discord".into(),
            method: "Bot API".into(),
            token_label: "Bot token".into(),
            token_env: "DISCORD_BOT_TOKEN".into(),
        },
        ProviderInfo {
            id: "slack".into(),
            name: "Slack".into(),
            method: "Socket Mode".into(),
            token_label: "Bot token (xoxb-)".into(),
            token_env: "SLACK_BOT_TOKEN".into(),
        },
        ProviderInfo {
            id: "whatsapp".into(),
            name: "WhatsApp".into(),
            method: "Cloud API".into(),
            token_label: "Access token".into(),
            token_env: String::new(),
        },
        ProviderInfo {
            id: "instagram".into(),
            name: "Instagram".into(),
            method: "Messenger API".into(),
            token_label: "Page access token".into(),
            token_env: "INSTAGRAM_ACCESS_TOKEN".into(),
        },
    ]
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

pub struct ChannelStore {
    path: PathBuf,
}

impl Default for ChannelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ChannelStore {
    pub fn new() -> Self {
        let path = dirs::home_dir()
            .unwrap_or_default()
            .join(".marsclaw")
            .join("channels.json");
        Self { path }
    }

    pub fn list(&self) -> anyhow::Result<Vec<Channel>> {
        let data = match fs::read_to_string(&self.path) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let channels: Vec<Channel> = serde_json::from_str(&data)?;
        Ok(channels)
    }

    pub fn add(&self, channel: Channel) -> anyhow::Result<()> {
        let mut channels = self.list()?;

        if let Some(existing) = channels.iter_mut().find(|c| c.id == channel.id) {
            *existing = channel;
        } else {
            channels.push(channel);
        }

        self.save(&channels)
    }

    pub fn remove(&self, id: &str) -> anyhow::Result<()> {
        let channels: Vec<Channel> = self.list()?.into_iter().filter(|c| c.id != id).collect();
        self.save(&channels)
    }

    pub fn get(&self, id: &str) -> anyhow::Result<Channel> {
        self.list()?
            .into_iter()
            .find(|c| c.id == id)
            .ok_or_else(|| anyhow::anyhow!("channel {id:?} not found"))
    }

    fn save(&self, channels: &[Channel]) -> anyhow::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let data = serde_json::to_string_pretty(channels)?;

        let file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&self.path)?;
        let mut writer = io::BufWriter::new(file);
        writer.write_all(data.as_bytes())?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CLI helpers
// ---------------------------------------------------------------------------

fn prompt(label: &str, default: &str) -> String {
    print!("{label}");
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line).ok();
    let trimmed = line.trim();

    if trimmed.is_empty() {
        default.to_string()
    } else {
        trimmed.to_string()
    }
}

fn confirm(question: &str) -> bool {
    let answer = prompt(&format!("{question} [Y/n] "), "");
    let lower = answer.to_lowercase();
    lower.is_empty() || lower == "y" || lower == "yes"
}

fn mask_token(token: &str) -> String {
    if token.len() < 8 {
        return "****".to_string();
    }
    format!("{}...{}", &token[..4], &token[token.len() - 4..])
}

// ---------------------------------------------------------------------------
// CLI commands
// ---------------------------------------------------------------------------

/// Interactive channel setup wizard.
pub fn run_add(store: &ChannelStore) -> anyhow::Result<()> {
    let providers = supported_providers();

    println!("\n  {BOLD}MarsClaw \u{2014} Add Channel{RESET}\n");

    println!("  {CYAN}Select a channel:{RESET}");
    for (i, p) in providers.iter().enumerate() {
        println!("    {}) {} ({})", i + 1, p.name, p.method);
    }
    println!();

    let choice = prompt("  Choice [1]: ", "1");
    let idx = choice
        .bytes()
        .next()
        .and_then(|b| b.checked_sub(b'1'))
        .map(|n| n as usize)
        .filter(|&n| n < providers.len())
        .unwrap_or(0);
    let provider = &providers[idx];

    println!("\n  {CYAN}{} Setup{RESET}", provider.name);

    let mut ch = Channel {
        id: String::new(),
        provider: provider.id.clone(),
        name: String::new(),
        token: None,
        bot_token: None,
        app_token: None,
        phone_number_id: None,
        access_token: None,
        verify_token: None,
        page_id: None,
        enabled: true,
    };

    match provider.id.as_str() {
        "telegram" => {
            println!("  \u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}");
            println!("  \u{2502} 1) Open Telegram \u{2192} chat with @BotFather \u{2502}");
            println!("  \u{2502} 2) Send /newbot (or /mybots)            \u{2502}");
            println!("  \u{2502} 3) Copy the token (123456:ABC...)       \u{2502}");
            println!("  \u{2502}                                         \u{2502}");
            println!("  \u{2502} Tip: set TELEGRAM_BOT_TOKEN in env      \u{2502}");
            println!("  \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}");
            println!();

            if let Ok(env_token) = std::env::var("TELEGRAM_BOT_TOKEN")
                && !env_token.is_empty()
            {
                println!("  Found TELEGRAM_BOT_TOKEN in environment.");
                if confirm("  Use it?") {
                    ch.token = Some(env_token);
                }
            }
            if ch.token.is_none() {
                let t = prompt(&format!("  {YELLOW}Enter Telegram bot token:{RESET} "), "");
                if t.is_empty() {
                    println!("  Aborted.");
                    return Ok(());
                }
                ch.token = Some(t);
            }

            let name = prompt("  Channel name [default]: ", "default");
            ch.name = name.clone();
            ch.id = format!("telegram-{name}");
        }
        "discord" => {
            println!("  \u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}");
            println!("  \u{2502} 1) Go to discord.com/developers/applications    \u{2502}");
            println!("  \u{2502} 2) Create app \u{2192} Bot \u{2192} Copy token                \u{2502}");
            println!("  \u{2502} 3) Enable MESSAGE CONTENT intent                \u{2502}");
            println!("  \u{2502} 4) Invite bot to your server with messages perm \u{2502}");
            println!("  \u{2502}                                                  \u{2502}");
            println!("  \u{2502} Tip: set DISCORD_BOT_TOKEN in env               \u{2502}");
            println!("  \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}");
            println!();

            if let Ok(env_token) = std::env::var("DISCORD_BOT_TOKEN")
                && !env_token.is_empty()
            {
                println!("  Found DISCORD_BOT_TOKEN in environment.");
                if confirm("  Use it?") {
                    ch.token = Some(env_token);
                }
            }
            if ch.token.is_none() {
                let t = prompt(&format!("  {YELLOW}Enter Discord bot token:{RESET} "), "");
                if t.is_empty() {
                    println!("  Aborted.");
                    return Ok(());
                }
                ch.token = Some(t);
            }

            let name = prompt("  Channel name [default]: ", "default");
            ch.name = name.clone();
            ch.id = format!("discord-{name}");
        }
        "slack" => {
            println!("  \u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}");
            println!("  \u{2502} 1) Go to api.slack.com/apps \u{2192} Create New App \u{2502}");
            println!("  \u{2502} 2) Enable Socket Mode \u{2192} get App Token (xapp-)\u{2502}");
            println!("  \u{2502} 3) OAuth \u{2192} Install \u{2192} get Bot Token (xoxb-)   \u{2502}");
            println!("  \u{2502} 4) Add scopes: chat:write, app_mentions:read \u{2502}");
            println!("  \u{2502}                                               \u{2502}");
            println!("  \u{2502} Tip: set SLACK_BOT_TOKEN in env              \u{2502}");
            println!("  \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}");
            println!();

            if let Ok(env_token) = std::env::var("SLACK_BOT_TOKEN")
                && !env_token.is_empty()
            {
                println!("  Found SLACK_BOT_TOKEN in environment.");
                if confirm("  Use it?") {
                    ch.bot_token = Some(env_token);
                }
            }
            if ch.bot_token.is_none() {
                let t = prompt(
                    &format!("  {YELLOW}Enter Slack bot token (xoxb-):{RESET} "),
                    "",
                );
                if t.is_empty() {
                    println!("  Aborted.");
                    return Ok(());
                }
                ch.bot_token = Some(t);
            }

            let app_token = std::env::var("SLACK_APP_TOKEN").unwrap_or_default();
            if !app_token.is_empty() {
                ch.app_token = Some(app_token);
            } else {
                let t = prompt("  Enter Slack app token (xapp-, optional): ", "");
                if !t.is_empty() {
                    ch.app_token = Some(t);
                }
            }

            let name = prompt("  Channel name [default]: ", "default");
            ch.name = name.clone();
            ch.id = format!("slack-{name}");
        }
        "whatsapp" => {
            println!("  \u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}");
            println!("  \u{2502} 1) Go to developers.facebook.com \u{2192} Create App     \u{2502}");
            println!("  \u{2502} 2) Add WhatsApp product \u{2192} get Phone Number ID     \u{2502}");
            println!("  \u{2502} 3) Generate permanent access token                \u{2502}");
            println!("  \u{2502} 4) Set webhook URL to: https://your-domain/webhook\u{2502}");
            println!("  \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}");
            println!();

            let phone = prompt(&format!("  {YELLOW}Enter Phone Number ID:{RESET} "), "");
            if phone.is_empty() {
                println!("  Aborted.");
                return Ok(());
            }
            ch.phone_number_id = Some(phone);
            ch.access_token = Some(prompt(
                &format!("  {YELLOW}Enter Access Token:{RESET} "),
                "",
            ));
            ch.verify_token = Some(prompt(
                "  Enter Verify Token (for webhook): ",
                "marsclaw-verify",
            ));

            let name = prompt("  Channel name [default]: ", "default");
            ch.name = name.clone();
            ch.id = format!("whatsapp-{name}");
        }
        "instagram" => {
            println!("  \u{250c}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2510}");
            println!("  \u{2502} 1) Go to developers.facebook.com \u{2192} Create App      \u{2502}");
            println!("  \u{2502} 2) Add Instagram product (Messenger API)           \u{2502}");
            println!("  \u{2502} 3) Connect Instagram Professional account          \u{2502}");
            println!("  \u{2502} 4) Generate Page Access Token (long-lived)         \u{2502}");
            println!("  \u{2502} 5) Subscribe to messages webhook                   \u{2502}");
            println!("  \u{2502} 6) Webhook URL: https://your-domain/webhook/ig     \u{2502}");
            println!("  \u{2502}                                                     \u{2502}");
            println!("  \u{2502} Requires: Instagram Professional/Business account  \u{2502}");
            println!("  \u{2502} Tip: set INSTAGRAM_ACCESS_TOKEN in env             \u{2502}");
            println!("  \u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2518}");
            println!();

            if let Ok(env_token) = std::env::var("INSTAGRAM_ACCESS_TOKEN")
                && !env_token.is_empty()
            {
                println!("  Found INSTAGRAM_ACCESS_TOKEN in environment.");
                if confirm("  Use it?") {
                    ch.access_token = Some(env_token);
                }
            }
            if ch.access_token.is_none() {
                let t = prompt(
                    &format!("  {YELLOW}Enter Page Access Token:{RESET} "),
                    "",
                );
                if t.is_empty() {
                    println!("  Aborted.");
                    return Ok(());
                }
                ch.access_token = Some(t);
            }

            ch.page_id = Some(prompt(
                &format!("  {YELLOW}Enter Instagram Page ID:{RESET} "),
                "",
            ));
            ch.verify_token = Some(prompt(
                "  Enter Verify Token (for webhook): ",
                "marsclaw-verify",
            ));

            let name = prompt("  Channel name [default]: ", "default");
            ch.name = name.clone();
            ch.id = format!("instagram-{name}");
        }
        _ => {}
    }

    store.add(ch.clone())?;

    println!(
        "\n  {GREEN}\u{2713} {} channel {:?} saved!{RESET}",
        provider.name, ch.name
    );
    println!("  Config: ~/.marsclaw/channels.json\n");

    match provider.id.as_str() {
        "telegram" => {
            let tok = ch.token.as_deref().unwrap_or("");
            println!("  Run:  marsclaw telegram");
            println!(
                "  Or:   TELEGRAM_BOT_TOKEN={} marsclaw telegram\n",
                mask_token(tok)
            );
        }
        "discord" => {
            let tok = ch.token.as_deref().unwrap_or("");
            println!("  Run:  marsclaw discord");
            println!(
                "  Or:   DISCORD_BOT_TOKEN={} marsclaw discord\n",
                mask_token(tok)
            );
        }
        "slack" => {
            println!("  Run:  marsclaw slack\n");
        }
        "whatsapp" => {
            println!("  Mount webhook:  marsclaw serve");
            println!("  Webhook URL:    https://your-domain/webhook/whatsapp\n");
        }
        "instagram" => {
            println!("  Mount webhook:  marsclaw serve");
            println!("  Webhook URL:    https://your-domain/webhook/ig\n");
        }
        _ => {}
    }

    Ok(())
}

/// List all configured channels.
pub fn run_list(store: &ChannelStore) -> anyhow::Result<()> {
    let channels = store.list()?;

    if channels.is_empty() {
        println!("\n  No channels configured.");
        println!("  Run: marsclaw channels add\n");
        return Ok(());
    }

    println!("\n  {BOLD}Configured Channels{RESET}\n");

    for ch in &channels {
        let status = if ch.enabled {
            format!("{GREEN}\u{25cf}{RESET}")
        } else {
            format!("{DIM}\u{25cb}{RESET}")
        };

        let token_display = match ch.provider.as_str() {
            "telegram" | "discord" => mask_token(ch.token.as_deref().unwrap_or("")),
            "slack" => mask_token(ch.bot_token.as_deref().unwrap_or("")),
            "whatsapp" => ch.phone_number_id.clone().unwrap_or_default(),
            "instagram" => mask_token(ch.access_token.as_deref().unwrap_or("")),
            _ => String::new(),
        };

        println!(
            "  {}  {:<12} {:<15} {}",
            status, ch.provider, ch.name, token_display
        );
    }
    println!();

    Ok(())
}

/// Remove a channel by ID (interactive selection if id is None).
pub fn run_remove(store: &ChannelStore, id: Option<&str>) -> anyhow::Result<()> {
    let resolved_id = match id {
        Some(i) => i.to_string(),
        None => {
            let channels = store.list()?;
            if channels.is_empty() {
                println!("No channels to remove.");
                return Ok(());
            }

            println!("\n  Select channel to remove:");
            for (i, ch) in channels.iter().enumerate() {
                println!("    {}) {} \u{2014} {}", i + 1, ch.provider, ch.name);
            }
            let choice = prompt("\n  Choice: ", "");
            let idx = choice
                .bytes()
                .next()
                .and_then(|b| b.checked_sub(b'1'))
                .map(|n| n as usize)
                .unwrap_or(0);
            if idx >= channels.len() {
                println!("  Invalid choice.");
                return Ok(());
            }
            channels[idx].id.clone()
        }
    };

    store.remove(&resolved_id)?;
    println!("  {GREEN}\u{2713} Channel {resolved_id:?} removed.{RESET}\n");
    Ok(())
}
