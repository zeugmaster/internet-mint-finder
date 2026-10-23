use crate::api::check_connection;
use crate::hooks::use_poll;
use crate::types::AppConfig;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// The "Circuit" brand mark: three mints orbiting a live heartbeat. Inlined as
/// raw SVG so it scales crisply and inherits no font styling.
const BRAND_MARK_SVG: &str = r##"<svg viewBox="0 0 48 48" fill="none" xmlns="http://www.w3.org/2000/svg" role="img" aria-label="Cashu Auditor">
  <circle cx="24" cy="24" r="17" stroke="#f0a43c" stroke-width="2" opacity="0.4"/>
  <path d="M13 24 H19 L21.5 16.5 L25 31.5 L27.5 24 H35" stroke="#ffc46b" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"/>
  <circle cx="24" cy="7" r="3.6" fill="#3ecf8e"/>
  <circle cx="9.3" cy="32.5" r="3.6" fill="#f0a43c"/>
  <circle cx="38.7" cy="32.5" r="3.6" fill="#f0a43c"/>
</svg>"##;

#[component]
pub fn Header() -> impl IntoView {
    let config = use_context::<Resource<AppConfig>>();
    let location = use_location();
    let tick = use_poll(15);
    let connection = Resource::new(move || tick.get(), |_| check_connection());

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
                        "Dashboard"
                    </a>
                    <a
                        href="/receive"
                        class="nav-link"
                        aria-current=move || active("/receive")
                    >
                        "Donate"
                    </a>
                    <a href="/auditors" class="nav-link" aria-current=move || active("/auditors")>
                        "Auditors"
                    </a>
                    <a href="/dashboard" class="nav-link" rel="external">
                        "Analytics"
                    </a>
                </nav>

                <div class="header-status" role="status" aria-live="polite">
                    <Transition fallback=|| view! { <span>"Connecting…"</span> }>
                        {move || connection.get().map(|result| {
                            let connected = result.is_ok();
                            view! {
                                <span class=if connected { "connection-dot connected" } else { "connection-dot disconnected" }></span>
                                <span>{if connected { "Connected" } else { "Reconnecting…" }}</span>
                            }
                        })}
                    </Transition>
                </div>
            </div>
        </header>
    }
}
