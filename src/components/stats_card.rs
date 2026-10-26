use crate::api::get_stats;
use crate::format::{format_ms, format_sats};
use crate::hooks::use_poll;
use leptos::prelude::*;

#[component]
pub fn StatsCard() -> impl IntoView {
    let tick = use_poll(15);
    let stats = Resource::new(move || tick.get(), |_| async move { get_stats().await });

    view! {
        <div class="kpi-grid">
            <Transition fallback=move || {
                view! {
                    {(0..4)
                        .map(|_| view! {
                            <div class="kpi">
                                <div class="skeleton" style="height: 10px; width: 50%;"></div>
                                <div class="skeleton mt-4" style="height: 26px; width: 75%;"></div>
                                <div class="skeleton mt-2" style="height: 10px; width: 40%;"></div>
                            </div>
                        })
                        .collect_view()}
                }
            }>
                {move || {
                    stats
                        .get()
                        .map(|res| match res {
                            Ok(data) => {
                                let volume_up = data.total_amount_swapped_24h > 0;
                                let swaps_up = data.total_swaps_24h > 0;
                                let avg_value = if data.average_swap_time > 0.0 { format_ms(data.average_swap_time) } else { "—".into() };
                                let avg_sub = if data.average_swap_time_24h > 0.0 { format!("24h: {}", format_ms(data.average_swap_time_24h)) } else { "No settlements in 24h".into() };
                                view! {
                                    <Kpi
                                        label="Volume audited"
                                        value=format_sats(data.total_amount_swapped)
                                        unit="sat"
                                        sub=format!("+{} sat / 24h", format_sats(data.total_amount_swapped_24h))
                                        sub_up=volume_up
                                    />
                                    <Kpi
                                        label="Audit attempts"
                                        value=format_sats(data.total_swaps)
                                        unit=""
                                        sub=format!("+{} / 24h", data.total_swaps_24h)
                                        sub_up=swaps_up
                                    />
                                    <Kpi
                                        label="Sats under audit"
                                        value=format_sats(data.total_balance)
                                        unit="sat"
                                        sub="across all mints".to_string()
                                        sub_up=false
                                    />
                                    <Kpi
                                        label="Avg settlement"
                                        value=avg_value
                                        unit=""
                                        sub=avg_sub
                                        sub_up=false
                                    />
                                }
                                    .into_any()
                            }
                            Err(_) => {
                                view! {
                                    <div class="callout callout-err" style="grid-column: 1 / -1;">
                                        "Overview temporarily unavailable. "
                                        <button type="button" class="text-link" on:click=move |_| stats.refetch()>"Try again"</button>
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}
            </Transition>
        </div>
    }
}

#[component]
fn Kpi(
    label: &'static str,
    value: String,
    unit: &'static str,
    sub: String,
    sub_up: bool,
) -> impl IntoView {
    view! {
        <div class="kpi">
            <div class="kpi-label">{label}</div>
            <div class="kpi-value">
                {value}
                <Show when=move || !unit.is_empty()>
                    <span class="unit">{unit}</span>
                </Show>
            </div>
            <div class="kpi-sub">
                <span class=if sub_up { "delta-up" } else { "delta-flat" }>{sub}</span>
            </div>
        </div>
    }
}
