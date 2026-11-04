use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum MintSort {
    #[default]
    Name,
    RatingHigh,
    RatingLow,
    MostReviews,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintPage {
    pub mints: Vec<MintRead>,
    pub total: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintRead {
    pub id: String,
    pub url: String,
    pub info: Option<String>,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub review_count: u64,
    pub review_avg: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReviewRead {
    pub event_id: String,
    pub published: bool,
    pub pubkey: String,
    pub npub: String,
    pub rating: Option<u8>,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReviewContext {
    pub mint_url: String,
    pub mint_pubkey: Option<String>,
    pub announcer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct MintReviews {
    pub reviews: Vec<ReviewRead>,
    pub total: u64,
    pub rating_count: u64,
    pub rating_avg: Option<f64>,
    pub context: ReviewContext,
}

pub const REVIEWS_LOADED: u64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub app_name: String,
}

pub fn mint_key(url: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url)
}
