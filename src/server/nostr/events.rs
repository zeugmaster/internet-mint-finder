//! NIP-87 decoding. Ratings are a client convention, not a separate event kind.
use crate::server::mint_url::normalize_mint_url;
use nostr_sdk::prelude::*;

pub const KIND_CASHU: u16 = 38172;
pub const KIND_REVIEW: u16 = 38000;
pub const MAX_EVENT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub url: String,
    pub identifier: String,
}

#[derive(Debug)]
pub struct Decoded {
    pub d: String,
    pub targets: Vec<Target>,
    pub rating: Option<u8>,
    pub content: String,
}

fn values<'a>(event: &'a Event, name: &'a str) -> impl Iterator<Item = &'a str> {
    event.tags.iter().filter_map(move |t| {
        let t = t.as_slice();
        (t.first()?.as_str() == name)
            .then(|| t.get(1).map(String::as_str))
            .flatten()
    })
}

pub fn mint_pubkey(value: &str) -> bool {
    // NUT-06 uses compressed secp256k1 keys; some older clients use x-only keys.
    (value.len() == 64
        || (value.len() == 66 && (value.starts_with("02") || value.starts_with("03"))))
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn decode(event: &Event) -> Option<Decoded> {
    if !matches!(event.kind.as_u16(), KIND_CASHU | KIND_REVIEW)
        || event.as_json().len() > MAX_EVENT_BYTES
        || event.created_at.as_secs() > Timestamp::now().as_secs().saturating_add(60)
        || event.verify().is_err()
    {
        return None;
    }
    let announcement = event.kind.as_u16() == KIND_CASHU;
    let d = event.tags.identifier().unwrap_or_default().to_string();
    let kinds: Vec<_> = values(event, "k").collect();
    if !announcement && !kinds.is_empty() && !kinds.contains(&"38172") {
        return None;
    }
    let mut targets = Vec::new();
    // Typed URLs from early clients may mix Cashu and Fedimint recommendations.
    for tag in event.tags.iter() {
        let t = tag.as_slice();
        if t.first().map(String::as_str) != Some("u") || t.get(2).is_some_and(|v| v == "fedimint") {
            continue;
        }
        if let Some(url) = t.get(1).and_then(|u| normalize_mint_url(u)) {
            targets.push(Target {
                url,
                identifier: String::new(),
            });
        }
    }
    for address in values(event, "a") {
        let mut parts = address.splitn(3, ':');
        if parts.next() != Some("38172") {
            continue;
        }
        if parts
            .next()
            .and_then(|p| PublicKey::from_hex(p).ok())
            .is_none()
        {
            continue;
        }
        if let Some(id) = parts.next().filter(|id| !id.is_empty() && id.len() <= 2048) {
            targets.push(Target {
                url: normalize_mint_url(id).unwrap_or_default(),
                identifier: address.to_string(),
            });
        }
    }
    // Legacy d=URL; current d=mint pubkey. A generic id alone is not a Cashu reference.
    if let Some(url) = normalize_mint_url(&d) {
        targets.push(Target {
            url,
            identifier: String::new(),
        });
    } else if !d.is_empty()
        && d.len() <= 2048
        && (announcement || kinds.contains(&"38172") || mint_pubkey(&d))
    {
        targets.push(Target {
            url: String::new(),
            identifier: d.clone(),
        });
    }
    targets.sort_by(|a, b| (&a.url, &a.identifier).cmp(&(&b.url, &b.identifier)));
    targets.dedup();
    if targets.is_empty() {
        return None;
    }
    if announcement && (d.is_empty() || !targets.iter().any(|t| !t.url.is_empty())) {
        return None;
    }
    let (rating, content) = if announcement {
        (None, event.content.clone())
    } else {
        parse_rating(&event.content)
    };
    Some(Decoded {
        d,
        targets,
        rating,
        content,
    })
}

pub fn parse_rating(content: &str) -> (Option<u8>, String) {
    let trimmed = content.trim();
    if let Some(rest) = trimmed.strip_prefix('[') {
        if let Some((prefix, text)) = rest.split_once(']') {
            if let Some((n, scale)) = prefix.split_once('/') {
                if scale.trim() == "5" {
                    if let Ok(n @ 0..=5) = n.trim().parse::<u8>() {
                        return (Some(n), text.trim().to_string());
                    }
                }
            }
        }
    }
    (None, trimmed.to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub fn event(keys: &Keys, kind: u16, tags: &[&[&str]], content: &str, at: u64) -> Event {
        EventBuilder::new(Kind::from_u16(kind), content)
            .tags(tags.iter().map(|t| Tag::parse(t.iter().copied()).unwrap()))
            .custom_created_at(Timestamp::from(at))
            .sign_with_keys(keys)
            .unwrap()
    }
    #[test]
    fn current_legacy_and_mixed_references() {
        let keys = Keys::generate();
        for tags in [
            vec![
                &["k", "38172"][..],
                &["d", "02abc"],
                &["u", "https://mint.example/"],
            ],
            vec![&["d", "https://mint.example"][..]],
            vec![
                &["u", "fed1abc", "fedimint"][..],
                &["u", "https://mint.example", "cashu"],
            ],
        ] {
            let decoded = decode(&event(&keys, KIND_REVIEW, &tags, "[4/5] Works", 100)).unwrap();
            assert_eq!(decoded.rating, Some(4));
            assert_eq!(decoded.content, "Works");
            assert!(decoded
                .targets
                .iter()
                .any(|t| t.url == "https://mint.example"));
        }
        let address = format!("38172:{}:mint-key", keys.public_key());
        let review = event(&keys, KIND_REVIEW, &[&["a", &address]], "Helpful", 100);
        assert_eq!(decode(&review).unwrap().targets[0].identifier, address);
        for (kind, tags) in [
            (1, vec![&["u", "https://mint.example"][..]]),
            (
                KIND_REVIEW,
                vec![&["k", "38173"][..], &["u", "https://mint.example"]],
            ),
            (KIND_REVIEW, vec![&["d", "unrelated"][..]]),
            (KIND_REVIEW, vec![&["u", "javascript:alert(1)"][..]]),
        ] {
            assert!(decode(&event(&keys, kind, &tags, "[5/5] Bad", 100)).is_none());
        }
        let mut forged = review.clone();
        forged.content = "Changed".into();
        assert!(decode(&forged).is_none());
        let future = event(
            &keys,
            KIND_REVIEW,
            &[&["u", "https://mint.example"]],
            "hi",
            Timestamp::now().as_secs() + 120,
        );
        assert!(decode(&future).is_none());
        assert_eq!(parse_rating(" [0/5] "), (Some(0), String::new()));
        assert_eq!(parse_rating("[6/5] bad"), (None, "[6/5] bad".into()));
    }
}
