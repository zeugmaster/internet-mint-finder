use crate::components::icons::{Moon, Sun};
use crate::types::AppConfig;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// The brand mark: an assay hallmark, a shield punch struck with an M. Inlined
/// as raw SVG in the inverted colors so it follows the theme.
const BRAND_MARK_SVG: &str = r##"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
  <path d="M3 3h18v11c0 4-4 6.6-9 8-5-1.4-9-4-9-8z" fill="var(--inverted)"/>
  <path d="M7.5 15.5v-7l4.5 4.5 4.5-4.5v7" fill="none" stroke="var(--on-inverted)" stroke-width="2" stroke-linejoin="round"/>
</svg>"##;

/// Brand, the three sections, and the theme switch. public/dashboard.html
/// repeats this markup, so keep the two in step.
#[component]
pub fn Header() -> impl IntoView {
    let config = use_context::<Resource<AppConfig>>();
    let location = use_location();

    let active = move |path: &'static str| {
        let current = location.pathname.get();
        let is_active = if path == "/" {
            current == "/" || current.starts_with("/mints/")
        } else {
            current.starts_with(path)
        };
        if is_active {
            Some("page")
        } else {
            None
        }
    };

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

                <nav class="site-nav" aria-label="Main navigation">
                    <a href="/" class="nav-link" aria-current=move || active("/")>
                        "Directory"
                    </a>
                    <a href="/auditors" class="nav-link" aria-current=move || active("/auditors")>
                        "Auditors"
                    </a>
                    <a href="/dashboard" class="nav-link" rel="external">
                        "Analytics"
                    </a>
                </nav>

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
