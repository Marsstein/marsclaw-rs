//! List files tool — walks directory tree with glob filtering.
//!
//! Ported from Go: internal/tool/list.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use serde::Deserialize;
use std::fmt::Write;
use std::path::Path;

const MAX_ENTRIES: usize = 500;
const DEFAULT_DEPTH: usize = 3;

/// Directories to always skip when listing.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "__pycache__",
    "target",
    ".git",
];

pub struct ListFilesTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    path: Option<String>,
    pattern: Option<String>,
    max_depth: Option<i32>,
}

impl ListFilesTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "list_files".into(),
            description:
                "List files in a directory, optionally filtered by glob pattern. Shows directory tree up to max_depth."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Directory path. Default: current directory"
                    },
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g. '*.go', '*.ts')"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Max directory depth. Default: 3"
                    }
                },
                "required": []
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
impl ToolExecutor for ListFilesTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;

        let root = match &args.path {
            Some(p) if !p.is_empty() => self.resolve_path(p),
            _ => self.work_dir.clone(),
        };

        let max_depth = match args.max_depth {
            Some(d) if d > 0 => d as usize,
            _ => DEFAULT_DEPTH,
        };

        let meta = tokio::fs::metadata(&root).await?;
        if !meta.is_dir() {
            return Ok(format!("{} is not a directory", root));
        }

        // Walk synchronously on a blocking thread to avoid async recursion complexity.
        let pattern = args.pattern.clone();
        let root_clone = root.clone();
        let results = tokio::task::spawn_blocking(move || {
            walk_dir(&root_clone, &pattern, max_depth)
        })
        .await?;

        if results.is_empty() {
            return Ok("No files found.".into());
        }

        let truncated = results.len() >= MAX_ENTRIES;
        let mut buf = String::new();
        for entry in &results {
            let _ = writeln!(buf, "{}", entry);
        }
        if truncated {
            let _ = write!(buf, "\n[Truncated at {} entries]\n", MAX_ENTRIES);
        }

        Ok(buf)
    }
}

/// Synchronous directory walk. Returns relative paths with directory markers.
fn walk_dir(root: &str, pattern: &Option<String>, max_depth: usize) -> Vec<String> {
    let mut results = Vec::new();
    let root_path = std::path::Path::new(root);
    walk_recursive(root_path, root_path, pattern, max_depth, 0, &mut results);
    results
}

fn walk_recursive(
    current: &Path,
    root: &Path,
    pattern: &Option<String>,
    max_depth: usize,
    depth: usize,
    results: &mut Vec<String>,
) {
    if results.len() >= MAX_ENTRIES {
        return;
    }

    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut sorted: Vec<_> = entries.filter_map(|e| e.ok()).collect();
    sorted.sort_by_key(|e| e.file_name());

    for entry in sorted {
        if results.len() >= MAX_ENTRIES {
            return;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let path = entry.path();
        let is_dir = path.is_dir();

        // Skip hidden directories.
        if is_dir && name_str.starts_with('.') {
            continue;
        }

        // Skip well-known noisy directories.
        if is_dir && SKIP_DIRS.contains(&name_str.as_ref()) {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();

        if is_dir {
            results.push(format!("[dir] {}", rel));
            if depth < max_depth {
                walk_recursive(&path, root, pattern, max_depth, depth + 1, results);
            }
            continue;
        }

        // Apply glob pattern filter.
        if let Some(pat) = pattern {
            let matched = glob::Pattern::new(pat)
                .map(|p| p.matches(&name_str))
                .unwrap_or(false);
            if !matched {
                continue;
            }
        }

        results.push(rel);
    }
}
