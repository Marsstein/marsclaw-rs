//! Scheduler: recurring task execution.
//!
//! Ported from Go: internal/scheduler/scheduler.go

use std::sync::Arc;

use chrono::{Datelike, Timelike, Utc};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::agent::Agent;
use crate::config::AgentConfig;
use crate::tool::Registry;
use crate::types::*;

// ---------------------------------------------------------------------------
// Task
// ---------------------------------------------------------------------------

/// A recurring scheduled job.
#[derive(Debug, Clone)]
pub struct Task {
    pub id: String,
    pub name: String,
    /// Cron-like: "0 9 * * 1-5" or interval: "every 30m".
    pub schedule: String,
    /// What the agent should do.
    pub prompt: String,
    /// Where to send results: "telegram:chatid", "log", etc.
    pub channel: String,
    pub enabled: bool,
}

// ---------------------------------------------------------------------------
// Sender
// ---------------------------------------------------------------------------

/// Delivers a task result to a channel.
pub type Sender = Arc<dyn Fn(&str, &str) + Send + Sync>;

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Runs recurring tasks on a polling loop.
pub struct Scheduler {
    tasks: Vec<Task>,
    provider: Arc<dyn Provider>,
    config: AgentConfig,
    registry: Registry,
    soul: String,
    sender: Sender,
}

impl Scheduler {
    pub fn new(
        tasks: Vec<Task>,
        provider: Arc<dyn Provider>,
        config: AgentConfig,
        registry: Registry,
        soul: String,
        sender: Sender,
    ) -> Self {
        Self {
            tasks,
            provider,
            config,
            registry,
            soul,
            sender,
        }
    }

    /// Run the scheduler loop. Blocks until the cancellation token fires.
    pub async fn run(&self, cancel: CancellationToken) {
        info!(tasks = self.tasks.len(), "scheduler started");

        // Check immediately on startup.
        self.check_tasks(&cancel).await;

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    info!("scheduler stopped");
                    return;
                }
                _ = interval.tick() => {
                    self.check_tasks(&cancel).await;
                }
            }
        }
    }

    async fn check_tasks(&self, cancel: &CancellationToken) {
        let now = Utc::now();
        for task in &self.tasks {
            if !task.enabled {
                continue;
            }
            if !should_run(&task.schedule, &now) {
                continue;
            }
            self.execute_task(task, cancel).await;
        }
    }

    async fn execute_task(&self, task: &Task, cancel: &CancellationToken) {
        info!(task = %task.name, "running scheduled task");

        let agent = Agent::new(
            self.provider.clone(),
            self.config.clone(),
            self.registry.executors().clone(),
            self.registry.defs().to_vec(),
        );

        let parts = ContextParts {
            soul_prompt: self.soul.clone(),
            history: vec![Message {
                role: Role::User,
                content: task.prompt.clone(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let result = agent.run(cancel.clone(), parts).await;

        let response = if result.response.is_empty() {
            format!("Task {:?} completed with no output.", task.name)
        } else {
            result.response
        };

        let msg = format!("**[Scheduled: {}]**\n\n{}", task.name, response);
        (self.sender)(&task.channel, &msg);

        info!(
            task = %task.name,
            stop = ?result.stop_reason,
            "scheduled task complete"
        );
    }
}

// ---------------------------------------------------------------------------
// Schedule parsing
// ---------------------------------------------------------------------------

/// Check if a task should execute at the given time.
///
/// Supports:
/// - "every 5m", "every 1h", "every 30s"
/// - Simple cron: "M H D M W" (minute hour day-of-month month day-of-week)
fn should_run(schedule: &str, now: &chrono::DateTime<Utc>) -> bool {
    if let Some(interval_str) = schedule.strip_prefix("every ") {
        return should_run_interval(interval_str, now);
    }
    should_run_cron(schedule, now)
}

fn should_run_interval(interval_str: &str, now: &chrono::DateTime<Utc>) -> bool {
    let secs = parse_duration_secs(interval_str);
    if secs <= 0 {
        return false;
    }
    // Run if current time aligns to the interval (within 30s window).
    now.timestamp() % secs < 30
}

fn parse_duration_secs(s: &str) -> i64 {
    let s = s.trim();
    if s.is_empty() {
        return 0;
    }

    let (num_str, suffix) = s.split_at(s.len().saturating_sub(1));
    let num: i64 = match num_str.parse() {
        Ok(n) => n,
        Err(_) => return 0,
    };

    match suffix {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        _ => 0,
    }
}

fn should_run_cron(schedule: &str, now: &chrono::DateTime<Utc>) -> bool {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() != 5 {
        return false;
    }

    // Only trigger on the exact minute (within 30s window).
    if now.second() >= 30 {
        return false;
    }

    match_cron_field(parts[0], now.minute() as i32)
        && match_cron_field(parts[1], now.hour() as i32)
        && match_cron_field(parts[2], now.day() as i32)
        && match_cron_field(parts[3], now.month() as i32)
        && match_cron_field(parts[4], now.weekday().num_days_from_sunday() as i32)
}

fn match_cron_field(field: &str, value: i32) -> bool {
    if field == "*" {
        return true;
    }

    // Handle ranges: "1-5"
    if let Some((lo_str, hi_str)) = field.split_once('-') {
        let lo: i32 = lo_str.parse().unwrap_or(-1);
        let hi: i32 = hi_str.parse().unwrap_or(-1);
        return value >= lo && value <= hi;
    }

    // Handle lists: "1,3,5"
    for part in field.split(',') {
        if let Ok(n) = part.trim().parse::<i32>() {
            if n == value {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn parse_duration_minutes() {
        assert_eq!(parse_duration_secs("5m"), 300);
        assert_eq!(parse_duration_secs("1h"), 3600);
        assert_eq!(parse_duration_secs("30s"), 30);
    }

    #[test]
    fn cron_field_wildcard() {
        assert!(match_cron_field("*", 0));
        assert!(match_cron_field("*", 59));
    }

    #[test]
    fn cron_field_exact() {
        assert!(match_cron_field("9", 9));
        assert!(!match_cron_field("9", 10));
    }

    #[test]
    fn cron_field_range() {
        assert!(match_cron_field("1-5", 3));
        assert!(!match_cron_field("1-5", 0));
        assert!(!match_cron_field("1-5", 6));
    }

    #[test]
    fn cron_field_list() {
        assert!(match_cron_field("1,3,5", 3));
        assert!(!match_cron_field("1,3,5", 4));
    }

    #[test]
    fn should_run_cron_match() {
        // Monday 9:00 AM UTC, second 0
        let dt = Utc.with_ymd_and_hms(2026, 3, 16, 9, 0, 0).unwrap();
        assert!(should_run("0 9 * * 1", &dt));
    }

    #[test]
    fn should_run_cron_no_match() {
        // Monday 10:00 AM UTC
        let dt = Utc.with_ymd_and_hms(2026, 3, 16, 10, 0, 0).unwrap();
        assert!(!should_run("0 9 * * 1", &dt));
    }
}
