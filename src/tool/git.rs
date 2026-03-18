//! Git read-only tool — safe git operations that cannot modify the repo.
//!
//! Ported from Go: internal/tool/git.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use serde::Deserialize;
use tokio::process::Command;

/// Git subcommands allowed through this tool (read-only only).
const ALLOWED_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "blame",
    "show",
    "branch",
    "tag",
    "shortlog",
    "rev-parse",
    "stash",
];

pub struct GitTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    subcommand: String,
    #[serde(default)]
    args: String,
}

impl GitTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "git".into(),
            description:
                "Run read-only git commands: status, log, diff, blame, show, branch, tag, shortlog. Safe -- cannot modify the repo."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "subcommand": {
                        "type": "string",
                        "enum": [
                            "status", "log", "diff", "blame", "show",
                            "branch", "tag", "shortlog", "rev-parse", "stash list"
                        ],
                        "description": "Git subcommand to run"
                    },
                    "args": {
                        "type": "string",
                        "description": "Additional arguments (e.g., '--oneline -10' for log, 'HEAD~3..HEAD' for diff)"
                    }
                },
                "required": ["subcommand"]
            }),
            danger_level: DangerLevel::None,
            read_only: true,
        }
    }
}

#[async_trait::async_trait]
impl ToolExecutor for GitTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;

        let sub = args.subcommand.trim().to_string();

        // Handle "stash list" as a special compound subcommand.
        let is_stash_list = sub == "stash list" || sub == "stash";
        if !ALLOWED_SUBCOMMANDS.contains(&sub.as_str()) && !is_stash_list {
            anyhow::bail!(
                "git subcommand {:?} not allowed (read-only: status, log, diff, blame, show, branch, tag, shortlog, rev-parse, stash list)",
                sub
            );
        }

        let mut cmd_args: Vec<String> = if sub == "stash list" {
            vec!["stash".into(), "list".into()]
        } else if sub == "stash" {
            // Only allow "stash list" variant.
            vec!["stash".into(), "list".into()]
        } else {
            vec![sub]
        };

        if !args.args.is_empty() {
            cmd_args.extend(args.args.split_whitespace().map(String::from));
        }

        let mut cmd = Command::new("git");
        cmd.args(&cmd_args);
        if !self.work_dir.is_empty() {
            cmd.current_dir(&self.work_dir);
        }

        let output = cmd.output().await?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            if !stderr.is_empty() {
                return Ok(format!("git error: {}", stderr));
            }
            return Ok(format!("git error: exit code {}", output.status.code().unwrap_or(-1)));
        }

        Ok(stdout.into_owned())
    }
}
