use crate::api::{
    get_mint_announcement, get_mint_auditor_views, get_mint_by_id, get_mint_donations,
    get_mint_metrics, get_mint_reviews, get_mint_uptime, get_swaps_by_mint,
};
use crate::components::auditor_table::AuditorTable;
use crate::components::badge::{CategoryPill, StateBadge};
use crate::components::hallmarks::Hallmarks;
use crate::components::icons::ArrowLeft;
use crate::components::reviews::{AnnouncementCard, MeasuredSummary, RatingSummary, ReviewList};
use crate::components::swap_list::SwapItem;
use crate::format::{absolute_time, format_ms, format_sats, relative_time, short_url};
use crate::hooks::use_poll;
use crate::types::{MintMetrics, MintRead, MintState, MintUptime, PaginationParams};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_params_map;

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

    let metrics = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_metrics(id).await },
    );

    let uptime = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_uptime(id).await },
    );

    let donations = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_donations(id, 6).await },
    );

    let swaps = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move {
            get_swaps_by_mint(id, None, PaginationParams { skip: 0, limit: 50 }).await
        },
    );

    let views = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_auditor_views(id).await },
    );

    let announcement = Resource::new(
        move || (mint_id.get(), tick.get()),
        |(id, _)| async move { get_mint_announcement(id).await },
    );

    // Loaded once per page, not on the poll: a refresh would rebuild the
    // review form and drop what someone is typing.
    let reviews = Resource::new(
        move || mint_id.get(),
        |id| async move { get_mint_reviews(id).await },
    );

    view! {
        <div class="shell">
            <a href="/" class="back-link"><ArrowLeft/>"Directory"</a>

            <Transition fallback=|| {
                view! {
                    <div class="skeleton" style="height: 36px; width: min(320px, 100%);"></div>
                    <div class="skeleton mt-4" style="height: 14px; width: min(480px, 100%);"></div>
                }
            }>
                {move || {
                    let metrics_data = metrics.get().and_then(|r| r.ok());
                    let uptime_data = uptime.get().and_then(|r| r.ok());
                    mint.get()
                        .map(|res| match res {
                            Ok(Some(mint)) => {
                                view! { <MintDetail mint=mint metrics=metrics_data uptime=uptime_data/> }
                                    .into_any()
                            }
                            Ok(None) => {
                                view! {
                                    <Title text="Mint not found — Internet Mint Finder"/>
                                    <div class="card card-pad empty-state">
                                        "This mint is not tracked by the auditor."
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(e) => {
                                view! {
                                    <div class="callout callout-err">
                                        {format!("Failed to load mint: {e}")}
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}
            </Transition>

            <section class="mt-6" aria-labelledby="reviews-title">
                <h2 class="section-title" id="reviews-title">"Reviews"</h2>
                <div class="reviews">
                    <div class="reviews-summary">
                        <Transition fallback=move || view! { <div class="skeleton" style="height: 120px;"></div> }>
                            {move || reviews.get().map(|res| {
                                let list = res.map(|d| d.reviews).unwrap_or_default();
                                view! { <RatingSummary reviews=list/> }
                            })}
                        </Transition>
                        <Transition fallback=|| ()>
                            {move || {
                                let mint_data = mint.get().and_then(|r| r.ok()).flatten();
                                let metrics_data = metrics.get().and_then(|r| r.ok());
                                let uptime_data = uptime.get().and_then(|r| r.ok());
                                view! { <MeasuredSummary mint=mint_data metrics=metrics_data uptime=uptime_data/> }
                            }}
                        </Transition>
                    </div>
                    <div class="reviews-main">
                        <Transition fallback=move || view! { <div class="skeleton skeleton-row"></div> }>
                            {move || reviews.get().map(|res| match res {
                                Ok(data) => view! { <ReviewList data/> }.into_any(),
                                Err(e) => view! { <div class="empty-state">{format!("Failed to load reviews: {e}")}</div> }.into_any(),
                            })}
                        </Transition>
                    </div>
                </div>
            </section>

            <div class="grid-main mt-6">
                <section>
                    <h2 class="section-title">"Vantage points"</h2>
                    <div class="card">
                        <Transition fallback=move || view! { <div class="skeleton skeleton-row"></div> }>
                            {move || views.get().map(|res| match res {
                                Ok(views) => view! { <AuditorTable views/> }.into_any(),
                                Err(e) => view! { <div class="empty-state">{format!("Failed to load auditors: {e}")}</div> }.into_any(),
                            })}
                        </Transition>
                    </div>
                    <h2 class="section-title mt-6">"Swap history"</h2>
                    <div class="card">
                        <div class="feed">
                            <Transition fallback=move || {
                                view! {
                                    {(0..4)
                                        .map(|_| view! { <div class="skeleton skeleton-row"></div> })
                                        .collect_view()}
                                }
                            }>
                                {move || {
                                    swaps
                                        .get()
                                        .map(|res| match res {
                                            Ok(data) => {
                                                if data.is_empty() {
                                                    view! {
                                                        <div class="empty-state">
                                                            "No swaps recorded for this mint yet. "
                                                            "Audits begin once it holds funds."
                                                        </div>
                                                    }
                                                        .into_any()
                                                } else {
                                                    data.into_iter()
                                                        .map(|swap| view! { <SwapItem swap=swap/> })
                                                        .collect_view()
                                                        .into_any()
                                                }
                                            }
                                            Err(e) => {
                                                view! {
                                                    <div class="empty-state">
                                                        {format!("Failed to load swaps: {e}")}
                                                    </div>
                                                }
                                                    .into_any()
                                            }
                                        })
                                }}
                            </Transition>
                        </div>
                    </div>
                </section>

                <section>
                    <h2 class="section-title">"On Nostr"</h2>
                    <Transition fallback=move || view! { <div class="skeleton skeleton-row"></div> }>
                        {move || announcement.get().map(|res| match res {
                            Ok(a) => view! { <AnnouncementCard announcement=a/> }.into_any(),
                            Err(e) => view! { <div class="empty-state">{format!("Failed to load announcement: {e}")}</div> }.into_any(),
                        })}
                    </Transition>
                    <h2 class="section-title mt-6">"Recent donations"</h2>
                    <div class="card card-pad">
                        <Transition fallback=move || {
                            view! { <div class="skeleton skeleton-row"></div> }
                        }>
                            {move || {
                                donations
                                    .get()
                                    .map(|res| match res {
                                        Ok(data) => {
                                            if data.is_empty() {
                                                view! {
                                                    <div class="empty-state">
                                                        "No donations yet. Be the first to fund "
                                                        "audits of this mint."
                                                    </div>
                                                }
                                                    .into_any()
                                            } else {
                                                view! {
                                                    <div class="donation-list">
                                                        {data
                                                            .into_iter()
                                                            .map(|d| {
                                                                let via = if d.via == "nut18" {
                                                                    "payment request"
                                                                } else {
                                                                    "token"
                                                                };
                                                                view! {
                                                                    <div class="donation-row">
                                                                        <span class="amount">
                                                                            {format!("+{} sat", format_sats(d.amount))}
                                                                        </span>
                                                                        <span class="meta">
                                                                            {format!("{} · {}", via, relative_time(d.created_at))}
                                                                        </span>
                                                                    </div>
                                                                }
                                                            })
                                                            .collect_view()}
                                                    </div>
                                                }
                                                    .into_any()
                                            }
                                        }
                                        Err(e) => {
                                            view! {
                                                <div class="empty-state">
                                                    {format!("Failed to load donations: {e}")}
                                                </div>
                                            }
                                                .into_any()
                                        }
                                    })
                            }}
                        </Transition>
                    </div>
                </section>
            </div>
        </div>
    }
}

#[component]
fn MintDetail(
    mint: MintRead,
    metrics: Option<MintMetrics>,
    uptime: Option<MintUptime>,
) -> impl IntoView {
    let metrics_available = metrics.is_some();
    let metrics = metrics.unwrap_or_default();
    let display_name = if mint.name.trim().is_empty() {
        short_url(&mint.url)
    } else {
        mint.name.clone()
    };
    let page_title = format!("{display_name} — Internet Mint Finder");

    // Pull the interesting fields out of the cached NUT-06 mint info.
    let info: Option<serde_json::Value> = mint
        .info
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok());
    let info_str = |key: &str| -> Option<String> {
        info.as_ref()
            .and_then(|v| v.get(key))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
    };
    let version = info_str("version");
    let description = info_str("description_long").or_else(|| info_str("description"));
    let motd = info_str("motd");

    let contacts: Vec<(String, String)> = info
        .as_ref()
        .and_then(|v| v.get("contact"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let method = c.get("method")?.as_str()?.to_string();
                    let target = c.get("info")?.as_str()?.to_string();
                    (!target.trim().is_empty()).then_some((method, target))
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

    let success_rate = metrics
        .success_rate_24h()
        .or_else(|| metrics.success_rate());
    let success_label = if metrics.attempts_24h > 0 {
        "Success rate (24h)"
    } else {
        "Success rate"
    };
    let success_value = success_rate
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "—".to_string());

    let avg_value = if metrics.successes > 0 {
        format_ms(if metrics.successes_24h > 0 {
            metrics.avg_latency_ms_24h
        } else {
            metrics.avg_latency_ms
        })
    } else {
        "—".to_string()
    };

    let last_success_value = if metrics_available {
        metrics
            .last_success_at
            .map(relative_time)
            .unwrap_or_else(|| "No settlements".to_string())
    } else {
        "Unavailable".to_string()
    };

    // Surface the freshest error attributable to *this* mint, whether it came
    // from a paid swap or a free health probe, with its category and timestamp.
    let incident_view = match (
        matches!(mint.state, MintState::Error | MintState::Warn),
        pick_incident(&metrics, uptime.as_ref()),
    ) {
        (true, Some((message, category, at))) => {
            let when = at
                .map(relative_time)
                .unwrap_or_else(|| "recently".to_string());
            let pill = category.map(|c| view! { <CategoryPill category=c/> });
            Some(
                view! {
                    <div class="incident">
                        <div class="incident-head">
                            <span class="incident-title">"Most recent failure"</span>
                            {pill}
                            <span class="incident-when mono">{when}</span>
                        </div>
                        <div class="incident-msg mono">{message}</div>
                    </div>
                }
                .into_any(),
            )
        }
        _ => None,
    };

    let donate_href = format!("/receive?mint={}", urlencode(&mint.url));
    let marks = mint.auditor_marks.clone();

    view! {
        <Title text=page_title/>

        <div class="detail-head">
            <h1 class="page-title">{display_name}</h1>
            <StateBadge state=mint.state/>
            {(!marks.is_empty()).then(|| view! { <Hallmarks marks=marks.clone()/> })}
        </div>
        <p class="detail-url">
            <a href=mint.url.clone() rel="external noopener" target="_blank">
                {mint.url.clone()}
            </a>
        </p>

        {incident_view}
        <Show when=move || !metrics_available>
            <div class="callout mt-4" role="status">"Audit metrics are temporarily unavailable. Retrying automatically."</div>
        </Show>

        <div class="stat-chips">
            <div class="stat-chip">
                <div class="label">{success_label}</div>
                <div class="value">{success_value}</div>
            </div>
            <div class="stat-chip">
                <div class="label">"Avg response"</div>
                <div class="value">{avg_value}</div>
            </div>
            <div class="stat-chip">
                <div class="label">"Audits run"</div>
                <div class="value">
                    {if metrics_available { format_sats(metrics.attempts) } else { "Unavailable".into() }}
                    <span class="text-dim" style="font-size: 12px;">
                        {if metrics_available { format!(" (+{} 24h)", metrics.attempts_24h) } else { String::new() }}
                    </span>
                </div>
            </div>
            <div class="stat-chip">
                <div class="label">"Last success"</div>
                <div class="value">{last_success_value}</div>
            </div>
            <div class="stat-chip">
                <div class="label">"Balance"</div>
                <div class="value">{if mint.balance_available { format!("{} sat", format_sats(mint.balance)) } else { "Unavailable".into() }}</div>
            </div>
            <div class="stat-chip">
                <div class="label">"Donation target"</div>
                <div class="value">{format!("{} sat", format_sats(mint.sum_donations))}</div>
            </div>
            <div class="stat-chip">
                <div class="label">"Swaps in / out"</div>
                <div class="value">{format!("{} / {}", mint.n_mints, mint.n_melts)}</div>
            </div>
            <div class="stat-chip">
                <div class="label">"Errors"</div>
                <div class="value">{mint.n_errors}</div>
            </div>
        </div>

        <UptimePanel uptime=uptime/>

        <div class="card card-pad">
            <div class="flex justify-between items-center flex-wrap gap-4">
                <dl class="kv-list">
                    <Show when={
                        let version = version.clone();
                        move || version.is_some()
                    }>
                        <dt>"Version"</dt>
                        <dd class="mono">{version.clone().unwrap_or_default()}</dd>
                    </Show>
                    <Show when={
                        let description = description.clone();
                        move || description.is_some()
                    }>
                        <dt>"About"</dt>
                        <dd>{description.clone().unwrap_or_default()}</dd>
                    </Show>
                    <Show when={
                        let motd = motd.clone();
                        move || motd.is_some()
                    }>
                        <dt>"Notice"</dt>
                        <dd>{motd.clone().unwrap_or_default()}</dd>
                    </Show>
                    <Show when={
                        let contacts = contacts.clone();
                        move || !contacts.is_empty()
                    }>
                        <dt>"Contact"</dt>
                        <dd class="mono" style="font-size: 12px;">
                            {contacts
                                .iter()
                                .map(|(method, target)| format!("{method}: {target}"))
                                .collect::<Vec<_>>()
                                .join(" · ")}
                        </dd>
                    </Show>
                    <Show when={
                        let nuts = nuts.clone();
                        move || !nuts.is_empty()
                    }>
                        <dt>"Supports"</dt>
                        <dd>
                            <div class="tag-list">
                                {nuts
                                    .iter()
                                    .map(|n| view! { <span class="tag">{format!("NUT-{n:02}")}</span> })
                                    .collect_view()}
                            </div>
                        </dd>
                    </Show>
                    <dt>"Last update"</dt>
                    <dd class="mono">{absolute_time(mint.updated_at)}</dd>
                </dl>
                <a href=donate_href class="btn btn-primary">"Fund this mint"</a>
            </div>
        </div>
    }
}

#[component]
fn UptimePanel(uptime: Option<MintUptime>) -> impl IntoView {
    let (
        window_label,
        uptime_value,
        current_label,
        checks_label,
        sampling_label,
        observed_label,
        last_checked_label,
        bars,
    ) = if let Some(uptime) = uptime {
        let window_label = format!("Last {}h", uptime.window_hours);
        let uptime_value = uptime
            .uptime_percent
            .map(|percent| format!("{percent:.1}%"))
            .unwrap_or_else(|| "No samples".to_string());
        let latest_observation = uptime
            .bars
            .iter()
            .rev()
            .find(|bar| bar.checks > 0)
            .map(|bar| bar.state);
        let current_label = match (
            latest_observation,
            uptime.current_up_since,
            uptime.last_checked_at,
        ) {
            (Some(MintState::Ok), Some(since), _) => {
                format!("Up for {}", elapsed_since(since))
            }
            (Some(MintState::Ok), None, _) => "Latest check reachable".to_string(),
            (Some(MintState::Warn), _, _) => "Latest check degraded".to_string(),
            (Some(MintState::Error), _, _) => "Latest check failed".to_string(),
            (Some(MintState::Unknown), _, _) => "Latest check unavailable".to_string(),
            (None, _, Some(_)) => "No checks in this window".to_string(),
            (None, _, None) => "No checks yet".to_string(),
        };
        let checks_label = if uptime.checks == 0 {
            "No checks in this window".to_string()
        } else {
            format!(
                "{} of {} checks operational · {}",
                uptime.successes,
                uptime.checks,
                count_noun(uptime.failures, "issue")
            )
        };
        let sampling_label = observation_interval(uptime.bucket_minutes);
        let observed_label = uptime
            .first_seen_at
            .map(|ts| format!("First seen {}", relative_time(ts)))
            .unwrap_or_else(|| "No audit history".to_string());
        let last_checked_label = uptime
            .last_checked_at
            .map(|ts| format!("Last check {}", relative_time(ts)))
            .unwrap_or_else(|| "Awaiting first check".to_string());

        (
            window_label,
            uptime_value,
            current_label,
            checks_label,
            sampling_label,
            observed_label,
            last_checked_label,
            uptime.bars,
        )
    } else {
        (
            "Last 24h".to_string(),
            "Unavailable".to_string(),
            "Unable to load checks".to_string(),
            "".to_string(),
            "Hourly buckets".to_string(),
            "".to_string(),
            "Retrying automatically".to_string(),
            Vec::new(),
        )
    };

    let timeline_label =
        format!("Observed audit uptime, {window_label}: {uptime_value}. {checks_label}.");
    let timeline_style = uptime_grid_style(bars.len());
    let window_start_label = format!("{} ago", window_label.trim_start_matches("Last "));
    let explainer = format!(
        "Availability observations grouped into {}. Info checks measure reachability; payment failures remain until a swap succeeds. Gaps mean no data, not downtime.",
        sampling_label.to_lowercase()
    );

    view! {
        <div class="card card-pad uptime-card">
            <div class="uptime-head">
                <div>
                    <div class="section-title compact">"Observed uptime"</div>
                    <div class="uptime-window mono">{window_label}</div>
                </div>
                <div class="uptime-primary">
                    <span class="uptime-value">{uptime_value}</span>
                    <span class="uptime-state">{current_label}</span>
                </div>
            </div>

            <p class="uptime-explainer">{explainer}</p>

            <div
                class="uptime-bars"
                style=timeline_style
                role="img"
                aria-label=timeline_label
            >
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
                            let (class, label) = match bar.state {
                                MintState::Ok => ("uptime-bar ok", "OK"),
                                MintState::Warn => ("uptime-bar warn", "WARN"),
                                MintState::Error => ("uptime-bar err", "ERROR"),
                                MintState::Unknown => ("uptime-bar unknown", "NO SAMPLE"),
                            };
                            let title = format!(
                                "{} to {}: {} ({} checks, {} issues)",
                                absolute_time(bar.start_at),
                                absolute_time(bar.end_at),
                                label,
                                bar.checks,
                                bar.failures
                            );
                            view! { <span class=class title=title aria-hidden="true"></span> }
                        })
                        .collect_view()
                        .into_any()
                }}
            </div>
            <div class="uptime-axis" aria-hidden="true">
                <span>{window_start_label}</span>
                <span>"Now"</span>
            </div>

            <div class="uptime-legend">
                <span><i class="lgd ok"></i>"Operational"</span>
                <span><i class="lgd warn"></i>"Degraded"</span>
                <span><i class="lgd err"></i>"Down"</span>
                <span><i class="lgd unknown"></i>"No data"</span>
            </div>

            <div class="uptime-meta">
                <span>{checks_label}</span>
                <span>{sampling_label}</span>
                <span>{observed_label}</span>
                <span>{last_checked_label}</span>
            </div>
        </div>
    }
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

fn count_noun(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn observation_interval(minutes: u64) -> String {
    match minutes {
        60 => "Hourly buckets".to_string(),
        minutes if minutes > 0 && minutes % 60 == 0 => {
            format!("{}-hour buckets", minutes / 60)
        }
        minutes => format!("{minutes}-minute buckets"),
    }
}

fn uptime_grid_style(bar_count: usize) -> String {
    format!("--uptime-columns: {}", bar_count.max(1))
}

/// Minimal percent-encoding for embedding a URL in a query string.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{observation_interval, uptime_grid_style};

    #[test]
    fn uptime_grid_tracks_the_number_of_server_buckets() {
        assert_eq!(uptime_grid_style(24), "--uptime-columns: 24");
        assert_eq!(uptime_grid_style(48), "--uptime-columns: 48");
        assert_eq!(uptime_grid_style(0), "--uptime-columns: 1");
    }

    #[test]
    fn observation_intervals_are_human_readable() {
        assert_eq!(observation_interval(30), "30-minute buckets");
        assert_eq!(observation_interval(60), "Hourly buckets");
        assert_eq!(observation_interval(120), "2-hour buckets");
    }
}
