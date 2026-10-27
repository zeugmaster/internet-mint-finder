use crate::types::MintState;
use leptos::prelude::*;

/// Health as an Interfere badge: one word on a tinted ground, so the state
/// never relies on colour alone.
#[component]
pub fn StateBadge(state: MintState) -> impl IntoView {
    let (class, label) = match state {
        MintState::Ok => ("badge badge-ok", "Healthy"),
        MintState::Warn => ("badge badge-warn", "Degraded"),
        MintState::Error => ("badge badge-err", "Failing"),
        MintState::Unknown => ("badge badge-muted", "Not checked"),
    };

    view! {
        <span class=class>{label}</span>
    }
}

/// Human-readable label for a coarse error category slug.
pub fn category_label(slug: &str) -> &'static str {
    match slug {
        "network" => "Network",
        "timeout" => "Timeout",
        "rate_limit" => "Rate limited",
        "mint" => "Mint error",
        "liquidity" => "Liquidity",
        "fee" => "Fee",
        "ssl" => "Certificate",
        "config" => "Configuration",
        // The auditor's own fault (database, wallet), not the mint's.
        "internal" => "Auditor error",
        _ => "Other",
    }
}

/// Small colored pill naming an error category (network / mint / timeout / …).
#[component]
pub fn CategoryPill(#[prop(into)] category: String) -> impl IntoView {
    let slug = if category.trim().is_empty() {
        "unknown".to_string()
    } else {
        category
    };
    let label = category_label(&slug);
    let class = format!("cat-pill cat-{slug}");
    view! { <span class=class>{label}</span> }
}
