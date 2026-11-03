//! REST API router and handlers.
//!
//! Mounted by `main.rs` alongside the Leptos SSR routes. Handlers stay thin:
//! parsing/extraction here, domain logic in [`crate::server::service`],
//! queries in [`crate::server::storage`] / [`crate::server::analytics`].

use crate::server::analytics;
use crate::server::cache::ResponseCache;
use crate::server::error::AuditorError;
use crate::server::filters::Filters;
use crate::server::service;
use crate::server::state::AppState;
use crate::types::{
    AnnouncementRead, AuditorMintView, AuditorRead, ChargeRequest, DonationPoll, DonationRead,
    MintMetrics, MintRead, MintReviews, MintStats, MintUptime, PaginationParams, SwapEventRead,
};
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri};
use axum::response::{Html, IntoResponse, Redirect};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use rusqlite::Connection;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tower_http::cors::{Any, CorsLayer};

fn cors() -> CorsLayer {
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}

/// Build the read-only `/api` router. Leptos routes are attached by the
/// caller.
pub fn api_router() -> Router<AppState> {
    Router::new()
        // CRUD API
        .route("/api/mints/", get(get_mints).post(add_mint))
        .route("/api/mints/{id}", get(get_mint_by_id))
        .route("/api/mints/{id}/metrics", get(get_mint_metrics))
        .route("/api/mints/{id}/uptime", get(get_mint_uptime))
        .route("/api/mints/{id}/donations", get(get_mint_donations))
        .route("/api/mints/{id}/auditors", get(get_mint_auditors))
        .route("/api/mints/{id}/announcement", get(get_mint_announcement))
        .route("/api/mints/{id}/reviews", get(get_mint_reviews))
        .route("/api/auditors", get(get_auditors))
        .route("/api/donations", get(get_donations))
        .route("/api/swaps/", get(get_swaps))
        .route("/api/swaps/mint/{mint_id}", get(get_swaps_by_mint))
        .route("/api/stats/", get(get_stats))
        .route("/api/health", get(health))
        // Analytics API
        .route("/api/filters", get(api_filters))
        .route("/api/kpis", get(api_kpis))
        .route("/api/scatter", get(api_scatter))
        .route("/api/top-quality", get(api_top_quality))
        .route("/api/timeseries/swaps", get(api_ts_swaps))
        .route("/api/timeseries/latency", get(api_ts_latency))
        .route("/api/histogram/latency", get(api_hist_latency))
        .route("/api/errors", get(api_errors))
        .route("/api/graph/", get(api_graph))
        // Reliability analytics
        .route("/api/uptime/grid", get(api_uptime_grid))
        .route("/api/failure-stages", get(api_failure_stages))
        .route("/api/blame", get(api_blame))
        .route("/api/errors/category", get(api_errors_category))
        // Money flow / TVL analytics
        .route("/api/tvl", get(api_tvl))
        .route("/api/flow", get(api_flow))
        .route("/api/fees", get(api_fees))
        .route("/api/donations/timeseries", get(api_donations_ts))
        .layer(cors())
}

/// Donation endpoints and the analytics page, mounted only when the instance
/// runs paid swaps (`AUDITOR_PAID_SWAPS`).
pub fn paid_router() -> Router<AppState> {
    Router::new()
        // NUT-18 Payment Request endpoints
        .route("/api/pr", get(api_get_pr))
        .route("/api/donate", post(api_donate))
        .route("/receive-token", post(receive_token_form))
        // Static analytics dashboard
        .route("/dashboard", get(dashboard_page))
        .layer(cors())
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct Health {
    workers: Vec<crate::server::workers::WorkerStatus>,
    status: &'static str,
    mints: u64,
    swaps: u64,
    auditors: u64,
    last_probe_at: Option<chrono::DateTime<chrono::Utc>>,
    last_swap_at: Option<chrono::DateTime<chrono::Utc>>,
    last_ingest_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Readiness for DNS failover: probes, worker progress and completed relay replay.
async fn health(State(state): State<AppState>) -> Result<(StatusCode, Json<Health>), AuditorError> {
    let mints = state.auditor_db.get_all_mints().await?.len() as u64;
    let swaps = state.auditor_db.get_total_swaps().await?;
    let auditors = state.auditor_db.get_auditors().await?.len() as u64;
    let (last_probe_at, last_swap_at) = state.auditor_db.local_activity().await?;
    let last_ingest_at = state.auditor_db.latest_peer_event_at().await?;

    let max_age =
        chrono::Duration::seconds(2 * state.auditor_config.health_check_interval_secs as i64);
    let stalled = mints > 0
        && last_probe_at.is_none_or(|at| chrono::Utc::now() - at > max_age)
        && state.started_at.elapsed() > max_age.to_std().unwrap_or_default();
    let workers = state.workers.status();
    let unhealthy = workers.iter().any(|worker| worker.stalled || !worker.ready);
    let (code, status) = if stalled || unhealthy {
        (StatusCode::SERVICE_UNAVAILABLE, "stalled")
    } else {
        (StatusCode::OK, "ok")
    };
    Ok((
        code,
        Json(Health {
            workers,
            status,
            mints,
            swaps,
            auditors,
            last_probe_at,
            last_swap_at,
            last_ingest_at,
        }),
    ))
}

/// Behind the reverse proxy, port 80 is proxied to the app instead of being
/// redirected by Caddy, so that the DNS health check (plain HTTP, any path)
/// sees a 5xx when the app is down. The app itself answers the redirect.
pub async fn https_redirect(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let forwarded_http = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("http"));
    if forwarded_http && request.uri().path() != "/api/health" {
        if let Some(host) = request.headers().get("host").and_then(|v| v.to_str().ok()) {
            if !is_onion_host(host, std::env::var("ONION_HOSTNAME").ok().as_deref()) {
                let target = format!("https://{host}{}", request.uri());
                return Redirect::permanent(&target).into_response();
            }
        }
    }
    next.run(request).await
}

fn is_onion_host(host: &str, configured: Option<&str>) -> bool {
    configured
        .filter(|s| s.ends_with(".onion"))
        .is_some_and(|onion| {
            host.split(':')
                .next()
                .is_some_and(|host| host.eq_ignore_ascii_case(onion))
        })
}

// ---------------------------------------------------------------------------
// CRUD handlers
// ---------------------------------------------------------------------------

async fn get_mints(
    Query(params): Query<PaginationParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<MintRead>>, AuditorError> {
    Ok(Json(
        service::mints_with_balance(&state, params.skip, params.limit).await?,
    ))
}

async fn get_mint_by_id(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<MintRead>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    service::mint_with_balance(&state, id)
        .await?
        .map(Json)
        .ok_or_else(|| AuditorError::NotFound(format!("mint {id}")))
}

async fn get_swaps(
    Query(params): Query<PaginationParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<SwapEventRead>>, AuditorError> {
    let swaps = state
        .auditor_db
        .get_swaps(params.skip, params.limit)
        .await?;
    Ok(Json(swaps.into_iter().map(SwapEventRead::from).collect()))
}

async fn get_swaps_by_mint(
    Path(mint_id): Path<String>,
    Query(params): Query<PaginationParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<SwapEventRead>>, AuditorError> {
    let mint_id = service::mint_local_id(&state, &mint_id).await?;
    let swaps = state
        .auditor_db
        .get_swaps_by_mint(mint_id, None, params.skip, params.limit)
        .await?;
    Ok(Json(swaps.into_iter().map(SwapEventRead::from).collect()))
}

async fn get_mint_metrics(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<MintMetrics>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    Ok(Json(state.auditor_db.get_mint_metrics(id, None).await?))
}

async fn get_mint_auditors(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Vec<AuditorMintView>>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    Ok(Json(service::mint_auditor_views(&state, id).await?))
}

async fn get_mint_announcement(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<Option<AnnouncementRead>>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    Ok(Json(service::mint_announcement(&state, id).await?))
}

async fn get_mint_reviews(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<MintReviews>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    Ok(Json(service::mint_reviews(&state, id).await?))
}

async fn get_auditors(
    State(state): State<AppState>,
) -> Result<Json<Vec<AuditorRead>>, AuditorError> {
    Ok(Json(service::list_auditors(&state).await?))
}

async fn get_mint_uptime(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<MintUptime>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    Ok(Json(state.auditor_db.get_mint_uptime(id).await?))
}

async fn get_mint_donations(
    Path(id): Path<String>,
    Query(params): Query<PaginationParams>,
    State(state): State<AppState>,
) -> Result<Json<Vec<DonationRead>>, AuditorError> {
    let id = service::mint_local_id(&state, &id).await?;
    let donations = state
        .auditor_db
        .get_donations_by_mint(id, params.limit)
        .await?;
    Ok(Json(
        donations.into_iter().map(DonationRead::from).collect(),
    ))
}

#[derive(Deserialize)]
struct DonationsQuery {
    after_id: Option<u64>,
}

async fn get_donations(
    Query(query): Query<DonationsQuery>,
    State(state): State<AppState>,
) -> Result<Json<DonationPoll>, AuditorError> {
    Ok(Json(service::poll_donations(&state, query.after_id).await?))
}

async fn get_stats(State(state): State<AppState>) -> Result<Json<MintStats>, AuditorError> {
    Ok(Json(service::compute_stats(&state).await?))
}

async fn add_mint(
    State(state): State<AppState>,
    Json(payload): Json<ChargeRequest>,
) -> Result<Json<MintRead>, AuditorError> {
    let outcome = service::receive_token(&state, &payload.token).await?;
    Ok(Json(outcome.mint))
}

#[derive(Deserialize)]
struct ReceiveTokenForm {
    token: String,
}

async fn receive_token_form(
    State(state): State<AppState>,
    Form(payload): Form<ReceiveTokenForm>,
) -> Redirect {
    let token = payload.token.trim();
    if token.is_empty() {
        return Redirect::to(&receive_token_redirect(&[
            ("token_result", "err"),
            ("message", "Paste a Cashu token before donating."),
        ]));
    }

    match service::receive_token(&state, token).await {
        Ok(outcome) => {
            let amount = outcome.received_amount.to_string();
            Redirect::to(&receive_token_redirect(&[
                ("token_result", "ok"),
                ("amount", &amount),
                ("mint", &outcome.mint.url),
            ]))
        }
        Err(err) => {
            let message = err.to_string();
            Redirect::to(&receive_token_redirect(&[
                ("token_result", "err"),
                ("message", &message),
            ]))
        }
    }
}

fn receive_token_redirect(params: &[(&str, &str)]) -> String {
    let query = params
        .iter()
        .fold(
            form_urlencoded::Serializer::new(String::new()),
            |mut ser, (key, value)| {
                ser.append_pair(key, value);
                ser
            },
        )
        .finish();
    format!("/receive?{query}")
}

// ---------------------------------------------------------------------------
// Analytics handlers (with response caching)
// ---------------------------------------------------------------------------

/// Run an analytics query on the blocking pool with TTL caching.
async fn cached_query<T, F>(
    state: &AppState,
    cache_key: &str,
    query_fn: F,
) -> Result<Json<T>, AuditorError>
where
    F: FnOnce(&Connection) -> Result<T, AuditorError> + Send + 'static,
    T: Serialize + DeserializeOwned + Send + 'static,
{
    if let Some(cached) = state.cache.get(cache_key) {
        if let Ok(val) = serde_json::from_str::<T>(&cached) {
            return Ok(Json(val));
        }
    }

    let result = state.auditor_db.run(query_fn).await?;
    state.cache.set(cache_key.to_string(), &result);
    Ok(Json(result))
}

fn default_group_by() -> String {
    "from_url".to_string()
}

fn default_limit() -> usize {
    10
}

fn default_bins() -> usize {
    30
}

fn filters_from_uri(uri: &Uri) -> Result<Filters, AuditorError> {
    let filters = Filters::from_query(uri.query().unwrap_or(""));
    filters.validate()?;
    Ok(filters)
}

fn query_value(uri: &Uri, name: &str) -> Option<String> {
    form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
        .find_map(|(key, value)| (key == name).then(|| value.into_owned()))
}

fn query_usize(uri: &Uri, name: &str, default: usize) -> usize {
    query_value(uri, name)
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

async fn api_filters(
    State(state): State<AppState>,
) -> Result<Json<analytics::FilterOptions>, AuditorError> {
    cached_query(&state, "filters", analytics::get_filter_options).await
}

async fn api_kpis(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<analytics::Kpis>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| analytics::kpis(c, &filters)).await
}

async fn api_scatter(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::ScatterPoint>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let group_by = query_value(&uri, "group_by").unwrap_or_else(default_group_by);
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::latency_error_scatter(c, &filters, &group_by)
    })
    .await
}

async fn api_top_quality(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::QualityMint>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let group_by = query_value(&uri, "group_by").unwrap_or_else(default_group_by);
    let limit = query_usize(&uri, "limit", default_limit());
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::top_quality_mints(c, &filters, &group_by, limit, 10)
    })
    .await
}

async fn api_ts_swaps(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::TimeseriesPoint>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::swaps_errors_timeseries(c, &filters)
    })
    .await
}

async fn api_ts_latency(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::LatencyPoint>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::latency_timeseries(c, &filters)
    })
    .await
}

async fn api_hist_latency(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<analytics::Histogram>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let bins = query_usize(&uri, "bins", default_bins());
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::latency_histogram(c, &filters, bins)
    })
    .await
}

async fn api_errors(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::ErrorCount>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let limit = query_usize(&uri, "limit", default_limit());
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::errors_by_reason(c, &filters, limit)
    })
    .await
}

async fn api_graph(
    State(state): State<AppState>,
) -> Result<Json<analytics::MintGraph>, AuditorError> {
    cached_query(&state, "graph", analytics::mint_graph).await
}

// ---------------------------------------------------------------------------
// Reliability analytics handlers
// ---------------------------------------------------------------------------

async fn api_uptime_grid(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<analytics::UptimeGrid>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| analytics::uptime_grid(c, &filters)).await
}

async fn api_failure_stages(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::FailureStage>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::failure_stages(c, &filters)
    })
    .await
}

async fn api_blame(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::BlameMint>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let limit = query_usize(&uri, "limit", default_limit());
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::blame_mints(c, &filters, limit)
    })
    .await
}

async fn api_errors_category(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::CategoryCount>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::errors_by_category(c, &filters)
    })
    .await
}

// ---------------------------------------------------------------------------
// Money flow / TVL analytics handlers
// ---------------------------------------------------------------------------

async fn api_tvl(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<analytics::TvlSeries>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| analytics::tvl_series(c, &filters)).await
}

async fn api_flow(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<analytics::FlowData>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| analytics::flow_data(c, &filters)).await
}

async fn api_fees(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::FeeRoute>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let limit = query_usize(&uri, "limit", default_limit());
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::fee_routes(c, &filters, limit)
    })
    .await
}

async fn api_donations_ts(
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Json<Vec<analytics::DonationPoint>>, AuditorError> {
    let filters = filters_from_uri(&uri)?;
    let key = ResponseCache::key(uri.path(), uri.query().unwrap_or(""));
    cached_query(&state, &key, move |c| {
        analytics::donations_timeseries(c, &filters)
    })
    .await
}

// ---------------------------------------------------------------------------
// NUT-18 Payment Request endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PrQuery {
    url: Option<String>,
}

#[derive(Serialize)]
struct PrResponse {
    pr: String,
}

/// GET /api/pr -- generate a Cashu payment request (NUT-18) for donations.
async fn api_get_pr(
    Query(query): Query<PrQuery>,
    State(state): State<AppState>,
) -> Result<Json<PrResponse>, AuditorError> {
    let pr = service::build_payment_request(&state, query.url.as_deref())?;
    Ok(Json(PrResponse { pr: pr.to_string() }))
}

#[derive(Serialize)]
struct DonateResponse {
    status: String,
    message: String,
}

/// POST /api/donate -- receive a NUT-18 payment payload (donation).
async fn api_donate(
    State(state): State<AppState>,
    Json(payload): Json<cdk::nuts::nut18::PaymentRequestPayload>,
) -> Result<Json<DonateResponse>, AuditorError> {
    let outcome = service::receive_donation(&state, payload).await?;
    Ok(Json(DonateResponse {
        status: "success".to_string(),
        message: format!("Received {} sats", outcome.received_amount),
    }))
}

// ---------------------------------------------------------------------------
// Static analytics dashboard
// ---------------------------------------------------------------------------

/// The analytics page repeats the app header, so it carries the instance name.
async fn dashboard_page(State(state): State<AppState>) -> Html<String> {
    Html(render_dashboard(&state.app_name))
}

fn render_dashboard(app_name: &str) -> String {
    let escaped = app_name
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;");
    include_str!("../../public/dashboard.html").replace("{{APP_NAME}}", &escaped)
}

#[cfg(test)]
mod dashboard_tests {
    use super::render_dashboard;

    #[test]
    fn dashboard_carries_the_escaped_instance_name() {
        let html = render_dashboard("Mint <Finder> & Co");
        assert!(html.contains("<span>Mint &lt;Finder&gt; &amp; Co</span>"));
        assert!(html.contains("<title>Analytics — Mint &lt;Finder&gt; &amp; Co</title>"));
        assert!(!html.contains("{{APP_NAME}}"));
    }
}

#[cfg(test)]
mod robustness_tests {
    use super::*;
    use crate::server::state::test_state;

    #[tokio::test]
    async fn malformed_public_queries_do_not_poison_the_database() {
        let state = test_state().await;
        for query in ["start=123456789%E2%98%83", "end=not-a-date"] {
            let uri = format!("/api/timeseries/swaps?{query}").parse().unwrap();
            assert!(matches!(
                api_ts_swaps(State(state.clone()), uri).await,
                Err(AuditorError::InvalidInput(_))
            ));
        }
        for bins in [0, 201, usize::MAX] {
            let uri = format!("/api/histogram/latency?bins={bins}")
                .parse()
                .unwrap();
            assert!(matches!(
                api_hist_latency(State(state.clone()), uri).await,
                Err(AuditorError::InvalidInput(_))
            ));
        }
        assert_eq!(
            api_kpis(State(state), "/api/kpis".parse().unwrap())
                .await
                .unwrap()
                .0
                .total_swaps,
            0
        );
    }

    #[test]
    fn only_the_configured_onion_host_bypasses_https_redirect() {
        assert!(is_onion_host("test.onion:80", Some("test.onion")));
        assert!(!is_onion_host("other.onion", Some("test.onion")));
        assert!(!is_onion_host(
            "test.onion.evil.example",
            Some("test.onion")
        ));
        assert!(!is_onion_host("test.onion", None));
    }

    #[tokio::test]
    async fn health_tracks_worker_readiness_without_requiring_paid_swaps() {
        let state = test_state().await;
        state
            .workers
            .register("paid auditing", std::time::Duration::from_secs(60), true);
        assert_eq!(
            health(State(state.clone())).await.unwrap().0,
            StatusCode::OK
        );
        state
            .workers
            .register("relay ingestion", std::time::Duration::from_secs(60), false);
        assert_eq!(
            health(State(state.clone())).await.unwrap().0,
            StatusCode::SERVICE_UNAVAILABLE
        );
        state.workers.ready("relay ingestion", true);
        assert_eq!(health(State(state)).await.unwrap().0, StatusCode::OK);
    }

    #[tokio::test]
    async fn mint_links_survive_different_replica_insertion_orders() {
        let a = test_state().await;
        let b = test_state().await;
        let url = "https://same.example";
        let first = a.auditor_db.ensure_mint_for_url(url).await.unwrap();
        b.auditor_db
            .ensure_mint_for_url("https://different.example")
            .await
            .unwrap();
        let second = b.auditor_db.ensure_mint_for_url(url).await.unwrap();
        assert_ne!(first.id, second.id);
        let key = crate::types::mint_key(url);
        for state in [a, b] {
            let result = get_mint_by_id(Path(key.clone()), State(state.clone()))
                .await
                .unwrap()
                .0;
            assert_eq!(result.url, url);
            assert_eq!(result.id, key);
            assert!(get_mint_by_id(Path("1".into()), State(state))
                .await
                .is_err());
        }
    }
}
