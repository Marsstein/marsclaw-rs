//! Edit file tool — exact string replacement.
//!
//! Ported from Go: internal/tool/edit.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use serde::Deserialize;
use std::path::Path;

pub struct EditFileTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    path: String,
    old_string: String,
    new_string: String,
}

impl EditFileTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "edit_file".into(),
            description:
                "Replace an exact string in a file. old_string must appear exactly once. Provide enough context to be unique."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to edit"
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Exact string to find and replace (must be unique in file)"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Replacement string"
                    }
                },
                "required": ["path", "old_string", "new_string"]
            }),
            danger_level: DangerLevel::Low,
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
impl ToolExecutor for EditFileTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;
        let path = self.resolve_path(&args.path);

        let content = tokio::fs::read_to_string(&path).await?;
        let count = content.matches(&args.old_string).count();

        if count == 0 {
            return Ok(format!("Error: old_string not found in {}", path));
        }
        if count > 1 {
            return Ok(format!(
                "Error: old_string appears {} times in {}. Provide more context to make it unique.",
                count, path
            ));
        }

        let updated = content.replacen(&args.old_string, &args.new_string, 1);
        tokio::fs::write(&path, &updated).await?;

        Ok(format!("Replaced 1 occurrence in {}", path))
    }
}
