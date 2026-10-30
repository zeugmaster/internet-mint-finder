use crate::api::{
    get_mint_by_id, get_mint_metrics, get_mint_reviews, get_mint_uptime,
};
use crate::components::badge::{CategoryPill, StateBadge};
use crate::components::icons::{Check, Cross, Help};
use crate::components::mint_avatar::MintAvatar;
use crate::components::mint_list::{CopyText, CopyUrl};
use crate::components::reviews::ReviewsBlock;
use crate::format::{
    absolute_time, format_sats, middle_ellipsis, relative_time, relative_time_from, short_url,
};
use crate::hooks::use_poll;
use crate::types::{MintMetrics, MintRead, MintState, MintUptime};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos::web_sys::HtmlElement;
use leptos_meta::Title;
use leptos_router::hooks::use_params_map;

/// One mint for a wallet user: is it up, what people say, and what the mint
/// says about itself. Audit detail (per-auditor results, swaps, funding) is
/// left off. Each section is a card with its heading outside, on the page.
/// The page polls every 15 seconds and keeps the last good data through a
/// failed refresh.
#[component]
pub fn MintDetailPage() -> impl IntoView {
    let params = use_params_map();
    let mint_id = Signal::derive(move || {
        params
            .get()
            .get("id")
            .and_then(|id| id.parse::<u64>().ok())
            .unwrap_or(0)
    });

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
                                <nav class="breadcrumb" aria-label="Breadcrumb"><a href="/">"Directory"</a></nav>
                                <div class="empty-state">"This mint isn't in the directory."</div>
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

                    <section aria-labelledby="about-title">
                        <h2 class="section-title" id="about-title">"About this mint"</h2>
                        <Transition fallback=|| ()>
                            <div class="arrives">
                                {move || current_mint().and_then(|r| r.ok()).flatten().map(|mint| view! { <About mint/> })}
                            </div>
                        </Transition>
                    </section>
                </div>
                }))}
            </Transition>
        </div>
    }
}

/// The mint's avatar, name, state and copyable URL, and the latest failure
/// verbatim when the mint is not healthy.
#[component]
fn MintHead(mint: MintRead, metrics: Option<MintMetrics>, uptime: Option<MintUptime>) -> impl IntoView {
    let metrics = metrics.unwrap_or_default();
    let display_name = if mint.name.trim().is_empty() {
        short_url(&mint.url)
    } else {
        mint.name.clone()
    };
    let page_title = format!("{display_name} — Internet Mint Finder");

    // The freshest error attributable to *this* mint, whether it came from a
    // paid swap or a free health probe, with its category and timestamp.
    let incident = match (
        matches!(mint.state, MintState::Error | MintState::Warn),
        pick_incident(&metrics, uptime.as_ref()),
    ) {
        (true, Some((message, category, at))) => {
            let when = at.map(relative_time).unwrap_or_else(|| "recently".to_string());
            let pill = category.map(|c| view! { <CategoryPill category=c/> });
            Some(view! {
                <div class="incident">
                    <div class="incident-head">
                        <span class="incident-title">"Latest failure"</span>
                        {pill}
                        <span class="incident-when mono">{when}</span>
                    </div>
                    <div class="incident-msg mono">{message}</div>
                </div>
            })
        }
        _ => None,
    };
    view! {
        <Title text=page_title/>
        // A breadcrumb in words; navigation never gets a back arrow.
        <nav class="breadcrumb" aria-label="Breadcrumb">
            <a href="/">"Directory"</a>
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
    }
}

/// What the mint says about itself, for a wallet user, in reading order: its
/// own words, then one card of what it offers and how to reach it.
#[component]
fn About(mint: MintRead) -> impl IntoView {
    let info: Option<serde_json::Value> = mint
        .info
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let Some(info) = info else {
        return view! {
            <p class="section-empty">"This mint hasn't shared any details about itself."</p>
        }
        .into_any();
    };
    let text = |key: &str| -> Option<String> {
        info.get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    let description = text("description_long").or_else(|| text("description"));
    let motd = text("motd");
    // "Nutshell/0.16.3" reads as a name and a version.
    let software = text("version").map(|v| v.replacen('/', " ", 1));
    let terms = text("tos_url").filter(|url| url.starts_with("https://"));
    let contacts = contacts(&info);
    let features = info.get("nuts").and_then(|v| v.as_object()).map(features);
    let has_meta = !contacts.is_empty() || software.is_some() || terms.is_some();
    let note = format!(
        "As listed by the mint. Fetched {}.",
        relative_time(mint.updated_at)
    );
    let fetched = absolute_time(mint.updated_at);

    view! {
        <div class="about">
            {description.map(|d| view! { <p class="about-text" dir="auto">{d}</p> })}
            {motd.map(|m| view! {
                <div class="callout about-notice">
                    <span class="about-notice-label">"Notice from the mint"</span>
                    <p dir="auto">{m}</p>
                </div>
            })}
            {if features.is_some() || has_meta {
                view! { <Offers features contacts software terms note fetched/> }.into_any()
            } else {
                view! {
                    <p class="section-empty">"The mint doesn't list what it offers or how to reach it."</p>
                }
                .into_any()
            }}
        </div>
    }
    .into_any()
}

/// What the mint offers and how to reach it, in one card: Lightning in and
/// out side by side with their limits, since those decide whether the mint
/// fits; the rest as a two-column checklist, a check for offered and a cross
/// for not; then the operator's contacts, software and terms; and at the
/// foot, where it all comes from.
#[component]
fn Offers(
    features: Option<Vec<Feature>>,
    contacts: Vec<Contact>,
    software: Option<String>,
    terms: Option<String>,
    note: String,
    fetched: String,
) -> impl IntoView {
    let has_contact = !contacts.is_empty() || software.is_some() || terms.is_some();
    view! {
        <div class="card offers">
            {match features {
                Some(list) => view! { <OfferRows list/> }.into_any(),
                None => view! {
                    <p class="offers-empty">"The mint doesn't list what it offers."</p>
                }
                .into_any(),
            }}
            // Two small ledgers on the card's two columns: how to reach the
            // operator, then the mint's own facts.
            {has_contact.then(|| view! {
                <div class="offers-meta">
                    {(!contacts.is_empty()).then(|| view! {
                        <dl class="offers-ledger">
                            {contacts.into_iter().map(|contact| view! {
                                <div>
                                    <dt>{contact.label}</dt>
                                    <dd><CopyText text=contact.value shown=contact.shown/></dd>
                                </div>
                            }).collect_view()}
                        </dl>
                    })}
                    {(software.is_some() || terms.is_some()).then(|| view! {
                        <dl class="offers-ledger">
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
                </div>
            })}
            <p class="offers-note" title=fetched>{note}</p>
        </div>
    }
}

/// The features: Lightning cells, then the checklist.
#[component]
fn OfferRows(list: Vec<Feature>) -> impl IntoView {
    let (lightning, rest): (Vec<Feature>, Vec<Feature>) =
        list.into_iter().partition(|f| matches!(f.nut, 4 | 5));
    view! {
        <dl class="offers-lightning">
            {lightning.into_iter().map(|feature| {
                let Feature { name, offered, detail, .. } = feature;
                view! {
                    <div class="offers-cell" class:is-off=!offered>
                        <dt>{name}</dt>
                        <dd><Pieces detail/></dd>
                    </div>
                }
            }).collect_view()}
        </dl>
        <ul class="offers-list" aria-label="Features">
            {rest.into_iter().map(|feature| {
                let Feature { name, offered, detail, .. } = feature;
                let unlisted = !offered && detail.is_empty();
                let has_detail = !detail.is_empty();
                view! {
                    <li class="offers-item" class:is-off=!offered>
                        {if offered { view! { <Check/> }.into_any() } else { view! { <Cross/> }.into_any() }}
                        <span>
                            {name}
                            {unlisted.then(|| view! { <span class="sr-only">", not listed by the mint"</span> })}
                        </span>
                        {has_detail.then(|| view! { <span class="offers-item-detail"><Pieces detail/></span> })}
                    </li>
                }
            }).collect_view()}
        </ul>
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

/// A wallet-facing capability in plain words, read from the mint's NUT-06
/// info.
#[derive(Debug, PartialEq)]
struct Feature {
    name: &'static str,
    nut: u8,
    offered: bool,
    detail: Vec<Piece>,
}

/// A feature's detail: words, with amounts kept apart so they set as figures.
#[derive(Debug, PartialEq)]
enum Piece {
    Text(String),
    Sats(u64),
}

/// The same list on every mint, most useful first, so two mints compare line
/// by line. Only what the mint lists as supported counts as offered.
fn features(nuts: &serde_json::Map<String, serde_json::Value>) -> Vec<Feature> {
    let listed = |nut: u8| {
        nuts.get(&nut.to_string())
            .and_then(|v| v.get("supported"))
            .is_some_and(|s| {
                s.as_bool() == Some(true) || s.as_array().is_some_and(|a| !a.is_empty())
            })
    };
    let plain = |nut: u8, name: &'static str| {
        let offered = listed(nut);
        Feature {
            name,
            nut,
            offered,
            detail: Vec::new(),
        }
    };
    // Clear (NUT-21) or blind (NUT-22) auth: the mint wants an account.
    let account = nuts.contains_key("21") || nuts.contains_key("22");
    vec![
        lightning(nuts, 4, "Add funds with Lightning"),
        lightning(nuts, 5, "Pay out to Lightning"),
        Feature {
            name: "Use without an account",
            nut: 21,
            offered: !account,
            detail: if account {
                text("Sign-in required")
            } else {
                Vec::new()
            },
        },
        plain(9, "Restore from a seed phrase"),
        plain(7, "See when sent ecash is claimed"),
        plain(11, "Lock ecash to a key"),
        plain(12, "Check ecash offline"),
        plain(8, "Refund unused Lightning fees"),
    ]
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
        nut,
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
        let named = |unit: &&str| {
            if *unit == "sat" {
                "sat".to_string()
            } else {
                unit.to_uppercase()
            }
        };
        if detail.is_empty() {
            let all: Vec<String> = units.iter().map(named).collect();
            detail.push(Piece::Text(format!("In {}", all.join(" and "))));
        } else {
            let others: Vec<String> = units.iter().filter(|u| **u != "sat").map(named).collect();
            detail.push(Piece::Text(format!(", also {}", others.join(" and "))));
        }
    }
    // An offered cell is never blank.
    if detail.is_empty() {
        detail = text("No limits listed");
    }
    Feature {
        name,
        nut,
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
            let value = c.get("info")?.as_str()?.trim().to_string();
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
    let latest = uptime.bars.iter().rev().find(|bar| bar.checks > 0).map(|bar| bar.state);
    let state = match (latest, uptime.current_up_since, uptime.last_checked_at) {
        (Some(MintState::Ok), Some(since), _) => format!("Up for {}", elapsed_since(since)),
        (Some(MintState::Ok), None, _) => "Up now".to_string(),
        (Some(MintState::Warn), _, _) => "Degraded at last check".to_string(),
        (Some(MintState::Error), _, _) => "Down at last check".to_string(),
        (Some(MintState::Unknown), _, _) => "No answer at last check".to_string(),
        (None, _, Some(_)) => format!("No checks in {}h", uptime.window_hours),
        (None, _, None) => "No checks yet".to_string(),
    };
    let window = format!("{}h", uptime.window_hours);
    let checks = if uptime.checks == 0 {
        format!("No checks in the last {window}")
    } else {
        format!("{} of {} checks passed", uptime.successes, uptime.checks)
    };
    let last = match (uptime.checks, uptime.last_checked_at) {
        (0, _) | (_, None) => String::new(),
        (_, Some(ts)) => format!(", the last one {}", relative_time(ts)),
    };
    let note = format!(
        "{checks}{last}. Each bar is {}; grey means no data, not downtime.",
        bar_span(uptime.bucket_minutes)
    );
    let label = format!("Observed uptime, last {window}: {value}. {checks}.");
    let style = uptime_grid_style(uptime.bars.len());
    let bars = uptime.bars;

    view! {
        <div class="card card-pad uptime-card">
            <div class="uptime-head">
                <span class="uptime-figure">
                    <span class="uptime-value">{value}</span>
                    // Shown on hover or focus (a tap focuses it on touch);
                    // Escape hides it again.
                    <span class="help-tip">
                        <button
                            type="button"
                            class="help-tip-button"
                            aria-describedby="uptime-note"
                            on:keydown=|ev: leptos::ev::KeyboardEvent| {
                                if ev.key() == "Escape" {
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
                                MintState::Ok => ("uptime-bar ok", "Up"),
                                MintState::Warn => ("uptime-bar warn", "Degraded"),
                                MintState::Error => ("uptime-bar err", "Down"),
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
    use super::{bar_span, contacts, features, uptime_grid_style, Piece};
    use serde_json::json;

    fn nuts(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().cloned().unwrap()
    }

    #[test]
    fn features_count_only_what_the_mint_lists_as_supported() {
        let list = features(&nuts(json!({
            "4": {"methods": [{"method": "bolt11", "unit": "sat", "min_amount": 1, "max_amount": 500000}], "disabled": false},
            "5": {"methods": [{"method": "bolt11", "unit": "sat", "min_amount": 100, "max_amount": 250000}], "disabled": true},
            "7": {"supported": true},
            "9": {"supported": false},
            "17": {"supported": [{"method": "bolt11", "unit": "sat", "commands": ["proof_state"]}]},
            "22": {"bat_max_mint": 50}
        })));
        let by_name = |name: &str| list.iter().find(|f| f.name == name).unwrap();

        let deposit = by_name("Add funds with Lightning");
        assert!(deposit.offered);
        assert_eq!(
            deposit.detail,
            [
                Piece::Text("Up to ".into()),
                Piece::Sats(500_000),
                Piece::Text(" sat".into())
            ]
        );
        let payout = by_name("Pay out to Lightning");
        assert!(!payout.offered);
        assert_eq!(payout.detail, [Piece::Text("Paused by the mint".into())]);
        assert!(by_name("See when sent ecash is claimed").offered);
        // Listed, but marked unsupported.
        assert!(!by_name("Restore from a seed phrase").offered);
        assert!(!by_name("Check ecash offline").offered);
        let account = by_name("Use without an account");
        assert!(!account.offered);
        assert_eq!(account.detail, [Piece::Text("Sign-in required".into())]);
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
            list[0].detail,
            [
                Piece::Sats(1000),
                Piece::Text(" to ".into()),
                Piece::Sats(2_000_000),
                Piece::Text(" sat".into()),
                Piece::Text(", also USD".into()),
            ]
        );
        assert_eq!(list[1].detail, [Piece::Text("In sat and EUR".into())]);
        assert!(list[1].offered);
    }

    #[test]
    fn contacts_get_plain_labels_and_npubs_cut_in_the_middle() {
        let list = contacts(&json!({"contact": [
            {"method": "email", "info": "ops@mint.example"},
            {"method": "nostr", "info": "npub1qe3e5wrvnsgpggtkytxteaqfprz0rgxr8c3l34kk3a9t7e2l3acslezefe"},
            {"method": "twitter", "info": "@mint"},
            {"method": "matrix", "info": " "}
        ]}));
        let labels: Vec<&str> = list.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Email", "Nostr", "X"]);
        assert_eq!(list[0].shown, "ops@mint.example");
        assert_eq!(list[1].shown, "npub1qe3e5w…ezefe");
        assert!(list[1].value.ends_with("ezefe") && list[1].value.len() == 63);
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
