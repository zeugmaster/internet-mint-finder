use crate::api::{get_mint_by_id, get_mint_reviews};
use crate::components::icons::{Check, Cross};
use crate::components::mint_avatar::MintAvatar;
use crate::components::mint_list::{CopyText, CopyUrl, DirectoryMemory};
use crate::components::reviews::ReviewsBlock;
use crate::format::{absolute_time, format_sats, middle_ellipsis, relative_time, short_url};
use crate::types::MintRead;
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_params_map;

const WORDS_CHARS: usize = 1100;
const WORDS_CHARS_ON_PHONES: usize = 460;

#[component]
pub fn MintDetailPage() -> impl IntoView {
    let params = use_params_map();
    let mint_id = Signal::derive(move || params.get().get("id").unwrap_or_default());
    let memory = expect_context::<DirectoryMemory>();
    memory.return_to.set_value(Some(memory.scroll.get_value()));
    let directory = memory.view;
    let mint = Resource::new(
        move || mint_id.get(),
        |id| async move { get_mint_by_id(id).await },
    );
    // Refetch on a successful submission only, so navigation and background sync
    // never replace a draft being typed.
    let reviews = Resource::new(
        move || mint_id.get(),
        |id| async move { get_mint_reviews(id).await },
    );
    #[cfg(feature = "hydrate")]
    {
        let listener = window_event_listener(
            leptos::ev::Custom::<leptos::web_sys::Event>::new("imf:review-posted"),
            move |_| reviews.refetch(),
        );
        on_cleanup(move || listener.remove());
    }
    view! {
        <div class="shell">
            <Transition fallback=|| view! { <div class="skeleton" style="height: 80px;"></div> }>
                {move || mint.get().map(|result| match result {
                    Ok(Some(mint)) => view! {
                        <div class="arrives">
                            <MintHead mint=mint.clone()/>
                            <div class="mint-sections">
                                <section aria-labelledby="reviews-title">
                                    <h2 class="section-title" id="reviews-title">"Reviews"</h2>
                                    <Transition fallback=|| view! { <div class="skeleton" style="height: 120px;"></div> }>
                                        {move || reviews.get().map(|result| match result {
                                            Ok(data) => view! { <ReviewsBlock data/> }.into_any(),
                                            Err(_) => view! {
                                                <div class="callout" role="alert">
                                                    <p>"Couldn't load reviews."</p>
                                                    <button class="btn" on:click=move |_| reviews.refetch()>"Try again"</button>
                                                </div>
                                            }.into_any(),
                                        })}
                                    </Transition>
                                </section>
                                <div class="mint-info"><MintInfoSections mint/></div>
                            </div>
                        </div>
                    }.into_any(),
                    Ok(None) => {
                        #[cfg(feature = "ssr")]
                        if let Some(response) = use_context::<leptos_axum::ResponseOptions>() { response.set_status(axum::http::StatusCode::NOT_FOUND); }
                        view! {
                            <Title text="Mint not found — Internet Mint Finder"/>
                            <nav class="breadcrumb" aria-label="Breadcrumb"><a href=move || directory.get()>"Directory"</a></nav>
                            <div class="empty-state"><h1 class="page-title">"This mint isn't in the directory."</h1></div>
                        }.into_any()
                    },
                    Err(_) => view! {
                        <div class="callout callout-err" role="alert">
                            <p>"Couldn't load this mint."</p>
                            <button class="btn" on:click=move |_| mint.refetch()>"Try again"</button>
                        </div>
                    }.into_any(),
                })}
            </Transition>
        </div>
    }
}

#[component]
fn MintHead(mint: MintRead) -> impl IntoView {
    let directory = expect_context::<DirectoryMemory>().view;
    let display_name = if mint.name.trim().is_empty() {
        short_url(&mint.url)
    } else {
        mint.name.clone()
    };
    let page_title = format!("{display_name} — Internet Mint Finder");
    let info = mint_info(&mint);
    let description =
        info_text(&info, "description_long").or_else(|| info_text(&info, "description"));
    let notice = info_text(&info, "motd");
    view! {
        <Title text=page_title/>
        // A breadcrumb in words; navigation never gets a back arrow.
        <nav class="breadcrumb" aria-label="Breadcrumb">
            <a href=move || directory.get()>"Directory"</a>
            <span class="breadcrumb-sep" aria-hidden="true">"/"</span>
            <span aria-current="page" dir="auto">{display_name.clone()}</span>
        </nav>
        <div class="detail-head">
            <MintAvatar name=display_name.clone() info=mint.info.clone()/>
            <div class="detail-id">
                <div class="detail-title">
                    <h1 class="page-title" dir="auto">{display_name}</h1>
                </div>
                <CopyUrl url=mint.url.clone()/>
            </div>
        </div>
        {description.map(|text| view! { <MintWords id="about-title" title="Description on Nostr" text/> })}
        {notice.map(|text| view! {
            <MintWords id="notice-title" title="Notice on Nostr" text fetched=mint.updated_at/>
        })}
    }
}

/// A section in the mint's own words, running text on the page. A notice
/// carries no date of its own, so it shows when it was last fetched: the one
/// sign that the mint is still saying it. Long words show their first eight
/// lines and a "Read more" (a details element, so it works before hydration).
#[component]
fn MintWords(
    id: &'static str,
    title: &'static str,
    text: String,
    #[prop(optional)] fetched: Option<DateTime<Utc>>,
) -> impl IntoView {
    let class = words_class(&text);
    let clamped = class != "mint-words";
    view! {
        <section class=class aria-labelledby=id>
            <div class="section-head">
                <h2 class="section-title" id=id>{title}</h2>
                {fetched.map(|at| view! {
                    <span title=absolute_time(at)>{format!("Published {}", relative_time(at))}</span>
                })}
            </div>
            <p dir="auto">{text}</p>
            {clamped.then(|| view! {
                <details class="words-more"><summary class="text-link">"Read more"</summary></details>
            })}
        </section>
    }
}

/// Whether the mint's words pass eight lines: on every screen, on a phone
/// alone, or not at all. Between the two the words simply run a little longer,
/// which beats a "Read more" with nothing behind it.
fn words_class(text: &str) -> &'static str {
    let chars = text.chars().count();
    if chars > WORDS_CHARS || text.lines().count() > 8 {
        "mint-words words-long"
    } else if chars > WORDS_CHARS_ON_PHONES {
        "mint-words words-long-on-phones"
    } else {
        "mint-words"
    }
}

/// Metadata from the mint announcement; null when absent, so every field reads
/// as blank and its section is skipped.
fn mint_info(mint: &MintRead) -> serde_json::Value {
    mint.info
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default()
}

/// A text field of the mint's info, if it says anything.
fn info_text(info: &serde_json::Value, key: &str) -> Option<String> {
    info.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// How to reach the operator, then what the mint offers, each skipped when
/// the mint lists nothing for it.
#[component]
fn MintInfoSections(mint: MintRead) -> impl IntoView {
    let info = mint_info(&mint);
    // "Nutshell/0.16.3" reads as a name and a version.
    let software = info_text(&info, "version").map(|v| v.replacen('/', " ", 1));
    let terms = info_text(&info, "tos_url").filter(|url| url.starts_with("https://"));
    let contacts = contacts(&info);
    let features = info
        .get("nuts")
        .and_then(|v| v.as_object())
        .filter(|nuts| !nuts.is_empty())
        .map(features);
    let lists_offers = features.is_some() || software.is_some() || terms.is_some();
    let note = format!(
        "As listed on Nostr. Published {}.",
        relative_time(mint.updated_at)
    );
    let fetched = absolute_time(mint.updated_at);

    view! {
        {(!contacts.is_empty()).then(|| view! {
            <section aria-labelledby="contact-title">
                <h2 class="section-title" id="contact-title">"Contact"</h2>
                <ContactCard contacts/>
            </section>
        })}
        {lists_offers.then(|| view! {
            <section aria-labelledby="offers-title">
                <h2 class="section-title" id="offers-title">"What it offers"</h2>
                <Offers features software terms note fetched/>
            </section>
        })}
    }
}

/// How to reach the operator: one card of copy buttons, read down the first
/// column, then the second.
#[component]
fn ContactCard(contacts: Vec<Contact>) -> impl IntoView {
    let rows = format!("--ledger-rows: {}", contacts.len().div_ceil(2));
    view! {
        <div class="card">
            <dl class="ledger" style=rows>
                {contacts.into_iter().map(|contact| view! {
                    <div>
                        <dt>{contact.label}</dt>
                        <dd><CopyText text=contact.value shown=contact.shown/></dd>
                    </div>
                }).collect_view()}
            </dl>
        </div>
    }
}

/// What the mint offers, in one card: Lightning in and out side by side with
/// their limits, since those decide whether the mint fits; then every other
/// NUT the mint names, by number and the spec's title, as a two-column
/// checklist, a check for supported and a cross for not; then its software
/// and terms; and at the foot, where it all comes from.
#[component]
fn Offers(
    features: Option<Features>,
    software: Option<String>,
    terms: Option<String>,
    note: String,
    fetched: String,
) -> impl IntoView {
    let has_facts = software.is_some() || terms.is_some();
    view! {
        <div class="card offers">
            {features.map(|features| view! { <OfferRows features/> })}
            // The mint's own facts, on the card's two columns.
            {has_facts.then(|| view! {
                <dl class="ledger">
                    {software.map(|s| view! { <div><dt>"Software"</dt><dd class="mono">{s}</dd></div> })}
                    // The address the link opens, like the mint's own.
                    {terms.map(|url| {
                        let shown = short_url(&url);
                        view! {
                            <div>
                                <dt>"Terms"</dt>
                                <dd>
                                    <a class="text-link mono" href=url target="_blank" rel="noopener noreferrer">
                                        {shown}
                                        <span class="sr-only">" (opens in a new tab)"</span>
                                    </a>
                                </dd>
                            </div>
                        }
                    })}
                </dl>
            })}
            <p class="offers-note" title=fetched>{note}</p>
        </div>
    }
}

/// The features: Lightning cells, then the NUT checklist.
#[component]
fn OfferRows(features: Features) -> impl IntoView {
    let Features { lightning, nuts } = features;
    // Read down the first column, then the second.
    let rows = format!("--nut-rows: {}", nuts.len().div_ceil(2));
    view! {
        <dl class="offers-lightning">
            {lightning.into_iter().map(|feature| {
                let Feature { name, offered, detail } = feature;
                view! {
                    <div class="offers-cell" class:is-off=!offered>
                        <dt>{name}</dt>
                        <dd><Pieces detail/></dd>
                    </div>
                }
            }).collect_view()}
        </dl>
        {(!nuts.is_empty()).then(|| view! {
            <ul class="offers-list" aria-label="NUTs" style=rows>
                {nuts.into_iter().map(|nut| {
                    let Nut { number, supported } = nut;
                    view! {
                        <li class="offers-item" class:is-off=!supported>
                            {if supported { view! { <Check/> }.into_any() } else { view! { <Cross/> }.into_any() }}
                            <span class="mono offers-nut">{format!("NUT-{number:02}")}</span>
                            " "
                            <span>
                                {nut_title(number)}
                                {(!supported).then(|| view! { <span class="sr-only">", not supported"</span> })}
                            </span>
                        </li>
                    }
                }).collect_view()}
            </ul>
        })}
    }
}

/// A feature's detail, with amounts set as figures.
#[component]
fn Pieces(detail: Vec<Piece>) -> impl IntoView {
    detail
        .into_iter()
        .map(|piece| match piece {
            Piece::Text(text) => view! { {text} }.into_any(),
            Piece::Sats(n) => view! { <span class="mono">{format_sats(n)}</span> }.into_any(),
        })
        .collect_view()
}

/// What the mint lists in its NUT-06 info: Lightning in and out with their
/// limits, then every optional NUT it names.
struct Features {
    lightning: [Feature; 2],
    nuts: Vec<Nut>,
}

/// Lightning in or out, read from the mint's NUT-06 info.
#[derive(Debug, PartialEq)]
struct Feature {
    name: &'static str,
    offered: bool,
    detail: Vec<Piece>,
}

/// An optional NUT the mint names, and whether it marks it supported.
#[derive(Debug, PartialEq)]
struct Nut {
    number: u16,
    supported: bool,
}

/// A feature's detail: words, with amounts kept apart so they set as figures.
#[derive(Debug, PartialEq)]
enum Piece {
    Text(String),
    Sats(u64),
}

/// Lightning in and out first, the same two cells on every mint so two mints
/// compare; then every NUT the mint names from 07 up, in order. 00 to 06 are
/// the base every mint implements, and 04 and 05 are the Lightning cells.
fn features(nuts: &serde_json::Map<String, serde_json::Value>) -> Features {
    let mut listed: Vec<Nut> = nuts
        .iter()
        .filter_map(|(key, value)| {
            let number: u16 = key.parse().ok()?;
            (number >= 7).then(|| Nut {
                number,
                supported: supported(value),
            })
        })
        .collect();
    listed.sort_by_key(|nut| nut.number);
    listed.dedup_by_key(|nut| nut.number);
    Features {
        lightning: [
            lightning(nuts, 4, "Add funds with Lightning"),
            lightning(nuts, 5, "Pay out to Lightning"),
        ],
        nuts: listed,
    }
}

/// Whether the mint marks a NUT supported. Most say so in `supported` (true,
/// or a non-empty list of methods); NUT-15 lists its methods (older mints as
/// a bare list); the rest, like NUT-19's cache and NUT-21 and 22's auth, are
/// on by being listed with their settings. Paused is not supported.
fn supported(value: &serde_json::Value) -> bool {
    let non_empty = |v: &serde_json::Value| v.as_array().is_some_and(|a| !a.is_empty());
    if let Some(on) = value.as_bool() {
        return on;
    }
    if value.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        return false;
    }
    if let Some(s) = value.get("supported") {
        return s.as_bool() == Some(true) || non_empty(s);
    }
    if let Some(methods) = value.get("methods") {
        return non_empty(methods);
    }
    if value.is_array() {
        return non_empty(value);
    }
    true
}

/// An optional NUT's title, as the spec's index (github.com/cashubtc/nuts)
/// gives it, in sentence case. A NUT newer than this list shows its number
/// alone.
fn nut_title(number: u16) -> Option<&'static str> {
    Some(match number {
        7 => "Token state check",
        8 => "Overpaid Lightning fees",
        9 => "Signature restore",
        10 => "Spending conditions",
        11 => "Pay-To-Pubkey (P2PK)",
        12 => "DLEQ proofs",
        13 => "Deterministic secrets",
        14 => "Hashed Timelock Contracts (HTLCs)",
        15 => "Partial multi-path payments (MPP)",
        16 => "Animated QR codes",
        17 => "WebSocket subscriptions",
        18 => "Payment requests",
        19 => "Cached responses",
        20 => "Signature on mint quote",
        21 => "Clear authentication",
        22 => "Blind authentication",
        23 => "Payment method: BOLT11",
        24 => "HTTP 402 Payment Required",
        25 => "Payment method: BOLT12",
        26 => "Payment request Bech32m encoding",
        27 => "Nostr mint backup",
        28 => "Pay to Blinded Key (P2BK)",
        29 => "Batched mint",
        30 => "Payment method: Onchain",
        _ => return None,
    })
}

/// Lightning in (NUT-04) or out (NUT-05): offered when the mint lists a
/// BOLT11 method and hasn't paused it. The detail is the sat limits, then any
/// other units it takes.
fn lightning(
    nuts: &serde_json::Map<String, serde_json::Value>,
    nut: u8,
    name: &'static str,
) -> Feature {
    let off = |why: &str| Feature {
        name,
        offered: false,
        detail: text(why),
    };
    let Some(entry) = nuts.get(&nut.to_string()) else {
        return off("Not listed");
    };
    if entry.get("disabled").and_then(|v| v.as_bool()) == Some(true) {
        return off("Paused by the mint");
    }
    let methods: Vec<&serde_json::Value> = entry
        .get("methods")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter(|m| m.get("method").and_then(|v| v.as_str()) == Some("bolt11"))
        .collect();
    if methods.is_empty() {
        return off("Not listed");
    }
    let mut units: Vec<&str> = Vec::new();
    for unit in methods
        .iter()
        .map(|m| m.get("unit").and_then(|v| v.as_str()).unwrap_or("sat"))
    {
        if !units.contains(&unit) {
            units.push(unit);
        }
    }
    let mut detail = methods
        .iter()
        .find(|m| m.get("unit").and_then(|v| v.as_str()).unwrap_or("sat") == "sat")
        .map(|m| {
            let amount = |key: &str| m.get(key).and_then(|v| v.as_u64());
            sat_range(
                amount("min_amount").filter(|&n| n > 1),
                amount("max_amount"),
            )
        })
        .unwrap_or_default();
    if units != ["sat"] {
        // Bitcoin's own units as written ("sat", "msat"); currencies in
        // capitals ("USD").
        let named = |unit: &&str| {
            if matches!(unit.to_lowercase().as_str(), "sat" | "msat") {
                unit.to_lowercase()
            } else {
                unit.to_uppercase()
            }
        };
        if detail.is_empty() {
            let all: Vec<String> = units.iter().map(named).collect();
            detail.push(Piece::Text(format!("In {}", and_list(&all))));
        } else {
            let others: Vec<String> = units.iter().filter(|u| **u != "sat").map(named).collect();
            detail.push(Piece::Text(format!(", also {}", and_list(&others))));
        }
    }
    // An offered cell is never blank.
    if detail.is_empty() {
        detail = text("No limits listed");
    }
    Feature {
        name,
        offered: true,
        detail,
    }
}

/// "Up to 500 000 sat", "1 000 to 500 000 sat", "From 1 000 sat", or nothing.
fn sat_range(min: Option<u64>, max: Option<u64>) -> Vec<Piece> {
    let sat = || Piece::Text(" sat".to_string());
    match (min, max) {
        (Some(min), Some(max)) => vec![
            Piece::Sats(min),
            Piece::Text(" to ".into()),
            Piece::Sats(max),
            sat(),
        ],
        (None, Some(max)) => vec![Piece::Text("Up to ".into()), Piece::Sats(max), sat()],
        (Some(min), None) => vec![Piece::Text("From ".into()), Piece::Sats(min), sat()],
        (None, None) => Vec::new(),
    }
}

/// "USD", "USD and EUR", "msat, USD and EUR".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn text(words: &str) -> Vec<Piece> {
    vec![Piece::Text(words.to_string())]
}

/// One way to reach the operator, labelled in plain words.
struct Contact {
    label: String,
    /// The full value, which a click copies.
    value: String,
    /// What the line shows: a Nostr key is cut in the middle.
    shown: String,
}

/// The mint's contact methods, each shown as something to copy.
fn contacts(info: &serde_json::Value) -> Vec<Contact> {
    info.get("contact")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let method = c.get("method")?.as_str()?.trim().to_lowercase();
            // A key written as a nostr: URI is still the key.
            let value = c.get("info")?.as_str()?.trim();
            let value = value.strip_prefix("nostr:").unwrap_or(value).to_string();
            if method.is_empty() || value.is_empty() {
                return None;
            }
            let label = match method.as_str() {
                "email" => "Email".to_string(),
                "nostr" => "Nostr".to_string(),
                "twitter" | "x" => "X".to_string(),
                other => {
                    let mut chars = other.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                }
            };
            let shown = if value.starts_with("npub1") {
                middle_ellipsis(&value)
            } else {
                value.clone()
            };
            Some(Contact {
                label,
                value,
                shown,
            })
        })
        .collect()
}

/// The 24-hour strip of hourly results and its figure, with what the figure
/// rests on behind a question mark beside it.
#[cfg(test)]
mod tests {
    use super::{and_list, contacts, features, nut_title, words_class, Nut, Piece};
    use serde_json::json;
    #[test]
    fn long_words_are_clamped_where_they_pass_eight_lines() {
        assert_eq!(words_class("A small mint."), "mint-words");
        assert_eq!(
            words_class(&"word ".repeat(100)),
            "mint-words words-long-on-phones"
        );
        assert_eq!(words_class(&"word ".repeat(300)), "mint-words words-long");
        assert_eq!(words_class(&"line\n".repeat(9)), "mint-words words-long");
    }

    fn nuts(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn features_list_every_optional_nut_the_mint_names_in_order() {
        let list = features(&nuts(json!({
            "4": {"methods": [{"method": "bolt11", "unit": "sat", "min_amount": 1, "max_amount": 500000}], "disabled": false},
            "5": {"methods": [{"method": "bolt11", "unit": "sat", "min_amount": 100, "max_amount": 250000}], "disabled": true},
            "6": {"supported": true},
            "22": {"bat_max_mint": 50},
            "9": {"supported": false},
            "17": {"supported": [{"method": "bolt11", "unit": "sat", "commands": ["proof_state"]}]},
            "15": {"methods": []},
            "14": [{"method": "bolt11", "unit": "sat"}],
            "7": {"supported": true},
            "12": {"supported": true, "disabled": true},
            "31": {"supported": true},
            "nut99": {"supported": true}
        })));
        let nut = |number: u16, supported: bool| Nut { number, supported };
        assert_eq!(
            list.nuts,
            [
                nut(7, true),
                // Listed, but marked unsupported, empty or paused.
                nut(9, false),
                nut(12, false),
                // The old bare list of methods.
                nut(14, true),
                nut(15, false),
                nut(17, true),
                // On by being listed with its settings.
                nut(22, true),
                nut(31, true),
            ]
        );
        assert_eq!(nut_title(11), Some("Pay-To-Pubkey (P2PK)"));
        assert_eq!(nut_title(31), None);

        let [deposit, payout] = &list.lightning;
        assert!(deposit.offered);
        assert_eq!(
            deposit.detail,
            [
                Piece::Text("Up to ".into()),
                Piece::Sats(500_000),
                Piece::Text(" sat".into())
            ]
        );
        assert!(!payout.offered);
        assert_eq!(payout.detail, [Piece::Text("Paused by the mint".into())]);
    }

    #[test]
    fn lightning_detail_names_limits_and_other_units() {
        let list = features(&nuts(json!({
            "4": {"methods": [
                {"method": "bolt11", "unit": "sat", "min_amount": 1000, "max_amount": 2000000},
                {"method": "bolt11", "unit": "usd"},
                {"method": "bolt12", "unit": "sat"}
            ]},
            "5": {"methods": [{"method": "bolt11", "unit": "sat"}, {"method": "bolt11", "unit": "eur"}]}
        })));
        assert_eq!(
            list.lightning[0].detail,
            [
                Piece::Sats(1000),
                Piece::Text(" to ".into()),
                Piece::Sats(2_000_000),
                Piece::Text(" sat".into()),
                Piece::Text(", also USD".into()),
            ]
        );
        assert_eq!(
            list.lightning[1].detail,
            [Piece::Text("In sat and EUR".into())]
        );
        assert!(list.lightning[1].offered);
    }

    #[test]
    fn contacts_get_plain_labels_and_npubs_cut_in_the_middle() {
        let list = contacts(&json!({"contact": [
            {"method": "email", "info": "ops@mint.example"},
            {"method": "nostr", "info": "npub1qe3e5wrvnsgpggtkytxteaqfprz0rgxr8c3l34kk3a9t7e2l3acslezefe"},
            {"method": "twitter", "info": "@mint"},
            {"method": "matrix", "info": " "},
            {"method": "nostr", "info": "nostr:npub1qe3e5wrvnsgpggtkytxteaqfprz0rgxr8c3l34kk3a9t7e2l3acslezefe"}
        ]}));
        let labels: Vec<&str> = list.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Email", "Nostr", "X", "Nostr"]);
        assert_eq!(list[3].shown, "npub1qe3e5w…ezefe");
        assert!(list[3].value.starts_with("npub1"));
        assert_eq!(list[0].shown, "ops@mint.example");
        assert_eq!(list[1].shown, "npub1qe3e5w…ezefe");
        assert!(list[1].value.ends_with("ezefe") && list[1].value.len() == 63);
    }

    #[test]
    fn units_are_listed_in_plain_english() {
        let list =
            |items: &[&str]| and_list(&items.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(list(&["USD"]), "USD");
        assert_eq!(list(&["sat", "EUR"]), "sat and EUR");
        assert_eq!(list(&["msat", "USD", "EUR"]), "msat, USD and EUR");
        let three = features(&nuts(json!({
            "4": {"methods": [
                {"method": "bolt11", "unit": "msat"},
                {"method": "bolt11", "unit": "usd"},
                {"method": "bolt11", "unit": "eur"}
            ]}
        })));
        assert_eq!(
            three.lightning[0].detail,
            [Piece::Text("In msat, USD and EUR".into())]
        );
    }
}
