//! Write file tool — creates or overwrites a file.
//!
//! Ported from Go: internal/tool/write.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use serde::Deserialize;
use std::path::Path;

pub struct WriteFileTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    path: String,
    content: String,
}

impl WriteFileTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "write_file".into(),
            description:
                "Write content to a file. Creates parent directories if needed. Overwrites existing files."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to write to"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write"
                    }
                },
                "required": ["path", "content"]
            }),
            danger_level: DangerLevel::Medium,
            read_only: false,
        }
    }

    fn resolve_path(&self, p: &str) -> String {
        let path = Path::new(p);
        if path.is_absolute() {
            return p.to_string();
        }
        Path::new(&self.work_dir).join(p).display().to_string()
    }
}

#[async_trait::async_trait]
impl ToolExecutor for WriteFileTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;
        let path = self.resolve_path(&args.path);

        if let Some(parent) = Path::new(&path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&path, &args.content).await?;

        Ok(format!("Written {} bytes to {}", args.content.len(), path))
    }
}
