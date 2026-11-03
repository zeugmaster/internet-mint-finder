//! Low-frequency, no-fee mint availability checks.
//!
//! Each hourly sample performs exactly one NUT-06 mint-info request, records
//! the result for the observed-uptime chart, and refreshes the capability cache
//! used by paid-swap selection. There are no quote-status reads or retries in
//! this worker.

use super::config::AuditorConfig;
use super::{probe_mint_info, swap_support_from_info, SwapSupport, SwapSupportCache};
use crate::server::classify;
use crate::server::error::AuditorError;
use crate::server::net::NetConfig;
use crate::server::storage::{AuditorDatabase, Mint};
use crate::types::MintState;
use cdk::wallet::WalletRepository;
use chrono::{Duration as ChronoDuration, Utc};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};
use tracing::warn;

/// Spread a round over at most one minute. The offset is stable for a given
/// mint, so checks stay one hour apart instead of moving around every round.
const STAGGER_WINDOW_SECS: u64 = 60;

/// Probe every tracked mint once and persist one sample per mint.
pub(super) async fn run_probes(
    wallet_repository: &Arc<WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
    config: &AuditorConfig,
    net: &NetConfig,
    support_cache: &SwapSupportCache,
) -> Result<(), AuditorError> {
    let mints = auditor_db.get_all_mints().await?;
    let permits = Arc::new(Semaphore::new(net.max_concurrent_probes.max(1)));
    let mut set: JoinSet<()> = JoinSet::new();

    for mint in mints {
        let wallet_repository = wallet_repository.clone();
        let auditor_db = auditor_db.clone();
        let config = config.clone();
        let net = net.clone();
        let support_cache = support_cache.clone();
        let permits = permits.clone();

        set.spawn(async move {
            let delay = stagger_delay(mint.id);
            if !delay.is_zero() {
                sleep(delay).await;
            }
            let _permit = permits
                .acquire_owned()
                .await
                .expect("health semaphore closed");
            probe_and_record(
                &wallet_repository,
                &auditor_db,
                &config,
                &net,
                &support_cache,
                mint,
            )
            .await;
        });
    }

    while let Some(result) = set.join_next().await {
        result.expect("health probe task failed");
    }
    Ok(())
}

/// Run one single-attempt probe and persist its result. Probe failures are data,
/// so they are logged and recorded rather than returned to the round runner.
async fn probe_and_record(
    wallet_repository: &Arc<WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
    config: &AuditorConfig,
    net: &NetConfig,
    support_cache: &SwapSupportCache,
    mint: Mint,
) {
    let started = Instant::now();
    let (state, error_message, category) =
        match probe_mint_info(wallet_repository, net, &mint.url).await {
            Ok(info) => {
                let support = swap_support_from_info(&info);
                support_cache
                    .write()
                    .await
                    .insert(mint.url.clone(), support);
                // The probe is the only contact with unfunded (directory-only)
                // mints, so it is what gives them a name and a NIP-87 pubkey.
                if let Ok(json) = serde_json::to_string(&info) {
                    let name = info.name.clone().unwrap_or_default();
                    if let Err(error) = auditor_db.update_mint_metadata(mint.id, name, json).await {
                        warn!("Could not store mint info for {}: {}", mint.url, error);
                    }
                }
                let (state, message) = health_state_from_support(&support);
                let category = message
                    .as_deref()
                    .and_then(|raw| classify::categorize(Some(raw)))
                    .map(str::to_string);
                (state, message, category)
            }
            Err(error) => {
                let raw = error.to_string();
                let rate_limited = is_rate_limited(&raw);
                let state = if rate_limited {
                    MintState::Warn
                } else {
                    MintState::Error
                };
                let category = if rate_limited {
                    Some("rate_limit".to_string())
                } else {
                    classify::categorize(Some(&raw)).map(str::to_string)
                };
                warn!("Health probe failed for {}: {}", mint.url, raw);
                (state, Some(classify::enrich_raw(&raw)), category)
            }
        };
    let latency_ms = started.elapsed().as_secs_f64() * 1000.0;

    if let Err(error) = auditor_db
        .insert_mint_check(
            mint.id,
            &mint.url,
            "HEALTH",
            state,
            error_message,
            category,
            Some(latency_ms),
        )
        .await
    {
        warn!("Could not record health check for {}: {}", mint.url, error);
        return;
    }

    let update = match state {
        MintState::Ok => auditor_db.record_mint_health_ok(mint.id).await,
        MintState::Warn => auditor_db.record_mint_health_warn(mint.id).await,
        MintState::Error => {
            match auditor_db
                .has_consecutive_health_errors(mint.id, config.health_failure_threshold)
                .await
            {
                Ok(true) => {
                    let retry_at = Utc::now()
                        + ChronoDuration::seconds(config.health_check_interval_secs as i64);
                    auditor_db
                        .record_mint_health_failure(mint.id, retry_at)
                        .await
                }
                Ok(false) => Ok(()),
                Err(error) => Err(error),
            }
        }
        MintState::Unknown => Ok(()),
    };

    if let Err(error) = update {
        warn!("Could not update health state for {}: {}", mint.url, error);
    }
}

/// A successful NUT-06 response proves the mint is online. Missing swap
/// capabilities make it degraded, not offline.
fn health_state_from_support(support: &SwapSupport) -> (MintState, Option<String>) {
    match (support.can_receive, support.can_spend) {
        (true, true) => (MintState::Ok, None),
        (false, true) => (
            MintState::Warn,
            Some("mint does not advertise sat/bolt11 mint support".to_string()),
        ),
        (true, false) => (
            MintState::Warn,
            Some("mint does not advertise sat/bolt11 melt support".to_string()),
        ),
        (false, false) => (
            MintState::Warn,
            Some("mint does not advertise sat/bolt11 mint or melt support".to_string()),
        ),
    }
}

fn is_rate_limited(raw: &str) -> bool {
    let lower = raw.to_ascii_lowercase();
    lower.contains("429") || lower.contains("too many requests") || lower.contains("rate limit")
}

fn stagger_delay(mint_id: u64) -> Duration {
    Duration::from_secs(mint_id.wrapping_mul(37) % STAGGER_WINDOW_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_support() -> SwapSupport {
        SwapSupport {
            can_receive: true,
            can_spend: true,
            ..SwapSupport::default()
        }
    }

    #[test]
    fn successful_info_response_is_online_even_without_swap_support() {
        assert_eq!(
            health_state_from_support(&full_support()),
            (MintState::Ok, None)
        );

        let (state, error) = health_state_from_support(&SwapSupport::default());
        assert_eq!(state, MintState::Warn);
        assert_eq!(
            error.as_deref(),
            Some("mint does not advertise sat/bolt11 mint or melt support")
        );
    }

    #[test]
    fn rate_limit_response_is_not_offline() {
        assert!(is_rate_limited("HTTP 429 Too Many Requests"));
        assert!(is_rate_limited("rate limit exceeded"));
        assert!(!is_rate_limited("connection refused"));
    }

    #[test]
    fn stagger_is_stable_and_less_than_one_minute() {
        assert_eq!(stagger_delay(42), stagger_delay(42));
        assert!(stagger_delay(42) < Duration::from_secs(STAGGER_WINDOW_SECS));
    }
}
