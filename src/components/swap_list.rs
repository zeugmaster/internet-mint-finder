use crate::api::get_swaps;
use crate::components::badge::CategoryPill;
use crate::components::icons::{ArrowRight, Chevron};
use crate::format::{absolute_time, format_ms, format_sats, relative_time, short_url};
use crate::hooks::use_poll;
use crate::types::{MintState, PaginationParams, SwapEventRead};
use leptos::prelude::*;

#[component]
pub fn SwapList() -> impl IntoView {
    let tick = use_poll(10);
    let issues_only = RwSignal::new(false);
    let swaps = Resource::new(
        move || tick.get(),
        |_| async move { get_swaps(PaginationParams { skip: 0, limit: 50 }).await },
    );

    view! {
        <div class="card">
            <div class="panel-toolbar feed-toolbar">
                <div class="segmented" role="group" aria-label="Filter test payments">
                    <button type="button" aria-pressed=move || !issues_only.get() on:click=move |_| issues_only.set(false)>"All"</button>
                    <button type="button" aria-pressed=move || issues_only.get() on:click=move |_| issues_only.set(true)>"Problems"</button>
                </div>
                <span class="toolbar-note">"Refreshes every 10 seconds"</span>
            </div>
            <div class="feed" tabindex="0" role="region" aria-label="Recent test payments">
                <Transition fallback=move || {
                    view! {
                        {(0..6)
                            .map(|_| view! { <div class="skeleton skeleton-row"></div> })
                            .collect_view()}
                    }
                }>
                    {move || {
                        swaps
                            .get()
                            .map(|res| match res {
                                Ok(data) => {
                                    let data: Vec<_> = data.into_iter().filter(|s| !issues_only.get() || matches!(s.state, MintState::Warn | MintState::Error)).collect();
                                    if data.is_empty() {
                                        view! {
                                            <div class="empty-state">
                                                <h3>{move || if issues_only.get() { "No recent problems" } else { "No test payments yet" }}</h3>
                                                <p>{move || if issues_only.get() { "None of the latest 50 payments failed or warned." } else { "They start once at least two mints hold donated ecash." }}</p>
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
                                Err(_) => {
                                    view! {
                                        <div class="empty-state">
                                            <h3>"Couldn't load test payments"</h3>
                                            <button type="button" class="btn btn-secondary mt-4" on:click=move |_| swaps.refetch()>"Try again"</button>
                                        </div>
                                    }
                                        .into_any()
                                }
                            })
                    }}
                </Transition>
            </div>
        </div>
    }
}

#[component]
pub fn SwapItem(swap: SwapEventRead) -> impl IntoView {
    let state_class = match swap.state {
        MintState::Ok => "state-ok",
        MintState::Warn => "state-warn",
        MintState::Error => "state-err",
        MintState::Unknown => "state-unknown",
    };

    let status_word = match swap.state {
        MintState::Ok => "Settled",
        MintState::Warn => "Warning",
        MintState::Error => "Failed",
        MintState::Unknown => "Pending",
    };

    let error = swap.error.clone();
    let category = swap.category.clone();
    let has_error = error.is_some();
    let has_time = swap.time_taken > 0.0;
    let time_str = format_ms(swap.time_taken);
    let via = swap.auditor.clone();

    view! {
        <div class=format!("feed-item {state_class}")>
            <div class="feed-top">
                <div class="feed-meta">
                    <span>{status_word}</span>
                    <time datetime=swap.created_at.to_rfc3339() title=absolute_time(swap.created_at)>{relative_time(swap.created_at)}</time>
                    <Show when=move || has_time>
                        <span>{time_str.clone()}</span>
                    </Show>
                    {via.map(|pk| view! { <span title=pk.clone()>{format!("via {}", pk.get(..4).unwrap_or(&pk))}</span> })}
                </div>
                <div class="flex items-baseline gap-2">
                    <span class="feed-amount">{format!("{} sat", format_sats(swap.amount))}</span>
                    <span class="feed-fee">{format!("{} sat fee", swap.fee)}</span>
                </div>
            </div>
            <div class="feed-route">
                <a class="hop" href=format!("/mints/{}", swap.from_id) title=swap.from_url.clone()>{short_url(&swap.from_url)}</a>
                <span class="arrow"><ArrowRight/></span>
                <a class="hop" href=format!("/mints/{}", swap.to_id) title=swap.to_url.clone()>{short_url(&swap.to_url)}</a>
            </div>
            <Show when=move || has_error>
                <details class="feed-error"><summary><Chevron/>"Show error"</summary>
                    {category.clone().map(|c| view! { <CategoryPill category=c/> })}
                    <span class="feed-error-text">{error.clone().unwrap_or_default()}</span>
                </details>
            </Show>
        </div>
    }
}
