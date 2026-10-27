use crate::api::get_auditors;
use crate::components::hallmarks::StampKey;
use crate::components::swap_list::SwapList;
use crate::format::{absolute_time, relative_time, short_pubkey};
use crate::hooks::use_poll;
use crate::types::AuditorRead;
use leptos::prelude::*;
use leptos_meta::Title;

/// Every auditor whose observations this instance shows: itself plus the
/// peers announced on Nostr. Each runs its own wallet and key.
#[component]
pub fn AuditorsPage() -> impl IntoView {
    let tick = use_poll(30);
    let auditors = Resource::new(move || tick.get(), |_| get_auditors());

    view! {
        <Title text="Auditors — Internet Mint Finder"/>
        <div class="shell">
            <div class="page-head">
                <div>
                    <h1 class="page-title">"Independent auditors."</h1>
                    <p class="page-subtitle">
                        "Auditors are independent servers that hold ecash and test mints with real payments. "
                        "Each one publishes its results under its own Nostr key; this site shows them side by side "
                        "and never merges them into one score."
                    </p>
                </div>
            </div>
            <div class="card">
                <Transition fallback=|| view! { <div class="loading-rows">{(0..2).map(|_| view! { <div class="skeleton skeleton-row"></div> }).collect_view()}</div> }>
                    {move || auditors.get().map(|result| match result {
                        Ok(list) => view! {
                            <div class="table-wrap" role="region" aria-label="Auditors" tabindex="0">
                                <table class="data-table stack-table">
                                    <thead><tr>
                                        <th scope="col">"Auditor"</th><th scope="col">"Website"</th>
                                        <th scope="col">"Software"</th><th scope="col" class="cell-right">"Tests every"</th>
                                        <th scope="col" class="cell-right">"Last seen"</th><th scope="col">"Fund"</th>
                                    </tr></thead>
                                    <tbody>{list.into_iter().map(|a| view! { <AuditorRow auditor=a/> }).collect_view()}</tbody>
                                </table>
                            </div>
                        }.into_any(),
                        Err(e) => view! { <div class="empty-state" role="alert" title=e.to_string()><h3>"Couldn't load auditors"</h3><p>"This retries every 15 seconds."</p></div> }.into_any(),
                    })}
                </Transition>
            </div>

            <div class="grid-main mt-6">
                <section>
                    <div class="section-head"><h2>"Recent test payments"</h2><span>"Latest 50"</span></div>
                    <SwapList/>
                </section>
                <section>
                    <h2 class="section-title">"How the audit works"</h2>
                    <ol class="steps-list">
                        <li class="step-card">
                            <h3><span class="step-num">"1"</span>"One mint issues an invoice"</h3>
                            <p>
                                "The auditor asks a mint for a Lightning invoice, just as your "
                                "wallet does when you top up."
                            </p>
                        </li>
                        <li class="step-card">
                            <h3><span class="step-num">"2"</span>"Another mint pays it"</h3>
                            <p>
                                "The auditor spends ecash it holds on a second mint to pay that "
                                "invoice over Lightning. Real sats move every time."
                            </p>
                        </li>
                        <li class="step-card">
                            <h3><span class="step-num">"3"</span>"Every result is published"</h3>
                            <p>
                                "How long it took, the fee, and any error message are recorded "
                                "word for word and shown here and on Analytics."
                            </p>
                        </li>
                    </ol>
                </section>
            </div>
        </div>
    }
}

#[component]
fn AuditorRow(auditor: AuditorRead) -> impl IntoView {
    let name = if auditor.name.trim().is_empty() {
        short_pubkey(&auditor.pubkey)
    } else {
        auditor.name.clone()
    };
    let cadence = if auditor.frequency_secs == 0 {
        "—".to_string()
    } else {
        format!("~{} min", auditor.frequency_secs.div_ceil(60))
    };
    let npub = auditor.npub.clone();
    let pubkey = auditor.pubkey.clone();
    let donate = auditor.donate.clone();
    let is_self = auditor.is_self;
    view! {
        <tr>
            <td>
                <div class="auditor-cell">
                    <StampKey pubkey=pubkey/>
                    <div>
                        <span class="mint-name">{name}{if is_self { " (this instance)" } else { "" }}</span>
                        <div class="mint-url" title=npub.clone()>{short_pubkey(&npub)}</div>
                    </div>
                </div>
            </td>
            <td data-label="Website">
                {if auditor.urls.is_empty() { view! { <span class="text-dim">"—"</span> }.into_any() } else {
                    auditor.urls.iter().map(|u| view! { <div><a class="text-link" href=u.clone() rel="external noopener">{u.clone()}</a></div> }).collect_view().into_any()
                }}
            </td>
            <td class="text-dim" data-label="Software">{auditor.software.clone()}</td>
            <td class="cell-num" data-label="Tests every">{cadence}</td>
            <td class="cell-num text-dim" data-label="Last seen">
                {match auditor.last_seen { Some(at) => view! { <span title=absolute_time(at)>{relative_time(at)}</span> }.into_any(), None => view! { <span>"—"</span> }.into_any() }}
            </td>
            <td data-label="Fund">
                {if is_self { view! { <a class="btn btn-primary btn-small" href="/receive">"Donate ecash"</a> }.into_any() }
                 else { match donate { Some(pr) => view! { <code class="mint-url" title=pr.clone()>{short_pubkey(&pr)}</code> }.into_any(), None => view! { <span class="text-dim">"—"</span> }.into_any() } }}
            </td>
        </tr>
    }
}
