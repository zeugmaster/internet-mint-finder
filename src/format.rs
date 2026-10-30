//! Display formatting helpers shared across the UI.

use chrono::{DateTime, Utc};

/// Format a sat amount with thin thousands separators: 1234567 -> "1 234 567".
pub fn format_sats(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    let offset = s.len() % 3;
    for (i, c) in s.chars().enumerate() {
        if i != 0 && (i + 3 - offset).is_multiple_of(3) {
            out.push('\u{2009}'); // thin space
        }
        out.push(c);
    }
    out
}

/// Strip scheme and trailing slash from a mint URL for compact display.
pub fn short_url(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_string()
}

/// Format a millisecond duration: "850 ms" or "1.4 s".
pub fn format_ms(ms: f64) -> String {
    if ms >= 10_000.0 {
        format!("{:.0} s", ms / 1000.0)
    } else if ms >= 1000.0 {
        format!("{:.1} s", ms / 1000.0)
    } else {
        format!("{:.0} ms", ms)
    }
}

/// Human relative time: "just now", "4m ago", "2h ago", "3d ago".
pub fn relative_time(dt: DateTime<Utc>) -> String {
    let secs = (Utc::now() - dt).num_seconds();
    if secs < 0 {
        return "just now".to_string();
    }
    match secs {
        0..=44 => "just now".to_string(),
        45..=89 => "1m ago".to_string(),
        90..=3599 => format!("{}m ago", secs / 60),
        3600..=86_399 => format!("{}h ago", secs / 3600),
        _ => format!("{}d ago", secs / 86_400),
    }
}

/// Absolute timestamp for tooltips / detail views.
pub fn absolute_time(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S UTC").to_string()
}

/// `relative_time` for after "from": "showing data from a moment ago"
/// rather than "from just now".
pub fn relative_time_from(dt: DateTime<Utc>) -> String {
    match relative_time(dt).as_str() {
        "just now" => "a moment ago".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sats_are_grouped() {
        assert_eq!(format_sats(0), "0");
        assert_eq!(format_sats(999), "999");
        assert_eq!(format_sats(1000), "1\u{2009}000");
        assert_eq!(format_sats(1234567), "1\u{2009}234\u{2009}567");
    }

    #[test]
    fn keys_are_cut_in_the_middle() {
        let npub = "npub1vhjteu3hq8x7wq3l0vdq8m5xa2c9e6ttl2dkqj253vsm2fj8a0xqeu3h";
        assert_eq!(short_npub(npub), "npub1vhjt…eu3h");
        assert_eq!(middle_ellipsis(npub), "npub1vhjteu…qeu3h");
        assert_eq!(short_npub("npub1short"), "npub1short");
    }

    #[test]
    fn urls_are_shortened() {
        assert_eq!(short_url("https://mint.example.com/"), "mint.example.com");
        assert_eq!(short_url("http://x.y"), "x.y");
    }

    #[test]
    fn times_after_from_read_naturally() {
        assert_eq!(relative_time_from(Utc::now()), "a moment ago");
        let earlier = Utc::now() - chrono::Duration::minutes(3);
        assert_eq!(relative_time_from(earlier), "3m ago");
    }

    #[test]
    fn ms_formatting() {
        assert_eq!(format_ms(850.0), "850 ms");
        assert_eq!(format_ms(1400.0), "1.4 s");
        assert_eq!(format_ms(12_000.0), "12 s");
    }
}

/// First characters of a long identifier (pubkey, npub, payment request).
pub fn short_pubkey(id: &str) -> String {
    const KEEP: usize = 12;
    if id.chars().count() <= KEEP {
        id.to_string()
    } else {
        format!("{}…", id.chars().take(KEEP).collect::<String>())
    }
}

/// A long key cut in the middle, so both ends stay checkable:
/// "npub1qe3e5w…ezefe".
pub fn middle_ellipsis(id: &str) -> String {
    cut_middle(id, 11, 5)
}

/// A reviewer's npub, shorter than `middle_ellipsis` for a line of a list:
/// the "npub1" that says what it is and four characters either side of the
/// cut, "npub1vhjt…eu3h".
pub fn short_npub(npub: &str) -> String {
    cut_middle(npub, 9, 4)
}

fn cut_middle(id: &str, head: usize, tail: usize) -> String {
    let chars: Vec<char> = id.chars().collect();
    if chars.len() <= head + tail + 1 {
        id.to_string()
    } else {
        let start: String = chars[..head].iter().collect();
        let end: String = chars[chars.len() - tail..].iter().collect();
        format!("{start}…{end}")
    }
}
