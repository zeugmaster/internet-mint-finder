//! Shared networking middleware for the auditor engine.
//!
//! Mint requests wrapped here have a wall-clock deadline so an unavailable
//! mint cannot stall a round indefinitely.

use crate::server::error::AuditorError;
use std::future::Future;
use std::time::Duration;
use tokio::time::timeout;

/// Network tunables. Every field can be overridden with an `AUDITOR_NET_*`
/// environment variable; unset values fall back to the defaults below.
#[derive(Debug, Clone)]
pub struct NetConfig {
    /// Deadline for a single mint network call (quotes, melt, mint, receive).
    pub request_timeout: Duration,
    /// Deadline for hourly mint availability and capability metadata reads.
    pub probe_timeout: Duration,
    /// Maximum number of concurrent mint capability reads. Bounds outbound
    /// connection fan-out when a health round inspects every tracked mint.
    pub max_concurrent_probes: usize,
}

impl Default for NetConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
            probe_timeout: Duration::from_secs(10),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn with_timeout_trips_on_slow_call() {
        let res: Result<(), AuditorError> = with_timeout("slow", Duration::from_secs(1), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<(), AuditorError>(())
        })
        .await;
        assert!(matches!(res, Err(AuditorError::Timeout(_))));
    }
}
