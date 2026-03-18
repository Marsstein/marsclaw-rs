//! Tool registry and built-in tool implementations.
//!
//! Ported from Go: internal/tool/registry.go

pub mod edit;
pub mod git;
pub mod list;
pub mod read;
pub mod search;
pub mod shell;
pub mod write;

use crate::types::{ToolCall, ToolDef, ToolExecutor};
use std::collections::HashMap;
use std::sync::Arc;

/// Holds available tools and their executors.
#[derive(Clone)]
pub struct Registry {
    defs: Vec<ToolDef>,
    executors: HashMap<String, Arc<dyn ToolExecutor>>,
}

impl Registry {
    /// Create an empty tool registry.
    pub fn new() -> Self {
        Self {
            defs: Vec::new(),
            executors: HashMap::new(),
        }
    }

    /// Register a tool definition with its executor.
    pub fn register(&mut self, def: ToolDef, executor: Arc<dyn ToolExecutor>) {
        self.executors.insert(def.name.clone(), executor);
        self.defs.push(def);
    }

    /// All registered tool definitions.
    pub fn defs(&self) -> &[ToolDef] {
        &self.defs
    }

    /// The executor map keyed by tool name.
    pub fn executors(&self) -> &HashMap<String, Arc<dyn ToolExecutor>> {
        &self.executors
    }

    /// Look up an executor by tool name and run it.
    pub async fn execute(&self, call: &ToolCall) -> anyhow::Result<String> {
        let executor = self
            .executors
            .get(&call.name)
            .ok_or_else(|| anyhow::anyhow!("unknown tool: {}", call.name))?;
        executor.execute(call).await
    }

    /// Create a registry pre-loaded with all built-in tools.
    pub fn default_registry(work_dir: &str) -> Self {
        let mut r = Self::new();
        r.register(
            read::ReadFileTool::def(),
            Arc::new(read::ReadFileTool::new(work_dir)),
        );
        r.register(
            write::WriteFileTool::def(),
            Arc::new(write::WriteFileTool::new(work_dir)),
        );
        r.register(
            edit::EditFileTool::def(),
            Arc::new(edit::EditFileTool::new(work_dir)),
        );
        r.register(
            shell::ShellTool::def(),
            Arc::new(shell::ShellTool::new(work_dir)),
        );
        r.register(
            list::ListFilesTool::def(),
            Arc::new(list::ListFilesTool::new(work_dir)),
        );
        r.register(
            search::SearchTool::def(),
            Arc::new(search::SearchTool::new(work_dir)),
        );
        r.register(
            git::GitTool::def(),
            Arc::new(git::GitTool::new(work_dir)),
        );
        r
    }

    /// Merge additional tool definitions and executors into this registry.
    pub fn merge(&mut self, defs: Vec<ToolDef>, executors: HashMap<String, Arc<dyn ToolExecutor>>) {
        for def in defs {
            if let Some(exec) = executors.get(&def.name) {
                self.executors.entry(def.name.clone()).or_insert_with(|| exec.clone());
                self.defs.push(def);
            }
        }
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}
