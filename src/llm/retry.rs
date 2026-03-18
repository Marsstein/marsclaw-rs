//! Retry with exponential backoff and jitter.

use std::future::Future;
use std::time::Duration;

use rand::Rng;

/// Whether an error is safe to retry.
pub fn is_retryable(err: &anyhow::Error) -> bool {
    let msg = err.to_string();
    msg.contains("429")
        || msg.contains("503")
        || msg.contains("529")
        || msg.contains("rate limit")
        || msg.contains("overloaded")
        || msg.contains("timeout")
        || msg.contains("timed out")
}

/// Parameters for exponential backoff.
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
        }
    }
}

/// Execute `f` with exponential backoff.
///
/// Delay doubles each attempt and receives 0-25 % jitter.
/// Non-retryable errors are returned immediately.
pub async fn with_retry<T, F, Fut>(config: &RetryConfig, f: F) -> anyhow::Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = anyhow::Result<T>>,
{
    let mut last_err: Option<anyhow::Error> = None;

    for attempt in 0..=config.max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(err) => {
                if !is_retryable(&err) {
                    return Err(err);
                }
                last_err = Some(err);

                if attempt == config.max_retries {
                    break;
                }

                let base = config.base_delay.as_millis() as u64;
                let delay_ms = base * 2u64.pow(attempt);
                let jitter = rand::rng().random_range(0..=delay_ms / 4);
                tokio::time::sleep(Duration::from_millis(delay_ms + jitter)).await;
            }
        }
    }

    Err(last_err.unwrap())
}
