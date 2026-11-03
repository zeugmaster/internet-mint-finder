use crate::components::brand::BrandTag;
use crate::components::icons::{Moon, Sun};
use crate::components::mint_list::DirectoryMemory;
use crate::types::AppConfig;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// Brand and the theme switch. The directory is the only section, so there is
/// no nav; the brand links home. On the directory itself the head carries the
/// tag, so the brand there is the name alone. public/dashboard.html carries
/// its own copy of this markup, with the nav it needs.
#[component]
pub fn Header() -> impl IntoView {
    let config = use_context::<Resource<AppConfig>>();
    let location = use_location();
    let on_home = move || location.pathname.get() == "/";
    let memory = expect_context::<DirectoryMemory>();

    view! {
        <header class="site-header">
            <div class="site-header-inner">
                // Home means the top of the list, not back to where the reader was.
                <a href="/" class="brand" on:click=move |_| memory.return_to.set_value(None)>
                    <Show when=move || !on_home()>
                        <BrandTag/>
                    </Show>
                    <span>
                        <Suspense fallback=|| "Internet Mint Finder".to_string()>
                            {move || {
                                config
                                    .and_then(|c| c.get())
                                    .map(|c| c.app_name.clone())
                                    .unwrap_or_else(|| "Internet Mint Finder".to_string())
                            }}
                        </Suspense>
                    </span>
                </a>

                // The click is handled by the theme script in the shell, so the
                // switch works before (and without) hydration.
                <button type="button" class="btn btn-ghost btn-icon theme-toggle" data-theme-toggle="">
                    <span class="when-light"><Moon/><span class="sr-only">"Use dark mode"</span></span>
                    <span class="when-dark"><Sun/><span class="sr-only">"Use light mode"</span></span>
                </button>
            </div>
        </header>
    }
}
