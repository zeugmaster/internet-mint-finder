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

pub fn shell(options: LeptosOptions) -> impl IntoView {
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
                <meta
                    name="description"
                    content="Find a Cashu mint that works: hourly uptime checks and reviews from people who use them."
                />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml"/>
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
