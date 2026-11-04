use super::{error::AppError, mint_url::normalize_mint_url, state::AppState};
use crate::types::*;
use base64::Engine;

pub fn mint_url(key: &str) -> Result<String, AppError> {
    if key.len() > 4096 {
        return Err(AppError::InvalidInput("Mint identifier is too long".into()));
    }
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(key)
        .ok()
        .and_then(|s| String::from_utf8(s).ok())
        .and_then(|s| normalize_mint_url(&s));
    raw.ok_or_else(|| {
        AppError::InvalidInput("Use the mint identifier returned by the directory".into())
    })
}

pub async fn mint(state: &AppState, id: &str) -> Result<Option<MintRead>, AppError> {
    state.db.mint(mint_url(id)?).await
}
pub async fn reviews(
    state: &AppState,
    id: &str,
    limit: u64,
    offset: u64,
) -> Result<MintReviews, AppError> {
    let mint = mint(state, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Mint not found".into()))?;
    state.db.reviews(mint.url, limit, offset).await
}
