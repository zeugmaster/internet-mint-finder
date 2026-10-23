use axum::extract::FromRef;
use bip39::Mnemonic;
use cdk::nuts::CurrencyUnit;
use cdk::wallet::{Wallet, WalletRepository, WalletRepositoryBuilder};
use cdk_sqlite::WalletSqliteDatabase;
use chrono::Utc;
use leptos::prelude::LeptosOptions;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{info, warn};

use super::auditor::AuditorConfig;
use super::cache::ResponseCache;
use super::mint_url::normalize_mint_url;
use super::nostr::NostrConfig;
use super::storage::{AuditorDatabase, Mint};
use crate::types::MintState;

#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub wallet_repository: Arc<WalletRepository>,
    pub app_name: String,
    pub auditor_db: Arc<AuditorDatabase>,
    pub auditor_config: AuditorConfig,
    pub cache: Arc<ResponseCache>,
    /// The BASE_URL for NUT-18 payment request HTTP transport.
    pub base_url: Option<String>,
    /// Optional local/dev aliases from token mint URLs to reachable mint URLs.
    pub mint_url_aliases: Arc<Vec<(String, String)>>,
    /// Nostr identity, relays and trusted auditors (`IMF_*`).
    pub nostr: Arc<NostrConfig>,
    /// Process start, so the health check tolerates the first probe round.
    pub started_at: std::time::Instant,
}

impl AppState {
    /// Get the wallet for a mint, creating it on first use. Saga recovery
    /// runs only when a wallet is first added, not on every lookup.
    pub async fn wallet_for_mint(&self, mint_url: &str) -> Result<Wallet, cdk::Error> {
        let mint = cdk::mint_url::MintUrl::from_str(mint_url)?;
        if let Ok(wallet) = self
            .wallet_repository
            .get_wallet(&mint, &CurrencyUnit::Sat)
            .await
        {
            return Ok(wallet);
        }

        self.wallet_repository.add_wallet(mint.clone()).await?;

        let wallet = self
            .wallet_repository
            .get_wallet(&mint, &CurrencyUnit::Sat)
            .await?;

        Self::recover_wallet_sagas(&wallet, mint_url).await;

        Ok(wallet)
    }

    pub async fn new(leptos_options: LeptosOptions) -> Result<Self, Box<dyn std::error::Error>> {
        dotenvy::dotenv().ok();

        let app_name =
            std::env::var("APP_NAME").unwrap_or_else(|_| "Internet Mint Finder".to_string());
        let app_dir = app_data_dir()?;

        if !app_dir.exists() {
            std::fs::create_dir_all(&app_dir)?;
        }

        // CDK wallet database (for wallet/ecash state)
        let wallet_repository = open_wallet_repository().await?;

        // Auditor database (separate SQLite for auditor data + analytics)
        let auditor_db_path = app_dir.join("auditor.db");
        let auditor_db = AuditorDatabase::open(&auditor_db_path)?;
        let auditor_db = Arc::new(auditor_db);

        let auditor_config = AuditorConfig::from_env();

        // Response cache for analytics endpoints
        let cache = Arc::new(ResponseCache::new());

        // BASE_URL for NUT-18 payment requests
        let base_url = normalize_base_url(std::env::var("BASE_URL").ok())?;
        let mint_url_aliases = Arc::new(parse_mint_url_aliases());
        let nostr = Arc::new(NostrConfig::from_env(base_url.as_deref())?);

        info!("App data directory: {:?}", app_dir);
        info!("Auditor DB initialized at {:?}", auditor_db_path);
        if !mint_url_aliases.is_empty() {
            info!("Configured {} mint URL aliases", mint_url_aliases.len());
        }

        let state = Self {
            leptos_options,
            wallet_repository,
            app_name,
            auditor_db,
            auditor_config,
            cache,
            base_url,
            mint_url_aliases,
            nostr,
            started_at: std::time::Instant::now(),
        };

        state.recover_empty_auditor_db_from_wallets().await;
        state.recover_known_wallets().await;

        Ok(state)
    }

    pub fn mint_url_alias(&self, mint_url: &str) -> Option<String> {
        let mint_url = normalize_mint_url(mint_url);
        self.mint_url_aliases
            .iter()
            .find_map(|(from, to)| (from == &mint_url).then(|| to.clone()))
    }

    /// If the auditor registry was wiped but the CDK wallet DB remains, rebuild
    /// a fresh registry from the SAT wallets that still hold local ecash. This
    /// intentionally restores only the mint list and donation targets; swap,
    /// check, snapshot, and donation history start fresh with the new DB.
    async fn recover_empty_auditor_db_from_wallets(&self) {
        let has_history = match self.auditor_db.has_mint_history().await {
            Ok(exists) => exists,
            Err(e) => {
                warn!(
                    "Could not inspect auditor DB for wallet-backed recovery: {}",
                    e
                );
                return;
            }
        };
        if has_history {
            return;
        }

        let wallets = self.wallet_repository.get_wallets().await;
        if wallets.is_empty() {
            return;
        }

        let mut recovered = 0u64;
        for wallet in wallets {
            if wallet.unit.clone() != CurrencyUnit::Sat {
                continue;
            }

            let balance = match wallet.total_balance().await {
                Ok(balance) => u64::from(balance),
                Err(e) => {
                    warn!(
                        "Could not read wallet balance for {} during auditor DB recovery: {}",
                        wallet.mint_url, e
                    );
                    continue;
                }
            };
            if balance == 0 {
                continue;
            }

            let mint_info = wallet.load_mint_info().await.ok();
            let now = Utc::now();
            let mint = Mint {
                id: 0,
                url: wallet.mint_url.to_string(),
                info: mint_info
                    .as_ref()
                    .and_then(|info| serde_json::to_string(info).ok()),
                name: mint_info.and_then(|info| info.name).unwrap_or_default(),
                sum_donations: balance,
                updated_at: now,
                next_update: Some(now),
                state: MintState::Unknown,
                n_errors: 0,
                n_mints: 0,
                n_melts: 0,
                latitude: None,
                longitude: None,
                deleted_at: None,
            };

            match self.auditor_db.upsert_mint(&mint).await {
                Ok(_) => {
                    recovered += 1;
                    info!(
                        "Recovered auditor mint registry entry for {} with {} sat",
                        mint.url, balance
                    );
                }
                Err(e) => warn!(
                    "Could not recover auditor mint registry entry for {}: {}",
                    mint.url, e
                ),
            }
        }

        if recovered > 0 {
            info!(
                "Recovered {} mint(s) into empty auditor DB from existing wallet DB",
                recovered
            );
        }
    }

    /// Recover incomplete wallet sagas for every known mint once at startup.
    async fn recover_known_wallets(&self) {
        let mints = match self.auditor_db.get_all_mints().await {
            Ok(mints) => mints,
            Err(e) => {
                warn!("Could not list mints for startup saga recovery: {}", e);
                return;
            }
        };

        for mint in mints {
            match self.wallet_for_mint(&mint.url).await {
                Ok(wallet) => Self::recover_wallet_sagas(&wallet, &mint.url).await,
                Err(e) => warn!("Could not open wallet for {}: {}", mint.url, e),
            }
        }
    }

    async fn recover_wallet_sagas(wallet: &Wallet, mint_url: &str) {
        match wallet.recover_incomplete_sagas().await {
            Ok(report) => {
                if report.recovered > 0
                    || report.compensated > 0
                    || report.skipped > 0
                    || report.failed > 0
                {
                    info!(
                        "Recovered wallet sagas for {}: recovered={}, compensated={}, skipped={}, failed={}",
                        mint_url,
                        report.recovered,
                        report.compensated,
                        report.skipped,
                        report.failed
                    );
                }
            }
            Err(e) => {
                warn!("Failed to recover wallet sagas for {}: {}", mint_url, e);
            }
        }
    }
}

/// The application data directory (`~/.<app-name>`), derived from `APP_NAME`.
/// Shared by the server and the `delete-mint` CLI so both resolve the same
/// `auditor.db`.
pub fn app_data_dir() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let app_name = std::env::var("APP_NAME").unwrap_or_else(|_| "Internet Mint Finder".to_string());
    let app_dir_name = format!(".{}", app_name.to_lowercase().replace(' ', "-"));
    Ok(home::home_dir()
        .ok_or("Could not find home directory")?
        .join(app_dir_name))
}

/// Path to the auditor's relational database (`auditor.db`).
pub fn auditor_db_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    Ok(app_data_dir()?.join("auditor.db"))
}

/// Path to the cdk wallet database (`wallet.db`).
pub fn wallet_db_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    Ok(app_data_dir()?.join("wallet.db"))
}

/// Open the cdk wallet repository: seed derived from `CDK_MNEMONIC`, backed by
/// `wallet.db`. Shared by the server (`AppState::new`) and the maintenance CLIs
/// (`wallet-doctor`, `cleanup-proofs`) so every path resolves the same store and
/// the same seed — a CLI that opened a different seed would "restore" a wallet
/// of unrelated proofs.
pub async fn open_wallet_repository() -> Result<Arc<WalletRepository>, Box<dyn std::error::Error>> {
    let mnemonic_str = std::env::var("CDK_MNEMONIC").map_err(|_| {
        "CDK_MNEMONIC is required; configure a private BIP-39 seed before starting the wallet"
    })?;
    let mnemonic = Mnemonic::from_str(&mnemonic_str)?;
    let seed = mnemonic.to_seed("");

    let app_dir = app_data_dir()?;
    if !app_dir.exists() {
        std::fs::create_dir_all(&app_dir)?;
    }

    let wallet_db_path = wallet_db_path()?;
    let wallet_db =
        WalletSqliteDatabase::new(wallet_db_path.to_str().ok_or("Invalid path")?).await?;
    let wallet_db = Arc::new(wallet_db);

    Ok(Arc::new(
        WalletRepositoryBuilder::new()
            .localstore(wallet_db)
            .seed(seed)
            .build()
            .await?,
    ))
}

fn normalize_base_url(raw: Option<String>) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Some(raw) = raw.filter(|value| !value.trim().is_empty()) else {
        return Ok(None);
    };
    let url = reqwest::Url::parse(raw.trim())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "BASE_URL must be an HTTP(S) URL without credentials, a query, or a fragment".into(),
        );
    }
    Ok(Some(url.as_str().trim_end_matches('/').to_string()))
}

fn parse_mint_url_aliases() -> Vec<(String, String)> {
    let Ok(raw) = std::env::var("AUDITOR_MINT_URL_ALIASES") else {
        return Vec::new();
    };

    raw.split(',')
        .filter_map(|entry| {
            let (from, to) = entry.split_once('=')?;
            let from = normalize_mint_url(from);
            let to = normalize_mint_url(to);
            if from.is_empty() || to.is_empty() {
                None
            } else {
                Some((from, to))
            }
        })
        .collect()
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_base_url;

    #[test]
    fn validates_payment_transport_base_urls() {
        assert_eq!(normalize_base_url(Some("  ".into())).unwrap(), None);
        assert_eq!(
            normalize_base_url(Some(" https://example.com/a/ ".into())).unwrap(),
            Some("https://example.com/a".into())
        );
        for invalid in [
            "javascript:alert(1)",
            "https://user:pass@example.com",
            "https://example.com?x=1",
            "https://example.com#path",
        ] {
            assert!(normalize_base_url(Some(invalid.into())).is_err());
        }
    }
}
