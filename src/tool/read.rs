//! Read file tool — returns numbered lines like `cat -n`.
//!
//! Ported from Go: internal/tool/read.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use serde::Deserialize;
use std::fmt::Write;
use std::path::Path;

const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB
const DEFAULT_LIMIT: i32 = 2000;

pub struct ReadFileTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    path: String,
    offset: Option<i32>,
    limit: Option<i32>,
}

impl ReadFileTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "read_file".into(),
            description: "Read a file from the filesystem. Returns numbered lines. Use offset/limit for large files.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative file path"
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Line offset to start from (0-based). Default: 0"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max lines to return. Default: 2000"
                    }
                },
                "required": ["path"]
            }),
            danger_level: DangerLevel::None,
            read_only: true,
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
impl ToolExecutor for ReadFileTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;
        let path = self.resolve_path(&args.path);

        let meta = tokio::fs::metadata(&path).await?;
        if meta.len() > MAX_FILE_SIZE {
            return Ok(format!(
                "Error: file {} is {} bytes (max {}). Use offset/limit for large files.",
                path,
                meta.len(),
                MAX_FILE_SIZE
            ));
        }

        let data = tokio::fs::read_to_string(&path).await?;
        let lines: Vec<&str> = data.split('\n').collect();

        let offset = args.offset.unwrap_or(0).max(0) as usize;
        if offset >= lines.len() {
            return Ok(format!(
                "File has {} lines, offset {} is beyond end.",
                lines.len(),
                offset
            ));
        }

        let limit = match args.limit {
            Some(l) if l > 0 => l as usize,
            _ => DEFAULT_LIMIT as usize,
        };

        let end = (offset + limit).min(lines.len());

        let mut buf = String::new();
        for i in offset..end {
            let _ = writeln!(buf, "{:4}\u{2502} {}", i + 1, lines[i]);
        }

        if end < lines.len() {
            let _ = write!(buf, "\n[... {} more lines ...]\n", lines.len() - end);
        }

        Ok(buf)
    }
}
