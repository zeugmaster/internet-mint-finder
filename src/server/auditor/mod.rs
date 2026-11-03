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
pub mod guard;
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
use tokio::task::JoinSet;
use tokio::time::{sleep, sleep_until, Duration, Instant, MissedTickBehavior};
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

/// Fetch a mint's NUT-06 info under a deadline.
pub(super) async fn probe_mint_info(
    _wallet_repository: &Arc<WalletRepository>,
    net: &NetConfig,
    url: &str,
) -> Result<MintInfo, AuditorError> {
    let mint_url = cdk::mint_url::MintUrl::from_str(url)
        .map_err(|_| AuditorError::InvalidInput(format!("invalid mint URL: {url}")))?;
    net::with_timeout(
        &format!("mint info {url}"),
        net.probe_timeout,
        crate::server::mint_transport::connector(mint_url).get_mint_info(),
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
}

impl Auditor {
    pub fn new(
        wallet_repository: Arc<WalletRepository>,
        auditor_db: Arc<AuditorDatabase>,
        config: AuditorConfig,
    ) -> Self {
        Self {
            wallet_repository,
            auditor_db,
            config,
            net: NetConfig::from_env(),
        }
    }

    /// Register the engine's workers with the application's shared lifecycle.
    pub async fn start(
        self,
        workers: &mut JoinSet<&'static str>,
        shutdown: watch::Receiver<bool>,
        guards: MintGuards,
        progress: Arc<crate::server::workers::Workers>,
    ) {
        info!(config = ?self.config, net = ?self.net, "Starting auditor");

        progress.register(
            "health probes",
            Duration::from_secs(self.config.health_check_interval_secs.saturating_mul(2)),
            true,
        );
        progress.register(
            "paid audits",
            Duration::from_secs(self.config.max_swap_delay_secs.saturating_add(1800)),
            true,
        );
        progress.register(
            "balance snapshots",
            Duration::from_secs(self.config.snapshot_interval_secs.saturating_add(120)),
            true,
        );

        // Retry batches can contain overlapping pairs; each wallet has one writer.
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
            let mut shutdown = shutdown.clone();

            let progress = progress.clone();
            workers.spawn(async move {
                info!(
                    "Auditor health check loop started (every {}s)",
                    interval_secs
                );
                let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs));
                ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
                while !*shutdown.borrow() {
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => {
                            info!("Auditor health check loop stopping");
                            break;
                        }
                        _ = ticker.tick() => {
                            // Probes are read-only network work and can stop immediately.
                            tokio::select! {
                                biased;
                                _ = shutdown.changed() => break,
                                result = health::run_probes(
                                    &wallet_repository, &auditor_db, &config, &net, &support_cache,
                                ) => {
                                    progress.beat("health probes");
                                    if let Err(error) = result {
                                        warn!(%error, "Health probe round failed");
                                    }
                                }
                            }
                        }
                    }
                }
                "health probes"
            });
        }

        // One paid worker schedules normal rounds and optional error retries.
        {
            let wallet_repository = self.wallet_repository.clone();
            let auditor_db = self.auditor_db.clone();
            let config = self.config.clone();
            let net = self.net.clone();
            let mut shutdown = shutdown.clone();

            let progress = progress.clone();
            workers.spawn(async move {
                let mut next_swap = Instant::now();
                let retry_interval = Duration::from_secs(config.error_retry_interval_secs);
                let mut next_retry =
                    (config.error_retry_interval_secs > 0).then(|| Instant::now() + retry_interval);
                while let Some(retry_errors) =
                    next_paid_round(&mut shutdown, next_swap, next_retry).await
                {
                    if let Err(error) = swap::run_round(
                        &wallet_repository,
                        &auditor_db,
                        &config,
                        &net,
                        &guards,
                        &support_cache,
                        retry_errors,
                        &shutdown,
                    )
                    .await
                    {
                        warn!(%error, retry_errors, "Paid round failed");
                    }
                    progress.beat("paid audits");
                    if retry_errors {
                        next_retry = Some(Instant::now() + retry_interval);
                    } else {
                        let delay = rand::thread_rng()
                            .gen_range(config.min_swap_delay_secs..=config.max_swap_delay_secs);
                        next_swap = Instant::now() + Duration::from_secs(delay);
                    }
                }
                "paid audits"
            });
        }

        // Snapshot loop
        {
            let wallet_repository = self.wallet_repository.clone();
            let auditor_db = self.auditor_db.clone();
            let interval = self.config.snapshot_interval_secs;
            let mut shutdown = shutdown.clone();

            let progress = progress.clone();
            workers.spawn(async move {
                info!("Auditor snapshot loop started (every {}s)", interval);
                while !*shutdown.borrow() {
                    tokio::select! {
                        biased;
                        _ = shutdown.changed() => {
                            info!("Auditor snapshot loop stopping");
                            break;
                        }
                        _ = sleep(Duration::from_secs(interval)) => {
                            progress.beat("balance snapshots");
                            if let Err(e) = snapshot::run(&wallet_repository, &auditor_db).await {
                                warn!("Snapshot round failed: {}", e);
                            }
                        }
                    }
                }
                "balance snapshots"
            });
        }
    }
}

/// Pick the earliest due round; a disabled retry interval has no deadline.
async fn next_paid_round(
    shutdown: &mut watch::Receiver<bool>,
    next_swap: Instant,
    next_retry: Option<Instant>,
) -> Option<bool> {
    if *shutdown.borrow() {
        return None;
    }
    let retry_errors = next_retry.is_some_and(|retry| retry < next_swap);
    let deadline = next_retry.filter(|_| retry_errors).unwrap_or(next_swap);
    tokio::select! {
        biased;
        _ = shutdown.changed() => None,
        _ = sleep_until(deadline) => Some(retry_errors),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdk::nuts::{CurrencyUnit, PaymentMethod};

    #[tokio::test(start_paused = true)]
    async fn paid_scheduler_preserves_both_cadences_and_prioritizes_shutdown() {
        let (stop, mut shutdown) = watch::channel(false);
        let start = Instant::now();
        let normal = start + Duration::from_secs(300);
        let retry = start + Duration::from_secs(20);
        assert_eq!(
            next_paid_round(&mut shutdown, start, Some(retry)).await,
            Some(false)
        );
        assert_eq!(
            next_paid_round(&mut shutdown, normal, Some(retry)).await,
            Some(true)
        );
        assert_eq!(Instant::now(), retry);
        assert_eq!(
            next_paid_round(&mut shutdown, normal, None).await,
            Some(false)
        );
        assert_eq!(Instant::now(), normal);
        stop.send(true).unwrap();
        assert_eq!(
            next_paid_round(&mut shutdown, start, Some(retry)).await,
            None
        );
    }

    #[tokio::test]
    async fn stats_skip_directory_entries_but_surface_missing_funded_wallets() {
        use crate::server::{state::AppState, storage::AuditorDatabase};
        use std::sync::Arc;

        let db = Arc::new(AuditorDatabase::open_in_memory().unwrap());
        let repository = Arc::new(
            cdk::wallet::WalletRepositoryBuilder::new()
                .localstore(Arc::new(
                    cdk_sqlite::WalletSqliteDatabase::new(":memory:")
                        .await
                        .unwrap(),
                ))
                .seed([0; 64])
                .build()
                .await
                .unwrap(),
        );
        let state = AppState {
            workers: Default::default(),
            wallet_guards: Default::default(),
            leptos_options: leptos::prelude::get_configuration(Some("Cargo.toml"))
                .unwrap()
                .leptos_options,
            wallet_repository: repository.clone(),
            app_name: "test".into(),
            auditor_db: db.clone(),
            auditor_config: Default::default(),
            cache: Arc::new(Default::default()),
            base_url: None,
            mint_url_aliases: Arc::new(vec![]),
            nostr: Arc::new(Default::default()),
            started_at: std::time::Instant::now(),
        };
        db.ensure_mint_for_url("https://directory.example")
            .await
            .unwrap();
        assert_eq!(
            crate::server::service::compute_stats(&state)
                .await
                .unwrap()
                .total_balance,
            0
        );
        db.record_donation("https://funded.example", 10, "token")
            .await
            .unwrap();
        state.cache.clear();
        assert!(crate::server::service::compute_stats(&state).await.is_err());
        assert!(snapshot::run(&repository, &db).await.is_err());
        let count: u64 = db
            .run(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM mint_snapshots", [], |r| r.get(0))?),
            )
            .await
            .unwrap();
        assert_eq!(
            count, 0,
            "an unavailable balance must not become a zero snapshot"
        );
        repository
            .create_wallet(
                "https://funded.example".parse().unwrap(),
                cdk::nuts::CurrencyUnit::Sat,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            crate::server::service::compute_stats(&state)
                .await
                .unwrap()
                .total_balance,
            0
        );
    }

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
