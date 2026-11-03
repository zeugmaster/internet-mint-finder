//! Dev-only: serve the analytics dashboard + `/api` against the local
//! `auditor.db`, WITHOUT starting the live auditor loops (no swaps, no network).
//! For eyeballing/screenshotting the dashboard against seeded data.
//!
//!   cargo +nightly run --features ssr --manifest-path ../Cargo.toml --example dev_dashboard
//!   (or temporarily copy to examples/ and run as above)
//!
//! Then open http://127.0.0.1:3000/dashboard

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use cdk_cashu_auditor::server::auditor::AuditorConfig;
    use cdk_cashu_auditor::server::cache::ResponseCache;
    use cdk_cashu_auditor::server::http::{api_router, paid_router};
    use cdk_cashu_auditor::server::state::{auditor_db_path, open_wallet_repository, AppState};
    use cdk_cashu_auditor::server::storage::AuditorDatabase;
    use leptos::prelude::*;
    use std::sync::Arc;

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;

    let auditor_db = Arc::new(AuditorDatabase::open(&auditor_db_path().unwrap()).unwrap());
    let wallet_repository = open_wallet_repository().await.unwrap();

    let state = AppState {
        leptos_options,
        wallet_repository,
        app_name: "Cashu Mint Auditor".to_string(),
        auditor_db,
        auditor_config: AuditorConfig::from_env(),
        cache: Arc::new(ResponseCache::new()),
        base_url: None,
        mint_url_aliases: Arc::new(Vec::new()),
        nostr: Arc::new(Default::default()),
        started_at: std::time::Instant::now(),
    };

    let app = api_router().merge(paid_router()).with_state(state);
    let addr = "127.0.0.1:3000";
    println!("dev dashboard on http://{addr}/dashboard");
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app.into_make_service()).await.unwrap();
}

#[cfg(not(feature = "ssr"))]
fn main() {}
