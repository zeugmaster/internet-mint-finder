//! The auditing engine.
//!
//! Continuously moves ecash between audited mints: it creates a mint quote
//! (BOLT11 invoice) on one mint and pays it by melting ecash on another,
//! recording latency, fees, and failures for every attempt. Three background
//! loops run concurrently:
//!
//! * [`swap`]     — the paid round-trip audit between mint pairs;
//! * [`health`]   — hourly, no-fee NUT-06 availability checks;
//! * [`snapshot`] — periodic balance snapshots for the time-series charts.

mod config;
mod guard;
mod health;
mod snapshot;
mod swap;

use guard::MintGuards;

pub use config::AuditorConfig;

use crate::server::error::AuditorError;
use crate::server::net::{self, NetConfig};
use crate::server::storage::AuditorDatabase;
use cdk::nuts::{CurrencyUnit, MintInfo, PaymentMethod};
use cdk::wallet::WalletRepository;
use rand::Rng;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::{watch, RwLock};
use tokio::time::{sleep, Duration, MissedTickBehavior};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Shared constants & helpers
// ---------------------------------------------------------------------------

const SWAP_UNIT: CurrencyUnit = CurrencyUnit::Sat;
const SWAP_METHOD: PaymentMethod = PaymentMethod::BOLT11;

/// What a mint advertises (per NUT-04 / NUT-05) for sat/bolt11 swaps.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct SwapSupport {
    pub can_receive: bool,
    pub can_spend: bool,
    pub receive_min_amount: Option<u64>,
    pub receive_max_amount: Option<u64>,
    pub spend_min_amount: Option<u64>,
    pub spend_max_amount: Option<u64>,
}

pub(super) type SwapSupportCache = Arc<RwLock<HashMap<String, SwapSupport>>>;

/// Live (local) wallet balance for a mint. Reads the local store only — no
/// network — so it needs no timeout. Returns 0 if the wallet can't be opened.
pub(super) async fn balance_for(wallet_repository: &Arc<WalletRepository>, url: &str) -> u64 {
    let Ok(mint_url) = cdk::mint_url::MintUrl::from_str(url) else {
        return 0;
    };
    match wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
    {
        Ok(wallet) => wallet.total_balance().await.map(u64::from).unwrap_or(0),
        Err(_) => 0,
    }
}

/// Fetch a mint's NUT-06 info under a deadline.
pub(super) async fn probe_mint_info(
    wallet_repository: &Arc<WalletRepository>,
    net: &NetConfig,
    url: &str,
) -> Result<MintInfo, AuditorError> {
    let mint_url = cdk::mint_url::MintUrl::from_str(url)
        .map_err(|_| AuditorError::InvalidInput(format!("invalid mint URL: {url}")))?;
    net::with_timeout(
        &format!("mint info {url}"),
        net.probe_timeout,
        wallet_repository.fetch_mint_info(&mint_url),
    )
    .await
}

/// Probe a mint's NUT-06 info under a deadline and derive its swap support.
pub(super) async fn probe_swap_support(
    wallet_repository: &Arc<WalletRepository>,
    net: &NetConfig,
    url: &str,
) -> Result<SwapSupport, AuditorError> {
    Ok(swap_support_from_info(
        &probe_mint_info(wallet_repository, net, url).await?,
    ))
}

/// Return the most recently observed capabilities for a mint. The hourly
/// health worker refreshes this cache; only a newly tracked mint or a startup
/// race falls back to a network fetch from the paid-swap path.
pub(super) async fn swap_support_for_round(
    wallet_repository: &Arc<WalletRepository>,
    net: &NetConfig,
    cache: &SwapSupportCache,
    url: &str,
) -> SwapSupport {
    if let Some(support) = cache.read().await.get(url).copied() {
        return support;
    }

    match probe_swap_support(wallet_repository, net, url).await {
        Ok(support) => {
            cache.write().await.insert(url.to_string(), support);
            support
        }
        Err(e) => {
            warn!("Could not load mint info for {}: {}", url, e);
            SwapSupport::default()
        }
    }
}

pub(super) fn swap_support_from_info(info: &MintInfo) -> SwapSupport {
    let receive_settings = (!info.nuts.nut04.disabled)
        .then(|| info.nuts.nut04.get_settings(&SWAP_UNIT, &SWAP_METHOD))
        .flatten();
    let spend_settings = (!info.nuts.nut05.disabled)
        .then(|| info.nuts.nut05.get_settings(&SWAP_UNIT, &SWAP_METHOD))
        .flatten();

    SwapSupport {
        can_receive: receive_settings.is_some(),
        can_spend: spend_settings.is_some(),
        receive_min_amount: receive_settings
            .as_ref()
            .and_then(|settings| settings.min_amount.map(u64::from)),
        receive_max_amount: receive_settings
            .as_ref()
            .and_then(|settings| settings.max_amount.map(u64::from)),
        spend_min_amount: spend_settings
            .as_ref()
            .and_then(|settings| settings.min_amount.map(u64::from)),
        spend_max_amount: spend_settings
            .as_ref()
            .and_then(|settings| settings.max_amount.map(u64::from)),
    }
}

// ---------------------------------------------------------------------------
// Auditor
// ---------------------------------------------------------------------------

pub struct Auditor {
    wallet_repository: Arc<WalletRepository>,
    auditor_db: Arc<AuditorDatabase>,
    config: AuditorConfig,
    net: NetConfig,
    shutdown: watch::Sender<bool>,
}

impl Auditor {
    pub fn new(
        wallet_repository: Arc<WalletRepository>,
        auditor_db: Arc<AuditorDatabase>,
        config: AuditorConfig,
    ) -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            wallet_repository,
            auditor_db,
            config,
            net: NetConfig::from_env(),
            shutdown,
        }
    }

    /// Spawn the hourly health loop and, when paid swaps are on, the swap and
    /// snapshot loops. Idempotent: calling twice only spawns once (subsequent
    /// calls warn).
    pub async fn start(&self) {
        if self.shutdown.receiver_count() > 0 {
            warn!("Auditor already running");
            return;
        }

        info!(config = ?self.config, net = ?self.net, "Starting auditor");

        // Per-mint locks shared by paid swaps and the optional error-retry loop,
        // so two attempts can never race the same mint's proof store while
        // disjoint mints still run in parallel.
        let guards = MintGuards::default();
        let mut initial_support = HashMap::new();
        match self.auditor_db.get_all_mints().await {
            Ok(mints) => {
                for mint in mints {
                    let Some(raw_info) = mint.info.as_deref() else {
                        continue;
                    };
                    match serde_json::from_str::<MintInfo>(raw_info) {
                        Ok(info) => {
                            initial_support.insert(mint.url, swap_support_from_info(&info));
                        }
                        Err(error) => {
                            warn!(
                                "Could not parse stored mint info for {}: {}",
                                mint.url, error
                            );
                        }
                    }
                }
            }
            Err(error) => warn!("Could not seed mint capability cache: {}", error),
        }
        let support_cache: SwapSupportCache = Arc::new(RwLock::new(initial_support));

        // Hourly, no-fee availability checks. The ticker is anchored to the
        // worker start time, so the small per-mint stagger inside `run_probes`
        // does not accumulate and slowly stretch the cadence.
        {
            let wallet_repository = self.wallet_repository.clone();
            let auditor_db = self.auditor_db.clone();
            let config = self.config.clone();
            let net = self.net.clone();
            let support_cache = support_cache.clone();
            let interval_secs = self.config.health_check_interval_secs;
            let mut shutdown = self.shutdown.subscribe();

            tokio::spawn(async move {
                info!(
                    "Auditor health check loop started (every {}s)",
                    interval_secs
                );
                let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => {
                            info!("Auditor health check loop stopping");
                            break;
                        }
                        _ = ticker.tick() => {
                            if let Err(e) = health::run_probes(
                                &wallet_repository,
                                &auditor_db,
                                &config,
                                &net,
                                &support_cache,
                            )
                            .await
                            {
                                warn!("Health probe round failed: {}", e);
                            }
                        }
                    }
                }
            });
        }

        // Everything below moves or tracks real ecash.
        if !self.config.paid_swaps {
            info!("Paid swaps are off (AUDITOR_PAID_SWAPS): only the free health checks run");
            return;
        }

        // Swap loop
        {
            let wallet_repository = self.wallet_repository.clone();
            let auditor_db = self.auditor_db.clone();
            let config = self.config.clone();
            let net = self.net.clone();
            let guards = guards.clone();
            let support_cache = support_cache.clone();
            let mut shutdown = self.shutdown.subscribe();

            tokio::spawn(async move {
                info!("Auditor swap loop started");
                loop {
                    if let Err(e) = swap::run_round(
                        &wallet_repository,
                        &auditor_db,
                        &config,
                        &net,
                        &guards,
                        &support_cache,
                    )
                    .await
                    {
                        warn!("Swap round ended with error: {}", e);
                    }

                    let delay = if config.min_swap_delay_secs == config.max_swap_delay_secs {
                        config.min_swap_delay_secs
                    } else {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(config.min_swap_delay_secs..=config.max_swap_delay_secs)
                    };

                    tokio::select! {
                        _ = shutdown.changed() => {
                            info!("Auditor swap loop stopping");
                            break;
                        }
                        _ = sleep(Duration::from_secs(delay)) => {}
                    }
                }
            });
        }

        // Snapshot loop
        {
            let wallet_repository = self.wallet_repository.clone();
            let auditor_db = self.auditor_db.clone();
            let interval = self.config.snapshot_interval_secs;
            let mut shutdown = self.shutdown.subscribe();

            tokio::spawn(async move {
                info!("Auditor snapshot loop started (every {}s)", interval);
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => {
                            info!("Auditor snapshot loop stopping");
                            break;
                        }
                        _ = sleep(Duration::from_secs(interval)) => {
                            if let Err(e) = snapshot::run(&wallet_repository, &auditor_db).await {
                                warn!("Snapshot round failed: {}", e);
                            }
                        }
                    }
                }
            });
        }

        // Error-retry loop: re-audit mints currently in the Error state far more
        // often than the slow main swap cadence, so a recovered mint returns to
        // OK quickly (ignores the bench cooldown on purpose).
        //
        // Disabled when `error_retry_interval_secs == 0` (the default), which is
        // the Python-parity behavior: the reference has no second audit path — a
        // failed mint simply becomes eligible again on the next random swap round.
        if self.config.error_retry_interval_secs > 0 {
            let wallet_repository = self.wallet_repository.clone();
            let auditor_db = self.auditor_db.clone();
            let config = self.config.clone();
            let net = self.net.clone();
            let guards = guards.clone();
            let support_cache = support_cache.clone();
            let interval = self.config.error_retry_interval_secs;
            let mut shutdown = self.shutdown.subscribe();

            tokio::spawn(async move {
                info!("Auditor error-retry loop started (every {}s)", interval);
                loop {
                    tokio::select! {
                        _ = shutdown.changed() => {
                            info!("Auditor error-retry loop stopping");
                            break;
                        }
                        _ = sleep(Duration::from_secs(interval)) => {
                            if let Err(e) = swap::run_error_retry_round(
                                &wallet_repository,
                                &auditor_db,
                                &config,
                                &net,
                                &guards,
                                &support_cache,
                            )
                            .await
                            {
                                warn!("Error-retry round failed: {}", e);
                            }
                        }
                    }
                }
            });
        }
    }

    /// Signal all background loops to stop.
    pub async fn stop(&self) {
        let _ = self.shutdown.send(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdk::nuts::{CurrencyUnit, PaymentMethod};

    #[test]
    fn swap_support_uses_sat_bolt11_nut04_and_nut05_settings() {
        let mut info = MintInfo::default();
        info.nuts.nut04.methods.push(cdk::nuts::MintMethodSettings {
            method: PaymentMethod::BOLT11,
            method_name: None,
            unit: CurrencyUnit::Sat,
            min_amount: Some(cdk::Amount::from(21)),
            max_amount: Some(cdk::Amount::from(210)),
            options: None,
        });
        info.nuts.nut05.methods.push(cdk::nuts::MeltMethodSettings {
            method: PaymentMethod::BOLT11,
            method_name: None,
            unit: CurrencyUnit::Sat,
            min_amount: Some(cdk::Amount::from(13)),
            max_amount: Some(cdk::Amount::from(130)),
            options: None,
        });

        let support = swap_support_from_info(&info);
        assert_eq!(
            support,
            SwapSupport {
                can_receive: true,
                can_spend: true,
                receive_min_amount: Some(21),
                receive_max_amount: Some(210),
                spend_min_amount: Some(13),
                spend_max_amount: Some(130),
            }
        );

        info.nuts.nut04.methods[0].unit = CurrencyUnit::Msat;
        let support = swap_support_from_info(&info);
        assert!(!support.can_receive);
        assert!(support.can_spend);
    }
}
