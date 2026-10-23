//! Nostr integration: outbox publisher and peer/NIP-87 ingest.
//!
//! This instance signs its own observations with `IMF_NOSTR_NSEC` and
//! publishes them to `IMF_RELAYS` ([`publish`]). Observations of the auditors
//! listed in `IMF_AUDITORS` are ingested from the same relays so every
//! instance shows the union. Without a key the server runs exactly as before:
//! nothing is published and nothing is ingested.

pub mod events;
pub mod ingest;
pub mod publish;

use crate::server::service;
use crate::server::state::AppState;
use nostr_sdk::prelude::*;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{info, warn};

pub const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://relay.cashumints.space",
];

/// `IMF_*` Nostr settings, read once at startup.
#[derive(Debug, Clone)]
pub struct NostrConfig {
    /// `IMF_NOSTR_NSEC`: this auditor's signing key (nsec or hex). Unset = off.
    pub keys: Option<Keys>,
    /// `IMF_RELAYS`: comma-separated relay URLs; defaults to [`DEFAULT_RELAYS`].
    pub relays: Vec<RelayUrl>,
    /// `IMF_AUDITORS`: comma-separated npub/hex keys whose events are ingested.
    pub auditors: Vec<PublicKey>,
    /// `IMF_PUBLIC_URLS`: URLs announced for this instance; defaults to `BASE_URL`.
    pub public_urls: Vec<String>,
    /// `IMF_CONTACT`: optional, pseudonymous.
    pub contact: Option<String>,
}

impl Default for NostrConfig {
    fn default() -> Self {
        Self {
            keys: None,
            relays: DEFAULT_RELAYS
                .iter()
                .filter_map(|u| RelayUrl::parse(u).ok())
                .collect(),
            auditors: Vec::new(),
            public_urls: Vec::new(),
            contact: None,
        }
    }
}

impl NostrConfig {
    pub fn from_env(base_url: Option<&str>) -> Result<Self, String> {
        fn var(key: &str) -> Option<String> {
            std::env::var(key)
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        }
        fn list(key: &str) -> Option<Vec<String>> {
            var(key).map(|v| {
                v.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
        }

        let defaults = Self::default();
        let keys = match var("IMF_NOSTR_NSEC") {
            Some(secret) => {
                Some(Keys::parse(&secret).map_err(|e| format!("IMF_NOSTR_NSEC is invalid: {e}"))?)
            }
            None => None,
        };
        let relays = match list("IMF_RELAYS") {
            Some(urls) => urls
                .iter()
                .map(|u| RelayUrl::parse(u).map_err(|e| format!("IMF_RELAYS entry {u:?}: {e}")))
                .collect::<Result<Vec<_>, _>>()?,
            None => defaults.relays,
        };
        let auditors = list("IMF_AUDITORS")
            .unwrap_or_default()
            .iter()
            .map(|k| PublicKey::parse(k).map_err(|e| format!("IMF_AUDITORS entry {k:?}: {e}")))
            .collect::<Result<Vec<_>, _>>()?;
        let public_urls = list("IMF_PUBLIC_URLS")
            .unwrap_or_else(|| base_url.map(str::to_string).into_iter().collect());

        Ok(Self {
            keys,
            relays,
            auditors,
            public_urls,
            contact: var("IMF_CONTACT"),
        })
    }

    /// Hex pubkey this instance publishes under, if a key is configured.
    pub fn self_pubkey_hex(&self) -> Option<String> {
        self.keys.as_ref().map(|k| k.public_key().to_hex())
    }
}

/// Running Nostr tasks; `stop` ends them and closes the relay connections.
pub struct NostrHandle {
    shutdown: watch::Sender<bool>,
    pub client: Option<Arc<Client>>,
}

impl NostrHandle {
    pub async fn stop(&self) {
        let _ = self.shutdown.send(true);
        if let Some(client) = &self.client {
            client.disconnect().await;
        }
    }
}

/// Connect to the relays, publish if a key is configured, ingest if peers are
/// listed. With neither, nothing runs.
pub async fn start(state: &AppState) -> NostrHandle {
    let (shutdown, shutdown_rx) = watch::channel(false);
    let keys = state.nostr.keys.clone();
    let self_pubkey = keys.as_ref().map(|k| k.public_key());
    let peers: Vec<PublicKey> = state
        .nostr
        .auditors
        .iter()
        .copied()
        .filter(|p| Some(*p) != self_pubkey)
        .collect();
    if keys.is_none() && peers.is_empty() {
        info!("Neither IMF_NOSTR_NSEC nor IMF_AUDITORS set: Nostr disabled");
        return NostrHandle {
            shutdown,
            client: None,
        };
    }

    let client = match &keys {
        Some(keys) => Client::new(keys.clone()),
        None => Client::builder().build(),
    };
    for url in &state.nostr.relays {
        if let Err(e) = client.add_relay(url.clone()).await {
            warn!("Could not add relay {}: {}", url, e);
        }
    }
    client.connect().await;
    let client = Arc::new(client);

    if !peers.is_empty() {
        info!("Ingesting observations of {} peer auditor(s)", peers.len());
        tokio::spawn(ingest::run(
            state.auditor_db.clone(),
            client.clone(),
            peers,
            shutdown_rx.clone(),
        ));
    }

    let Some(keys) = keys else {
        return NostrHandle {
            shutdown,
            client: Some(client),
        };
    };

    let cfg = &state.auditor_config;
    let announcement = events::Announcement {
        name: state.app_name.clone(),
        urls: state.nostr.public_urls.clone(),
        donate: service::build_payment_request(state, None)
            .ok()
            .map(|pr| pr.to_string()),
        software: format!("imf {}", env!("CARGO_PKG_VERSION")),
        frequency_secs: (cfg.min_swap_delay_secs + cfg.max_swap_delay_secs) / 2,
        contact: state.nostr.contact.clone(),
    };

    info!(
        "Publishing audit events as {} to {} relay(s)",
        keys.public_key().to_bech32().unwrap_or_default(),
        state.nostr.relays.len()
    );
    tokio::spawn(publish::run(
        state.auditor_db.clone(),
        client.clone(),
        keys,
        announcement,
        shutdown_rx,
    ));

    NostrHandle {
        shutdown,
        client: Some(client),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::storage::{AuditorDatabase, SwapEvent};
    use crate::types::MintState;
    use nostr_relay_builder::prelude::MockRelay;
    use std::time::Duration;

    /// The mirror property end to end: instance A publishes its rows through a
    /// relay, instance B (no key of its own) ingests them and can answer the
    /// same per-mint questions about A's observations.
    #[tokio::test]
    async fn one_instance_publishes_and_another_ingests_through_a_relay() {
        let relay = MockRelay::run().await.unwrap();
        let url = relay.url().await;

        let keys_a = Keys::generate();
        let db_a = AuditorDatabase::open_in_memory().unwrap();
        db_a.record_donation("https://a.example", 100, "token")
            .await
            .unwrap();
        db_a.record_donation("https://b.example", 100, "token")
            .await
            .unwrap();
        let a = db_a
            .get_mint_by_url("https://a.example")
            .await
            .unwrap()
            .unwrap();
        let b = db_a
            .get_mint_by_url("https://b.example")
            .await
            .unwrap()
            .unwrap();
        let mut swap = SwapEvent::success(&a, &b, 21, 1, 800.0);
        swap.fee_reserve = Some(2);
        db_a.record_swap_success(&swap).await.unwrap();
        db_a.insert_mint_check(
            a.id,
            &a.url,
            "HEALTH",
            MintState::Ok,
            None,
            None,
            Some(30.0),
        )
        .await
        .unwrap();

        let client_a = Client::new(keys_a.clone());
        client_a.add_relay(url.clone()).await.unwrap();
        client_a.connect().await;
        client_a.wait_for_connection(Duration::from_secs(5)).await;
        publish::flush_outbox(&db_a, &client_a, &keys_a).await;
        assert!(db_a.unpublished_swaps(10).await.unwrap().is_empty());
        assert!(db_a.unpublished_checks(10).await.unwrap().is_empty());

        let db_b = AuditorDatabase::open_in_memory().unwrap();
        let client_b = Client::builder().build();
        client_b.add_relay(url).await.unwrap();
        client_b.connect().await;
        client_b.wait_for_connection(Duration::from_secs(5)).await;
        let peers = vec![keys_a.public_key()];
        let filter = Filter::new()
            .kinds([
                Kind::from_u16(events::KIND_OBSERVATION),
                Kind::from_u16(events::KIND_ANNOUNCEMENT),
            ])
            .authors(peers.clone());
        ingest::backfill(&db_b, &client_b, &filter, &peers, 0, "test").await;

        let swaps = db_b.get_swaps(0, 10).await.unwrap();
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].auditor_pubkey, keys_a.public_key().to_hex());
        assert_eq!(swaps[0].amount, 21);
        assert_eq!(swaps[0].fee_reserve, Some(2));
        let a_on_b = db_b
            .get_mint_by_url("https://a.example")
            .await
            .unwrap()
            .unwrap();
        let b_on_b = db_b
            .get_mint_by_url("https://b.example")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(db_b.get_mint_uptime(a_on_b.id).await.unwrap().checks, 1);
        let metrics = db_b
            .get_mint_metrics(b_on_b.id, Some(keys_a.public_key().to_hex()))
            .await
            .unwrap();
        assert_eq!((metrics.attempts, metrics.successes), (1, 1));
        assert!(db_b.unpublished_swaps(10).await.unwrap().is_empty());
        assert_eq!(db_b.get_auditors().await.unwrap().len(), 1);
    }
}
