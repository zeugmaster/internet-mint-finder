//! Shared networking middleware for the auditor engine.
//!
//! Every outbound call to a (possibly unreachable) mint goes through one of
//! these helpers. The CDK wallet HTTP client ships with no request timeout, so
//! a single dead or hung mint can otherwise block a sequential probe loop
//! forever — which is exactly why the observed-uptime monitor only ever
//! recorded one sample per mint. Bounding every call with a wall-clock deadline
//! (and retrying idempotent reads with backoff) keeps one bad mint from
//! stalling the audit of all the others.

use crate::server::error::AuditorError;
use std::future::Future;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::warn;

/// Network tunables. Every field can be overridden with an `AUDITOR_NET_*`
/// environment variable; unset values fall back to the defaults below.
#[derive(Debug, Clone)]
pub struct NetConfig {
    /// Deadline for a single mint network call (quotes, melt, mint, receive).
    pub request_timeout: Duration,
    /// Deadline for hourly mint availability and capability metadata reads.
    pub probe_timeout: Duration,
    /// Number of attempts for an idempotent read (1 = no retry).
    pub max_attempts: u32,
    /// Base backoff between retry attempts (grows linearly with the attempt).
    pub retry_backoff: Duration,
    /// Maximum number of concurrent mint capability reads. Bounds outbound
    /// connection fan-out when a health round inspects every tracked mint.
    pub max_concurrent_probes: usize,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
            probe_timeout: Duration::from_secs(10),
            max_attempts: 3,
            retry_backoff: Duration::from_millis(400),
            max_concurrent_probes: 16,
        }
    }
}

impl NetConfig {
    pub fn from_env() -> Self {
        fn secs(key: &str, default: Duration) -> Duration {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_secs)
                .unwrap_or(default)
        }
        fn millis(key: &str, default: Duration) -> Duration {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .map(Duration::from_millis)
                .unwrap_or(default)
        }
        fn count(key: &str, default: u32) -> u32 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
                .filter(|n| *n >= 1)
                .unwrap_or(default)
        }
        fn count_usize(key: &str, default: usize) -> usize {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|n| *n >= 1)
                .unwrap_or(default)
        }

        let defaults = Self::default();
        Self {
            request_timeout: secs("AUDITOR_NET_REQUEST_TIMEOUT_SECS", defaults.request_timeout),
            probe_timeout: secs("AUDITOR_NET_PROBE_TIMEOUT_SECS", defaults.probe_timeout),
            max_attempts: count("AUDITOR_NET_MAX_ATTEMPTS", defaults.max_attempts),
            retry_backoff: millis("AUDITOR_NET_RETRY_BACKOFF_MS", defaults.retry_backoff),
            max_concurrent_probes: count_usize(
                "AUDITOR_NET_MAX_CONCURRENT_PROBES",
                defaults.max_concurrent_probes,
            ),
        }
    }
}

/// Run a single fallible network call under a wall-clock deadline.
///
/// `label` is used only for diagnostics. A call that does not resolve in time
/// becomes [`AuditorError::Timeout`] instead of hanging the caller.
pub async fn with_timeout<F, T, E>(
    label: &str,
    deadline: Duration,
    op: F,
) -> Result<T, AuditorError>
where
    F: Future<Output = Result<T, E>>,
    E: Into<AuditorError>,
{
    match timeout(deadline, op).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.into()),
        Err(_elapsed) => Err(AuditorError::Timeout(format!(
            "{label} did not respond within {}s",
            deadline.as_secs().max(1)
        ))),
    }
}

/// Retry an *idempotent* read (e.g. fetching mint info or a quote status)
/// with linear backoff, bounding each attempt by `deadline`.
///
/// Only the final failure is returned; intermediate failures are logged at
/// `warn`. Use this only for operations that are safe to repeat — never for a
/// melt or any value-moving call.
pub async fn retry<F, Fut, T>(
    cfg: &NetConfig,
    label: &str,
    deadline: Duration,
    mut make: F,
) -> Result<T, AuditorError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AuditorError>>,
{
    let attempts = cfg.max_attempts.max(1);
    let mut last_err: Option<AuditorError> = None;

    for attempt in 1..=attempts {
        match with_timeout(label, deadline, make()).await {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt < attempts {
                    warn!("{label}: attempt {attempt}/{attempts} failed ({err}); retrying");
                    sleep(cfg.retry_backoff * attempt).await;
                }
                last_err = Some(err);
            }
        }
    }

    Err(last_err
        .unwrap_or_else(|| AuditorError::Internal(format!("{label}: no attempts were made"))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn with_timeout_trips_on_slow_call() {
        let res: Result<(), AuditorError> = with_timeout("slow", Duration::from_secs(1), async {
            sleep(Duration::from_secs(5)).await;
            Ok::<(), AuditorError>(())
        })
        .await;
        assert!(matches!(res, Err(AuditorError::Timeout(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn retry_succeeds_after_transient_failures() {
        let cfg = NetConfig {
            max_attempts: 3,
            retry_backoff: Duration::from_millis(10),
            ..NetConfig::default()
        };
        let mut calls = 0u32;
        let res = retry(&cfg, "flaky", Duration::from_secs(1), || {
            calls += 1;
            let n = calls;
            async move {
                if n < 3 {
                    Err(AuditorError::Unreachable("nope".into()))
                } else {
                    Ok(42u32)
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(res, 42);
        assert_eq!(calls, 3);
    }

    #[tokio::test(start_paused = true)]
    async fn retry_gives_up_and_returns_last_error() {
        let cfg = NetConfig {
            max_attempts: 2,
            retry_backoff: Duration::from_millis(10),
            ..NetConfig::default()
        };
        let res: Result<u32, _> = retry(&cfg, "down", Duration::from_secs(1), || async {
            Err(AuditorError::Unreachable("down".into()))
        })
        .await;
        assert!(matches!(res, Err(AuditorError::Unreachable(_))));
    }
}
