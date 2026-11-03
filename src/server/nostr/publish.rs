//! Outbox publisher: signs and sends this instance's unpublished rows, the
//! per-mint status events and the auditor announcement.
//!
//! The swap and health loops write rows exactly as before; this task polls
//! for rows whose `published_at` is NULL, so a relay outage loses nothing and
//! the engine never waits on the network.

use super::events::{self, Announcement};
use crate::server::storage::AuditorDatabase;
use crate::types::MintState;
use nostr_sdk::prelude::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::{info, warn};

const OUTBOX_INTERVAL: Duration = Duration::from_secs(10);
const OUTBOX_BATCH: u64 = 100;
const STATUS_INTERVAL: Duration = Duration::from_secs(60);
const STATUS_MAX_AGE: Duration = Duration::from_secs(3600);
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(24 * 3600);

pub async fn run(
    db: Arc<AuditorDatabase>,
    client: Arc<Client>,
    keys: Keys,
    announcement: Announcement,
    archive: RelayUrl,
    progress: Arc<crate::server::workers::Workers>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut last_announce: Option<Instant> = None;
    let mut last_status_pass: Option<Instant> = None;
    let mut last_status: HashMap<u64, (MintState, Instant)> = HashMap::new();

    while !*shutdown.borrow() {
        progress.beat("relay publishing");
        if last_announce.is_none_or(|t| t.elapsed() >= ANNOUNCE_INTERVAL)
            && send(
                &client,
                &keys,
                &archive,
                events::announcement_builder(&announcement),
            )
            .await
            .is_some()
        {
            info!("Published auditor announcement");
            last_announce = Some(Instant::now());
        }

        flush_outbox(&db, &client, &keys, &archive, &progress).await;

        if last_status_pass.is_none_or(|t| t.elapsed() >= STATUS_INTERVAL) {
            publish_status(&db, &client, &keys, &archive, &progress, &mut last_status).await;
            last_status_pass = Some(Instant::now());
        }

        tokio::select! {
            _ = tokio::time::sleep(OUTBOX_INTERVAL) => {}
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

/// Public relays are best effort; only the configured archive completes delivery.
async fn send(
    client: &Client,
    keys: &Keys,
    archive: &RelayUrl,
    builder: EventBuilder,
) -> Option<String> {
    let event = match builder.sign_with_keys(keys) {
        Ok(event) => event,
        Err(e) => {
            warn!("Could not sign event: {}", e);
            return None;
        }
    };
    let public: Vec<_> = client
        .relays()
        .await
        .into_keys()
        .filter(|url| url != archive)
        .collect();
    // A slow public relay must not discard a successful acknowledgement from our archive.
    let (archived, _) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(20),
            client.send_event_to([archive.clone()], &event)
        ),
        tokio::time::timeout(
            Duration::from_secs(20),
            client.send_event_to(public, &event)
        ),
    );
    match archived {
        Ok(Ok(output)) if output.success.contains(archive) => Some(event.id.to_hex()),
        Ok(Ok(output)) => {
            warn!(
                "Archive relay did not accept event {}: {:?}",
                event.id, output.failed
            );
            None
        }
        Ok(Err(e)) => {
            warn!("Could not publish event {}: {}", event.id, e);
            None
        }
        Err(_) => {
            warn!("Relay publication timed out; keeping the outbox row");
            None
        }
    }
}

/// Publish rows in id order and mark each as published. Stops at the first
/// send failure so a relay outage is retried next tick instead of logged once
/// per row.
pub(super) async fn flush_outbox(
    db: &AuditorDatabase,
    client: &Client,
    keys: &Keys,
    archive: &RelayUrl,
    progress: &crate::server::workers::Workers,
) {
    let swaps = match db.unpublished_swaps(OUTBOX_BATCH).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Could not read swap outbox: {}", e);
            return;
        }
    };
    for swap in swaps {
        progress.beat("relay publishing");
        let Some(event_id) = send(client, keys, archive, events::swap_builder(&swap)).await else {
            return;
        };
        if let Err(e) = db.mark_swap_published(swap.id, &event_id).await {
            warn!("Could not mark swap {} published: {}", swap.id, e);
        }
    }

    let checks = match db.unpublished_checks(OUTBOX_BATCH).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Could not read check outbox: {}", e);
            return;
        }
    };
    for check in checks {
        progress.beat("relay publishing");
        let Some(event_id) = send(client, keys, archive, events::check_builder(&check)).await
        else {
            return;
        };
        if let Err(e) = db.mark_check_published(check.id, &event_id).await {
            warn!("Could not mark check {} published: {}", check.id, e);
        }
    }
}

/// One addressable status event per locally audited mint, re-sent when the
/// state changes or the last one is older than an hour.
async fn publish_status(
    db: &AuditorDatabase,
    client: &Client,
    keys: &Keys,
    archive: &RelayUrl,
    progress: &crate::server::workers::Workers,
    last: &mut HashMap<u64, (MintState, Instant)>,
) {
    let mints = match db.get_audited_mints().await {
        Ok(mints) => mints,
        Err(e) => {
            warn!("Could not list mints for status events: {}", e);
            return;
        }
    };
    for mint in &mints {
        progress.beat("relay publishing");
        let fresh = last
            .get(&mint.id)
            .is_some_and(|(state, at)| *state == mint.state && at.elapsed() < STATUS_MAX_AGE);
        if fresh {
            continue;
        }
        let metrics = match db.get_mint_metrics(mint.id, Some(String::new())).await {
            Ok(metrics) => metrics,
            Err(e) => {
                warn!("Could not compute metrics for {}: {}", mint.url, e);
                continue;
            }
        };
        let pubkey = mint.pubkey();
        let builder = events::status_builder(&mint.url, pubkey.as_deref(), mint.state, &metrics);
        if send(client, keys, archive, builder).await.is_none() {
            return;
        }
        last.insert(mint.id, (mint.state, Instant::now()));
    }
}
