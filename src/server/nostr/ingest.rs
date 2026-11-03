//! Resumable public-directory and peer ingestion, with a separate cursor per relay/feed.
use super::events::{self, to_datetime, Observation};
use crate::server::error::AuditorError;
use crate::server::mint_url::normalize_mint_url;
use crate::server::storage::{Auditor, AuditorDatabase};
use crate::server::workers::Workers;
use nostr_sdk::prelude::*;
use rusqlite::params;
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast, watch};
use tracing::{debug, warn};

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const PAGE: usize = 500;
const MAX_PAGES: usize = 20;
const LIVE_OVERLAP_SECS: u64 = 600;

pub async fn run(
    db: Arc<AuditorDatabase>,
    client: Arc<Client>,
    peers: Vec<PublicKey>,
    mut shutdown: watch::Receiver<bool>,
    progress: Arc<Workers>,
) {
    let mut feeds = vec![Filter::new().kinds([
        Kind::from_u16(events::KIND_NIP87_CASHU),
        Kind::from_u16(events::KIND_NIP87_REVIEW),
    ])];
    if !peers.is_empty() {
        feeds.push(
            Filter::new()
                .kinds([
                    Kind::from_u16(events::KIND_OBSERVATION),
                    Kind::from_u16(events::KIND_ANNOUNCEMENT),
                ])
                .authors(peers.iter().copied()),
        );
    }
    let mut notifications = client.notifications();
    for filter in &feeds {
        if let Err(error) = client
            .subscribe(
                filter.clone().since(Timestamp::from(
                    Timestamp::now().as_secs().saturating_sub(LIVE_OVERLAP_SECS),
                )),
                None,
            )
            .await
        {
            warn!(%error, "Could not subscribe; periodic replay will retry");
        }
    }
    let mut replay = tokio::time::interval(Duration::from_secs(60));
    replay.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        progress.beat("relay ingestion");
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = replay.tick() => {
                let mut ready = false;
                for relay in client.relays().await.into_values() {
                    if !relay.is_connected() { continue; }
                    let mut complete = true;
                    for filter in &feeds {
                        progress.beat("relay ingestion");
                        let result = tokio::select! {
                            _ = shutdown.changed() => return,
                            result = backfill_relay(&db, &relay, filter, &peers, &progress) => result,
                        };
                        match result {
                            Ok(done) => complete &= done,
                            Err(error) => { complete = false; warn!(relay = %relay.url(), %error, "Replay will resume from its last stored page"); }
                        }
                    }
                    ready |= complete;
                }
                progress.ready("relay ingestion", ready);
            }
            notification = notifications.recv() => match notification {
                Ok(RelayPoolNotification::Event { event, .. }) => handle_event(&db, &peers, &event).await,
                Ok(_) => {},
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(n, "Ingest fell behind; replaying from completed cursors");
                    progress.ready("relay ingestion", false);
                    replay.reset_immediately();
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    }
}

// The SDK fetch helper treats a timeout/disconnect like a completed partial page.
async fn fetch_page(
    relay: &Relay,
    filter: Filter,
    limit: usize,
) -> Result<Vec<Event>, AuditorError> {
    let mut notifications = relay.notifications();
    let id = SubscriptionId::generate();
    let result = tokio::time::timeout(FETCH_TIMEOUT, async {
        relay
            .subscribe_with_id(id.clone(), filter.clone(), SubscribeOptions::default())
            .await
            .map_err(|e| AuditorError::Unreachable(e.to_string()))?;
        collect_page(&mut notifications, &id, &filter, limit).await
    })
    .await
    .map_err(|_| AuditorError::Unreachable("relay page timed out before EOSE".into()));
    let _ = relay.unsubscribe(&id).await;
    result?
}

async fn collect_page(
    notifications: &mut broadcast::Receiver<RelayNotification>,
    id: &SubscriptionId,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<Event>, AuditorError> {
    let mut events = std::collections::HashMap::new();
    loop {
        let notification = notifications
            .recv()
            .await
            .map_err(|e| AuditorError::Unreachable(e.to_string()))?;
        match notification {
            RelayNotification::Message {
                message:
                    RelayMessage::Event {
                        subscription_id,
                        event,
                    },
            } if subscription_id.as_ref() == id
                && event.verify().is_ok()
                && filter.match_event(&event, MatchEventOptions::new()) =>
            {
                events.insert(event.id, event.into_owned());
                if events.len() > limit {
                    return Err(AuditorError::Unreachable("relay ignored page limit".into()));
                }
            }
            RelayNotification::Message {
                message: RelayMessage::EndOfStoredEvents(subscription_id),
            } if subscription_id.as_ref() == id => return Ok(events.into_values().collect()),
            RelayNotification::Message {
                message:
                    RelayMessage::Closed {
                        subscription_id, ..
                    },
            } if subscription_id.as_ref() == id => {
                return Err(AuditorError::Unreachable(
                    "relay closed page before EOSE".into(),
                ))
            }
            RelayNotification::RelayStatus {
                status: RelayStatus::Disconnected | RelayStatus::Terminated | RelayStatus::Banned,
            } => {
                return Err(AuditorError::Unreachable(
                    "relay disconnected before EOSE".into(),
                ))
            }
            RelayNotification::Shutdown | RelayNotification::AuthenticationFailed => {
                return Err(AuditorError::Unreachable(
                    "relay unavailable before EOSE".into(),
                ))
            }
            _ => {}
        }
    }
}

/// A scan boundary advances only after every page in that interval was persisted.
async fn scan_window(
    db: &AuditorDatabase,
    key: String,
    now: u64,
) -> Result<(u64, u64, u64), AuditorError> {
    db.run(move |c| {
        c.execute("INSERT OR IGNORE INTO ingest_progress(feed) VALUES(?1)", [&key])?;
        c.execute("UPDATE ingest_progress SET backfill_start=?2, backfill_until=?2 WHERE feed=?1 AND backfill_until IS NULL", params![key,now])?;
        Ok(c.query_row("SELECT completed_until,backfill_until,backfill_start FROM ingest_progress WHERE feed=?1", [key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)
    }).await
}

async fn save_cursor(
    db: &AuditorDatabase,
    key: String,
    until: Option<u64>,
    completed: u64,
) -> Result<(), AuditorError> {
    db.run(move |c| {
        c.execute("UPDATE ingest_progress SET backfill_until=?2, completed_until=CASE WHEN ?2 IS NULL THEN ?3 ELSE completed_until END, backfill_start=CASE WHEN ?2 IS NULL THEN NULL ELSE backfill_start END WHERE feed=?1", params![key,until,completed])?;
        Ok(())
    }).await
}

async fn backfill_relay(
    db: &AuditorDatabase,
    relay: &Relay,
    base: &Filter,
    peers: &[PublicKey],
    progress: &Workers,
) -> Result<bool, AuditorError> {
    let key = format!("{}:{}", relay.url(), base.as_json());
    let (completed, mut until, started) =
        scan_window(db, key.clone(), Timestamp::now().as_secs()).await?;
    let since = completed.saturating_sub(LIVE_OVERLAP_SECS);
    let mut limit = PAGE;
    for _ in 0..MAX_PAGES {
        progress.beat("relay ingestion");
        let filter = base
            .clone()
            .since(Timestamp::from(since))
            .until(Timestamp::from(until))
            .limit(limit);
        let events = fetch_page(relay, filter, limit).await?;
        if events.is_empty() {
            save_cursor(db, key, None, started).await?;
            return Ok(true);
        }
        let oldest = events
            .iter()
            .map(|e| e.created_at.as_secs())
            .min()
            .unwrap_or(until);
        for event in events.iter() {
            store_event(db, peers, event).await?;
        }
        if oldest >= until && events.len() >= limit {
            if limit >= 20_000 {
                return Err(AuditorError::Internal(
                    "relay pagination saturated at one timestamp; cursor retained".into(),
                ));
            }
            // shortcut: refuse more than 20,000 events in one second; use relay-specific pagination if this limit is reached.
            limit = (limit * 2).min(20_000);
            continue;
        }
        if oldest >= until {
            if oldest <= since {
                save_cursor(db, key, None, started).await?;
                return Ok(true);
            }
            until = oldest - 1;
        } else {
            until = oldest;
        }
        save_cursor(db, key.clone(), Some(until), started).await?;
    }
    Ok(false)
}

#[cfg(test)]
pub(super) async fn backfill(
    db: &AuditorDatabase,
    client: &Client,
    base: &Filter,
    peers: &[PublicKey],
    _: u64,
    _: &str,
) {
    for relay in client.relays().await.into_values() {
        backfill_relay(db, &relay, base, peers, &Workers::default())
            .await
            .unwrap();
    }
}

/// The live subscription never advances a replay cursor.
pub async fn handle_event(db: &AuditorDatabase, peers: &[PublicKey], event: &Event) {
    if let Err(error) = store_event(db, peers, event).await {
        warn!(%error, event = %event.id, "Could not persist relay event; replay will retry");
    }
}

async fn store_event(
    db: &AuditorDatabase,
    peers: &[PublicKey],
    event: &Event,
) -> Result<(), AuditorError> {
    if event.verify().is_err()
        || event.created_at.as_secs() > Timestamp::now().as_secs().saturating_add(60)
    {
        return Ok(());
    }
    let kind = event.kind.as_u16();
    if kind == events::KIND_NIP87_CASHU || kind == events::KIND_NIP87_REVIEW {
        return store_nip87(db, event).await;
    }
    if !peers.contains(&event.pubkey) {
        return Ok(());
    }
    let pubkey = event.pubkey.to_hex();
    let seen_at = to_datetime(event.created_at);
    match kind {
        events::KIND_ANNOUNCEMENT => {
            let Some(a) = events::decode_announcement(event) else {
                return Ok(());
            };
            db.upsert_auditor(&Auditor {
                pubkey,
                name: a.name,
                urls: a.urls,
                donate: a.donate,
                software: a.software,
                frequency_secs: a.frequency_secs,
                contact: a.contact,
                announced_at: Some(seen_at),
                last_seen: seen_at,
            })
            .await?;
        }
        events::KIND_OBSERVATION => {
            let result = match events::decode_observation(event) {
                Some(Observation::Swap { swap, failed_url }) => {
                    store_swap(db, swap, failed_url).await
                }
                Some(Observation::Check(check)) => store_check(db, check).await,
                None => {
                    debug!(event = %event.id, "Ignoring undecodable observation");
                    return Ok(());
                }
            };
            match result {
                Err(AuditorError::InvalidInput(_)) => return Ok(()),
                Err(error) if is_duplicate(&error) => {}
                result => result?,
            }
            db.touch_auditor(&pubkey, seen_at).await?;
        }
        _ => {}
    }
    Ok(())
}

async fn store_swap(
    db: &AuditorDatabase,
    mut swap: crate::server::storage::SwapEvent,
    failed_url: Option<String>,
) -> Result<(), AuditorError> {
    if !swap.time_taken.is_finite() || swap.time_taken < 0.0 {
        return Err(AuditorError::InvalidInput(
            "invalid peer swap latency".into(),
        ));
    }
    for url in [&swap.from_url, &swap.to_url] {
        crate::server::mint_transport::parse_url(url)
            .map_err(|e| AuditorError::InvalidInput(e.to_string()))?;
    }
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
    swap.failed_mint_id = failed_url.map(|u| normalize_mint_url(&u)).and_then(|u| {
        if u == from.url {
            Some(from.id)
        } else if u == to.url {
            Some(to.id)
        } else {
            None
        }
    });
    db.insert_swap(&swap).await.map(|_| ())
}

async fn store_check(
    db: &AuditorDatabase,
    mut check: crate::server::storage::MintCheck,
) -> Result<(), AuditorError> {
    if check.latency_ms.is_some_and(|v| !v.is_finite() || v < 0.0) {
        return Err(AuditorError::InvalidInput(
            "invalid peer probe latency".into(),
        ));
    }
    crate::server::mint_transport::parse_url(&check.mint_url)
        .map_err(|e| AuditorError::InvalidInput(e.to_string()))?;
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
    crate::server::mint_transport::parse_url(raw).ok()?;
    Some(normalize_mint_url(raw))
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
            fee_known: true,
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

#[cfg(test)]
mod robustness_tests {
    use super::*;
    use std::borrow::Cow;

    #[tokio::test]
    async fn interrupted_backfill_resumes_its_page_without_advancing_completed_time() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        assert_eq!(
            scan_window(&db, "relay-a:public".into(), 100)
                .await
                .unwrap(),
            (0, 100, 100)
        );
        save_cursor(&db, "relay-a:public".into(), Some(40), 100)
            .await
            .unwrap();
        assert_eq!(
            scan_window(&db, "relay-a:public".into(), 200)
                .await
                .unwrap(),
            (0, 40, 100)
        );
        assert_eq!(
            scan_window(&db, "relay-b:public".into(), 200)
                .await
                .unwrap(),
            (0, 200, 200)
        );
        save_cursor(&db, "relay-a:public".into(), None, 100)
            .await
            .unwrap();
        assert_eq!(
            scan_window(&db, "relay-a:public".into(), 200)
                .await
                .unwrap(),
            (100, 200, 200)
        );
    }

    #[tokio::test]
    async fn a_partial_relay_page_is_not_a_completed_replay() {
        let id = SubscriptionId::generate();
        let filter = Filter::new().kind(Kind::TextNote);
        let event = EventBuilder::text_note("a stored event")
            .sign_with_keys(&Keys::generate())
            .unwrap();
        for completed in [false, true] {
            let (tx, mut rx) = broadcast::channel(8);
            tx.send(RelayNotification::Message {
                message: RelayMessage::Event {
                    subscription_id: Cow::Owned(id.clone()),
                    event: Cow::Owned(event.clone()),
                },
            })
            .unwrap();
            if completed {
                tx.send(RelayNotification::Message {
                    message: RelayMessage::EndOfStoredEvents(Cow::Owned(id.clone())),
                })
                .unwrap();
            } else {
                tx.send(RelayNotification::RelayStatus {
                    status: RelayStatus::Disconnected,
                })
                .unwrap();
            }
            let result = collect_page(&mut rx, &id, &filter, 500).await;
            assert_eq!(result.is_ok(), completed);
            if completed {
                assert_eq!(result.unwrap().len(), 1);
            }
        }
        let (tx, mut rx) = broadcast::channel(1);
        tx.send(RelayNotification::RelayStatus {
            status: RelayStatus::Connected,
        })
        .unwrap();
        tx.send(RelayNotification::RelayStatus {
            status: RelayStatus::Connected,
        })
        .unwrap();
        assert!(
            collect_page(&mut rx, &id, &filter, 500).await.is_err(),
            "lag must leave the replay cursor unchanged"
        );
    }

    #[tokio::test]
    async fn standalone_ingests_public_mints_but_ignores_future_events() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let keys = Keys::generate();
        for (url, time, expected) in [
            ("https://real.example", Timestamp::now(), true),
            (
                "https://future.example",
                Timestamp::from(Timestamp::now().as_secs() + 3600),
                false,
            ),
        ] {
            let event = EventBuilder::new(Kind::from_u16(events::KIND_NIP87_CASHU), "")
                .tags([
                    Tag::parse(["d", "mint-key"]).unwrap(),
                    Tag::parse(["u", url]).unwrap(),
                ])
                .custom_created_at(time)
                .sign_with_keys(&keys)
                .unwrap();
            store_event(&db, &[], &event).await.unwrap();
            assert_eq!(db.get_mint_by_url(url).await.unwrap().is_some(), expected);
        }
        assert_eq!(
            scan_window(&db, "public".into(), 100).await.unwrap(),
            (0, 100, 100)
        );
    }
}
