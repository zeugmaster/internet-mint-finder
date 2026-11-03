//! Encoding of auditor rows as Nostr events and back.
//!
//! Pure functions, no I/O. An event's `created_at` is the observation time of
//! the row, so re-signing the same row yields the same event id and relays
//! de-duplicate a retried publish.

use crate::server::storage::{
    state_to_str, str_to_state, MintAnnouncement, MintCheck, Review, SwapEvent,
};
use crate::types::{MintMetrics, MintState};
use chrono::{DateTime, TimeZone, Utc};
use nostr_sdk::prelude::*;
use std::borrow::Cow;
use std::collections::BTreeMap;

pub const KIND_ANNOUNCEMENT: u16 = 10172;
pub const KIND_STATUS: u16 = 30172;
pub const KIND_OBSERVATION: u16 = 1172;
/// NIP-87 Cashu mint announcement and recommendation (review).
pub const KIND_NIP87_CASHU: u16 = 38172;
pub const KIND_NIP87_REVIEW: u16 = 38000;

/// Tags shared by every observation type. On a check, every other tag is a
/// `detail` field (and `detail` values are always strings on the wire).
const COMMON_TAGS: &[&str] = &["t", "u", "state", "latency_ms", "stage", "category"];

fn tag<'a>(name: impl Into<Cow<'a, str>>, value: impl Into<String>) -> Tag {
    Tag::custom(TagKind::Custom(name.into()), [value.into()])
}

fn value<'a>(event: &'a Event, name: &'static str) -> Option<&'a str> {
    event.tags.find(TagKind::from(name)).and_then(Tag::content)
}

fn ts(dt: &DateTime<Utc>) -> Timestamp {
    Timestamp::from(dt.timestamp().max(0) as u64)
}

pub fn to_datetime(ts: Timestamp) -> DateTime<Utc> {
    Utc.timestamp_opt(ts.as_secs() as i64, 0)
        .single()
        .unwrap_or_else(Utc::now)
}

// ---------------------------------------------------------------------------
// Observations (kind 1172)
// ---------------------------------------------------------------------------

pub fn swap_builder(swap: &SwapEvent) -> EventBuilder {
    let mut tags = vec![
        tag("t", "swap"),
        tag("u", swap.from_url.clone()),
        tag("u", swap.to_url.clone()),
        tag("from", swap.from_url.clone()),
        tag("to", swap.to_url.clone()),
        tag("amount", swap.amount.to_string()),
        tag("fee", swap.fee.to_string()),
        tag("state", state_to_str(&swap.state)),
        tag("latency_ms", swap.time_taken.to_string()),
    ];
    if let Some(reserve) = swap.fee_reserve {
        tags.push(tag("fee_reserve", reserve.to_string()));
    }
    if let Some(stage) = &swap.stage {
        tags.push(tag("stage", stage.clone()));
    }
    if let Some(category) = &swap.category {
        tags.push(tag("category", category.clone()));
    }
    if let Some(failed) = swap.failed_mint_id {
        let url = if failed == swap.from_id {
            &swap.from_url
        } else {
            &swap.to_url
        };
        tags.push(tag("failed", url.clone()));
    }
    EventBuilder::new(
        Kind::from_u16(KIND_OBSERVATION),
        swap.error.clone().unwrap_or_default(),
    )
    .tags(tags)
    .custom_created_at(ts(&swap.created_at))
}

pub fn check_builder(check: &MintCheck) -> EventBuilder {
    let kind = if check.kind.eq_ignore_ascii_case("HEALTH") {
        "probe".to_string()
    } else {
        check.kind.to_lowercase()
    };
    let mut tags = vec![
        tag("t", kind),
        tag("u", check.mint_url.clone()),
        tag("state", state_to_str(&check.state)),
    ];
    if let Some(latency) = check.latency_ms {
        tags.push(tag("latency_ms", latency.to_string()));
    }
    if let Some(category) = &check.category {
        tags.push(tag("category", category.clone()));
    }
    let detail: Option<BTreeMap<String, serde_json::Value>> = check
        .detail
        .as_deref()
        .and_then(|d| serde_json::from_str(d).ok());
    for (key, val) in detail.unwrap_or_default() {
        if COMMON_TAGS.contains(&key.as_str()) {
            continue;
        }
        let text = match val {
            serde_json::Value::String(s) => s,
            other => other.to_string(),
        };
        tags.push(tag(key, text));
    }
    EventBuilder::new(
        Kind::from_u16(KIND_OBSERVATION),
        check.error.clone().unwrap_or_default(),
    )
    .tags(tags)
    .custom_created_at(ts(&check.created_at))
}

/// A decoded kind-1172 event. Mint ids are unknown on the wire: the ingester
/// resolves URLs to local rows (`failed_url` for the blamed mint of a swap).
#[derive(Debug, Clone)]
pub enum Observation {
    Swap {
        swap: SwapEvent,
        failed_url: Option<String>,
    },
    Check(MintCheck),
}

pub fn decode_observation(event: &Event) -> Option<Observation> {
    if event.kind.as_u16() != KIND_OBSERVATION {
        return None;
    }
    let state = str_to_state(value(event, "state")?);
    let auditor_pubkey = event.pubkey.to_hex();
    let event_id = Some(event.id.to_hex());
    let created_at = to_datetime(event.created_at);
    let error = (!event.content.is_empty()).then(|| event.content.clone());
    let category = value(event, "category").map(str::to_string);
    let latency = value(event, "latency_ms").and_then(|v| v.parse::<f64>().ok());
    let kind = value(event, "t")?;

    if kind == "swap" {
        let swap = SwapEvent {
            id: 0,
            from_id: 0,
            to_id: 0,
            from_url: value(event, "from")?.to_string(),
            to_url: value(event, "to")?.to_string(),
            amount: value(event, "amount")?.parse().ok()?,
            fee: value(event, "fee")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0),
            created_at,
            time_taken: latency.unwrap_or(0.0),
            state,
            error,
            failed_mint_id: None,
            stage: value(event, "stage").map(str::to_string),
            category,
            fee_reserve: value(event, "fee_reserve").and_then(|v| v.parse().ok()),
            auditor_pubkey,
            event_id,
        };
        return Some(Observation::Swap {
            swap,
            failed_url: value(event, "failed").map(str::to_string),
        });
    }

    let mut detail = serde_json::Map::new();
    for t in event.tags.iter() {
        let name = t.kind().to_string();
        if COMMON_TAGS.contains(&name.as_str()) {
            continue;
        }
        if let Some(v) = t.content() {
            detail.insert(name, serde_json::Value::String(v.to_string()));
        }
    }
    Some(Observation::Check(MintCheck {
        id: 0,
        mint_id: 0,
        mint_url: value(event, "u")?.to_string(),
        kind: if kind == "probe" {
            "HEALTH".to_string()
        } else {
            kind.to_uppercase()
        },
        state,
        error,
        category,
        latency_ms: latency,
        created_at,
        auditor_pubkey,
        event_id,
        detail: (!detail.is_empty()).then(|| serde_json::Value::Object(detail).to_string()),
    }))
}

// ---------------------------------------------------------------------------
// Mint status (kind 30172) and auditor announcement (kind 10172)
// ---------------------------------------------------------------------------

pub fn status_builder(
    url: &str,
    mint_pubkey: Option<&str>,
    state: MintState,
    metrics: &MintMetrics,
) -> EventBuilder {
    let mut tags = vec![
        Tag::identifier(url),
        tag("u", url),
        tag("state", state_to_str(&state)),
    ];
    if let Some(pubkey) = mint_pubkey {
        tags.push(tag("pubkey", pubkey));
    }
    if let Some(rate) = metrics.success_rate_24h() {
        tags.push(tag("success_rate_24h", format!("{rate:.3}")));
    }
    if let Some(at) = metrics.last_success_at {
        tags.push(tag("last_ok", at.timestamp().to_string()));
    }
    if let Some(at) = metrics.last_error_at {
        tags.push(tag("last_error_at", at.timestamp().to_string()));
    }
    if let Some(category) = &metrics.last_error_category {
        tags.push(tag("category", category.clone()));
    }
    EventBuilder::new(Kind::from_u16(KIND_STATUS), "").tags(tags)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Announcement {
    pub name: String,
    pub urls: Vec<String>,
    pub donate: Option<String>,
    pub software: String,
    pub frequency_secs: u64,
    pub contact: Option<String>,
}

pub fn announcement_builder(a: &Announcement) -> EventBuilder {
    let mut tags = vec![tag("name", a.name.clone())];
    tags.extend(a.urls.iter().map(|u| tag("url", u.clone())));
    if let Some(donate) = &a.donate {
        tags.push(tag("donate", donate.clone()));
    }
    tags.push(tag("software", a.software.clone()));
    tags.push(tag("frequency", a.frequency_secs.to_string()));
    if let Some(contact) = &a.contact {
        tags.push(tag("contact", contact.clone()));
    }
    EventBuilder::new(Kind::from_u16(KIND_ANNOUNCEMENT), "").tags(tags)
}

pub fn decode_announcement(event: &Event) -> Option<Announcement> {
    if event.kind.as_u16() != KIND_ANNOUNCEMENT {
        return None;
    }
    Some(Announcement {
        name: value(event, "name").unwrap_or_default().to_string(),
        urls: event
            .tags
            .filter(TagKind::from("url"))
            .filter_map(Tag::content)
            .map(str::to_string)
            .collect(),
        donate: value(event, "donate").map(str::to_string),
        software: value(event, "software").unwrap_or_default().to_string(),
        frequency_secs: value(event, "frequency")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0),
        contact: value(event, "contact").map(str::to_string),
    })
}

// ---------------------------------------------------------------------------
// NIP-87 (kinds 38172 / 38000), read-only: we never publish these.
// ---------------------------------------------------------------------------

pub fn decode_mint_announcement(event: &Event) -> Option<MintAnnouncement> {
    if event.kind.as_u16() != KIND_NIP87_CASHU {
        return None;
    }
    Some(MintAnnouncement {
        pubkey: event.pubkey.to_hex(),
        d: event.tags.identifier()?.to_string(),
        event_id: event.id.to_hex(),
        mint_url: value(event, "u").map(str::to_string),
        nuts: value(event, "nuts").map(str::to_string),
        network: value(event, "n").map(str::to_string),
        content: event.content.clone(),
        created_at: to_datetime(event.created_at),
    })
}

/// Only Cashu reviews (`k` = 38172); Fedimint ones are out of scope. A
/// review dated more than ten minutes ahead is refused, as relays do
/// (NIP-22): it would sort above every real one for good.
pub fn decode_review(event: &Event) -> Option<Review> {
    if event.kind.as_u16() != KIND_NIP87_REVIEW || value(event, "k")? != "38172" {
        return None;
    }
    if to_datetime(event.created_at) > Utc::now() + chrono::Duration::minutes(10) {
        return None;
    }
    let (rating, content) = parse_rating(&event.content);
    Some(Review {
        pubkey: event.pubkey.to_hex(),
        d: event.tags.identifier()?.to_string(),
        event_id: event.id.to_hex(),
        mint_url: value(event, "u").map(str::to_string),
        rating,
        content,
        created_at: to_datetime(event.created_at),
    })
}

/// `[4/5] text` → `(Some(4), "text")`; anything else keeps the content as is.
pub fn parse_rating(content: &str) -> (Option<u8>, String) {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            if let Some((n, d)) = rest[..end].split_once('/') {
                if d.trim() == "5" {
                    if let Ok(n) = n.trim().parse::<u8>() {
                        if n <= 5 {
                            return (Some(n), rest[end + 1..].trim().to_string());
                        }
                    }
                }
            }
        }
    }
    (None, trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nip87_events_decode_and_ratings_parse() {
        let keys = Keys::generate();
        let announcement =
            EventBuilder::new(Kind::from_u16(KIND_NIP87_CASHU), r#"{"name":"Mint A"}"#)
                .tags([
                    Tag::identifier("02abc"),
                    tag("u", "https://a.example/"),
                    tag("nuts", "1,2,3"),
                    tag("n", "mainnet"),
                ])
                .sign_with_keys(&keys)
                .unwrap();
        let decoded = decode_mint_announcement(&announcement).unwrap();
        assert_eq!(decoded.d, "02abc");
        assert_eq!(decoded.mint_url.as_deref(), Some("https://a.example/"));
        assert_eq!(decoded.network.as_deref(), Some("mainnet"));
        assert_eq!(decoded.nuts.as_deref(), Some("1,2,3"));

        let review = EventBuilder::new(Kind::from_u16(KIND_NIP87_REVIEW), "[4/5] fast and cheap")
            .tags([
                tag("k", "38172"),
                Tag::identifier("02abc"),
                tag("u", "https://a.example"),
            ])
            .sign_with_keys(&keys)
            .unwrap();
        let decoded = decode_review(&review).unwrap();
        assert_eq!(decoded.rating, Some(4));
        assert_eq!(decoded.content, "fast and cheap");
        assert_eq!(decoded.d, "02abc");

        let fedimint = EventBuilder::new(Kind::from_u16(KIND_NIP87_REVIEW), "[5/5] x")
            .tags([tag("k", "38173"), Tag::identifier("fed1")])
            .sign_with_keys(&keys)
            .unwrap();
        assert!(decode_review(&fedimint).is_none());

        let ahead = EventBuilder::new(Kind::from_u16(KIND_NIP87_REVIEW), "[5/5] from the future")
            .tags([tag("k", "38172"), Tag::identifier("02abc")])
            .custom_created_at(Timestamp::from_secs(Timestamp::now().as_secs() + 3600))
            .sign_with_keys(&keys)
            .unwrap();
        assert!(decode_review(&ahead).is_none());

        assert_eq!(
            parse_rating("[6/5] too high"),
            (None, "[6/5] too high".into())
        );
        assert_eq!(parse_rating("  [0/5]  "), (Some(0), String::new()));
        assert_eq!(
            parse_rating("no rating here"),
            (None, "no rating here".into())
        );
    }

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    fn sample_swap() -> SwapEvent {
        SwapEvent {
            id: 7,
            from_id: 1,
            to_id: 2,
            from_url: "https://a.example".into(),
            to_url: "https://b.example".into(),
            amount: 42,
            fee: 1,
            created_at: at(1_700_000_000),
            time_taken: 1234.5,
            state: MintState::Error,
            error: Some("paying invoice — timeout".into()),
            failed_mint_id: Some(2),
            stage: Some("paying invoice".into()),
            category: Some("timeout".into()),
            fee_reserve: Some(3),
            auditor_pubkey: String::new(),
            event_id: None,
        }
    }

    #[test]
    fn swap_round_trips_and_is_deterministic() {
        let keys = Keys::generate();
        let swap = sample_swap();
        let event = swap_builder(&swap).sign_with_keys(&keys).unwrap();
        let again = swap_builder(&swap).sign_with_keys(&keys).unwrap();
        assert_eq!(event.id, again.id, "same row must give the same event id");
        assert_eq!(event.created_at.as_secs(), 1_700_000_000);

        let Some(Observation::Swap {
            swap: decoded,
            failed_url,
        }) = decode_observation(&event)
        else {
            panic!("expected a swap");
        };
        assert_eq!(failed_url.as_deref(), Some("https://b.example"));
        assert_eq!(decoded.from_url, swap.from_url);
        assert_eq!(decoded.to_url, swap.to_url);
        assert_eq!(decoded.amount, 42);
        assert_eq!(decoded.fee, 1);
        assert_eq!(decoded.fee_reserve, Some(3));
        assert_eq!(decoded.time_taken, 1234.5);
        assert_eq!(decoded.state, MintState::Error);
        assert_eq!(decoded.error, swap.error);
        assert_eq!(decoded.stage, swap.stage);
        assert_eq!(decoded.category, swap.category);
        assert_eq!(decoded.created_at, swap.created_at);
        assert_eq!(decoded.auditor_pubkey, keys.public_key().to_hex());
        assert_eq!(decoded.event_id, Some(event.id.to_hex()));
    }

    #[test]
    fn check_round_trips_with_detail() {
        let keys = Keys::generate();
        let check = MintCheck {
            id: 1,
            mint_id: 3,
            mint_url: "https://c.example".into(),
            kind: "CONNECTIVITY".into(),
            state: MintState::Ok,
            error: None,
            category: None,
            latency_ms: Some(88.0),
            created_at: at(1_700_000_100),
            auditor_pubkey: String::new(),
            event_id: None,
            detail: Some(r#"{"transport":"tor","ws":"true"}"#.into()),
        };
        let event = check_builder(&check).sign_with_keys(&keys).unwrap();
        assert_eq!(value(&event, "t"), Some("connectivity"));
        let Some(Observation::Check(decoded)) = decode_observation(&event) else {
            panic!("expected a check");
        };
        assert_eq!(decoded.kind, "CONNECTIVITY");
        assert_eq!(decoded.mint_url, check.mint_url);
        assert_eq!(decoded.latency_ms, Some(88.0));
        assert_eq!(decoded.error, None);
        let detail: serde_json::Value = serde_json::from_str(&decoded.detail.unwrap()).unwrap();
        assert_eq!(detail["transport"], "tor");
        assert_eq!(detail["ws"], "true");

        let mut health = check.clone();
        health.kind = "HEALTH".into();
        health.detail = None;
        health.error = Some("503".into());
        let event = check_builder(&health).sign_with_keys(&keys).unwrap();
        assert_eq!(value(&event, "t"), Some("probe"));
        let Some(Observation::Check(decoded)) = decode_observation(&event) else {
            panic!("expected a check");
        };
        assert_eq!(decoded.kind, "HEALTH");
        assert_eq!(decoded.detail, None);
        assert_eq!(decoded.error.as_deref(), Some("503"));
    }

    #[test]
    fn status_and_announcement_carry_their_keys() {
        let keys = Keys::generate();
        let metrics = MintMetrics {
            attempts_24h: 10,
            successes_24h: 9,
            last_success_at: Some(at(1_700_000_000)),
            ..Default::default()
        };
        let event = status_builder("https://a.example", Some("02ab"), MintState::Ok, &metrics)
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(event.kind.as_u16(), KIND_STATUS);
        assert_eq!(event.tags.identifier(), Some("https://a.example"));
        assert_eq!(value(&event, "success_rate_24h"), Some("0.900"));
        assert_eq!(value(&event, "last_ok"), Some("1700000000"));
        assert_eq!(value(&event, "pubkey"), Some("02ab"));

        let announcement = Announcement {
            name: "IMF A".into(),
            urls: vec!["https://a.imf.example".into(), "http://x.onion".into()],
            donate: Some("creqb1...".into()),
            software: "imf 0.1.0".into(),
            frequency_secs: 600,
            contact: None,
        };
        let event = announcement_builder(&announcement)
            .sign_with_keys(&keys)
            .unwrap();
        assert_eq!(event.kind.as_u16(), KIND_ANNOUNCEMENT);
        assert_eq!(decode_announcement(&event), Some(announcement));
        assert!(decode_observation(&event).is_none());
    }
}
