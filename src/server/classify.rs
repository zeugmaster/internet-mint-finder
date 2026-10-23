//! Error string classifier.
//!
//! Compresses free-form error strings from swap events into short, groupable
//! labels suitable for bar charts and aggregations. Ported from the Python
//! scraper's `errors.py`.

use once_cell::sync::Lazy;
use regex::Regex;

struct Pattern {
    re: Regex,
    label: &'static str,
}

static PATTERNS: Lazy<Vec<Pattern>> = Lazy::new(|| {
    vec![
        // Proxy / Tor
        p(
            r"(?i)proxy server could not connect.*socks",
            "Proxy: SOCKS failure",
        ),
        p(
            r"(?i)proxy server could not connect.*host unreachable",
            "Proxy: host unreachable",
        ),
        p(r"(?i)proxy server could not connect", "Proxy: other"),
        // Lightning / mint payment outcomes
        p(r"(?i)insufficient[_ ]balance", "Mint: insufficient balance"),
        p(r"(?i)no[_ ]route", "Mint: no route"),
        p(r"(?i)proofs are pending", "Mint: proofs pending"),
        p(r"(?i)token pending", "Mint: token pending"),
        // Mint quote lifecycle (CDK wallet)
        p(
            r"(?i)mint amount rejected|amount must be between",
            "Mint: amount outside limits",
        ),
        p(r"(?i)NoneType.*has no attribute", "Mint: backend exception"),
        p(
            r"(?i)minting.*disabled|melting.*disabled",
            "Mint: operation disabled",
        ),
        p(r"(?i)amount undefined", "Mint: amount undefined"),
        p(r"(?i)unknown quote", "Mint: unknown quote"),
        p(
            r"(?i)never became mintable|quote .*unpaid|not been paid",
            "Mint: quote unpaid",
        ),
        p(
            r"(?i)(lightning )?payment (failed|unsuccessful)",
            "Mint: payment failed",
        ),
        p(r"(?i)payment has already failed", "Mint: payment failed"),
        // Mint operational
        p(r"(?i)database or disk is full", "Mint: db full"),
        p(r"(?i)validation errors? for", "Mint: validation error"),
        p(r"(?i)expecting value:.*char 0", "Mint: empty response"),
        p(r"(?i)does not support endpoint", "Mint: endpoint missing"),
        // Transport-layer errors
        p(
            r"(?i)certificate.*(expired|verify failed)",
            "SSL: cert expired",
        ),
        p(r"(?i)\[ssl: ", "SSL: other"),
        p(r"(?i)server error '5\d{2}", "HTTP 5xx"),
        p(
            r"(?i)\b429\b|too many requests|rate limit",
            "HTTP 429: rate limited",
        ),
        p(r"(?i)(server|client) error '4\d{2}", "HTTP 4xx"),
        p(r"(?i)server disconnected", "Server disconnected"),
        // Timeouts (check before generic connection errors so "connection timed
        // out" and our own "did not respond within" both land here).
        p(
            r"(?i)did not respond within|(read|connect|request) timeout|timed? ?out|mint timeout|deadline",
            "Timeout",
        ),
        p(
            r"(?i)connection (reset|refused|aborted|closed)|mint unreachable|no route to host|network is unreachable|error trying to connect|dns error|failed to lookup|broken pipe",
            "Connection error",
        ),
        p(r"(?i)fee reserve of .* too high", "Fee too high"),
        // CDK / wallet specific
        p(r"(?i)Insufficient funds", "Insufficient funds"),
        p(r"(?i)already spent", "Wallet: spent proof"),
        p(r"(?i)quote already in use", "Wallet: quote reserved"),
        p(r"(?i)token not verified", "Wallet: invalid proof"),
        p(r"(?i)keyset", "Keyset issue"),
        // Generic Mint Error catch-all
        p(r"(?i)^mint error[: ]", "Mint: other"),
        p(r"(?i)could not pay invoice.*mint error", "Mint: other"),
    ]
});

fn p(pattern: &str, label: &'static str) -> Pattern {
    Pattern {
        re: Regex::new(pattern).expect("invalid regex in error classifier"),
        label,
    }
}

/// Classify a raw error string into a short label.
///
/// Returns `None` for empty/null input, a label from the fixed vocabulary,
/// or `"Other"` as a fallback.
pub fn shorten_error(raw: Option<&str>) -> Option<&'static str> {
    let s = raw?.trim();
    if s.is_empty() {
        return None;
    }
    for pat in PATTERNS.iter() {
        if pat.re.is_match(s) {
            return Some(pat.label);
        }
    }
    Some("Other")
}

/// Map a raw error string onto a stable, coarse category slug used for UI
/// coloring and grouping: one of `network`, `timeout`, `rate_limit`, `mint`,
/// `liquidity`, `fee`, `ssl`, `config`, `internal`, `unknown`. Returns `None`
/// for empty input.
pub fn categorize(raw: Option<&str>) -> Option<&'static str> {
    Some(category_for_label(shorten_error(raw)?))
}

/// Preserve known stored categories (including typed wallet faults), while
/// letting newer classifiers recognize previously unclassified history.
pub fn resolve_category<'a>(stored: Option<&'a str>, raw: Option<&str>) -> Option<&'a str> {
    stored
        .filter(|category| !category.is_empty() && *category != "unknown")
        .or_else(|| categorize(raw))
        .or(stored.filter(|category| !category.is_empty()))
}

/// Map a classifier label onto its coarse category slug.
pub fn category_for_label(label: &str) -> &'static str {
    match label {
        "Connection error" | "Server disconnected" => "network",
        l if l.starts_with("Proxy:") => "network",
        "Timeout" => "timeout",
        "HTTP 429: rate limited" => "rate_limit",
        "Insufficient funds" => "liquidity",
        "Wallet: invalid proof" | "Wallet: spent proof" | "Wallet: quote reserved" => "internal",
        "Mint: insufficient balance" => "liquidity",
        "Fee too high" => "fee",
        "SSL: cert expired" | "SSL: other" => "ssl",
        "Mint: endpoint missing" | "HTTP 4xx" => "config",
        "Mint: amount outside limits" | "Mint: operation disabled" => "config",
        l if l.starts_with("Mint:") || l == "HTTP 5xx" || l == "Keyset issue" => "mint",
        _ => "unknown",
    }
}

/// A friendly, one-line human explanation for a category slug, for tooltips
/// and empty-detail fallbacks.
pub fn category_explainer(category: &str) -> &'static str {
    match category {
        "network" => "The mint could not be reached over the network.",
        "timeout" => "The mint did not respond in time.",
        "rate_limit" => "The mint is online but temporarily rate-limited the check.",
        "mint" => "The mint accepted the request but reported an error.",
        "liquidity" => "Not enough spendable balance to complete the swap.",
        "fee" => "The mint's fee for this swap was above the configured limit.",
        "ssl" => "The mint's TLS certificate could not be validated.",
        "config" => "The request was rejected as invalid or unsupported.",
        "internal" => "The auditor's local wallet state needs repair.",
        _ => "An unclassified error occurred.",
    }
}

/// Append a short, plain-language hint to cryptic CDK error strings so an
/// operator sees *what it means*, not just the raw enum text. Leaves
/// already-clear messages untouched.
pub fn enrich_raw(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let hint = if lower.contains("nonetype") && lower.contains("has no attribute") {
        Some("the mint's server raised an exception; its operator needs to inspect the server logs")
    } else if lower.contains("amount must be between `0` and `0` is `0`") {
        Some("CDK omitted the mint's limit details; these zeros do not describe its supported amounts")
    } else if lower.contains("amount undefined") {
        Some("the mint marked the quote paid but returned no mintable amount")
    } else if lower.contains("token not verified") {
        Some("the mint rejected a cryptographically invalid local proof; the wallet will restore and retry")
    } else if lower.contains("unknown quote") {
        Some("the mint no longer recognizes this quote (it may have expired or been pruned)")
    } else if lower.contains("never became mintable") {
        Some("the mint did not confirm payment of the invoice in time")
    } else {
        None
    };

    match hint {
        Some(h) => format!("{raw} ({h})"),
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_known_patterns() {
        assert_eq!(
            shorten_error(Some("Mint error: insufficient_balance to pay invoice")),
            Some("Mint: insufficient balance")
        );
        assert_eq!(
            shorten_error(Some("connection refused by peer")),
            Some("Connection error")
        );
        assert_eq!(
            shorten_error(Some("Insufficient funds")),
            Some("Insufficient funds")
        );
        assert_eq!(
            shorten_error(Some("wallet error: Token not verified")),
            Some("Wallet: invalid proof")
        );
        assert_eq!(
            shorten_error(Some("Fee reserve of 12% is too high. ...")),
            Some("Fee too high")
        );
    }

    #[test]
    fn unknown_errors_fall_back_to_other() {
        assert_eq!(
            shorten_error(Some("flux capacitor misaligned")),
            Some("Other")
        );
    }

    #[test]
    fn empty_input_yields_none() {
        assert_eq!(shorten_error(None), None);
        assert_eq!(shorten_error(Some("   ")), None);
    }

    #[test]
    fn classifies_quote_and_transport_failures() {
        assert_eq!(
            shorten_error(Some("wallet error: Amount undefined")),
            Some("Mint: amount undefined")
        );
        assert_eq!(
            shorten_error(Some("mint unreachable: error trying to connect")),
            Some("Connection error")
        );
        assert_eq!(
            shorten_error(Some("mint timeout: did not respond within 10s")),
            Some("Timeout")
        );
    }

    #[test]
    fn classifies_cdk_payment_failures() {
        // CDK surfaces wallet melt failures as "payment failed" without the
        // "lightning" prefix the original pattern required, so every one of
        // these was falling through to "Other"/"unknown".
        assert_eq!(
            shorten_error(Some("paying the invoice — wallet error: Payment failed")),
            Some("Mint: payment failed")
        );
        assert_eq!(
            shorten_error(Some("melt error: lightning payment unsuccessful")),
            Some("Mint: payment failed")
        );
        assert_eq!(
            categorize(Some("wallet error: Payment failed")),
            Some("mint")
        );
    }

    #[test]
    fn categories_group_labels() {
        assert_eq!(categorize(Some("connection refused")), Some("network"));
        assert_eq!(
            categorize(Some("mint timeout: did not respond within 10s")),
            Some("timeout")
        );
        assert_eq!(categorize(Some("Amount undefined")), Some("mint"));
        assert_eq!(
            categorize(Some("HTTP 429 Too Many Requests")),
            Some("rate_limit")
        );
        assert_eq!(categorize(Some("Insufficient funds")), Some("liquidity"));
        assert_eq!(categorize(Some("Token not verified")), Some("internal"));
        assert_eq!(
            category_explainer("internal"),
            "The auditor's local wallet state needs repair."
        );
        assert_eq!(
            categorize(Some("Fee reserve of 12% is too high")),
            Some("fee")
        );
        assert_eq!(categorize(None), None);
    }

    #[test]
    fn invoice_failures_have_actionable_categories_and_preserve_raw_detail() {
        let raw = "requesting invoice — wallet error: Unknown error response: `code: 0, detail: 'NoneType' object has no attribute 'get'`";
        let enriched = enrich_raw(raw);
        assert!(enriched.starts_with(raw));
        assert!(enriched.contains("operator needs to inspect the server logs"));
        assert_eq!(categorize(Some(&enriched)), Some("mint"));
        assert_eq!(
            categorize(Some("mint amount rejected: requested 5 sat")),
            Some("config")
        );
        let legacy = "Amount must be between `0` and `0` is `0`";
        assert_eq!(categorize(Some(legacy)), Some("config"));
        assert!(enrich_raw(legacy).contains("zeros do not describe"));
        assert_eq!(categorize(Some("Minting is disabled")), Some("config"));
    }

    #[test]
    fn enrich_explains_cryptic_errors() {
        assert!(enrich_raw("Amount undefined").contains("no mintable amount"));
        assert!(enrich_raw("Token not verified").contains("restore and retry"));
        assert_eq!(enrich_raw("connection refused"), "connection refused");
    }
}
