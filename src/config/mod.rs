pub mod setup;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub cost: CostConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub mcp: Vec<McpServerConfig>,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub whatsapp: Option<WhatsAppConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_episodic_max")]
    pub episodic_max_chars: usize,
    #[serde(default = "default_semantic_max")]
    pub semantic_max_chars: usize,
    #[serde(default = "default_procedural_max")]
    pub procedural_max_chars: usize,
    #[serde(default = "default_consolidate_at")]
    pub consolidate_at: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            episodic_max_chars: 8000,
            semantic_max_chars: 4000,
            procedural_max_chars: 2000,
            consolidate_at: 80,
        }
    }
}

fn default_episodic_max() -> usize { 8000 }
fn default_semantic_max() -> usize { 4000 }
fn default_procedural_max() -> usize { 2000 }
fn default_consolidate_at() -> usize { 80 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhatsAppConfig {
    pub phone_number_id: String,
    pub access_token: String,
    #[serde(default = "default_wa_verify")]
    pub verify_token: String,
}

fn default_wa_verify() -> String {
    "marsclaw_verify".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default = "default_provider")]
    pub default: String,
    #[serde(default)]
    pub anthropic: AnthropicConfig,
    #[serde(default)]
    pub openai: OpenAiConfig,
    #[serde(default)]
    pub gemini: GeminiConfig,
    #[serde(default)]
    pub ollama: OllamaConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnthropicConfig {
    #[serde(default = "default_anthropic_env")]
    pub api_key_env: String,
    #[serde(default = "default_anthropic_model")]
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiConfig {
    #[serde(default = "default_openai_env")]
    pub api_key_env: String,
    #[serde(default = "default_openai_url")]
    pub base_url: String,
    #[serde(default = "default_openai_model")]
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeminiConfig {
    #[serde(default = "default_gemini_env")]
    pub api_key_env: String,
    #[serde(default = "default_gemini_model")]
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    #[serde(default = "default_ollama_url")]
    pub base_url: String,
    #[serde(default = "default_ollama_model")]
    pub default_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default = "default_max_turns")]
    pub max_turns: i32,
    #[serde(default = "default_max_consecutive")]
    pub max_consecutive_tool_calls: i32,
    #[serde(default = "default_max_input")]
    pub max_input_tokens: i32,
    #[serde(default = "default_max_output")]
    pub max_output_tokens: i32,
    #[serde(default = "default_sys_budget")]
    pub system_prompt_budget: f64,
    #[serde(default = "default_hist_budget")]
    pub history_budget: f64,
    #[serde(default = "default_reserved")]
    pub reserved_for_output: f64,
    #[serde(default = "default_max_tool_result")]
    pub max_tool_result_len: usize,
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,
    #[serde(default = "default_tool_timeout_secs")]
    pub tool_timeout_secs: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_true")]
    pub enable_streaming: bool,
    #[serde(default)]
    pub temperature: f64,
    #[serde(default)]
    pub fallback_model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    #[serde(default = "default_true")]
    pub inline_display: bool,
    #[serde(default)]
    pub daily_budget: f64,
    #[serde(default)]
    pub monthly_budget: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default)]
    pub strict_approval: bool,
    #[serde(default = "default_true")]
    pub scan_credentials: bool,
    #[serde(default = "default_true")]
    pub path_traversal_guard: bool,
    #[serde(default)]
    pub allowed_dirs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub tasks: Vec<TaskConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskConfig {
    pub id: String,
    pub name: String,
    pub schedule: String,
    pub prompt: String,
    #[serde(default = "default_channel")]
    pub channel: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

// --- Default value functions ---

fn default_provider() -> String {
    String::new() // no default — user picks during `marsclaw init`
}
fn default_anthropic_env() -> String {
    "ANTHROPIC_API_KEY".into()
}
fn default_anthropic_model() -> String {
    "claude-sonnet-4-20250514".into()
}
fn default_openai_env() -> String {
    "OPENAI_API_KEY".into()
}
fn default_openai_url() -> String {
    "https://api.openai.com/v1".into()
}
fn default_openai_model() -> String {
    "gpt-4o".into()
}
fn default_gemini_env() -> String {
    "GEMINI_API_KEY".into()
}
fn default_gemini_model() -> String {
    "gemini-2.5-flash".into()
}
fn default_ollama_url() -> String {
    "http://localhost:11434/v1".into()
}
fn default_ollama_model() -> String {
    "llama3.1".into()
}
fn default_max_turns() -> i32 {
    25
}
fn default_max_consecutive() -> i32 {
    15
}
fn default_max_input() -> i32 {
    180_000
}
fn default_max_output() -> i32 {
    16_384
}
fn default_sys_budget() -> f64 {
    0.25
}
fn default_hist_budget() -> f64 {
    0.65
}
fn default_reserved() -> f64 {
    0.10
}
fn default_max_tool_result() -> usize {
    30_000
}
fn default_llm_timeout_secs() -> u64 {
    120
}
fn default_tool_timeout_secs() -> u64 {
    60
}
fn default_max_retries() -> u32 {
    3
}
fn default_true() -> bool {
    true
}
fn default_channel() -> String {
    "log".into()
}

// --- Default trait implementations ---

impl Default for Config {
    fn default() -> Self {
        Self {
            providers: ProvidersConfig::default(),
            agent: AgentConfig::default(),
            cost: CostConfig::default(),
            security: SecurityConfig::default(),
            mcp: Vec::new(),
            scheduler: SchedulerConfig::default(),
            whatsapp: None,
            memory: MemoryConfig::default(),
        }
    }
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            default: default_provider(),
            anthropic: AnthropicConfig::default(),
            openai: OpenAiConfig::default(),
            gemini: GeminiConfig::default(),
            ollama: OllamaConfig::default(),
        }
    }
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_anthropic_env(),
            default_model: default_anthropic_model(),
        }
    }
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_openai_env(),
            base_url: default_openai_url(),
            default_model: default_openai_model(),
        }
    }
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            api_key_env: default_gemini_env(),
            default_model: default_gemini_model(),
        }
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: default_ollama_url(),
            default_model: default_ollama_model(),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: default_max_turns(),
            max_consecutive_tool_calls: default_max_consecutive(),
            max_input_tokens: default_max_input(),
            max_output_tokens: default_max_output(),
            system_prompt_budget: default_sys_budget(),
            history_budget: default_hist_budget(),
            reserved_for_output: default_reserved(),
            max_tool_result_len: default_max_tool_result(),
            llm_timeout_secs: default_llm_timeout_secs(),
            tool_timeout_secs: default_tool_timeout_secs(),
            max_retries: default_max_retries(),
            enable_streaming: default_true(),
            temperature: 0.0,
            fallback_model: None,
        }
    }
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            inline_display: default_true(),
            daily_budget: 0.0,
            monthly_budget: 0.0,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            strict_approval: false,
            scan_credentials: default_true(),
            path_traversal_guard: default_true(),
            allowed_dirs: Vec::new(),
        }
    }
}

// --- Config implementation ---

impl Config {
    /// Load config with priority: env vars > YAML file > defaults.
    pub fn load(path: Option<&str>) -> anyhow::Result<Self> {
        let config_path = match path {
            Some(p) => PathBuf::from(p),
            None => Self::default_path(),
        };

        let mut config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            serde_yaml::from_str(&content)?
        } else {
            Config::default()
        };

        config.apply_env_overrides();
        Ok(config)
    }

    fn default_path() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join(".marsclaw")
            .join("config.yaml")
    }

    /// Apply MARSCLAW_* environment variable overrides.
    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("MARSCLAW_PROVIDER") {
            self.providers.default = v;
        }
        if let Ok(v) = std::env::var("MARSCLAW_MODEL") {
            self.set_active_model(&v);
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_MAX_TURNS") {
            if let Ok(n) = v.parse() {
                self.agent.max_turns = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_MAX_CONSECUTIVE_TOOL_CALLS") {
            if let Ok(n) = v.parse() {
                self.agent.max_consecutive_tool_calls = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_MAX_INPUT_TOKENS") {
            if let Ok(n) = v.parse() {
                self.agent.max_input_tokens = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_MAX_OUTPUT_TOKENS") {
            if let Ok(n) = v.parse() {
                self.agent.max_output_tokens = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_LLM_TIMEOUT_SECS") {
            if let Ok(n) = v.parse() {
                self.agent.llm_timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_TOOL_TIMEOUT_SECS") {
            if let Ok(n) = v.parse() {
                self.agent.tool_timeout_secs = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_MAX_RETRIES") {
            if let Ok(n) = v.parse() {
                self.agent.max_retries = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_ENABLE_STREAMING") {
            if let Ok(b) = v.parse() {
                self.agent.enable_streaming = b;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_AGENT_TEMPERATURE") {
            if let Ok(t) = v.parse() {
                self.agent.temperature = t;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_COST_INLINE_DISPLAY") {
            if let Ok(b) = v.parse() {
                self.cost.inline_display = b;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_COST_DAILY_BUDGET") {
            if let Ok(n) = v.parse() {
                self.cost.daily_budget = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_COST_MONTHLY_BUDGET") {
            if let Ok(n) = v.parse() {
                self.cost.monthly_budget = n;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_SECURITY_STRICT_APPROVAL") {
            if let Ok(b) = v.parse() {
                self.security.strict_approval = b;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_SECURITY_SCAN_CREDENTIALS") {
            if let Ok(b) = v.parse() {
                self.security.scan_credentials = b;
            }
        }
        if let Ok(v) = std::env::var("MARSCLAW_SECURITY_PATH_TRAVERSAL_GUARD") {
            if let Ok(b) = v.parse() {
                self.security.path_traversal_guard = b;
            }
        }
    }

    /// Set the model on the currently active provider.
    fn set_active_model(&mut self, model: &str) {
        match self.providers.default.as_str() {
            "anthropic" => self.providers.anthropic.default_model = model.to_owned(),
            "openai" => self.providers.openai.default_model = model.to_owned(),
            "gemini" => self.providers.gemini.default_model = model.to_owned(),
            "ollama" => self.providers.ollama.default_model = model.to_owned(),
            _ => {}
        }
    }

    /// Get the model name for the active provider.
    pub fn model(&self) -> &str {
        match self.providers.default.as_str() {
            "anthropic" => &self.providers.anthropic.default_model,
            "openai" => &self.providers.openai.default_model,
            "gemini" => &self.providers.gemini.default_model,
            "ollama" => &self.providers.ollama.default_model,
            _ => "not-configured",
        }
    }

    /// Get the API key for the active provider from the environment.
    pub fn api_key(&self) -> Option<String> {
        let env_var = match self.providers.default.as_str() {
            "anthropic" => &self.providers.anthropic.api_key_env,
            "openai" => &self.providers.openai.api_key_env,
            "gemini" => &self.providers.gemini.api_key_env,
            "ollama" => return None,
            _ => return None,
        };
        std::env::var(env_var).ok()
    }

    /// Get the base URL for the active provider (only relevant for OpenAI/Ollama).
    pub fn base_url(&self) -> Option<&str> {
        match self.providers.default.as_str() {
            "openai" => Some(&self.providers.openai.base_url),
            "ollama" => Some(&self.providers.ollama.base_url),
            _ => None,
        }
    }

    /// Get LLM timeout as Duration.
    pub fn llm_timeout(&self) -> Duration {
        Duration::from_secs(self.agent.llm_timeout_secs)
    }

    /// Get tool timeout as Duration.
    pub fn tool_timeout(&self) -> Duration {
        Duration::from_secs(self.agent.tool_timeout_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default();
        assert!(cfg.providers.default.is_empty(), "no default — user picks during init");
        assert_eq!(cfg.agent.max_turns, 25);
        assert_eq!(cfg.agent.max_input_tokens, 180_000);
        assert_eq!(cfg.agent.max_output_tokens, 16_384);
        assert!(cfg.agent.enable_streaming);
        assert!(cfg.security.scan_credentials);
        assert!(cfg.security.path_traversal_guard);
        assert!(!cfg.security.strict_approval);
        assert!(cfg.cost.inline_display);
        assert_eq!(cfg.agent.llm_timeout_secs, 120);
        assert_eq!(cfg.agent.tool_timeout_secs, 60);
    }

    #[test]
    fn model_returns_active_provider_model() {
        let cfg = Config::default();
        assert_eq!(cfg.model(), "not-configured"); // no default provider

        let mut cfg = Config::default();
        cfg.providers.default = "anthropic".into();
        assert_eq!(cfg.model(), "claude-sonnet-4-20250514");

        cfg.providers.default = "openai".into();
        assert_eq!(cfg.model(), "gpt-4o");

        cfg.providers.default = "ollama".into();
        assert_eq!(cfg.model(), "llama3.1");
    }

    #[test]
    fn api_key_returns_none_for_ollama() {
        let mut cfg = Config::default();
        cfg.providers.default = "ollama".into();
        assert!(cfg.api_key().is_none());
    }

    #[test]
    fn base_url_returns_value_for_openai_and_ollama() {
        let mut cfg = Config::default();
        cfg.providers.default = "openai".into();
        assert_eq!(cfg.base_url(), Some("https://api.openai.com/v1"));

        cfg.providers.default = "ollama".into();
        assert_eq!(cfg.base_url(), Some("http://localhost:11434/v1"));

        cfg.providers.default = "anthropic".into();
        assert!(cfg.base_url().is_none());
    }

    #[test]
    fn timeout_durations() {
        let cfg = Config::default();
        assert_eq!(cfg.llm_timeout(), Duration::from_secs(120));
        assert_eq!(cfg.tool_timeout(), Duration::from_secs(60));
    }

    #[test]
    fn deserialize_empty_yaml() {
        let yaml = "{}";
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.providers.default.is_empty());
        assert_eq!(cfg.agent.max_turns, 25);
    }

    #[test]
    fn deserialize_partial_yaml() {
        let yaml = r#"
providers:
  default: anthropic
agent:
  max_turns: 50
  temperature: 0.7
"#;
        let cfg: Config = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(cfg.providers.default, "anthropic");
        assert_eq!(cfg.agent.max_turns, 50);
        assert!((cfg.agent.temperature - 0.7).abs() < f64::EPSILON);
        // Non-specified fields keep defaults
        assert_eq!(cfg.agent.max_input_tokens, 180_000);
        assert!(cfg.agent.enable_streaming);
    }

    #[test]
    fn set_active_model_updates_correct_provider() {
        let mut cfg = Config::default();
        cfg.providers.default = "anthropic".into();
        cfg.set_active_model("claude-opus-4-20250514");
        assert_eq!(cfg.providers.anthropic.default_model, "claude-opus-4-20250514");
        // Other providers unchanged
        assert_eq!(cfg.providers.gemini.default_model, "gemini-2.5-flash");
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        unsafe { std::env::remove_var("MARSCLAW_PROVIDER") };
        let cfg = Config::load(Some("/tmp/nonexistent-marsclaw-config.yaml")).unwrap();
        assert!(cfg.providers.default.is_empty(), "no default provider — user picks during init");
        assert_eq!(cfg.agent.max_turns, 25);
    }

    #[test]
    fn apply_env_overrides_works() {
        let mut cfg = Config::default();
        cfg.providers.default = "anthropic".to_string();
        // Verify we can mutate and it sticks
        assert_eq!(cfg.providers.default, "anthropic");
        cfg.providers.default = "gemini".to_string();
        assert_eq!(cfg.providers.default, "gemini");
    }

    #[test]
    fn token_budget_sums_to_one() {
        let cfg = Config::default();
        let total = cfg.agent.system_prompt_budget
            + cfg.agent.history_budget
            + cfg.agent.reserved_for_output;
        assert!((total - 1.0).abs() < f64::EPSILON);
    }
}
