use crate::format::{relative_time, short_pubkey};
use crate::types::{AuditorMark, MintState};
use leptos::prelude::*;

/// The two-character mark an auditor strikes: its key's first two hex digits,
/// or "loc" for this instance when it runs without a Nostr key.
pub fn stamp_label(pubkey: &str) -> String {
    match pubkey.get(..2) {
        Some(prefix) if !pubkey.is_empty() => prefix.to_string(),
        _ => "loc".to_string(),
    }
}

/// One stamp per auditor: each auditor's own latest 24 h verdict on a mint,
/// side by side and never merged. Struck clean, partial, or voided. Each
/// stamp names its auditor on hover, focus, and tap.
#[component]
pub fn Hallmarks(marks: Vec<AuditorMark>) -> impl IntoView {
    let clean = marks.iter().filter(|m| m.state == MintState::Ok).count();
    let summary = format!(
        "Hallmarks: {clean} of {} auditors saw it settle without failures in 24h",
        marks.len()
    );
    view! {
        <span class="hallmarks" role="group" aria-label=summary>
            {marks
                .into_iter()
                .map(|mark| {
                    let (class, verdict) = match mark.state {
                        MintState::Ok => ("hallmark struck", "settled"),
                        MintState::Warn => ("hallmark partial", "settled after a failure"),
                        MintState::Error => ("hallmark voided", "failed"),
                        MintState::Unknown => ("hallmark", "no verdict"),
                    };
                    let who = if mark.pubkey.is_empty() {
                        "This instance".to_string()
                    } else {
                        format!("Auditor {}", short_pubkey(&mark.pubkey))
                    };
                    let tip = format!("{who}: {verdict} {}", relative_time(mark.at));
                    view! {
                        // A button so keyboard and touch reach the tip; the
                        // click must not trigger the directory row's link.
                        <button
                            type="button"
                            class=class
                            aria-label=tip.clone()
                            data-tip=tip
                            on:click=|ev| ev.stop_propagation()
                        >
                            {stamp_label(&mark.pubkey)}
                        </button>
                    }
                })
                .collect_view()}
        </span>
    }
}

/// An auditor's mark on its own, as a key beside its name in auditor lists.
#[component]
pub fn StampKey(#[prop(into)] pubkey: String) -> impl IntoView {
    view! { <span class="hallmark key" aria-hidden="true">{stamp_label(&pubkey)}</span> }
}
