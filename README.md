# MarsClaw

**The fastest AI agent runtime. Written in Rust.**

Single binary. Zero dependencies. 5MB. Deploy anywhere in seconds.

```bash
cargo install marsclaw
marsclaw init    # interactive setup wizard
marsclaw chat    # start chatting
marsclaw serve   # launch web dashboard
```

## Why Rust?

| | MarsClaw (Rust) | Go agents | Python agents |
|---|---|---|---|
| **Binary size** | 5 MB | 18 MB | 200+ MB |
| **Memory (idle)** | ~3 MB RSS | ~15 MB | ~80 MB |
| **Cold start** | <10ms | ~50ms | ~2s |
| **Dependencies** | 0 | 0 | 50+ packages |
| **Type safety** | Compile-time | Runtime | Runtime |

## Features

- **Multi-provider LLM support** — OpenAI, Gemini, Ollama, Anthropic
- **Built-in tools** — file read/write/edit, shell, search, git (read-only)
- **Agent loop** — automatic tool calling with token budgeting and retry
- **Web dashboard** — embedded single-page UI, no separate frontend build
- **SSE streaming** — real-time response streaming over HTTP
- **Channel integrations** — Telegram, Discord, Slack, WhatsApp, Instagram
- **Skills system** — installable prompt packs (coder, devops, writer, analyst, compliance)
- **Scheduler** — cron-based task automation
- **MCP support** — connect Zapier, n8n, filesystem, and custom MCP servers
- **Security** — credential scanning, path traversal guards, tool approval workflow
- **SQLite persistence** — conversation history with zero config
- **Cost tracking** — per-model pricing with daily budget limits

## Quick Start

```bash
# Install
cargo install marsclaw

# Or build from source
git clone https://github.com/marsstein/marsclaw-rs.git
cd marsclaw-rs
cargo build --release
# Binary at target/release/marsclaw (5MB)

# Setup
marsclaw init

# Chat
marsclaw chat

# Or with a single prompt
marsclaw chat "explain this codebase"

# Web UI
marsclaw serve --addr :8080

# Connect Telegram
marsclaw channels add
marsclaw telegram
```

## CLI Reference

```
marsclaw [OPTIONS] [COMMAND]

Commands:
  chat              Chat interactively or run single prompt
  serve             Start HTTP server + Web UI
  channels add      Connect a messaging channel
  channels list     Show configured channels
  channels remove   Remove a channel
  skills list       Show available skills
  skills install    Install a skill from URL
  skills use        Activate a skill
  init              Interactive setup wizard

Options:
  -c, --config      Config file path
  -m, --model       Override model
  -v, --verbose     Debug logging
```

## Architecture

```
src/
  main.rs           CLI entry point (clap)
  agent/            Core agent loop + context builder
  llm/              LLM providers (OpenAI, Gemini, Ollama) + cost tracking
  tool/             Built-in tools (read, write, edit, shell, search, git)
  server/           HTTP server (axum) + embedded Web UI
  store/            SQLite persistence (rusqlite)
  channels/         Channel management (Telegram, Discord, Slack, WhatsApp, Instagram)
  skills/           Installable prompt packs
  scheduler/        Cron-based task automation
  security/         Credential scanning + path traversal guards
  config/           YAML + env var configuration
  terminal/         Interactive REPL
  setup/            Setup wizard
  mcp/              MCP protocol client
  types/            Shared types and traits
```

## Configuration

Config lives at `~/.marsclaw/config.yaml`:

```yaml
providers:
  default: gemini
  gemini:
    api_key_env: GEMINI_API_KEY
    default_model: gemini-2.5-flash

agent:
  max_turns: 25
  enable_streaming: true
  temperature: 0.0

cost:
  daily_budget: 10.0

security:
  scan_credentials: true
  path_traversal_guard: true
```

Environment variables override config: `MARSCLAW_PROVIDER=ollama`, `MARSCLAW_AGENT_MAX_TURNS=50`, etc.

## License

Apache-2.0
