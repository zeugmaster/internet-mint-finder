use crate::components::brand::HeroTag;
use crate::components::mint_list::MintList;
use leptos::prelude::*;
use leptos_meta::Title;

/// The directory: just the mints, with their ratings and uptime, under a
/// head that carries the brand tag at full size.
#[component]
pub fn HomePage() -> impl IntoView {
    view! {
        <Title text="Directory — Internet Mint Finder"/>
        <div class="shell">
            <div class="page-head has-tag">
                <h1 class="page-title">"Find a Cashu mint."</h1>
                <p class="page-subtitle">
                    "Cashu mints found on Nostr, with the reviews posted there. "
                    "Each mint is checked every hour to see if it's online."
                </p>
                <HeroTag/>
            </div>
            <MintList/>
        </div>
    }
}
