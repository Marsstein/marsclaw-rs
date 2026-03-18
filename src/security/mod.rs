//! Safety checker — validates tool calls and redacts credentials.
//!
//! Ported from Go: internal/security/safety.go

use crate::types::{ApprovalFn, DangerLevel, ToolCall, ToolDef};
use regex::Regex;
use std::collections::HashMap;
use std::fmt;
use std::path::Path;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Returned when a safety check fails.
#[derive(Debug, Clone)]
pub struct SafetyError {
    pub code: String,
    pub message: String,
}

impl fmt::Display for SafetyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "safety/{}: {}", self.code, self.message)
    }
}

impl std::error::Error for SafetyError {}

impl SafetyError {
    /// True if this was a human denial.
    pub fn is_denied(&self) -> bool {
        self.code == "human_denied"
    }
}

// ---------------------------------------------------------------------------
// Checker
// ---------------------------------------------------------------------------

pub struct SafetyChecker {
    tool_defs: HashMap<String, ToolDef>,
    approval_fn: Option<ApprovalFn>,
    allowed_dirs: Vec<String>,
    strict_approval: bool,
    scan_creds: bool,
    path_guard: bool,
}

/// Configuration for the safety checker.
pub struct SafetyConfig {
    pub strict_approval: bool,
    pub scan_credentials: bool,
    pub path_traversal_guard: bool,
    pub allowed_dirs: Vec<String>,
}

impl SafetyChecker {
    pub fn new(
        cfg: SafetyConfig,
        tools: &[ToolDef],
        approval_fn: Option<ApprovalFn>,
    ) -> Self {
        let mut tool_defs = HashMap::with_capacity(tools.len());
        for tool in tools {
            tool_defs.insert(tool.name.clone(), tool.clone());
        }

        Self {
            tool_defs,
            approval_fn,
            allowed_dirs: cfg.allowed_dirs,
            strict_approval: cfg.strict_approval,
            scan_creds: cfg.scan_credentials,
            path_guard: cfg.path_traversal_guard,
        }
    }

    /// Validate a tool call before execution.
    pub fn validate(&self, call: &ToolCall) -> Result<(), SafetyError> {
        let def = self.tool_defs.get(&call.name).ok_or_else(|| SafetyError {
            code: "unknown_tool".into(),
            message: format!("tool {:?} is not registered", call.name),
        })?;

        if call.arguments.is_object() {
            let parsed = call
                .arguments
                .as_object()
                .expect("already checked is_object");

            if self.path_guard {
                self.check_path_traversal(parsed)?;
            }
        }

        if let Some(ref approval_fn) = self.approval_fn {
            let needs_approval = def.danger_level == DangerLevel::High
                || (def.danger_level == DangerLevel::Medium && self.strict_approval);

            if needs_approval {
                let reason = format!(
                    "tool {:?} has danger level {:?}",
                    call.name, def.danger_level
                );
                if !approval_fn(call, &reason) {
                    return Err(SafetyError {
                        code: "human_denied".into(),
                        message: format!("user denied execution of {:?}", call.name),
                    });
                }
            }
        }

        Ok(())
    }

    /// Scan content for credential patterns and redact them.
    pub fn scan_credentials(&self, content: &str) -> (String, bool) {
        if !self.scan_creds {
            return (content.to_string(), false);
        }

        let patterns: &[(&str, &str)] = &[
            (
                r"(?i)(aws_secret_access_key|aws_access_key_id)\s*[=:]\s*\S+",
                "[REDACTED_CREDENTIAL]",
            ),
            (
                r#"(?i)(api[_\-]?key|api[_\-]?secret|access[_\-]?token|auth[_\-]?token)\s*[=:]\s*["']?\S{20,}"#,
                "[REDACTED_CREDENTIAL]",
            ),
            (
                r"(?i)-----BEGIN\s+(RSA\s+)?PRIVATE\s+KEY-----",
                "[REDACTED_CREDENTIAL]",
            ),
            (r"ghp_[0-9a-zA-Z]{36}", "[REDACTED_CREDENTIAL]"),
            (r"sk-[0-9a-zA-Z]{40,}", "[REDACTED_CREDENTIAL]"),
            (
                r#"(?i)password\s*[=:]\s*["']?\S{8,}"#,
                "[REDACTED_CREDENTIAL]",
            ),
        ];

        let mut result = content.to_string();
        let mut found = false;

        for (pattern, replacement) in patterns {
            let re = Regex::new(pattern).expect("credential pattern must compile");
            if re.is_match(&result) {
                found = true;
                result = re.replace_all(&result, *replacement).to_string();
            }
        }

        (result, found)
    }

    fn check_path_traversal(
        &self,
        args: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), SafetyError> {
        if self.allowed_dirs.is_empty() {
            return Ok(());
        }

        for (key, val) in args {
            let str_val = match val.as_str() {
                Some(s) => s,
                None => continue,
            };

            if !looks_like_path(str_val) {
                continue;
            }

            let cleaned = Path::new(str_val)
                .canonicalize()
                .unwrap_or_else(|_| Path::new(str_val).to_path_buf());
            let cleaned_str = cleaned.to_string_lossy();

            if cleaned_str.contains("..") {
                return Err(SafetyError {
                    code: "path_traversal".into(),
                    message: format!(
                        "parameter {key:?} contains path traversal: {str_val:?}"
                    ),
                });
            }

            let allowed = self.allowed_dirs.iter().any(|dir| {
                let clean_dir = Path::new(dir)
                    .canonicalize()
                    .unwrap_or_else(|_| Path::new(dir).to_path_buf());
                let clean_dir_str = clean_dir.to_string_lossy();
                cleaned_str == clean_dir_str.as_ref()
                    || cleaned_str.starts_with(&format!(
                        "{}/",
                        clean_dir_str.trim_end_matches('/')
                    ))
            });

            if !allowed {
                return Err(SafetyError {
                    code: "path_traversal".into(),
                    message: format!(
                        "parameter {key:?} path {str_val:?} is outside allowed directories"
                    ),
                });
            }
        }

        Ok(())
    }
}

fn looks_like_path(s: &str) -> bool {
    if s.starts_with('/') || s.starts_with("./") || s.starts_with("../") {
        return true;
    }
    s.contains('/') && !s.contains("://")
}

// ---------------------------------------------------------------------------
// SafetyCheck trait impl (bridges security module into the agent loop)
// ---------------------------------------------------------------------------

impl crate::agent::SafetyCheck for SafetyChecker {
    fn validate(&self, call: &ToolCall) -> Result<(), String> {
        SafetyChecker::validate(self, call).map_err(|e| {
            if e.is_denied() {
                format!("DENIED:{}", e.message)
            } else {
                e.message
            }
        })
    }

    fn scan_credentials(&self, content: &str) -> (String, bool) {
        SafetyChecker::scan_credentials(self, content)
    }
}
