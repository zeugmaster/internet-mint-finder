//! Ingest the observations and announcements of the auditors in
//! `IMF_AUDITORS` so this instance shows the union of every vantage point.
//!
//! Peer rows land in the same `swaps` / `mint_checks` tables with the peer's
//! pubkey and the relay event id; the unique index on `event_id` makes every
//! insert idempotent, so backfill and the live subscription may overlap freely.
//! Mint URLs are resolved to local `mints` rows, creating unfunded ones as
//! needed ("observed via peers" in the UI).

use super::events::{self, to_datetime, Observation};
use crate::server::error::AuditorError;
use crate::server::mint_url::normalize_mint_url;
use crate::server::storage::{Auditor, AuditorDatabase};
use nostr_sdk::prelude::*;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, watch};
use tracing::{debug, info, warn};

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const PAGE: usize = 500;
const MAX_PAGES: usize = 400;
/// The live subscription starts this far before "now" so nothing published
/// while the backfill runs is missed.
const LIVE_OVERLAP_SECS: u64 = 600;

pub async fn run(
    db: Arc<AuditorDatabase>,
    client: Arc<Client>,
    peers: Vec<PublicKey>,
    mut shutdown: watch::Receiver<bool>,
) {
    // Peer auditors' own kinds are trusted by author; the NIP-87 directory
    // kinds are public and come from anyone.
    let mut feeds = vec![(
        "NIP-87 directory",
        Filter::new().kinds([
            Kind::from_u16(events::KIND_NIP87_CASHU),
            Kind::from_u16(events::KIND_NIP87_REVIEW),
        ]),
        watermark(db.latest_nip87_event_at()).await,
    )];
    if !peers.is_empty() {
        feeds.push((
            "peer auditors",
            Filter::new()
                .kinds([
                    Kind::from_u16(events::KIND_OBSERVATION),
                    Kind::from_u16(events::KIND_ANNOUNCEMENT),
                ])
                .authors(peers.iter().copied()),
            watermark(db.latest_peer_event_at()).await,
        ));
    }

    let mut notifications = client.notifications();
    for (label, filter, since) in &feeds {
        let live_since = Timestamp::now()
            .as_secs()
            .saturating_sub(LIVE_OVERLAP_SECS)
            .max(*since);
        if let Err(e) = client
            .subscribe(filter.clone().since(Timestamp::from(live_since)), None)
            .await
        {
            warn!("Could not subscribe to {}: {}", label, e);
        }
    }
    for (label, filter, since) in &feeds {
        backfill(&db, &client, filter, &peers, *since, label).await;
    }

    loop {
        tokio::select! {
            notification = notifications.recv() => match notification {
                Ok(RelayPoolNotification::Event { event, .. }) => {
                    handle_event(&db, &peers, &event).await;
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!("Ingest fell behind by {} notification(s)", n);
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            _ = shutdown.changed() => {
                if *shutdown.borrow() {
                    return;
                }
            }
        }
    }
}

async fn watermark(
    query: impl Future<Output = Result<Option<chrono::DateTime<chrono::Utc>>, AuditorError>>,
) -> u64 {
    match query.await {
        Ok(Some(at)) => at.timestamp().max(0) as u64,
        Ok(None) => 0,
        Err(e) => {
            warn!("Could not read ingest watermark: {}", e);
            0
        }
    }
}

/// Page backwards from now to `since` with `until`; stops when a page is empty
/// or no longer moves the cursor. Duplicates are harmless (unique keys).
pub(super) async fn backfill(
    db: &AuditorDatabase,
    client: &Client,
    base: &Filter,
    peers: &[PublicKey],
    since: u64,
    label: &str,
) {
    let mut until = Timestamp::now();
    let mut total = 0usize;
    for _ in 0..MAX_PAGES {
        let filter = base
            .clone()
            .since(Timestamp::from(since))
            .until(until)
            .limit(PAGE);
        let events = match client.fetch_events(filter, FETCH_TIMEOUT).await {
            Ok(events) => events,
            Err(e) => {
                warn!("Backfill fetch failed: {}", e);
                break;
            }
        };
        if events.is_empty() {
            break;
        }
        let oldest = events.iter().map(|e| e.created_at).min().unwrap_or(until);
        for event in events.iter() {
            handle_event(db, peers, event).await;
            total += 1;
        }
        if oldest >= until {
            break;
        }
        until = oldest;
    }
    info!("Nostr backfill ({}) processed {} event(s)", label, total);
}

/// Store one event. Audit kinds are accepted only from listed auditors even if
/// a relay sends more; NIP-87 kinds are accepted from anyone.
pub async fn handle_event(db: &AuditorDatabase, peers: &[PublicKey], event: &Event) {
    let kind = event.kind.as_u16();
    if kind == events::KIND_NIP87_CASHU || kind == events::KIND_NIP87_REVIEW {
        if let Err(e) = store_nip87(db, event).await {
            warn!("Could not store NIP-87 event {}: {}", event.id, e);
        }
        return;
    }
    if !peers.contains(&event.pubkey) {
        return;
    }
    let pubkey = event.pubkey.to_hex();
    let seen_at = to_datetime(event.created_at);

    match kind {
        events::KIND_ANNOUNCEMENT => {
            let Some(a) = events::decode_announcement(event) else {
                return;
            };
            let auditor = Auditor {
                pubkey,
                name: a.name,
                urls: a.urls,
                donate: a.donate,
                software: a.software,
                frequency_secs: a.frequency_secs,
                contact: a.contact,
                announced_at: Some(seen_at),
                last_seen: seen_at,
            };
            if let Err(e) = db.upsert_auditor(&auditor).await {
                warn!("Could not store auditor announcement: {}", e);
            }
        }
        events::KIND_OBSERVATION => {
            let result = match events::decode_observation(event) {
                Some(Observation::Swap { swap, failed_url }) => {
                    store_swap(db, swap, failed_url).await
                }
                Some(Observation::Check(check)) => store_check(db, check).await,
                None => {
                    debug!("Ignoring undecodable observation {}", event.id);
                    return;
                }
            };
            match result {
                Ok(()) => {}
                Err(e) if is_duplicate(&e) => {}
                Err(e) => warn!("Could not store peer observation {}: {}", event.id, e),
            }
            if let Err(e) = db.touch_auditor(&pubkey, seen_at).await {
                warn!("Could not update auditor last_seen: {}", e);
            }
        }
        _ => {}
    }
}

async fn store_swap(
    db: &AuditorDatabase,
    mut swap: crate::server::storage::SwapEvent,
    failed_url: Option<String>,
) -> Result<(), AuditorError> {
    let from = db
        .ensure_mint_for_url(&normalize_mint_url(&swap.from_url))
        .await?;
    let to = db
        .ensure_mint_for_url(&normalize_mint_url(&swap.to_url))
        .await?;
    swap.from_id = from.id;
    swap.to_id = to.id;
    swap.from_url = from.url.clone();
    swap.to_url = to.url.clone();
    swap.failed_mint_id =
        failed_url
            .map(|u| normalize_mint_url(&u))
            .map(|u| if u == from.url { from.id } else { to.id });
    db.insert_swap(&swap).await.map(|_| ())
}

async fn store_check(
    db: &AuditorDatabase,
    mut check: crate::server::storage::MintCheck,
) -> Result<(), AuditorError> {
    let mint = db
        .ensure_mint_for_url(&normalize_mint_url(&check.mint_url))
        .await?;
    check.mint_id = mint.id;
    check.mint_url = mint.url;
    db.insert_peer_check(&check).await.map(|_| ())
}

/// Announced mints get a directory row (unfunded, probed hourly like any
/// other); reviews only attach to mints that are already known, so a review
/// alone cannot plant a junk entry.
async fn store_nip87(db: &AuditorDatabase, event: &Event) -> Result<(), AuditorError> {
    if let Some(mut a) = events::decode_mint_announcement(event) {
        a.mint_url = a.mint_url.as_deref().and_then(http_mint_url);
        if let Some(url) = &a.mint_url {
            db.ensure_mint_for_url(url).await?;
        }
        return db.upsert_mint_announcement(&a).await;
    }
    if let Some(mut r) = events::decode_review(event) {
        r.mint_url = r.mint_url.as_deref().and_then(http_mint_url);
        return db.upsert_review(&r).await;
    }
    Ok(())
}

fn http_mint_url(raw: &str) -> Option<String> {
    let url = normalize_mint_url(raw);
    (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
}

fn is_duplicate(error: &AuditorError) -> bool {
    error.to_string().contains("UNIQUE constraint failed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::storage::{MintCheck, SwapEvent};
    use crate::types::MintState;
    use chrono::{TimeZone, Utc};

    fn peer_swap(from: &str, to: &str, state: MintState) -> SwapEvent {
        SwapEvent {
            id: 0,
            from_id: 1,
            to_id: 2,
            from_url: from.into(),
            to_url: to.into(),
            amount: 21,
            fee: 1,
            created_at: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
            time_taken: 900.0,
            state,
            error: (state == MintState::Error).then(|| "melt failed".to_string()),
            failed_mint_id: (state == MintState::Error).then_some(2),
            stage: None,
            category: None,
            fee_reserve: Some(2),
            auditor_pubkey: String::new(),
            event_id: None,
        }
    }

    #[tokio::test]
    async fn peer_events_are_stored_once_and_strangers_are_ignored() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let peer = Keys::generate();
        let stranger = Keys::generate();
        let peers = vec![peer.public_key()];

        // A failed swap blaming the receiver, published by the peer.
        let swap = peer_swap("https://A.example/", "https://b.example", MintState::Error);
        let event = events::swap_builder(&swap).sign_with_keys(&peer).unwrap();
        handle_event(&db, &peers, &event).await;
        handle_event(&db, &peers, &event).await; // duplicate delivery

        let swaps = db.get_swaps(0, 10).await.unwrap();
        assert_eq!(swaps.len(), 1);
        let stored = &swaps[0];
        assert_eq!(stored.from_url, "https://a.example", "URL normalized");
        assert_eq!(stored.auditor_pubkey, peer.public_key().to_hex());
        assert_eq!(stored.event_id.as_deref(), Some(event.id.to_hex().as_str()));
        let b = db
            .get_mint_by_url("https://b.example")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.failed_mint_id, Some(b.id));
        assert_eq!(stored.to_id, b.id);
        assert_eq!(b.sum_donations, 0, "peer mint is unfunded here");
        assert!(
            db.unpublished_swaps(10).await.unwrap().is_empty(),
            "peer rows never enter the outbox"
        );

        // A probe from the peer, and the same probe from a stranger.
        let check = MintCheck {
            id: 0,
            mint_id: 0,
            mint_url: "https://a.example".into(),
            kind: "HEALTH".into(),
            state: MintState::Ok,
            error: None,
            category: None,
            latency_ms: Some(40.0),
            // Recent, so it falls inside the 24 h uptime window queried below.
            created_at: Utc::now() - chrono::Duration::minutes(1),
            auditor_pubkey: String::new(),
            event_id: None,
            detail: None,
        };
        handle_event(
            &db,
            &peers,
            &events::check_builder(&check).sign_with_keys(&peer).unwrap(),
        )
        .await;
        handle_event(
            &db,
            &peers,
            &events::check_builder(&check)
                .sign_with_keys(&stranger)
                .unwrap(),
        )
        .await;
        let a = db
            .get_mint_by_url("https://a.example")
            .await
            .unwrap()
            .unwrap();
        let uptime = db.get_mint_uptime(a.id).await.unwrap();
        assert_eq!(uptime.checks, 1);

        // Announcement creates the auditor row; observations bump last_seen.
        let announcement = events::Announcement {
            name: "Peer".into(),
            urls: vec!["https://b.imf.example".into()],
            donate: None,
            software: "imf 0.1.0".into(),
            frequency_secs: 600,
            contact: None,
        };
        handle_event(
            &db,
            &peers,
            &events::announcement_builder(&announcement)
                .sign_with_keys(&peer)
                .unwrap(),
        )
        .await;
        let auditors = db.get_auditors().await.unwrap();
        assert_eq!(auditors.len(), 1);
        assert_eq!(auditors[0].name, "Peer");
        assert_eq!(auditors[0].urls, vec!["https://b.imf.example".to_string()]);
        assert!(db.latest_peer_event_at().await.unwrap().is_some());

        // NIP-87 from anyone: the announcement creates a directory row, the
        // review attaches by URL, and a newer review by the same author replaces.
        let announcer = Keys::generate();
        let ann = EventBuilder::new(Kind::from_u16(events::KIND_NIP87_CASHU), r#"{"name":"C"}"#)
            .tags([
                Tag::identifier("02c0ffee"),
                Tag::custom(TagKind::custom("u"), ["https://C.example/"]),
            ])
            .sign_with_keys(&announcer)
            .unwrap();
        handle_event(&db, &peers, &ann).await;
        let c = db
            .get_mint_by_url("https://c.example")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(c.sum_donations, 0);
        let found = db
            .announcement_for_mint(&c.url, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.d, "02c0ffee");
        for (secs, text) in [
            (1_700_000_000u64, "[2/5] meh"),
            (1_700_000_100, "[5/5] fixed"),
        ] {
            let review = EventBuilder::new(Kind::from_u16(events::KIND_NIP87_REVIEW), text)
                .tags([
                    Tag::custom(TagKind::custom("k"), ["38172"]),
                    Tag::identifier("02c0ffee"),
                    Tag::custom(TagKind::custom("u"), ["https://c.example"]),
                ])
                .custom_created_at(Timestamp::from(secs))
                .sign_with_keys(&stranger)
                .unwrap();
            handle_event(&db, &peers, &review).await;
        }
        let reviews = db.reviews_for_mint(&c.url, None, 10).await.unwrap();
        assert_eq!(reviews.len(), 1);
        assert_eq!(reviews[0].rating, Some(5));
        let stats = db.directory_stats(c.id, &c.url, None).await.unwrap();
        assert_eq!(
            (stats.review_count, stats.review_avg, stats.announced),
            (1, Some(5.0), true)
        );
        let stats_b = db.directory_stats(b.id, &b.url, None).await.unwrap();
        assert_eq!(
            (stats_b.auditors_24h, stats_b.auditors_ok_24h),
            (0, 0),
            "old swap is outside 24 h"
        );

        // Metrics can be scoped: pooled, local only, or one peer.
        assert_eq!(db.get_mint_metrics(b.id, None).await.unwrap().attempts, 1);
        assert_eq!(
            db.get_mint_metrics(b.id, Some(String::new()))
                .await
                .unwrap()
                .attempts,
            0
        );
        assert_eq!(
            db.get_mint_metrics(b.id, Some(peer.public_key().to_hex()))
                .await
                .unwrap()
                .last_error
                .as_deref(),
            Some("melt failed")
        );
    }
}
