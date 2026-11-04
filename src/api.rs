//! Leptos adapters over the same storage/service path as the REST API.
use crate::types::*;
use leptos::prelude::*;

#[cfg(feature = "ssr")]
fn app_state() -> Result<crate::server::state::AppState, ServerFnError> {
    use_context::<crate::server::state::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState not found"))
}
#[server]
pub async fn check_connection() -> Result<(), ServerFnError> {
    Ok(app_state()?.db.check().await?)
}
#[server]
pub async fn search_mints(
    search: String,
    sort: MintSort,
    page: u64,
) -> Result<MintPage, ServerFnError> {
    Ok(app_state()?.db.search_mints(search, sort, page).await?)
}
#[server(name = GetAppConfig)]
pub async fn get_app_config() -> Result<AppConfig, ServerFnError> {
    Ok(AppConfig {
        app_name: app_state()?.app_name,
    })
}
#[server]
pub async fn get_mint_by_id(id: String) -> Result<Option<MintRead>, ServerFnError> {
    Ok(crate::server::service::mint(&app_state()?, &id).await?)
}
#[server]
pub async fn get_mint_reviews(id: String) -> Result<MintReviews, ServerFnError> {
    Ok(crate::server::service::reviews(&app_state()?, &id, REVIEWS_LOADED, 0).await?)
}
