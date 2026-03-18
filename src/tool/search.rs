//! Search/grep tool — regex search through files.
//!
//! Ported from Go: internal/tool/search.go

use crate::types::{DangerLevel, ToolCall, ToolDef, ToolExecutor};
use regex::Regex;
use serde::Deserialize;
use std::fmt::Write;
use std::io::BufRead;
use std::path::Path;

const DEFAULT_MAX_RESULTS: usize = 50;

/// Directories to skip during search.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "vendor",
    "__pycache__",
    "target",
    ".git",
];

/// File extensions considered binary (skipped during search).
const BINARY_EXTS: &[&str] = &[
    ".png", ".jpg", ".jpeg", ".gif", ".ico", ".svg", ".pdf", ".zip", ".tar", ".gz", ".bz2",
    ".xz", ".exe", ".dll", ".so", ".dylib", ".o", ".a", ".wasm", ".bin", ".dat", ".db",
    ".sqlite",
];

pub struct SearchTool {
    work_dir: String,
}

#[derive(Deserialize)]
struct Args {
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    max_results: Option<i32>,
}

struct SearchMatch {
    file: String,
    line: usize,
    text: String,
}

impl SearchTool {
    pub fn new(work_dir: &str) -> Self {
        Self {
            work_dir: work_dir.to_string(),
        }
    }

    pub fn def() -> ToolDef {
        ToolDef {
            name: "search".into(),
            description:
                "Search file contents using regex. Walks the directory tree, skipping hidden/binary/vendor dirs."
                    .into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern to search for"
                    },
                    "path": {
                        "type": "string",
                        "description": "Root directory to search. Default: current directory"
                    },
                    "glob": {
                        "type": "string",
                        "description": "Filename glob filter (e.g. '*.go')"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Max matches to return. Default: 50"
                    }
                },
                "required": ["pattern"]
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
impl ToolExecutor for SearchTool {
    async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let args: Args = serde_json::from_value(call.arguments.clone())?;

        let re = Regex::new(&args.pattern)?;

        let root = match &args.path {
            Some(p) if !p.is_empty() => self.resolve_path(p),
            _ => self.work_dir.clone(),
        };

        let max_results = match args.max_results {
            Some(m) if m > 0 => m as usize,
            _ => DEFAULT_MAX_RESULTS,
        };

        let glob_pattern = args.glob.clone();
        let root_clone = root.clone();

        let matches = tokio::task::spawn_blocking(move || {
            search_walk(&root_clone, &re, &glob_pattern, max_results)
        })
        .await?;

        if matches.is_empty() {
            return Ok(format!("No matches for {:?}", args.pattern));
        }

        let truncated = matches.len() >= max_results;
        let mut buf = String::new();
        for m in &matches {
            let text = if m.text.len() > 200 {
                format!("{}...", &m.text[..200])
            } else {
                m.text.clone()
            };
            let _ = writeln!(buf, "{}:{}: {}", m.file, m.line, text.trim());
        }
        if truncated {
            let _ = write!(buf, "\n[Truncated at {} results]\n", max_results);
        }

        Ok(buf)
    }
}

/// Walk the directory tree and collect regex matches.
fn search_walk(
    root: &str,
    re: &Regex,
    glob_pattern: &Option<String>,
    max_results: usize,
) -> Vec<SearchMatch> {
    let mut matches = Vec::new();
    let root_path = Path::new(root);
    search_recursive(root_path, root_path, re, glob_pattern, max_results, &mut matches);
    matches
}

fn search_recursive(
    current: &Path,
    root: &Path,
    re: &Regex,
    glob_pattern: &Option<String>,
    max_results: usize,
    matches: &mut Vec<SearchMatch>,
) {
    if matches.len() >= max_results {
        return;
    }

    let entries = match std::fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if matches.len() >= max_results {
            return;
        }

        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if path.is_dir() {
            if name_str.starts_with('.') {
                continue;
            }
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            search_recursive(&path, root, re, glob_pattern, max_results, matches);
            continue;
        }

        // Skip binary files.
        if is_binary_ext(&name_str) {
            continue;
        }

        // Apply glob filter.
        if let Some(pat) = glob_pattern {
            let matched = glob::Pattern::new(pat)
                .map(|p| p.matches(&name_str))
                .unwrap_or(false);
            if !matched {
                continue;
            }
        }

        let remaining = max_results - matches.len();
        let found = scan_file(&path, root, re, remaining);
        matches.extend(found);
    }
}

/// Scan a single file for regex matches.
fn scan_file(path: &Path, root: &Path, re: &Regex, limit: usize) -> Vec<SearchMatch> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };

    let rel = path
        .strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string();

    let reader = std::io::BufReader::new(file);
    let mut results = Vec::new();

    for (line_num, line_result) in reader.lines().enumerate() {
        let line = match line_result {
            Ok(l) => l,
            Err(_) => break, // likely binary content
        };
        if re.is_match(&line) {
            results.push(SearchMatch {
                file: rel.clone(),
                line: line_num + 1,
                text: line,
            });
            if results.len() >= limit {
                break;
            }
        }
    }

    results
}

/// Check if a filename has a binary extension.
fn is_binary_ext(name: &str) -> bool {
    let lower = name.to_lowercase();
    BINARY_EXTS.iter().any(|ext| lower.ends_with(ext))
}
