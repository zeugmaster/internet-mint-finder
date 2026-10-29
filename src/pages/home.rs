use crate::components::mint_list::MintList;
use leptos::prelude::*;
use leptos_meta::Title;

/// The directory: just the mints, with their ratings and uptime.
#[component]
pub fn HomePage() -> impl IntoView {
    view! {
        <Title text="Directory — Internet Mint Finder"/>
        <div class="shell">
            <div class="page-head">
                <div>
                    <h1 class="page-title">"Find a Cashu mint."</h1>
                    <p class="page-subtitle">
                        "Cashu mints found on Nostr, rated by the people who use them. "
                        "Each one is checked every hour to see if it's online."
                    </p>
                </div>
            </div>
            <MintList/>
        </div>
    }
}
