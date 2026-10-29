use crate::components::icons::{Moon, Sun};
use crate::types::AppConfig;
use leptos::prelude::*;

/// The brand mark: the IMF tag, a holographic metal sticker, built from the
/// same layers as the files in public/brand. Decoration beside the name, so
/// hidden from screen readers.
#[component]
fn BrandTag() -> impl IntoView {
    view! {
        <span class="brand-tag" aria-hidden="true">
            <span class="brand-tag-layer metal"></span>
            <span class="brand-tag-layer holo"></span>
            <span class="brand-tag-layer glint"></span>
            <span class="brand-tag-layer grain"></span>
            <span class="brand-tag-layer bevel"></span>
            <span class="brand-tag-imf">"IMF"</span>
        </span>
    }
}

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
                    <BrandTag/>
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
