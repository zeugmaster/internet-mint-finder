//! Public directory ingestion and delivery of already-signed reviews. No server signing key.
pub mod events;
pub mod ingest;

use crate::server::state::AppState;
use nostr_sdk::prelude::*;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.damus.io",
    "wss://nos.lol",
    "wss://relay.primal.net",
    "wss://relay.cashumints.space",
    "wss://relay.nostr.net",
];
#[derive(Debug, Clone)]
pub struct NostrConfig {
    pub relays: Vec<RelayUrl>,
}
impl Default for NostrConfig {
    fn default() -> Self {
        Self {
            relays: DEFAULT_RELAYS
                .iter()
                .map(|u| RelayUrl::parse(u).expect("default relay URL"))
                .collect(),
        }
    }
}
impl NostrConfig {
    pub fn from_env() -> Result<Self, String> {
        match std::env::var("IMF_RELAYS") {
            Err(_) => Ok(Self::default()),
            Ok(raw) => {
                let mut relays = raw
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(|u| RelayUrl::parse(u).map_err(|e| format!("Invalid IMF_RELAYS URL: {e}")))
                    .collect::<Result<Vec<_>, _>>()?;
                relays.sort();
                relays.dedup();
                if relays.is_empty() {
                    return Err("IMF_RELAYS must contain at least one relay".into());
                }
                Ok(Self { relays })
            }
        }
    }
}

pub async fn start(
    state: &AppState,
    workers: &mut tokio::task::JoinSet<&'static str>,
    shutdown: watch::Receiver<bool>,
) -> Arc<Client> {
    let client = Client::builder().build();
    for url in &state.nostr.relays {
        client
            .add_relay(url.clone())
            .await
            .expect("validated relay URL");
    }
    client.connect().await;
    let client = Arc::new(client);
    state
        .workers
        .register("relay ingestion", Duration::from_secs(180), false);
    {
        let (db, client, shutdown, progress) = (
            state.db.clone(),
            client.clone(),
            shutdown.clone(),
            state.workers.clone(),
        );
        workers.spawn(async move {
            ingest::run(db, client, shutdown, progress).await;
            "relay ingestion"
        });
    }
    state
        .workers
        .register("review publishing", Duration::from_secs(60), true);
    {
        let state = state.clone();
        let client = client.clone();
        let mut shutdown = shutdown;
        workers.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                state.workers.beat("review publishing");
                tokio::select! { _ = shutdown.changed() => break, _ = tick.tick() => {} }
                let pending = match state.db.pending().await {
                    Ok(events) => events,
                    Err(error) => { tracing::error!(%error,"Cannot read review outbox"); continue; }
                };
                for event in pending {
                    state.workers.beat("review publishing");
                    let result = tokio::select! {
                        _ = shutdown.changed() => return "review publishing",
                        result = tokio::time::timeout(Duration::from_secs(10),client.send_event(&event)) => result,
                    };
                    match result {
                        Ok(Ok(output)) if !output.success.is_empty() => {
                            if let Err(error) = state.db.mark_published(event.id.to_hex()).await { tracing::error!(%error,"Cannot acknowledge review delivery"); }
                        }
                        _ => {
                            tracing::warn!(event_id=%event.id,"Review delivery pending; will retry");
                            if let Err(error) = state.db.retry_later(event.id.to_hex()).await { tracing::error!(%error,"Cannot reschedule review delivery"); }
                        }
                    }
                }
            }
            "review publishing"
        });
    }
    client
}
