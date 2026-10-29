use crate::api::{
    get_mint_by_id, get_mint_metrics, get_mint_reviews, get_mint_uptime,
};
use crate::components::badge::{CategoryPill, StateBadge};
use crate::components::mint_avatar::MintAvatar;
use crate::components::mint_list::CopyUrl;
use crate::components::reviews::ReviewsBlock;
use crate::format::{absolute_time, relative_time, short_url};
use crate::hooks::use_poll;
use crate::types::{MintMetrics, MintRead, MintState, MintUptime};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_params_map;

/// One mint for a wallet user: is it up, what people say, and what the mint
/// says about itself. Audit detail (per-auditor results, swaps, funding) is
/// left off. The one card holds the
/// uptime chart, an instrument read as a whole; text sits on the page with
/// 0.5px rules; headings sit outside.
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

    view! {
        <div class="shell">
            <Transition fallback=|| {
                view! {
                    <div class="skeleton" style="height: 36px; width: min(320px, 100%);"></div>
                    <div class="skeleton mt-4" style="height: 14px; width: min(480px, 100%);"></div>
                }
            }>
                {move || {
                    let metrics_data = metrics.get().and_then(|r| r.ok());
                    let uptime_data = uptime.get().and_then(|r| r.ok());
                    mint.get().map(|res| match res {
                        Ok(Some(mint)) => {
                            view! { <MintHead mint=mint metrics=metrics_data uptime=uptime_data/> }.into_any()
                        }
                        Ok(None) => view! {
                            <Title text="Mint not found — Internet Mint Finder"/>
                            <nav class="breadcrumb" aria-label="Breadcrumb"><a href="/">"Directory"</a></nav>
                            <div class="empty-state">"This mint isn't in the directory."</div>
                        }
                        .into_any(),
                        Err(e) => view! {
                            <div class="callout callout-err" title=e.to_string()>"Couldn't load this mint. Refresh the page to try again."</div>
                        }
                        .into_any(),
                    })
                }}
            </Transition>

            <Transition fallback=|| ()>
                {move || exists.get().map(|found| matches!(found, Ok(true)).then(|| view! {
                <div class="mint-sections">
                    <section aria-labelledby="uptime-title">
                        <h2 class="section-title" id="uptime-title">"Observed uptime"</h2>
                        <Transition fallback=|| view! { <div class="skeleton" style="height: 136px;"></div> }>
                            {move || uptime.get().map(|res| view! { <UptimePanel uptime=res.ok()/> })}
                        </Transition>
                    </section>

                    <section aria-labelledby="reviews-title">
                        <h2 class="section-title" id="reviews-title">"Reviews"</h2>
                        <Transition fallback=|| view! { <div class="skeleton" style="height: 120px;"></div> }>
                            {move || reviews.get().map(|res| match res {
                                Ok(data) => view! { <ReviewsBlock data/> }.into_any(),
                                Err(e) => view! { <p class="section-empty" title=e.to_string()>"Couldn't load reviews. Refresh the page to try again."</p> }.into_any(),
                            })}
                        </Transition>
                    </section>

                    <section aria-labelledby="about-title">
                        <h2 class="section-title" id="about-title">"About this mint"</h2>
                        <dl class="kv-ledger">
                            <Transition fallback=|| ()>
                                {move || mint.get().and_then(|r| r.ok()).flatten().map(|mint| view! { <AboutRows mint/> })}
                            </Transition>
                        </dl>
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
    let metrics_available = metrics.is_some();
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
            <span aria-current="page">{display_name.clone()}</span>
        </nav>
        <div class="detail-head">
            <MintAvatar name=display_name.clone() info=mint.info.clone()/>
            <div class="detail-id">
                <div class="detail-title">
                    <h1 class="page-title">{display_name}</h1>
                    <StateBadge state=mint.state/>
                </div>
                <CopyUrl url=mint.url.clone()/>
            </div>
        </div>
        {incident}
        <Show when=move || !metrics_available>
            <div class="callout mt-4" role="status">"Some audit data didn't load. Retrying every 15 seconds."</div>
        </Show>
    }
}

/// The mint's own NUT-06 description, as label and value rows.
#[component]
fn AboutRows(mint: MintRead) -> impl IntoView {
    let info: Option<serde_json::Value> = mint
        .info
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let text = |key: &str| -> Option<String> {
        info.as_ref()
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
    };
    let description = text("description_long").or_else(|| text("description"));
    let motd = text("motd");
    let version = text("version");
    let contacts: Vec<String> = info
        .as_ref()
        .and_then(|v| v.get("contact"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let method = c.get("method")?.as_str()?;
                    let target = c.get("info")?.as_str()?;
                    (!target.trim().is_empty()).then(|| format!("{method}: {target}"))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut nuts: Vec<u64> = info
        .as_ref()
        .and_then(|v| v.get("nuts"))
        .and_then(|v| v.as_object())
        .map(|map| map.keys().filter_map(|k| k.parse().ok()).collect())
        .unwrap_or_default();
    nuts.sort_unstable();

    view! {
        {description.map(|d| view! { <dt>"About"</dt><dd>{d}</dd> })}
        {motd.map(|m| view! { <dt>"Notice"</dt><dd>{m}</dd> })}
        {(!contacts.is_empty()).then(|| view! { <dt>"Contact"</dt><dd class="mono">{contacts.join(" · ")}</dd> })}
        {version.map(|v| view! { <dt>"Version"</dt><dd class="mono">{v}</dd> })}
        {(!nuts.is_empty()).then(|| view! {
            <dt>"Supports"</dt>
            <dd><div class="tag-list">
                {nuts.into_iter().map(|n| view! { <span class="tag">{format!("NUT-{n:02}")}</span> }).collect_view()}
            </div></dd>
        })}
        <dt>"Info updated"</dt>
        <dd class="mono">{absolute_time(mint.updated_at)}</dd>
    }
}

/// The 24-hour strip of hourly results, its figure, and one line on what it
/// rests on.
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
                <span class="uptime-value">{value}</span>
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
            <p class="uptime-note">{note}</p>
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
    use super::{bar_span, uptime_grid_style};

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
