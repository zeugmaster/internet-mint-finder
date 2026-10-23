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
    mut shutdown: watch::Receiver<bool>,
) {
    let mut last_announce: Option<Instant> = None;
    let mut last_status_pass: Option<Instant> = None;
    let mut last_status: HashMap<u64, (MintState, Instant)> = HashMap::new();

    loop {
        if last_announce.is_none_or(|t| t.elapsed() >= ANNOUNCE_INTERVAL)
            && send(&client, &keys, events::announcement_builder(&announcement))
                .await
                .is_some()
        {
            info!("Published auditor announcement");
            last_announce = Some(Instant::now());
        }

        flush_outbox(&db, &client, &keys).await;

        if last_status_pass.is_none_or(|t| t.elapsed() >= STATUS_INTERVAL) {
            publish_status(&db, &client, &keys, &mut last_status).await;
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

/// Sign and send one event. `Some(id)` once at least one relay accepted it.
async fn send(client: &Client, keys: &Keys, builder: EventBuilder) -> Option<String> {
    let event = match builder.sign_with_keys(keys) {
        Ok(event) => event,
        Err(e) => {
            warn!("Could not sign event: {}", e);
            return None;
        }
    };
    match client.send_event(&event).await {
        Ok(output) if !output.success.is_empty() => Some(event.id.to_hex()),
        Ok(output) => {
            warn!("No relay accepted event {}: {:?}", event.id, output.failed);
            None
        }
        Err(e) => {
            warn!("Could not publish event {}: {}", event.id, e);
            None
        }
    }
}

/// Publish rows in id order and mark each as published. Stops at the first
/// send failure so a relay outage is retried next tick instead of logged once
/// per row.
pub(super) async fn flush_outbox(db: &AuditorDatabase, client: &Client, keys: &Keys) {
    let swaps = match db.unpublished_swaps(OUTBOX_BATCH).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!("Could not read swap outbox: {}", e);
            return;
        }
    };
    for swap in swaps {
        let Some(event_id) = send(client, keys, events::swap_builder(&swap)).await else {
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
        let Some(event_id) = send(client, keys, events::check_builder(&check)).await else {
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
    last: &mut HashMap<u64, (MintState, Instant)>,
) {
    let mints = match db.get_all_mints().await {
        Ok(mints) => mints,
        Err(e) => {
            warn!("Could not list mints for status events: {}", e);
            return;
        }
    };
    for mint in mints.iter().filter(|m| m.sum_donations > 0) {
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
        if send(client, keys, builder).await.is_none() {
            return;
        }
        last.insert(mint.id, (mint.state, Instant::now()));
    }
}
