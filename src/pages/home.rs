use crate::components::mint_list::MintList;
use leptos::prelude::*;
use leptos_meta::Title;

/// The directory: just the mints. Totals live on Analytics; audit activity
/// and how the audit works live on Auditors.
#[component]
pub fn HomePage() -> impl IntoView {
    view! {
        <Title text="Directory — Internet Mint Finder"/>
        <div class="shell">
            <div class="page-head">
                <div>
                    <h1 class="page-title">"Find a Cashu mint."</h1>
                    <p class="page-subtitle">
                        "Cashu mints found on Nostr. Independent auditors check that each one is online "
                        "and send real payments through the ones they fund."
                    </p>
                </div>
            </div>
            <MintList/>
        </div>
    }
}
