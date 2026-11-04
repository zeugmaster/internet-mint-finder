#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use imf::app::{shell, App};
    use imf::server::http::api_router;
    use imf::server::state::AppState;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use std::time::Duration;
    use tracing::{error, info};
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = std::env::var("SITE_ADDR").unwrap_or_else(|_| leptos_options.site_addr.to_string());

    let state = AppState::new(leptos_options)
        .await
        .expect("Failed to initialize AppState");

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    let (shutdown, shutdown_rx) = tokio::sync::watch::channel(false);
    let mut workers = tokio::task::JoinSet::new();
    let nostr = imf::server::nostr::start(&state, &mut workers, shutdown_rx.clone()).await;

    {
        let progress = state.workers.clone();
        let mut stop = shutdown_rx.clone();
        workers.spawn(async move {
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = tokio::time::sleep(Duration::from_secs(30)) => {
                        let stalled: Vec<_> = progress.status().into_iter().filter(|w| w.stalled).map(|w| w.name).collect();
                        if !stalled.is_empty() { error!(?stalled, "Worker watchdog requests restart"); break; }
                    }
                }
            }
            "worker watchdog"
        });
    }

    let routes = generate_route_list(App);

    let app = Router::new()
        .merge(api_router())
        .leptos_routes(&state, routes, {
            let state = state.clone();
            move || {
                provide_context(state.clone());
                shell(state.leptos_options.clone())
            }
        })
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .with_state(state);

    info!("listening on http://{}", &addr);
    workers.spawn(async move {
        axum::serve(listener, app.into_make_service())
            .with_graceful_shutdown(async move {
                let mut shutdown = shutdown_rx;
                if !*shutdown.borrow() {
                    let _ = shutdown.changed().await;
                }
            })
            .await
            .expect("HTTP server failed");
        "HTTP server"
    });

    let mut failed = tokio::select! {
        _ = shutdown_signal() => false,
        result = workers.join_next() => {
            error!(?result, "Worker exited unexpectedly; stopping the application");
            true
        }
    };
    let _ = shutdown.send(true);
    failed |= !drain_workers(&mut workers, Duration::from_secs(30)).await;
    if tokio::time::timeout(Duration::from_secs(5), nostr.disconnect())
        .await
        .is_err()
    {
        error!("Relay disconnect timed out");
        failed = true;
    }
    info!("shutdown complete");
    if failed {
        std::process::exit(1);
    }
}

/// Let active work finish before cancelling workers that exceed the grace period.
#[cfg(feature = "ssr")]
async fn drain_workers(
    workers: &mut tokio::task::JoinSet<&'static str>,
    grace: std::time::Duration,
) -> bool {
    let drained = tokio::time::timeout(grace, async {
        let mut clean = true;
        while let Some(result) = workers.join_next().await {
            match result {
                Ok(worker) => tracing::info!(worker, "Worker stopped"),
                Err(error) => {
                    tracing::error!(%error, "Worker failed during shutdown");
                    clean = false;
                }
            }
        }
        clean
    })
    .await;
    match drained {
        Ok(clean) => clean,
        Err(_) => {
            tracing::error!(
                remaining = workers.len(),
                "Shutdown grace period expired; cancelling remaining workers"
            );
            workers.shutdown().await;
            false
        }
    }
}

#[cfg(all(test, feature = "ssr"))]
mod lifecycle_tests {
    use super::drain_workers;
    use std::time::Duration;
    use tokio::{sync::oneshot, task::JoinSet};

    #[tokio::test(start_paused = true)]
    async fn shutdown_waits_for_active_work_and_cancels_hung_workers() {
        let mut workers = JoinSet::new();
        let (finished, result) = oneshot::channel();
        workers.spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            finished.send(()).unwrap();
            "review"
        });
        assert!(drain_workers(&mut workers, Duration::from_secs(3)).await);
        result.await.unwrap();
        workers.spawn(std::future::pending());
        assert!(!drain_workers(&mut workers, Duration::from_secs(3)).await);
        assert!(workers.is_empty());
    }

    #[tokio::test]
    async fn shutdown_reports_worker_panics() {
        let mut workers = JoinSet::new();
        workers.spawn(async { panic!("failed worker") });
        assert!(!drain_workers(&mut workers, Duration::from_secs(3)).await);
    }
}

#[cfg(feature = "ssr")]
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.expect("Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}

#[cfg(not(feature = "ssr"))]
fn main() {}
