use super::{error::AppError, nostr::events, service, state::AppState};
use crate::types::{MintPage, MintRead, MintReviews, MintSort, REVIEWS_LOADED};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{Method, StatusCode},
    routing::get,
    Json, Router,
};
use nostr_sdk::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::{Any, CorsLayer};

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/api/health/live", get(live))
        .route("/api/health/ready", get(ready))
        .route("/api/health", get(ready))
        .route("/api/mints", get(mints))
        .route("/api/mints/{id}", get(mint))
        .route("/api/mints/{id}/reviews", get(reviews).post(submit_review))
        .route("/api/reviews/{id}", get(review_status))
        .layer(DefaultBodyLimit::max(events::MAX_EVENT_BYTES))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET]),
        )
}

async fn live(State(state): State<AppState>) -> Result<(StatusCode, Json<Value>), AppError> {
    state.db.check().await?;
    let workers = state.workers.status();
    let healthy = workers.iter().all(|w| !w.stalled);
    Ok((
        if healthy {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({"live":healthy,"workers":workers})),
    ))
}
async fn ready(State(state): State<AppState>) -> Result<(StatusCode, Json<Value>), AppError> {
    state.db.check().await?;
    let workers = state.workers.status();
    let synced = workers
        .iter()
        .any(|w| w.name == "relay ingestion" && w.ready);
    let cached = state.db.has_data().await?;
    let ready = (cached || synced) && workers.iter().all(|w| !w.stalled);
    let pending: u64 = state
        .db
        .run(|c| Ok(c.query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))?))
        .await?;
    Ok((
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(
            json!({"ready":ready,"synced":synced,"cached":cached,"pending_reviews":pending,"workers":workers}),
        ),
    ))
}
#[derive(Default, Deserialize)]
struct DirectoryQuery {
    #[serde(default)]
    search: String,
    #[serde(default)]
    sort: MintSort,
    #[serde(default)]
    page: u64,
}
async fn mints(
    State(state): State<AppState>,
    Query(query): Query<DirectoryQuery>,
) -> Result<Json<MintPage>, AppError> {
    if query.search.len() > 2048 {
        return Err(AppError::InvalidInput("Search is too long".into()));
    }
    Ok(Json(
        state
            .db
            .search_mints(query.search, query.sort, query.page)
            .await?,
    ))
}
async fn mint(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MintRead>, AppError> {
    Ok(Json(service::mint(&state, &id).await?.ok_or_else(
        || AppError::NotFound("Mint not found".into()),
    )?))
}
#[derive(Deserialize)]
struct ReviewsQuery {
    #[serde(default = "review_limit")]
    limit: u64,
    #[serde(default)]
    offset: u64,
}
fn review_limit() -> u64 {
    REVIEWS_LOADED
}
async fn reviews(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(q): Query<ReviewsQuery>,
) -> Result<Json<MintReviews>, AppError> {
    Ok(Json(
        service::reviews(&state, &id, q.limit, q.offset).await?,
    ))
}
async fn submit_review(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(event): Json<Event>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    if event.kind.as_u16() != events::KIND_REVIEW {
        return Err(AppError::InvalidInput(
            "Expected a kind 38000 review".into(),
        ));
    }
    let decoded = events::decode(&event).ok_or_else(|| {
        AppError::InvalidInput("Invalid signature, timestamp or Cashu review references".into())
    })?;
    if decoded.rating.is_none()
        || decoded.content.is_empty()
        || decoded.content.chars().count() > 1000
        || event.tags.len() > 64
    {
        return Err(AppError::InvalidInput(
            "Choose a 0–5 rating and write 1–1000 characters".into(),
        ));
    }
    let mint = service::mint(&state, &id)
        .await?
        .ok_or_else(|| AppError::NotFound("Mint not found".into()))?;
    if !decoded.targets.iter().any(|t| t.url == mint.url)
        || decoded
            .targets
            .iter()
            .any(|t| !t.url.is_empty() && t.url != mint.url)
    {
        return Err(AppError::InvalidInput(
            "Review must reference this mint's URL".into(),
        ));
    }
    if !state.db.store(&event, decoded, true).await? {
        return Err(AppError::InvalidInput(
            "A newer review already exists for this event address".into(),
        ));
    }
    let published = state
        .db
        .published(event.id.to_hex())
        .await?
        .unwrap_or(false);
    Ok((
        if published {
            StatusCode::OK
        } else {
            StatusCode::ACCEPTED
        },
        Json(json!({"event_id":event.id.to_hex(),"saved":true,"published":published})),
    ))
}
async fn review_status(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    EventId::from_hex(&id).map_err(|_| AppError::InvalidInput("Invalid event id".into()))?;
    let published = state
        .db
        .published(id.clone())
        .await?
        .ok_or_else(|| AppError::NotFound("Review not found or superseded".into()))?;
    Ok(Json(
        json!({"event_id":id,"saved":true,"published":published}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use events::tests::event;
    use tower::ServiceExt;

    #[tokio::test]
    async fn signed_post_is_durable_visible_idempotent_and_cannot_forge_author() {
        let state = super::super::state::test_state();
        let keys = Keys::generate();
        let announcement = event(
            &keys,
            events::KIND_CASHU,
            &[&["d", "mint"], &["u", "https://mint.example"]],
            r#"{"name":"Example"}"#,
            100,
        );
        state
            .db
            .store(&announcement, events::decode(&announcement).unwrap(), false)
            .await
            .unwrap();
        let app = api_router().with_state(state.clone());
        let key = crate::types::mint_key("https://mint.example");
        let url = format!("/api/mints/{key}/reviews");
        let review = event(
            &keys,
            events::KIND_REVIEW,
            &[
                &["k", "38172"],
                &["d", "mint"],
                &["u", "https://mint.example/"],
            ],
            "[4/5] Fast",
            200,
        );
        for _ in 0..2 {
            let res = app
                .clone()
                .oneshot(
                    Request::post(&url)
                        .header("content-type", "application/json")
                        .body(Body::from(review.as_json()))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::ACCEPTED);
        }
        assert_eq!(state.db.pending().await.unwrap().len(), 1);
        let res = app
            .clone()
            .oneshot(Request::get(&url).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let data: MintReviews =
            serde_json::from_slice(&to_bytes(res.into_body(), 10000).await.unwrap()).unwrap();
        assert_eq!(data.total, 1);
        assert_eq!(data.rating_avg, Some(4.0));
        assert_eq!(data.reviews[0].content, "Fast");
        let mut forged = review.clone();
        forged.content = "[5/5] Forged".into();
        let res = app
            .clone()
            .oneshot(
                Request::post(&url)
                    .header("content-type", "application/json")
                    .body(Body::from(forged.as_json()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let res = app
            .clone()
            .oneshot(
                Request::post(&url)
                    .header("content-type", "application/json")
                    .body(Body::from("x".repeat(events::MAX_EVENT_BYTES + 1)))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        state.db.mark_published(review.id.to_hex()).await.unwrap();
        assert!(state.db.pending().await.unwrap().is_empty());
        for route in ["/api/health/live", "/api/health/ready"] {
            assert_eq!(
                app.clone()
                    .oneshot(Request::get(route).body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::OK
            );
        }
        for route in ["/api/donate", "/api/pr", "/api/swaps", "/api/auditors"] {
            assert_eq!(
                app.clone()
                    .oneshot(Request::get(route).body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                StatusCode::NOT_FOUND
            );
        }
    }
    #[tokio::test]
    async fn full_pages_render_the_preserved_ui_with_extension_login() {
        use crate::app::{shell, App};
        use leptos::prelude::*;
        use leptos_axum::{generate_route_list, LeptosRoutes};
        let state = super::super::state::test_state();
        let keys = Keys::generate();
        for (url, name) in [
            ("https://copper.example", "Copper"),
            ("https://paper-moon.example", "Paper Moon"),
        ] {
            let announcement = event(
                &keys,
                38172,
                &[&["d", url], &["u", url]],
                &json!({"name":name,"about":"A community mint, discovered on Nostr."}).to_string(),
                100,
            );
            state
                .db
                .store(&announcement, events::decode(&announcement).unwrap(), false)
                .await
                .unwrap();
            let review = event(
                &keys,
                38000,
                &[&["k", "38172"], &["d", url], &["u", url]],
                "[4/5] Payments worked well. <script>bad()</script>",
                200,
            );
            state
                .db
                .store(&review, events::decode(&review).unwrap(), false)
                .await
                .unwrap();
        }
        let routes = generate_route_list(App);
        let render_state = state.clone();
        let app = Router::new()
            .merge(api_router())
            .leptos_routes(&state, routes, move || {
                provide_context(render_state.clone());
                shell(render_state.leptos_options.clone())
            })
            .with_state(state);
        let key = crate::types::mint_key("https://copper.example");
        for (url, file) in [
            ("/".to_string(), "directory.html"),
            (format!("/mints/{key}"), "mint.html"),
        ] {
            let res = app
                .clone()
                .oneshot(Request::get(&url).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let html =
                String::from_utf8(to_bytes(res.into_body(), 2_000_000).await.unwrap().to_vec())
                    .unwrap();
            assert!(html.contains("Sign in with Nostr"));
            assert!(!html.contains("review-nsec"));
            assert!(!html.contains("Observed uptime"));
            assert!(html.contains("Copper"));
            if file == "mint.html" {
                assert!(html.contains("Payments worked well."));
                assert!(!html.contains("<script>bad()</script>"));
                assert!(html.contains("review-form"));
            }
            if let Ok(dir) = std::env::var("IMF_TEST_HTML_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(std::path::Path::new(&dir).join(file), html).unwrap();
            }
        }
    }
}
