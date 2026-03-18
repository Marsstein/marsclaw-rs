<p align="center">
  <img src="assets/banner.svg" alt="MarsClaw — The world's fastest AI agent runtime" width="100%">
</p>

<p align="center">
  <a href="https://github.com/Marsstein/marsclaw-rs/actions"><img src="https://img.shields.io/github/actions/workflow/status/Marsstein/marsclaw-rs/ci.yml?style=flat-square&label=build" alt="Build"></a>
  <a href="https://crates.io/crates/marsclaw"><img src="https://img.shields.io/crates/v/marsclaw?style=flat-square&color=orange" alt="Crates.io"></a>
  <a href="https://github.com/Marsstein/marsclaw-rs/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-blue?style=flat-square" alt="License"></a>
  <a href="https://github.com/Marsstein/marsclaw-rs"><img src="https://img.shields.io/github/stars/Marsstein/marsclaw-rs?style=flat-square" alt="Stars"></a>
  <a href="https://marsclaw.dev"><img src="https://img.shields.io/badge/demo-marsclaw.dev-brightgreen?style=flat-square" alt="Demo"></a>
</p>

<p align="center">
  <a href="#quick-start">Quick Start</a> &middot;
  <a href="#why-rust">Why Rust?</a> &middot;
  <a href="#features">Features</a> &middot;
  <a href="#architecture">Architecture</a> &middot;
  <a href="#configuration">Configuration</a> &middot;
  <a href="#comparison">Comparison</a>
</p>

---

```bash
cargo install marsclaw
marsclaw init    # interactive setup wizard
marsclaw chat    # start chatting
marsclaw serve   # launch web dashboard
```

## Why Rust?

Every other AI agent framework runs on Python or TypeScript. They need 200MB of dependencies, 2 seconds to cold start, and 80MB of RAM just to sit idle.

MarsClaw is different.

| | **MarsClaw** (Rust) | Go agents | Python agents |
|---|:---:|:---:|:---:|
| **Binary size** | **5 MB** | 18 MB | 200+ MB |
| **Memory (idle)** | **~3 MB** RSS | ~15 MB | ~80 MB |
| **Cold start** | **<10ms** | ~50ms | ~2s |
| **Runtime deps** | **0** | 0 | 50+ packages |
| **Type safety** | **Compile-time** | Runtime | Runtime |
| **Single binary** | **Yes** | Yes | No |

One `curl` or `cargo install`. That's it. No Docker, no venv, no node_modules.

## Features

### LLM Providers
Connect to any major LLM provider out of the box:
- **Anthropic** — Claude 4, Sonnet, Haiku (native Messages API)
- **OpenAI** — GPT-4o, GPT-4, o1 (+ any OpenAI-compatible endpoint)
- **Google Gemini** — Gemini 2.5 Flash, Pro
- **Ollama** — Llama 3, Mistral, CodeLlama, any local model
- **Any OpenAI-compatible API** — Groq, Together, DeepSeek, Azure, vLLM, LM Studio

### Agent Core
- **Autonomous agent loop** — tool calling with automatic retry and error recovery
- **Token budgeting** — system 25%, history 65%, output 10% with smart truncation
- **SSE streaming** — real-time token streaming over HTTP and terminal
- **Multi-agent orchestration** — pipeline, parallel, debate, and supervisor patterns
- **Sub-agent delegation** — agents that spawn and coordinate child agents
- **Cost tracking** — per-model pricing, daily/monthly budgets, inline display

### Built-in Tools (7)
| Tool | Description |
|------|-------------|
| `read_file` | Read files with line ranges |
| `write_file` | Create or overwrite files |
| `edit_file` | Surgical find-and-replace edits |
| `shell` | Execute shell commands with timeout |
| `list_files` | Recursive directory listing with glob |
| `search` | Content search across files (regex) |
| `git` | Read-only git operations (log, diff, status) |

### Channel Integrations (5)
Deploy your agent to any messaging platform:
- **Telegram** — long-polling bot with /start, /clear, /help
- **Discord** — Gateway WebSocket with real-time messaging
- **Slack** — Socket Mode with event-driven responses
- **WhatsApp** — Cloud API webhook (mounts on serve)
- **Instagram** — Messenger API integration

### Platform Features
- **Web dashboard** — embedded single-page UI, zero frontend build
- **Skills system** — installable prompt packs (coder, devops, writer, analyst, compliance)
- **Scheduler** — cron-based task automation
- **MCP support** — JSON-RPC 2.0 client for Zapier, n8n, filesystem, custom servers
- **Persistent memory** — episodic, semantic, procedural memory with SQLite
- **Hook system** — lifecycle events (before/after tool calls, LLM calls, errors)
- **Security** — credential scanning, path traversal guards, tool approval workflow
- **SQLite persistence** — conversation history with zero config
- **YAML + env config** — `MARSCLAW_*` env vars override everything

## Quick Start

```bash
# Install from crates.io
cargo install marsclaw

# Or build from source (produces 5MB binary)
git clone https://github.com/Marsstein/marsclaw-rs.git
cd marsclaw-rs && cargo build --release

# Interactive setup — pick your provider, connect channels
marsclaw init

# Chat interactively
marsclaw chat

# Single prompt
marsclaw chat "explain this codebase and suggest improvements"

# Web dashboard
marsclaw serve --addr :8080

# Connect messaging channels
marsclaw channels add      # interactive setup
marsclaw telegram           # run Telegram bot
marsclaw discord            # run Discord bot
marsclaw slack              # run Slack bot

# Manage skills
marsclaw skills list
marsclaw skills use coder
```

## CLI Reference

```
marsclaw [OPTIONS] [COMMAND]

Commands:
  chat              Chat interactively or run single prompt
  serve             Start HTTP server + Web UI
  telegram          Run as Telegram bot
  discord           Run as Discord bot
  slack             Run as Slack bot
  whatsapp          Run WhatsApp webhook bot
  channels add      Connect a messaging channel
  channels list     Show configured channels
  channels remove   Remove a channel
  skills list       Show available skills
  skills install    Install a skill from URL
  skills use        Set the active skill
  init              Interactive setup wizard

Options:
  -c, --config      Config file path
  -m, --model       Override model (e.g., claude-sonnet-4-20250514)
  -v, --verbose     Debug logging
  -h, --help        Print help
  -V, --version     Print version
```

## Architecture

```
src/                        10,500+ lines of Rust
  main.rs                   CLI entry point (clap)
  agent/                    Agent loop + context builder + sub-agent orchestrator
  llm/                      4 providers (Anthropic, OpenAI, Gemini, Ollama) + cost + retry
  tool/                     7 built-in tools (read, write, edit, shell, list, search, git)
  server/                   HTTP server (axum) + embedded Web UI
  store/                    SQLite persistence (rusqlite)
  telegram/                 Telegram bot (long-polling)
  discord/                  Discord bot (Gateway WebSocket)
  slack/                    Slack bot (Socket Mode)
  whatsapp/                 WhatsApp bot (Cloud API webhook)
  channels/                 Channel management CLI
  orchestration/            Multi-agent patterns (pipeline, parallel, debate, supervisor)
  memory/                   Persistent memory system (SQLite)
  hooks/                    Agent lifecycle hooks
  mcp/                      MCP JSON-RPC 2.0 client
  skills/                   Installable prompt packs
  scheduler/                Cron-based task automation
  security/                 Credential scanning + path guards
  config/                   YAML + env var configuration
  terminal/                 Interactive REPL
  setup/                    Setup wizard
  types/                    Shared types and traits
```

## Configuration

Config lives at `~/.marsclaw/config.yaml`:

```yaml
providers:
  default: anthropic
  anthropic:
    api_key_env: ANTHROPIC_API_KEY
    default_model: claude-sonnet-4-20250514
  gemini:
    api_key_env: GEMINI_API_KEY
    default_model: gemini-2.5-flash
  ollama:
    default_model: llama3.1

agent:
  max_turns: 25
  enable_streaming: true
  temperature: 0.0

cost:
  daily_budget: 10.0

security:
  scan_credentials: true
  path_traversal_guard: true

# MCP servers
mcp:
  - name: n8n
    command: npx
    args: ["-y", "@anthropic/mcp-n8n", "--webhook-url", "http://localhost:5678"]

# WhatsApp webhook (mounted on serve)
whatsapp:
  phone_number_id: "123456789"
  access_token: "EAAx..."
  verify_token: "marsclaw_verify"

# Scheduled tasks
scheduler:
  tasks:
    - id: daily-report
      name: "Daily Summary"
      schedule: "0 9 * * *"
      prompt: "Generate a daily summary of recent changes"
      channel: log
```

Environment variables override config: `MARSCLAW_PROVIDER=ollama`, `MARSCLAW_MODEL=llama3.1`, etc.

## Comparison

| Framework | Language | Binary/Install | Cold Start | Memory | Agents |
|-----------|----------|----------------|------------|--------|--------|
| **MarsClaw** | **Rust** | **5 MB binary** | **<10ms** | **3 MB** | **Multi-agent** |
| Claude Code | TypeScript | npm install | ~3s | ~150 MB | Single |
| Aider | Python | pip install | ~2s | ~120 MB | Single |
| Goose | Python | pip install | ~2s | ~100 MB | Multi |
| Cursor Agent | Electron | 300+ MB app | ~5s | ~500 MB | Single |
| OpenHands | Python | Docker | ~10s | ~2 GB | Multi |

## Contributing

```bash
git clone https://github.com/Marsstein/marsclaw-rs.git
cd marsclaw-rs
cargo build             # debug build
cargo test              # run 43 tests
cargo clippy            # lint
cargo build --release   # optimized 5MB binary
```

## License

Apache-2.0
