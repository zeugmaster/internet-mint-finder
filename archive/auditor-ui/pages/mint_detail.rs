use crate::api::{get_mint_by_id, get_mint_metrics, get_mint_reviews, get_mint_uptime};
use crate::components::badge::{CategoryPill, StateBadge};
use crate::components::icons::{Check, Cross, Help};
use crate::components::mint_avatar::MintAvatar;
use crate::components::mint_list::{CopyText, CopyUrl, DirectoryMemory};
use crate::components::reviews::ReviewsBlock;
use crate::format::{
    absolute_time, format_sats, middle_ellipsis, relative_time, relative_time_from, short_url,
};
use crate::hooks::use_poll;
use crate::types::{MintMetrics, MintRead, MintState, MintUptime, MintUptimeBar};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos::web_sys::HtmlElement;
use leptos_meta::Title;
use leptos_router::hooks::use_params_map;

/// How much of a failure message the mint page shows before "Read more".
const INCIDENT_CHARS: usize = 300;

/// About eight lines of the mint's words across the page's column, and across
/// a phone's.
const WORDS_CHARS: usize = 1100;
const WORDS_CHARS_ON_PHONES: usize = 460;

/// One mint for a wallet user: is it up, what people say, and what the mint
/// says about itself. Audit detail (per-auditor results, swaps, funding) is
/// left off. Each section is a card with its heading outside, on the page.
/// The page polls every 15 seconds and keeps the last good data through a
/// failed refresh.
#[component]
pub fn MintDetailPage() -> impl IntoView {
    let params = use_params_map();
    let mint_id = Signal::derive(move || params.get().get("id").unwrap_or_default());

    // Going back returns to this place in the list.
    let memory = expect_context::<DirectoryMemory>();
    memory.return_to.set_value(Some(memory.scroll.get_value()));
    let directory = memory.view;

    let tick = use_poll(15);

    let mint = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_by_id(id).await },
    );

    // Whether the mint exists, checked once per page (not on the poll), so the
    // sections below are skipped for an unknown id without being rebuilt on
    // every refresh.
    let exists = Resource::new(
        move || mint_id.get(),
        |id| async move { get_mint_by_id(id).await.map(|m| m.is_some()) },
    );

    let metrics = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_metrics(id).await },
    );

    let uptime = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_uptime(id).await },
    );

    // Loaded once per page, not on the poll: a refresh would rebuild the
    // review form and drop what someone is typing.
    let reviews = Resource::new(
        move || mint_id.get(),
        |id| async move { get_mint_reviews(id).await },
    );

    // The last good answer to each polled request. A failed refresh keeps
    // showing it, with a note, instead of wiping the page; only a first load
    // that fails shows an error. Effects run in the browser only, so the
    // server renders straight from the requests.
    let last_mint = RwSignal::new(None::<Option<MintRead>>);
    let last_metrics = RwSignal::new(None::<MintMetrics>);
    let last_uptime = RwSignal::new(None::<MintUptime>);
    let updated_at = RwSignal::new(None::<DateTime<Utc>>);
    Effect::new(move |_| {
        if let Some(Ok(found)) = mint.get() {
            last_mint.set(Some(found));
            updated_at.set(Some(Utc::now()));
        }
    });
    Effect::new(move |_| {
        if let Some(Ok(data)) = metrics.get() {
            last_metrics.set(Some(data));
        }
    });
    Effect::new(move |_| {
        if let Some(Ok(data)) = uptime.get() {
            last_uptime.set(Some(data));
        }
    });
    let current_mint = move || {
        mint.get()
            .map(|res| res.or_else(|e| last_mint.get().ok_or(e)))
    };
    let current_uptime = move || {
        uptime
            .get()
            .and_then(|r| r.ok())
            .or_else(|| last_uptime.get())
    };
    let stale_note = move || {
        tick.track();
        let failed = matches!(mint.get(), Some(Err(_)))
            || matches!(metrics.get(), Some(Err(_)))
            || matches!(uptime.get(), Some(Err(_)));
        let when = updated_at.get().map(relative_time_from)?;
        failed.then(|| {
            view! {
                <div class="callout mt-4" role="status">
                    {format!("Couldn't refresh. Showing data from {when}, retrying.")}
                </div>
            }
        })
    };

    view! {
        <div class="shell">
            <Transition fallback=|| {
                view! {
                    <div class="skeleton" style="height: 36px; width: min(320px, 100%);"></div>
                    <div class="skeleton mt-4" style="height: 14px; width: min(480px, 100%);"></div>
                }
            }>
                // Built once, so it fades in where the skeleton was (`.arrives`)
                // and the refresh redraws only what's inside.
                <div class="arrives">
                {move || {
                    let metrics_data = metrics.get().and_then(|r| r.ok()).or_else(|| last_metrics.get());
                    let uptime_data = current_uptime();
                    current_mint().map(|res| match res {
                        Ok(Some(mint)) => view! {
                            <MintHead mint=mint metrics=metrics_data uptime=uptime_data/>
                            {stale_note}
                        }
                        .into_any(),
                        Ok(None) => {
                            // A real 404, so dead mint pages drop out of search
                            // results. The route renders after its data (Async),
                            // so the status is still unsent here.
                            #[cfg(feature = "ssr")]
                            if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
                                response.set_status(axum::http::StatusCode::NOT_FOUND);
                            }
                            view! {
                                <Title text="Mint not found — Internet Mint Finder"/>
                                <nav class="breadcrumb" aria-label="Breadcrumb"><a href=move || directory.get()>"Directory"</a></nav>
                                <div class="empty-state"><h1 class="page-title">"This mint isn't in the directory."</h1></div>
                            }
                            .into_any()
                        }
                        Err(e) => view! {
                            <div class="callout callout-err" role="alert" title=e.to_string()>
                                "Couldn't load this mint. Retrying every 15 seconds."
                            </div>
                        }
                        .into_any(),
                    })
                }}
                </div>
            </Transition>

            <Transition fallback=|| ()>
                {move || exists.get().map(|found| matches!(found, Ok(true)).then(|| view! {
                <div class="mint-sections">
                    <section aria-labelledby="uptime-title">
                        <h2 class="section-title" id="uptime-title">"Observed uptime"</h2>
                        <Transition fallback=|| view! { <div class="skeleton" style="height: 136px;"></div> }>
                            <div class="arrives">
                                {move || uptime.get().map(|_| view! { <UptimePanel uptime=current_uptime()/> })}
                            </div>
                        </Transition>
                    </section>

                    <section aria-labelledby="reviews-title">
                        <h2 class="section-title" id="reviews-title">"Reviews"</h2>
                        <Transition fallback=|| view! { <div class="skeleton" style="height: 120px;"></div> }>
                            <div class="arrives">
                                {move || reviews.get().map(|res| match res {
                                    Ok(data) => view! { <ReviewsBlock data/> }.into_any(),
                                    Err(e) => view! { <p class="section-empty" title=e.to_string()>"Couldn't load reviews. Refresh the page to try again."</p> }.into_any(),
                                })}
                            </div>
                        </Transition>
                    </section>

                    <Transition fallback=|| ()>
                        <div class="arrives mint-info">
                            {move || current_mint().and_then(|r| r.ok()).flatten().map(|mint| view! { <MintInfoSections mint/> })}
                        </div>
                    </Transition>
                </div>
                }))}
            </Transition>
        </div>
    }
}

/// The mint's avatar, name, state and copyable URL, the latest failure
/// verbatim when the mint is not healthy or its newest uptime bar failed, then
/// the mint's own words: its description and any notice, each skipped when the
/// mint doesn't give one.
#[component]
fn MintHead(
    mint: MintRead,
    metrics: Option<MintMetrics>,
    uptime: Option<MintUptime>,
) -> impl IntoView {
    let directory = expect_context::<DirectoryMemory>().view;
    let metrics = metrics.unwrap_or_default();
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

    // The freshest error attributable to *this* mint, whether it came from a
    // paid swap or a free health probe, with its category and timestamp.
    let newest_failed = uptime
        .as_ref()
        .and_then(newest_bar_state)
        .is_some_and(|state| matches!(state, MintState::Error | MintState::Warn));
    let incident = match (
        newest_failed || matches!(mint.state, MintState::Error | MintState::Warn),
        pick_incident(&metrics, uptime.as_ref()),
    ) {
        (true, Some((message, category, at))) => {
            let when = at
                .map(relative_time)
                .unwrap_or_else(|| "recently".to_string());
            let pill = category.map(|c| view! { <CategoryPill category=c/> });
            // A mint's raw error can be a whole HTML error page: the first
            // 300 characters, and all of it behind "Read more".
            let full = (message.chars().count() > INCIDENT_CHARS).then(|| message.clone());
            let message = if full.is_some() {
                format!(
                    "{}…",
                    message
                        .chars()
                        .take(INCIDENT_CHARS)
                        .collect::<String>()
                        .trim_end()
                )
            } else {
                message
            };
            Some(view! {
                <div class="incident">
                    <div class="incident-head">
                        <span class="incident-title">"Latest failure"</span>
                        {pill}
                        <span class="incident-when mono">{when}</span>
                    </div>
                    <div class="incident-msg mono">{message}</div>
                    {full.map(|full| view! {
                        <details class="incident-more">
                            <summary class="text-link">"Read more"</summary>
                            <div class="incident-msg mono">{full}</div>
                        </details>
                    })}
                </div>
            })
        }
        _ => None,
    };
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
                    <StateBadge state=mint.state/>
                </div>
                <CopyUrl url=mint.url.clone()/>
            </div>
        </div>
        {incident}
        {description.map(|text| view! { <MintWords id="about-title" title="Description from the mint" text/> })}
        {notice.map(|text| view! {
            <MintWords id="notice-title" title="Notice from the mint" text fetched=mint.updated_at/>
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
                    <span title=absolute_time(at)>{format!("Fetched {}", relative_time(at))}</span>
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

/// The mint's NUT-06 info; null when it has shared none, so every field reads
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
        "As listed by the mint. Fetched {}.",
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
#[component]
fn UptimePanel(uptime: Option<MintUptime>) -> impl IntoView {
    let Some(uptime) = uptime else {
        return view! {
            <div class="card card-pad uptime-card">
                <p class="text-dim">"Couldn't load uptime. Retrying every 15 seconds."</p>
            </div>
        }
        .into_any();
    };
    let value = uptime
        .uptime_percent
        .map(|percent| format!("{percent:.1}%"))
        .unwrap_or_else(|| "No data".to_string());
    let state = uptime_state(&uptime);
    let window = format!("{}h", uptime.window_hours);
    let checks = if uptime.checks == 0 {
        format!("No checks in the last {window}")
    } else {
        format!(
            "{} of {} passed",
            uptime.successes,
            plural(uptime.checks, "check", "checks")
        )
    };
    let last = match (uptime.checks, uptime.last_checked_at) {
        (0, _) | (_, None) => String::new(),
        (_, Some(ts)) => format!(", the last one {}", relative_time(ts)),
    };
    // When it failed, for anyone who can't hover a bar: a finger, the keyboard
    // and a screen reader get it here.
    let failed = failed_bars(&uptime.bars)
        .map(|words| format!(" {words}"))
        .unwrap_or_default();
    let note = format!(
        "{checks}{last}.{failed} Each bar is {}; grey means no data, not downtime.",
        bar_span(uptime.bucket_minutes)
    );
    let label = format!("Observed uptime, last {window}: {value}. {checks}.{failed}");
    let style = uptime_grid_style(uptime.bars.len());
    let bars = uptime.bars;

    let tip_open = RwSignal::new(false);
    let tip = NodeRef::<leptos::html::Span>::new();
    #[cfg(feature = "hydrate")]
    {
        use leptos::wasm_bindgen::JsCast;
        let outside = window_event_listener(leptos::ev::pointerdown, move |ev| {
            let target = ev.target();
            let node = target
                .as_ref()
                .and_then(|t| t.dyn_ref::<leptos::web_sys::Node>());
            if !tip.get_untracked().is_some_and(|tip| tip.contains(node)) {
                tip_open.set(false);
            }
        });
        on_cleanup(move || outside.remove());
    }

    view! {
        <div class="card card-pad uptime-card">
            <div class="uptime-head">
                <span class="uptime-figure">
                    <span class="uptime-value">{value}</span>
                    // Shown on hover (a mouse) or focus (the keyboard), and
                    // toggled by a tap, since iOS doesn't focus a tapped
                    // button; a press elsewhere or Escape hides it again.
                    <span class="help-tip" class:is-open=move || tip_open.get() node_ref=tip>
                        <button
                            type="button"
                            class="help-tip-button"
                            aria-describedby="uptime-note"
                            aria-expanded=move || if tip_open.get() { "true" } else { "false" }
                            on:click=move |_| tip_open.update(|open| *open = !*open)
                            on:keydown=move |ev: leptos::ev::KeyboardEvent| {
                                if ev.key() == "Escape" {
                                    tip_open.set(false);
                                    let _ = event_target::<HtmlElement>(&ev).blur();
                                }
                            }
                        >
                            <Help/>
                            <span class="sr-only">"How this is measured"</span>
                        </button>
                        <span class="help-tip-text" role="tooltip" id="uptime-note">{note}</span>
                    </span>
                </span>
                <span class="uptime-state">{state}</span>
            </div>
            <div class="uptime-bars" style=style role="img" aria-label=label>
                {if bars.is_empty() {
                    view! {
                        <div class="uptime-empty" aria-hidden="true">
                            <span></span><span></span><span></span><span></span><span></span><span></span>
                        </div>
                    }
                    .into_any()
                } else {
                    bars.into_iter()
                        .map(|bar| {
                            let (class, word) = match bar.state {
                                MintState::Ok => ("uptime-bar ok", "Healthy"),
                                MintState::Warn => ("uptime-bar warn", "Degraded"),
                                MintState::Error => ("uptime-bar err", "Failing"),
                                MintState::Unknown => ("uptime-bar unknown", "No data"),
                            };
                            let title = format!(
                                "{} to {}: {word} ({}, {})",
                                absolute_time(bar.start_at),
                                absolute_time(bar.end_at),
                                plural(bar.checks, "check", "checks"),
                                plural(bar.failures, "failure", "failures"),
                            );
                            view! { <span class=class title=title aria-hidden="true"></span> }
                        })
                        .collect_view()
                        .into_any()
                }}
            </div>
            <div class="uptime-axis" aria-hidden="true">
                <span>{format!("{window} ago")}</span>
                <span>"Now"</span>
            </div>
        </div>
    }
    .into_any()
}

/// The state of the newest bar holding any checks. A bar is the worst result
/// of its hour, so this can be a failure even when the last check passed.
fn newest_bar_state(uptime: &MintUptime) -> Option<MintState> {
    uptime
        .bars
        .iter()
        .rev()
        .find(|bar| bar.checks > 0)
        .map(|bar| bar.state)
}

/// The line across from the uptime figure. It answers for the last check, so
/// a passed check reads as up even if the newest bar holds an earlier failure.
fn uptime_state(uptime: &MintUptime) -> String {
    match (
        newest_bar_state(uptime),
        uptime.current_up_since,
        uptime.last_checked_at,
    ) {
        (Some(_), Some(since), _) => match elapsed_since(since).as_str() {
            "just now" => "Up just now".to_string(),
            elapsed => format!("Up for {elapsed}"),
        },
        (Some(MintState::Ok), None, _) => "Up now".to_string(),
        (Some(MintState::Warn), None, _) => "Degraded at last check".to_string(),
        (Some(MintState::Error), None, _) => "Failed at last check".to_string(),
        (Some(MintState::Unknown), None, _) => "No answer at last check".to_string(),
        (None, _, Some(_)) => format!("Not checked in {}h", uptime.window_hours),
        (None, _, None) => "Not checked yet".to_string(),
    }
}

/// How many bars hold a failure and when the latest began, or nothing when
/// none do.
fn failed_bars(bars: &[MintUptimeBar]) -> Option<String> {
    let failed = bars.iter().filter(|bar| bar.failures > 0).count();
    let latest = bars.iter().rev().find(|bar| bar.failures > 0)?;
    let when = absolute_time(latest.start_at);
    Some(if failed == 1 {
        format!("1 bar shows failures, from {when}.")
    } else {
        format!("{failed} bars show failures, the latest from {when}.")
    })
}

/// Pick the most recent error attributable to this mint, preferring whichever
/// of the swap-derived metrics or the health-probe uptime carries the newer
/// event. Returns `(message, category, when)`.
fn pick_incident(
    metrics: &MintMetrics,
    uptime: Option<&MintUptime>,
) -> Option<(String, Option<String>, Option<DateTime<Utc>>)> {
    let from_metrics = metrics.last_error.clone().map(|m| {
        (
            m,
            metrics.last_error_category.clone(),
            metrics.last_error_at,
        )
    });
    let from_uptime = uptime.and_then(|u| {
        u.last_error
            .clone()
            .map(|m| (m, u.last_error_category.clone(), u.last_error_at))
    });

    match (from_metrics, from_uptime) {
        (Some(a), Some(b)) => Some(if b.2 > a.2 { b } else { a }),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

fn elapsed_since(dt: DateTime<Utc>) -> String {
    let secs = (Utc::now() - dt).num_seconds().max(0);
    match secs {
        0..=89 => "just now".to_string(),
        90..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How long one uptime bar covers, as a phrase: "one hour", "2 hours".
fn bar_span(minutes: u64) -> String {
    match minutes {
        60 => "one hour".to_string(),
        minutes if minutes > 0 && minutes % 60 == 0 => format!("{} hours", minutes / 60),
        minutes => format!("{minutes} minutes"),
    }
}

fn uptime_grid_style(bar_count: usize) -> String {
    format!("--uptime-columns: {}", bar_count.max(1))
}

#[cfg(test)]
mod tests {
    use super::{
        and_list, bar_span, contacts, failed_bars, features, nut_title, uptime_grid_style,
        uptime_state, words_class, Nut, Piece,
    };
    use crate::types::{MintState, MintUptime, MintUptimeBar};
    use chrono::{Duration, TimeZone, Utc};
    use serde_json::json;

    fn bar(state: MintState, checks: u64, failures: u64) -> MintUptimeBar {
        let start_at = Utc.with_ymd_and_hms(2026, 11, 3, 14, 0, 0).unwrap();
        MintUptimeBar {
            start_at,
            end_at: start_at + Duration::hours(1),
            state,
            checks,
            failures,
        }
    }

    #[test]
    fn the_uptime_line_answers_for_the_last_check() {
        // Passed, in an hour that also holds a failure: up, not down.
        let passed = MintUptime {
            current_up_since: Some(Utc::now() - Duration::minutes(5)),
            last_checked_at: Some(Utc::now()),
            bars: vec![bar(MintState::Error, 5, 1)],
            ..Default::default()
        };
        assert_eq!(uptime_state(&passed), "Up for 5m");

        // The first check after a failure can have just passed.
        let just = MintUptime {
            current_up_since: Some(Utc::now()),
            ..passed.clone()
        };
        assert_eq!(uptime_state(&just), "Up just now");

        let failed = MintUptime {
            current_up_since: None,
            ..passed.clone()
        };
        assert_eq!(uptime_state(&failed), "Failed at last check");

        // Up long ago, but nothing checked in the window: not "up".
        let silent = MintUptime {
            current_up_since: Some(Utc::now() - Duration::days(9)),
            last_checked_at: Some(Utc::now() - Duration::days(2)),
            bars: vec![bar(MintState::Unknown, 0, 0)],
            ..Default::default()
        };
        assert_eq!(uptime_state(&silent), "Not checked in 24h");
        assert_eq!(uptime_state(&MintUptime::default()), "Not checked yet");
    }

    #[test]
    fn failed_bars_say_how_many_and_when_the_latest_began() {
        assert_eq!(failed_bars(&[bar(MintState::Ok, 1, 0)]), None);
        assert_eq!(
            failed_bars(&[bar(MintState::Error, 1, 1)]).as_deref(),
            Some("1 bar shows failures, from 2026-11-03 14:00:00 UTC.")
        );
        assert_eq!(
            failed_bars(&[bar(MintState::Error, 1, 1), bar(MintState::Warn, 2, 1)]).as_deref(),
            Some("2 bars show failures, the latest from 2026-11-03 14:00:00 UTC.")
        );
    }

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

    #[test]
    fn uptime_grid_tracks_the_number_of_server_buckets() {
        assert_eq!(uptime_grid_style(24), "--uptime-columns: 24");
        assert_eq!(uptime_grid_style(48), "--uptime-columns: 48");
        assert_eq!(uptime_grid_style(0), "--uptime-columns: 1");
    }

    #[test]
    fn bar_spans_are_human_readable() {
        assert_eq!(bar_span(30), "30 minutes");
        assert_eq!(bar_span(60), "one hour");
        assert_eq!(bar_span(120), "2 hours");
    }
}
