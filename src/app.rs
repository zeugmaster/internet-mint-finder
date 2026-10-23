use crate::api::{get_app_config, get_auditors};
use crate::components::header::Header;
use crate::pages::auditors::AuditorsPage;
use crate::pages::home::HomePage;
use crate::pages::mint_detail::MintDetailPage;
use crate::pages::receive::ReceivePage;
use leptos::prelude::*;
use leptos_meta::{provide_meta_context, HashedStylesheet, MetaTags, Title};
use leptos_router::{
    components::{Route, Router, Routes},
    ParamSegment, StaticSegment,
};

pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <meta name="color-scheme" content="dark"/>
                <meta name="theme-color" content="#0a0d13"/>
                <meta
                    name="description"
                    content="Continuous, real-money audits of Cashu mints: uptime, latency, fees, and failures, measured by actually moving ecash."
                />
                <link rel="icon" href="/favicon.svg" type="image/svg+xml"/>
                <AutoReload options=options.clone() />
                <HashedStylesheet id="leptos" options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
                <script src="/review.js" defer></script>
                <link rel="preconnect" href="https://fonts.googleapis.com"/>
                <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin=""/>
                <link
                    href="https://fonts.googleapis.com/css2?family=Archivo:wght@400;500;600;700;800;900&family=JetBrains+Mono:wght@400;500;600;700&display=swap"
                    rel="stylesheet"
                />
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
                        <Route path=StaticSegment("receive") view=ReceivePage/>
                        <Route path=StaticSegment("auditors") view=AuditorsPage/>
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
        <div class="shell shell-narrow text-center">
            <div class="card card-pad empty-state" style="margin-top: 48px;">
                <span class="glyph">"∅"</span>
                <h1 class="page-title" style="font-size: 22px;">"Page not found"</h1>
                <p class="mt-2">"The page you are looking for does not exist."</p>
                <div class="mt-6">
                    <a href="/" class="btn btn-primary">"Back to console"</a>
                </div>
            </div>
        </div>
    }
}

#[component]
fn Footer() -> impl IntoView {
    // Mirrors come from the auditors' Nostr announcements, not from config, so
    // a visitor can find another instance even if this domain disappears.
    let auditors = Resource::new(|| (), |_| get_auditors());
    view! {
        <footer class="site-footer">
            <div class="site-footer-inner">
                <span>
                    "Audits run by moving real ecash between mints. "
                    "Results are measurements, not endorsements. "
                    <Suspense fallback=|| ()>
                        {move || auditors.get().and_then(|r| r.ok()).map(|list| {
                            let mirrors: Vec<String> = list.into_iter().filter(|a| !a.is_self).flat_map(|a| a.urls).collect();
                            (!mirrors.is_empty()).then(|| view! {
                                "Mirrors: "
                                {mirrors.into_iter().map(|u| view! { <a href=u.clone() rel="external noopener">{u.clone()}</a>" " }).collect_view()}
                            })
                        })}
                    </Suspense>
                </span>
                <span class="footer-meta">
                    <a
                        href="https://github.com/cashubtc/cdk"
                        rel="external noopener"
                        target="_blank"
                    >
                        "built on CDK"
                    </a>
                    " · NUT-18 donations · "
                    <a href="/auditors">"auditors"</a>
                </span>
            </div>
        </footer>
    }
}
