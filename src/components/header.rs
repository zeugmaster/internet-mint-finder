use crate::components::icons::{Moon, Sun};
use crate::types::AppConfig;
use leptos::prelude::*;

/// The brand mark: an assay hallmark, a shield punch struck with an M. Inlined
/// as raw SVG in the inverted colors so it follows the theme.
const BRAND_MARK_SVG: &str = r##"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
  <path d="M3 3h18v11c0 4-4 6.6-9 8-5-1.4-9-4-9-8z" fill="var(--inverted)"/>
  <path d="M7.5 15.5v-7l4.5 4.5 4.5-4.5v7" fill="none" stroke="var(--on-inverted)" stroke-width="2" stroke-linejoin="round"/>
</svg>"##;

/// Brand and the theme switch. The directory is the only section, so there is
/// no nav; the brand links home. public/dashboard.html carries its own copy
/// of this markup, with the nav it needs.
#[component]
pub fn Header() -> impl IntoView {
    let config = use_context::<Resource<AppConfig>>();

    view! {
        <header class="site-header">
            <div class="site-header-inner">
                <a href="/" class="brand">
                    <span class="brand-mark" inner_html=BRAND_MARK_SVG></span>
                    <span>
                        <Suspense fallback=|| "Mint Finder".to_string()>
                            {move || {
                                config
                                    .and_then(|c| c.get())
                                    .map(|c| c.app_name.clone())
                                    .unwrap_or_else(|| "Mint Finder".to_string())
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
