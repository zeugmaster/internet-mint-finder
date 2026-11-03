#![recursion_limit = "512"]

//! Isolated UI preview with synthetic records and empty, in-memory wallets.
//! No auditor loops, production files, or mint requests. Do not send real ecash.
//! Build assets first: cargo +nightly leptos build
//! Run: RUSTFLAGS="--cfg erase_components" cargo +nightly run --no-default-features --features ssr --example preview
#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use cdk::nuts::CurrencyUnit;
    use cdk::wallet::WalletRepositoryBuilder;
    use cdk_cashu_auditor::app::{shell, App};
    use cdk_cashu_auditor::server::{
        auditor::AuditorConfig,
        cache::ResponseCache,
        http::api_router,
        state::AppState,
        storage::{Auditor, AuditorDatabase, Review, SwapEvent},
    };
    use cdk_cashu_auditor::types::MintState;
    use chrono::{Duration, Utc};
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use std::sync::Arc;

    let db = Arc::new(AuditorDatabase::open_in_memory()?);
    let wallets = Arc::new(
        WalletRepositoryBuilder::new()
            .localstore(Arc::new(
                cdk_sqlite::WalletSqliteDatabase::new(":memory:").await?,
            ))
            .seed([0; 64])
            .build()
            .await?,
    );
    let names = [
        "Northstar",
        "Copper",
        "Paper Moon",
        "Juniper",
        "Signal",
        "Atlas",
        "Common Ground",
    ];
    let mut mints = Vec::new();
    for (i, name) in names.iter().enumerate() {
        let url = format!("https://{}.example", name.to_lowercase().replace(' ', "-"));
        let mut mint = db
            .record_donation(&url, (i as u64 + 1) * 1200, "token")
            .await?;
        mint.name = name.to_string();
        mint.state = match i {
            2 => MintState::Warn,
            4 => MintState::Error,
            6 => MintState::Unknown,
            _ => MintState::Ok,
        };
        mint.n_mints = 90 + i as u64 * 23;
        mint.n_melts = 85 + i as u64 * 19;
        mint.n_errors = if i == 4 { 12 } else { i as u64 };
        let mut info = serde_json::json!({"version": "Preview/1.0", "description": "Synthetic mint for interface development. No real funds.", "nuts": {"4": {}, "5": {}, "7": {}, "18": {}}});
        // One real icon, one that fails to load (the initial shows), the rest none.
        // Northstar also has a key, so its review form is open in the preview.
        match i {
            0 => {
                info["icon_url"] = "https://northstar.example/icon.png".into();
                info["pubkey"] =
                    "02a9acc1e48c25eeeb9289b5031cc57da9fe72f3fe2861d264bdc074209b107ba2".into();
            }
            1 => info["icon_url"] = "https://github.com/cashubtc.png".into(),
            _ => {}
        }
        mint.info = Some(info.to_string());
        db.upsert_mint(&mint).await?;
        wallets
            .create_wallet(url.parse()?, CurrencyUnit::Sat, None)
            .await?;
        for hour in 0..24 {
            let state = if i == 4 && hour < 5 { "ERROR" } else { "OK" };
            let at = (Utc::now() - Duration::hours(hour))
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string();
            let url = url.clone();
            let id = mint.id;
            db.run(move |conn| { conn.execute("INSERT INTO mint_checks(mint_id, mint_url, state, created_at) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![id, url, state, at])?; Ok(()) }).await?;
        }
        mints.push(mint);
    }
    // Reviews: six for Northstar (five rated, one not), one for Copper, none
    // elsewhere, so the summary, "Show all" and the empty state all show.
    let samples: [(usize, Option<u8>, &str, i64); 7] = [
        (0, Some(5), "Been using it with my wallet for months. Mints and melts go through in a couple of seconds, and it has never lost a payment.", 2),
        (0, Some(4), "Reliable. Lightning withdrawals sometimes take a while to find a route, but they always land.", 6),
        (0, Some(5), "Fees are fair and the operator answered my question on Nostr within the hour.", 11),
        (0, Some(3), "Works, but it was down for an afternoon last week without any notice.", 19),
        (0, Some(5), "My go-to mint for zaps.", 33),
        (0, None, "Haven't moved much through it yet. So far so good.", 40),
        (1, Some(4), "Small mint, quick swaps. Would like to see it announced on Nostr.", 4),
    ];
    for (k, (mint, rating, text, days)) in samples.into_iter().enumerate() {
        db.upsert_review(&Review {
            pubkey: nostr_sdk::prelude::Keys::generate().public_key().to_hex(),
            d: format!("preview-{mint}"),
            event_id: format!("{k:064x}"),
            mint_url: Some(mints[mint].url.clone()),
            rating,
            content: text.into(),
            created_at: Utc::now() - Duration::days(days),
        })
        .await?;
    }
    for i in 0..100 {
        let from = &mints[i % 7];
        let to = &mints[(i + 1) % 7];
        let mut swap = SwapEvent::success(
            from,
            to,
            20 + i as u64 % 80,
            1,
            850.0 + (i % 12) as f64 * 230.0,
        );
        swap.created_at = Utc::now() - Duration::minutes(i as i64 * 13 + 2);
        if i % 6 == 1 {
            swap.state = MintState::Error;
            swap.error = Some("Mint did not respond within 30 seconds. A later audit will retry after the cooldown.".into());
            swap.category = Some("timeout".into());
            swap.failed_mint_id = Some(to.id);
            swap.stage = Some("paying invoice".into());
            swap.fee = 0;
        }
        db.insert_swap(&swap).await?;
    }
    // Two synthetic peer auditors, so each mint shows several hallmarks.
    let peers = ["3f".repeat(32), "a9".repeat(32)];
    for (peer, name) in peers.iter().zip(["Harbor auditor", "Ridge auditor"]) {
        db.upsert_auditor(&Auditor {
            pubkey: peer.clone(),
            name: name.into(),
            urls: vec![format!(
                "https://{}.example",
                name.split(' ').next().unwrap_or("peer").to_lowercase()
            )],
            donate: None,
            software: "imf 0.1.0".into(),
            frequency_secs: 600,
            contact: None,
            announced_at: Some(Utc::now() - Duration::hours(6)),
            last_seen: Utc::now() - Duration::minutes(4),
        })
        .await?;
    }
    for i in 0..60 {
        let peer = &peers[i % 2];
        let from = &mints[(i * 3) % 7];
        let to = &mints[(i * 3 + 2) % 7];
        let mut swap = SwapEvent::success(
            from,
            to,
            30 + i as u64 % 50,
            1,
            900.0 + (i % 9) as f64 * 310.0,
        );
        swap.created_at = Utc::now() - Duration::minutes(i as i64 * 19 + 4);
        swap.auditor_pubkey = peer.clone();
        swap.event_id = Some(format!("{i:064x}"));
        if i % 7 == 3 {
            swap.state = MintState::Error;
            swap.error = Some("Lightning payment failed: no route found.".into());
            swap.category = Some("network".into());
            swap.failed_mint_id = Some(to.id);
            swap.fee = 0;
        }
        db.insert_swap(&swap).await?;
    }
    let state = AppState {
        workers: Default::default(),
        wallet_guards: Default::default(),
        leptos_options: get_configuration(Some("Cargo.toml"))?.leptos_options,
        wallet_repository: wallets,
        app_name: "Internet Mint Finder".into(),
        auditor_db: db,
        auditor_config: AuditorConfig::default(),
        cache: Arc::new(ResponseCache::new()),
        base_url: None,
        mint_url_aliases: Arc::new(Vec::new()),
        nostr: Arc::new(Default::default()),
        started_at: std::time::Instant::now(),
    };
    // Reject value-moving requests in the preview, including server functions.
    let app = axum::Router::new()
        .merge(api_router())
        .leptos_routes(&state, generate_route_list(App), {
            let state = state.clone();
            move || {
                provide_context(state.clone());
                shell(state.leptos_options.clone())
            }
        })
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .layer(axum::middleware::from_fn(
            |request: axum::extract::Request, next: axum::middleware::Next| async move {
                if request.method() == axum::http::Method::POST
                    && (request.uri().path().contains("receive")
                        || request.uri().path().contains("ReceiveToken")
                        || request.uri().path() == "/api/donate"
                        || request.uri().path() == "/api/mints/")
                {
                    return axum::response::IntoResponse::into_response((
                        axum::http::StatusCode::FORBIDDEN,
                        "Preview does not accept donations",
                    ));
                }
                next.run(request).await
            },
        ))
        .with_state(state);
    println!("Synthetic preview: http://127.0.0.1:3100 (no real ecash)");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3100").await?;
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(not(feature = "ssr"))]
fn main() {}
