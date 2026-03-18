//! Cost tracking in microdollars (1 USD = 1,000,000).

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;

use crate::types::CostRecorder;

/// Per-model pricing in microdollars per million tokens.
struct ModelPricing {
    input_per_m_tokens: i64,
    output_per_m_tokens: i64,
}

/// Thread-safe cost tracker.
pub struct CostTracker {
    pricing: Mutex<HashMap<String, ModelPricing>>,
    session_cost: AtomicI64,
    daily_cost: AtomicI64,
    daily_limit: AtomicI64,
    monthly_limit: AtomicI64,
}

impl CostTracker {
    pub fn new() -> Self {
        let mut pricing = HashMap::new();

        let add = |m: &mut HashMap<String, ModelPricing>,
                   name: &str,
                   inp: i64,
                   out: i64| {
            m.insert(name.to_owned(), ModelPricing {
                input_per_m_tokens: inp,
                output_per_m_tokens: out,
            });
        };

        // Anthropic
        add(&mut pricing, "claude-sonnet-4-20250514", 3_000_000, 15_000_000);
        add(&mut pricing, "claude-opus-4-20250514", 15_000_000, 75_000_000);
        add(&mut pricing, "claude-haiku-4-20250514", 800_000, 4_000_000);

        // OpenAI
        add(&mut pricing, "gpt-4o", 2_500_000, 10_000_000);
        add(&mut pricing, "gpt-4o-mini", 150_000, 600_000);

        // Google Gemini
        add(&mut pricing, "gemini-2.5-flash", 150_000, 600_000);
        add(&mut pricing, "gemini-2.5-pro", 1_250_000, 10_000_000);
        add(&mut pricing, "gemini-2.0-flash", 100_000, 400_000);
        add(&mut pricing, "gemini-1.5-flash", 75_000, 300_000);
        add(&mut pricing, "gemini-1.5-pro", 1_250_000, 5_000_000);

        // Ollama (free, local)
        for name in &[
            "llama3.1",
            "llama3.2",
            "mistral",
            "qwen2.5-coder",
            "deepseek-r1",
            "codellama",
        ] {
            add(&mut pricing, name, 0, 0);
        }

        Self {
            pricing: Mutex::new(pricing),
            session_cost: AtomicI64::new(0),
            daily_cost: AtomicI64::new(0),
            daily_limit: AtomicI64::new(0),
            monthly_limit: AtomicI64::new(0),
        }
    }

    /// Set daily cost limit in dollars.
    pub fn set_daily_limit(&self, dollars: f64) {
        self.daily_limit
            .store((dollars * 1_000_000.0) as i64, Ordering::Relaxed);
    }

    /// Set monthly cost limit in dollars.
    pub fn set_monthly_limit(&self, dollars: f64) {
        self.monthly_limit
            .store((dollars * 1_000_000.0) as i64, Ordering::Relaxed);
    }

    /// Session total in dollars.
    pub fn session_cost(&self) -> f64 {
        self.session_cost.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }

    /// Daily total in dollars.
    pub fn daily_cost(&self) -> f64 {
        self.daily_cost.load(Ordering::Relaxed) as f64 / 1_000_000.0
    }
}

impl Default for CostTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl CostRecorder for CostTracker {
    fn record(&self, model: &str, input_tokens: i32, output_tokens: i32) -> i64 {
        let pricing = self.pricing.lock().unwrap();
        let Some(p) = pricing.get(model) else {
            return 0;
        };

        let input_cost = input_tokens as i64 * p.input_per_m_tokens / 1_000_000;
        let output_cost = output_tokens as i64 * p.output_per_m_tokens / 1_000_000;
        let cost = input_cost + output_cost;

        self.session_cost.fetch_add(cost, Ordering::Relaxed);
        self.daily_cost.fetch_add(cost, Ordering::Relaxed);

        cost
    }

    fn format_cost_line(&self, model: &str, input_tokens: i32, output_tokens: i32) -> String {
        let session = self.session_cost();
        let mut out = String::new();
        let _ = write!(
            out,
            "-- {} | {} in / {} out | ${:.3} session --",
            model,
            format_token_count(input_tokens),
            format_token_count(output_tokens),
            session,
        );
        out
    }

    fn over_budget(&self) -> bool {
        let limit = self.daily_limit.load(Ordering::Relaxed);
        if limit <= 0 {
            return false;
        }
        self.daily_cost.load(Ordering::Relaxed) >= limit
    }
}

fn format_token_count(n: i32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        format!("{n}")
    }
}
