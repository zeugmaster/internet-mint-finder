use crate::api::search_mints;
use crate::components::badge::StateBadge;
use crate::format::{absolute_time, format_sats, relative_time, short_url};
use crate::hooks::use_poll;
use crate::types::{MintRead, MintState};
use leptos::prelude::*;

#[component]
pub fn MintList() -> impl IntoView {
    let tick = use_poll(15);
    let search = RwSignal::new(String::new());
    let applied_search = RwSignal::new(String::new());
    let state_filter = RwSignal::new(None::<MintState>);
    let page = RwSignal::new(0u64);
    Effect::new(move |_| {
        let value = search.get();
        if let Ok(handle) = set_timeout_with_handle(
            move || {
                applied_search.set(value);
                page.set(0);
            },
            std::time::Duration::from_millis(250),
        ) {
            on_cleanup(move || handle.clear());
        }
    });
    let mints = Resource::new(
        move || {
            (
                tick.get(),
                applied_search.get(),
                state_filter.get(),
                page.get(),
            )
        },
        |(_, search, state, page)| async move { search_mints(search, state, page).await },
    );

    view! {
        <div class="card mint-panel">
            <div class="panel-toolbar">
                <div class="search-field">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/></svg>
                    <input type="search" aria-label="Search mints by name or URL" placeholder="Find a mint…"
                        prop:value=move || search.get() on:input=move |ev| search.set(event_target_value(&ev))/>
                </div>
                <select class="filter-select" aria-label="Filter mints by health" on:change=move |ev| {
                    state_filter.set(match event_target_value(&ev).as_str() {
                        "healthy" => Some(MintState::Ok), "degraded" => Some(MintState::Warn),
                        "failing" => Some(MintState::Error), "pending" => Some(MintState::Unknown), _ => None,
                    });
                    page.set(0);
                }>
                    <option value="all">"All statuses"</option><option value="healthy">"Healthy"</option>
                    <option value="degraded">"Degraded"</option><option value="failing">"Failing"</option>
                    <option value="pending">"Pending"</option>
                </select>
            </div>
            <Transition fallback=|| view! { <div class="loading-rows" role="status" aria-label="Loading mints">
                {(0..5).map(|_| view! { <div class="skeleton skeleton-row"></div> }).collect_view()}
            </div> }>
                {move || mints.get().map(|result| match result {
                    Ok(data) => {
                        let total = data.total;
                        let has_next = page.get().saturating_add(1).saturating_mul(20) < total;
                        let empty = data.mints.is_empty();
                        view! {
                            <div class="table-wrap" role="region" aria-label="Mint health and balances" tabindex="0">
                                <table class="data-table">
                                    <caption class="sr-only">"Tracked mints and their latest audit results"</caption>
                                    <thead><tr>
                                        <th scope="col">"Mint"</th><th scope="col">"Status"</th>
                                        <th scope="col" class="cell-center">"Auditors (24h)"</th>
                                        <th scope="col" class="cell-right">"Success (24h)"</th>
                                        <th scope="col" class="cell-right">"Reviews"</th>
                                        <th scope="col" class="cell-right">"Balance"</th>
                                        <th scope="col" class="cell-right">"Updated"</th>
                                    </tr></thead>
                                    <tbody>{data.mints.into_iter().map(|mint| view! { <MintRow mint/> }).collect_view()}</tbody>
                                </table>
                            </div>
                            <Show when=move || empty>
                                <div class="empty-state">
                                    <h3>"No mints to show"</h3>
                                    <p>"Try a different search or status. Mints appear from NIP-87 announcements on Nostr or from a donation of their ecash."</p>
                                    <a href="/receive" class="text-link">"Add a mint with ecash →"</a>
                                </div>
                            </Show>
                            <div class="panel-footer">
                                <span>{format!("{} {}", total, if total == 1 { "mint" } else { "mints" })}</span>
                                <div class="pagination">
                                    <button type="button" class="btn btn-ghost btn-small" disabled=move || page.get() == 0
                                        on:click=move |_| page.update(|p| *p = p.saturating_sub(1))>"← Previous"</button>
                                    <span class="page-number">{move || format!("{} / {}", page.get() + 1, total.div_ceil(20).max(1))}</span>
                                    <button type="button" class="btn btn-ghost btn-small" disabled=!has_next
                                        on:click=move |_| page.update(|p| *p = p.saturating_add(1))>"Next →"</button>
                                </div>
                            </div>
                        }.into_any()
                    }
                    Err(_) => view! { <div class="empty-state" role="alert">
                        <h3>"Mints are temporarily unavailable"</h3><p>"Check your connection or try again."</p>
                        <button type="button" class="btn btn-secondary mt-4" on:click=move |_| mints.refetch()>"Try again"</button>
                    </div> }.into_any(),
                })}
            </Transition>
        </div>
    }
}

#[component]
fn MintRow(mint: MintRead) -> impl IntoView {
    let detail_href = format!("/mints/{}", mint.id);
    let display_name = if mint.name.trim().is_empty() {
        short_url(&mint.url)
    } else {
        mint.name.clone()
    };
    let url_short = short_url(&mint.url);
    let url_title = mint.url.clone();

    // Balance vs donation target meter (capped at 100%).
    let target = mint.sum_donations.max(1);
    let fill_pct = ((mint.balance as f64 / target as f64) * 100.0).min(100.0);

    let err_class = if mint.auditors_ok_24h < mint.auditors_24h {
        "err-count some"
    } else {
        "err-count none"
    };

    let navigate = leptos_router::hooks::use_navigate();
    let row_href = detail_href.clone();
    let unfunded = mint.sum_donations == 0;
    let peer_only = unfunded && mint.auditors_24h > 0;
    let fund_href = format!("/receive?mint={}", urlencode(&mint.url));
    let auditors_cell = if mint.auditors_24h > 0 {
        format!("{} / {} OK", mint.auditors_ok_24h, mint.auditors_24h)
    } else {
        String::new()
    };
    let success_cell = mint
        .success_rate_24h
        .map(|r| format!("{:.0}%", r * 100.0))
        .unwrap_or_else(|| "—".to_string());
    let reviews_cell = match (mint.review_count, mint.review_avg) {
        (0, _) => "—".to_string(),
        (n, Some(avg)) => format!("★ {avg:.1} · {n}"),
        (n, None) => n.to_string(),
    };

    view! {
        <tr
            class="row-link"
            on:click=move |_| navigate(&row_href, Default::default())
        >
            <td>
                // Real link so the row is keyboard- and screen-reader-accessible;
                // the row click is a pointer convenience on top.
                <a
                    class="mint-name"
                    href=detail_href.clone()
                    on:click=|ev| ev.stop_propagation()
                >
                    {display_name}
                </a>
                <div class="mint-url" title=url_title>{url_short}</div>
            </td>
            <td>
                <StateBadge state=mint.state/>
                <Show when=move || peer_only>
                    <div class="mint-url" title="Known from peer auditors; donate from this mint to audit it here too">"observed via peers"</div>
                </Show>
            </td>
            <td class="cell-center">
                {if auditors_cell.is_empty() {
                    view! { <a class="text-link" href=fund_href.clone() on:click=|ev| ev.stop_propagation()>"fund an audit →"</a> }.into_any()
                } else {
                    view! { <span class=err_class>{auditors_cell.clone()}</span> }.into_any()
                }}
            </td>
            <td class="cell-num">{success_cell}</td>
            <td class="cell-num" title=format!("{} review(s) on Nostr", mint.review_count)>{reviews_cell}</td>
            <td class="cell-num">
                {if unfunded { "—".to_string() } else if mint.balance_available { format!("{} sat", format_sats(mint.balance)) } else { "Unavailable".to_string() }}
                <div class="meter" hidden=unfunded || !mint.balance_available title=format!("target {} sat", format_sats(mint.sum_donations))>
                    <div class="meter-fill" style=format!("width: {fill_pct:.0}%")></div>
                </div>
            </td>
            <td class="cell-num text-dim" title=absolute_time(mint.updated_at)>{relative_time(mint.updated_at)}</td>
        </tr>
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
