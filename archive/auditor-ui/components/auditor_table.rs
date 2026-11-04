use crate::components::badge::StateBadge;
use crate::components::hallmarks::StampKey;
use crate::format::{format_ms, relative_time, short_pubkey};
use crate::types::AuditorMintView;
use leptos::prelude::*;

/// One row per auditor: its own state and 24 h numbers for a mint. Rows are
/// shown side by side on purpose; nothing is merged into a single verdict.
#[component]
pub fn AuditorTable(views: Vec<AuditorMintView>) -> impl IntoView {
    view! {
        <div class="table-wrap" role="region" aria-label="Results per auditor" tabindex="0">
            <table class="data-table stack-table">
                <thead><tr>
                    <th scope="col">"Auditor"</th><th scope="col">"State"</th>
                    <th scope="col" class="cell-right">"24h attempts"</th>
                    <th scope="col" class="cell-right">"24h success"</th>
                    <th scope="col" class="cell-right">"Avg settle"</th>
                    <th scope="col" class="cell-right">"Last error"</th>
                </tr></thead>
                <tbody>
                    {views.into_iter().map(|v| {
                        let name = if v.auditor.name.trim().is_empty() { short_pubkey(&v.auditor.npub) } else { v.auditor.name.clone() };
                        let label = if v.auditor.is_self { format!("{name} (this instance)") } else { name };
                        let rate = v.metrics.success_rate_24h().map(|r| format!("{:.0}%", r * 100.0)).unwrap_or_else(|| "—".into());
                        let settle = if v.metrics.avg_latency_ms_24h > 0.0 { format_ms(v.metrics.avg_latency_ms_24h) } else { "—".into() };
                        let last_error = v.metrics.last_error_at.map(relative_time).unwrap_or_else(|| "—".into());
                        let error_title = v.metrics.last_error.clone().unwrap_or_default();
                        view! {
                            <tr>
                                <td><div class="auditor-cell"><StampKey pubkey=v.auditor.pubkey.clone()/><div><span class="mint-name">{label}</span><div class="mint-url" title=v.auditor.npub.clone()>{short_pubkey(&v.auditor.npub)}</div></div></div></td>
                                <td data-label="State"><StateBadge state=v.state/></td>
                                <td class="cell-num" data-label="24h attempts">{v.metrics.attempts_24h}</td>
                                <td class="cell-num" data-label="24h success">{rate}</td>
                                <td class="cell-num" data-label="Avg settle">{settle}</td>
                                <td class="cell-num text-dim" data-label="Last error" title=error_title>{last_error}</td>
                            </tr>
                        }
                    }).collect_view()}
                </tbody>
            </table>
        </div>
    }
}
