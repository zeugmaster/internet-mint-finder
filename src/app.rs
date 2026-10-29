use crate::api::get_app_config;
use crate::components::header::Header;
use crate::pages::home::HomePage;
use crate::pages::mint_detail::MintDetailPage;
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, HashedStylesheet, MetaTags, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};

/// Runs before first paint: the saved theme, else the system setting. The
/// footer toggle is handled here too, by delegation, so it works before (and
/// without) hydration.
const THEME_SCRIPT: &str = r#"(function(){var d=document.documentElement;d.dataset.theme=localStorage.getItem('theme')||(matchMedia('(prefers-color-scheme: dark)').matches?'dark':'light');document.addEventListener('click',function(e){if(!e.target.closest('[data-theme-toggle]'))return;var next=d.dataset.theme==='dark'?'light':'dark';d.dataset.theme=next;localStorage.setItem('theme',next);});})();"#;

/// The site description, shared by search results and link previews.
const DESCRIPTION: &str =
    "Find a Cashu mint that works: hourly uptime checks and reviews from people who use them.";

/// An Open Graph tag. The view macro doesn't know the `property` attribute,
/// so the tag is built directly, as leptos_meta does.
fn og(property: &'static str, content: impl Into<String>) -> impl IntoView {
    leptos::html::meta()
        .attr("property", property)
        .content(content.into())
}

pub fn shell(options: LeptosOptions) -> impl IntoView {
    // Link previews want an absolute image URL; without BASE_URL a path is
    // the best there is, and most previewers resolve it.
    #[cfg(feature = "ssr")]
    let preview_image = use_context::<crate::server::state::AppState>()
        .and_then(|state| state.base_url.clone())
        .map(|base| format!("{}/brand/og-image.png", base.trim_end_matches('/')))
        .unwrap_or_else(|| "/brand/og-image.png".to_string());
    #[cfg(not(feature = "ssr"))]
    let preview_image = "/brand/og-image.png".to_string();

    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="color-scheme" content="light dark"/>
                <meta name="theme-color" media="(prefers-color-scheme: light)" content="#f6f6f6"/>
                <meta name="theme-color" media="(prefers-color-scheme: dark)" content="#000000"/>
                <script inner_html=THEME_SCRIPT></script>
                <meta name="description" content=DESCRIPTION/>
                // The favicon is drawn on the 16px pixel grid; the PNGs are for
                // browsers without SVG icons. All of them come from scripts/brand.
                <link rel="icon" href="/favicon.svg" type="image/svg+xml"/>
                <link rel="icon" href="/favicon-32.png" sizes="32x32" type="image/png"/>
                <link rel="icon" href="/favicon-16.png" sizes="16x16" type="image/png"/>
                <link rel="apple-touch-icon" href="/apple-touch-icon.png"/>
                {og("og:site_name", "Internet Mint Finder")}
                {og("og:title", "Internet Mint Finder")}
                {og("og:description", DESCRIPTION)}
                {og("og:image", preview_image)}
                {og("og:image:width", "1200")}
                {og("og:image:height", "630")}
                {og("og:image:alt", "The Internet Mint Finder tag: IMF struck in a holographic metal sticker")}
                <meta name="twitter:card" content="summary_large_image"/>
                <AutoReload options=options.clone() />
                <HashedStylesheet id="leptos" options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
                <script src="/nostr-tools.js" defer></script>
                <script src="/review.js" defer></script>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    let config = Resource::new(
        || (),
        |_| async move { get_app_config().await.unwrap_or_default() },
    );
    provide_context(config);

    view! {
        <Router>
            <div class="page-wrapper">
                <Header/>
                <a href="#main-content" class="skip-link">"Skip to content"</a>
                <main id="main-content" class="flex-grow" tabindex="-1">
                    <Routes fallback=NotFound>
                        <Route path=StaticSegment("") view=HomePage/>
                        <Route
                            path=(StaticSegment("mints"), ParamSegment("id"))
                            view=MintDetailPage
                        />
                    </Routes>
                </main>
                <Footer/>
            </div>
        </Router>
    }
}

#[component]
fn NotFound() -> impl IntoView {
    view! {
        <Title text="Page not found — Internet Mint Finder"/>
        <div class="shell shell-narrow">
            <div class="card card-pad empty-state">
                <h1 class="page-title">"Page not found"</h1>
                <p>"There's nothing at this address."</p>
                <div class="mt-6">
                    <a href="/" class="btn btn-primary">"Go to the directory"</a>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Footer() -> impl IntoView {
    view! {
        <footer class="site-footer">
            <div class="site-footer-inner">
                <p>"Uptime is measured by this server every hour. Results are measurements, not endorsements."</p>
            </div>
        </footer>
    }
}
