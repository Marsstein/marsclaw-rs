//! Shell command tool — executes commands via `sh -c`.
//!
//! Ported from Go: internal/tool/shell.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use serde::Deserialize;
use std::time::Duration;
use tokio::process::Command;

const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 300;

pub struct ShellTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    command: String,
    timeout: Option<u64>,
}

impl ShellTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "shell".into(),
            description:
                "Execute a shell command and return its output. Use for running build commands, git, tests, etc."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout": {
                        "type": "integer",
                        "description": "Timeout in seconds. Default: 30, max: 300"
                    }
                },
                "required": ["command"]
            }),
            danger_level: DangerLevel::High,
            read_only: false,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for ShellTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;

        let timeout_secs = args
            .timeout
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .max(1)
            .min(MAX_TIMEOUT_SECS);
        let timeout = Duration::from_secs(timeout_secs);

        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&args.command);
        if !self.work_dir.is_empty() {
            cmd.current_dir(&self.work_dir);
        }

        let result = tokio::time::timeout(timeout, cmd.output()).await;

        let output = match result {
            Ok(Ok(output)) => output,
            Ok(Err(e)) => return Err(anyhow::anyhow!("failed to spawn command: {}", e)),
            Err(_) => {
                return Ok(format!(
                    "Command timed out after {}s",
                    timeout_secs
                ));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        let mut buf = String::new();
        if !stdout.is_empty() {
            buf.push_str(&stdout);
        }
        if !stderr.is_empty() {
            if !buf.is_empty() {
                buf.push('\n');
            }
            buf.push_str("STDERR:\n");
            buf.push_str(&stderr);
        }

        if !output.status.success() {
            let code = output.status.code().unwrap_or(-1);
            return Ok(format!("Exit code: {}\n{}", code, buf));
        }

        Ok(buf)
    }
}
