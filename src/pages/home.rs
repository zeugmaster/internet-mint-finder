use crate::components::brand::Masthead;
use crate::components::mint_list::MintList;
use crate::types::AppConfig;
use leptos::prelude::*;
use leptos_meta::Title;

/// The directory: just the mints, with their ratings and uptime, under a
/// masthead after the IMF's: the sticker with the seal struck on it and the
/// name in steel capitals under it.
#[component]
pub fn HomePage() -> impl IntoView {
    let config = use_context::<Resource<AppConfig>>();

    view! {
        <Title text="Directory — Internet Mint Finder"/>
        <div class="shell">
            <Masthead>
                <Suspense fallback=|| "Internet Mint Finder".to_string()>
                    {move || {
                        config
                            .and_then(|c| c.get())
                            .map(|c| c.app_name.clone())
                            .unwrap_or_else(|| "Internet Mint Finder".to_string())
                    }}
                </Suspense>
            </Masthead>
            <MintList/>
        </div>
    }
}
