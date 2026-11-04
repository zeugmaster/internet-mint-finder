//! Resumable announcement and review ingestion, with a separate cursor per relay.
use super::events;
use crate::server::error::AppError;
use crate::server::storage::Database;
use crate::server::workers::Workers;
use nostr_sdk::prelude::*;
use rusqlite::params;
use std::{sync::Arc, time::Duration};
use tokio::sync::{broadcast, watch};
use tracing::warn;

const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const PAGE: usize = 500;
const MAX_PAGES: usize = 20;
const LIVE_OVERLAP_SECS: u64 = 600;

pub async fn run(
    db: Arc<Database>,
    client: Arc<Client>,
    mut shutdown: watch::Receiver<bool>,
    progress: Arc<Workers>,
) {
    let feeds = [Filter::new().kinds([
        Kind::from_u16(events::KIND_CASHU),
        Kind::from_u16(events::KIND_REVIEW),
    ])];
    let mut notifications = client.notifications();
    for filter in &feeds {
        if let Err(error) = client.subscribe(filter.clone().limit(0), None).await {
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
                            result = backfill_relay(&db, &relay, filter, &progress) => result,
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
                Ok(RelayPoolNotification::Event { event, .. }) => handle_event(&db, &event).await,
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
async fn fetch_page(relay: &Relay, filter: Filter, limit: usize) -> Result<Vec<Event>, AppError> {
    let mut notifications = relay.notifications();
    let id = SubscriptionId::generate();
    let result = tokio::time::timeout(FETCH_TIMEOUT, async {
        relay
            .subscribe_with_id(id.clone(), filter.clone(), SubscribeOptions::default())
            .await
            .map_err(|e| AppError::Unreachable(e.to_string()))?;
        collect_page(&mut notifications, &id, &filter, limit).await
    })
    .await
    .map_err(|_| AppError::Unreachable("relay page timed out before EOSE".into()));
    let _ = relay.unsubscribe(&id).await;
    result?
}

async fn collect_page(
    notifications: &mut broadcast::Receiver<RelayNotification>,
    id: &SubscriptionId,
    filter: &Filter,
    limit: usize,
) -> Result<Vec<Event>, AppError> {
    let mut events = std::collections::HashMap::new();
    loop {
        let notification = notifications
            .recv()
            .await
            .map_err(|e| AppError::Unreachable(e.to_string()))?;
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
                    return Err(AppError::Unreachable("relay ignored page limit".into()));
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
                return Err(AppError::Unreachable(
                    "relay closed page before EOSE".into(),
                ))
            }
            RelayNotification::RelayStatus {
                status: RelayStatus::Disconnected | RelayStatus::Terminated | RelayStatus::Banned,
            } => {
                return Err(AppError::Unreachable(
                    "relay disconnected before EOSE".into(),
                ))
            }
            RelayNotification::Shutdown | RelayNotification::AuthenticationFailed => {
                return Err(AppError::Unreachable(
                    "relay unavailable before EOSE".into(),
                ))
            }
            _ => {}
        }
    }
}

/// A scan boundary advances only after every page in that interval was persisted.
async fn scan_window(db: &Database, key: String, now: u64) -> Result<(u64, u64, u64), AppError> {
    db.run(move |c| {
        c.execute("INSERT OR IGNORE INTO ingest_progress(feed) VALUES(?1)", [&key])?;
        // Late historical events can be older than an incremental timestamp cursor.
        c.execute("UPDATE ingest_progress SET completed_until=0,last_full_scan=?2 WHERE feed=?1 AND backfill_until IS NULL AND (last_full_scan=0 OR last_full_scan+21600<=?2)", params![key,now])?;
        c.execute("UPDATE ingest_progress SET backfill_start=?2, backfill_until=?2 WHERE feed=?1 AND backfill_until IS NULL", params![key,now])?;
        Ok(c.query_row("SELECT completed_until,backfill_until,backfill_start FROM ingest_progress WHERE feed=?1", [key], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)
    }).await
}

async fn save_cursor(
    db: &Database,
    key: String,
    until: Option<u64>,
    completed: u64,
) -> Result<(), AppError> {
    db.run(move |c| {
        c.execute("UPDATE ingest_progress SET backfill_until=?2, completed_until=CASE WHEN ?2 IS NULL THEN ?3 ELSE completed_until END, backfill_start=CASE WHEN ?2 IS NULL THEN NULL ELSE backfill_start END WHERE feed=?1", params![key,until,completed])?;
        Ok(())
    }).await
}

async fn backfill_relay(
    db: &Database,
    relay: &Relay,
    base: &Filter,
    progress: &Workers,
) -> Result<bool, AppError> {
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
            store_event(db, event).await?;
        }
        if oldest >= until && events.len() >= limit {
            if limit >= 20_000 {
                return Err(AppError::Internal(
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

/// A live event never advances the replay cursor.
async fn handle_event(db: &Database, event: &Event) {
    if let Err(error) = store_event(db, event).await {
        warn!(%error, event = %event.id, "Could not persist relay event; replay will retry");
    }
}
async fn store_event(db: &Database, event: &Event) -> Result<(), AppError> {
    if let Some(decoded) = events::decode(event) {
        db.store(event, decoded, false).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;
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
    async fn replay_cursors_are_per_relay_and_resume_after_failure() {
        let db = Database::open_in_memory().unwrap();
        assert_eq!(
            scan_window(&db, "a".into(), 100).await.unwrap(),
            (0, 100, 100)
        );
        save_cursor(&db, "a".into(), Some(40), 100).await.unwrap();
        assert_eq!(
            scan_window(&db, "a".into(), 200).await.unwrap(),
            (0, 40, 100)
        );
        assert_eq!(
            scan_window(&db, "b".into(), 200).await.unwrap(),
            (0, 200, 200)
        );
        save_cursor(&db, "a".into(), None, 100).await.unwrap();
        assert_eq!(
            scan_window(&db, "a".into(), 200).await.unwrap(),
            (100, 200, 200)
        );
        // A long-running replay must finish before a historical rescan starts.
        assert_eq!(
            scan_window(&db, "a".into(), 22_000).await.unwrap(),
            (100, 200, 200)
        );
        save_cursor(&db, "a".into(), None, 200).await.unwrap();
        assert_eq!(
            scan_window(&db, "a".into(), 22_000).await.unwrap(),
            (0, 22_000, 22_000)
        );
    }
    #[tokio::test]
    #[ignore = "requires permission to open loopback sockets"]
    async fn backfill_reads_multiple_pages_and_replays_without_double_counting() {
        use nostr_relay_builder::prelude::{LocalRelay, RelayBuilder};
        // Fail immediately if loopback is forbidden; MockRelay's port search spins in that case.
        let listener = std::net::TcpListener::bind("127.0.0.1:0")
            .expect("loopback listener permission required");
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let relay_db = Arc::new(MemoryDatabase::with_opts(MemoryDatabaseOptions {
            events: true,
            max_events: Some(2000),
        }));
        let local_relay = LocalRelay::new(
            RelayBuilder::default()
                .port(port)
                .database(relay_db.clone()),
        );
        local_relay.run().await.unwrap();
        let url = local_relay.url().await;
        let client = Client::builder().build();
        client.add_relay(url.clone()).await.unwrap();
        client.connect().await;
        client.wait_for_connection(Duration::from_secs(5)).await;
        assert!(
            client.relay(url.clone()).await.unwrap().is_connected(),
            "loopback connection permission required"
        );
        let keys = Keys::generate();
        for n in 0..(PAGE + 1) {
            let url = format!("https://mint-{n}.example");
            let event = events::tests::event(
                &keys,
                38000,
                &[&["d", &url]],
                "[4/5] Works",
                Timestamp::now().as_secs() - 100,
            );
            relay_db.save_event(&event).await.unwrap();
        }

        let db = Database::open_in_memory().unwrap();
        let filter = Filter::new().kind(Kind::from_u16(38000));
        let relay = client.relay(url).await.unwrap();
        for _ in 0..2 {
            assert!(backfill_relay(&db, &relay, &filter, &Workers::default())
                .await
                .unwrap());
            assert_eq!(
                db.search_mints(String::new(), crate::types::MintSort::Name, 0)
                    .await
                    .unwrap()
                    .total,
                (PAGE + 1) as u64
            );
        }
        client.disconnect().await;
    }
}
