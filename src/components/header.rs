use crate::api::{check_connection, get_auditors};
use crate::hooks::use_poll;
use crate::types::{AppConfig, AuditorRead};
use chrono::Utc;
use leptos::prelude::*;
use leptos_router::hooks::use_location;

/// The brand mark: an assay hallmark, a shield punch struck with an M. Inlined
/// as raw SVG in the ink colors so it follows the theme.
const BRAND_MARK_SVG: &str = r##"<svg viewBox="0 0 24 24" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
  <path d="M3 3h18v11c0 4-4 6.6-9 8-5-1.4-9-4-9-8z" fill="var(--ink)"/>
  <path d="M7.5 15.5v-7l4.5 4.5 4.5-4.5v7" fill="none" stroke="var(--surface)" stroke-width="2" stroke-linejoin="round"/>
</svg>"##;

#[component]
pub fn Header() -> impl IntoView {
    let config = use_context::<Resource<AppConfig>>();
    let location = use_location();
    let tick = use_poll(15);
    let connection = Resource::new(move || tick.get(), |_| check_connection());
    let auditors = Resource::new(move || tick.get(), |_| get_auditors());

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
                            let label = match auditors.get() {
                                Some(Ok(list)) if connected => live_label(&list),
                                _ if connected => "Connected".to_string(),
                                _ => "Reconnecting…".to_string(),
                            };
                            view! {
                                <span class=if connected { "connection-dot connected" } else { "connection-dot disconnected" }></span>
                                <span title="This instance plus peers seen publishing in the last 24 hours">{label}</span>
                            }
                        })}
                    </Transition>
                </div>
            </div>
        </header>
    }
}

/// "3 auditors live": this instance plus every peer seen in the last 24 h.
fn live_label(list: &[AuditorRead]) -> String {
    let cutoff = Utc::now() - chrono::Duration::hours(24);
    let live = list
        .iter()
        .filter(|a| a.is_self || a.last_seen.is_some_and(|at| at >= cutoff))
        .count();
    if live == 1 {
        "1 auditor live".to_string()
    } else {
        format!("{live} auditors live")
    }
}
