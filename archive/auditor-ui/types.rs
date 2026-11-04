use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum MintState {
    Ok,
    Warn,
    Error,
    #[default]
    Unknown,
}

/// One auditor's latest 24 h verdict on a mint, shown as its hallmark.
/// `Ok`: its latest attributable attempt settled and none failed in 24 h.
/// `Warn`: the latest settled, but an earlier one in 24 h failed.
/// `Error`: its latest attributable attempt failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AuditorMark {
    /// Hex pubkey; empty for this instance when it has no Nostr key.
    pub pubkey: String,
    pub state: MintState,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginationParams {
    #[serde(default)]
    pub skip: u64,
    #[serde(default = "default_limit")]
    pub limit: u64,
}

fn default_limit() -> u64 {
    100
}

/// Directory order, chosen by the visitor. Ratings come from public Nostr
/// reviews; no order here is an editorial ranking.
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
pub struct ChargeRequest {
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintRead {
    /// Stable across instances: URL-safe base64 of the canonical mint URL.
    pub id: String,
    pub url: String,
    pub info: Option<String>,
    pub name: String,
    pub balance: u64,
    /// False when the local wallet balance could not be read; do not display 0.
    pub balance_available: bool,
    pub sum_donations: u64,
    pub updated_at: DateTime<Utc>,
    pub next_update: Option<DateTime<Utc>>,
    pub state: MintState,
    pub n_errors: u64,
    pub n_mints: u64,
    pub n_melts: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latitude: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub longitude: Option<f64>,
    /// Directory columns (NIP-87 reviews and the 24 h auditor verdicts).
    #[serde(default)]
    pub review_count: u64,
    #[serde(default)]
    pub review_avg: Option<f64>,
    #[serde(default)]
    pub announced: bool,
    #[serde(default)]
    pub auditors_24h: u64,
    #[serde(default)]
    pub auditors_ok_24h: u64,
    #[serde(default)]
    pub success_rate_24h: Option<f64>,
    /// Every auditor's own 24 h verdict, this instance first.
    #[serde(default)]
    pub auditor_marks: Vec<AuditorMark>,
}

/// A NIP-87 announcement as shown on the mint page.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AnnouncementRead {
    pub pubkey: String,
    pub npub: String,
    pub d: String,
    pub mint_url: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub picture: Option<String>,
    pub nuts: Vec<String>,
    pub network: Option<String>,
    pub created_at: DateTime<Utc>,
    /// `Some(true)` when the announcement is signed by the mint's own key,
    /// `None` when the mint's key has not been probed yet.
    pub verified: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReviewRead {
    pub pubkey: String,
    pub npub: String,
    pub rating: Option<u8>,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// What the browser needs to address a new review (NIP-07 signing).
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ReviewContext {
    pub mint_url: String,
    pub mint_pubkey: Option<String>,
    pub announcer: Option<String>,
    pub relays: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct MintReviews {
    /// The latest reviews, at most `REVIEWS_LOADED`.
    pub reviews: Vec<ReviewRead>,
    /// Every stored review, as the directory counts them.
    pub total: u64,
    /// How many carry a star rating, and their average.
    pub rating_count: u64,
    pub rating_avg: Option<f64>,
    pub context: ReviewContext,
}

/// How many reviews the mint page loads.
pub const REVIEWS_LOADED: u64 = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapEventRead {
    pub id: u64,
    pub from_id: String,
    pub to_id: String,
    pub from_url: String,
    pub to_url: String,
    pub amount: u64,
    pub fee: u64,
    #[serde(default = "crate::types::known_fee")]
    pub fee_known: bool,
    pub created_at: DateTime<Utc>,
    pub time_taken: f64,
    pub state: MintState,
    pub error: Option<String>,
    /// Which mint actually caused a failure (`None` for successes / legacy rows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_mint_id: Option<String>,
    /// The swap stage at which a failure occurred (e.g. "paying invoice").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Coarse error category slug for UI coloring (network/mint/timeout/…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Hex pubkey of the peer auditor that observed this swap; `None` = this instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auditor: Option<String>,
}

/// An auditor whose observations this instance shows: itself or a Nostr peer.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct AuditorRead {
    /// Hex pubkey; empty when this instance has no Nostr key.
    pub pubkey: String,
    pub npub: String,
    pub name: String,
    pub urls: Vec<String>,
    /// NUT-18 payment request funding this auditor.
    pub donate: Option<String>,
    pub software: String,
    pub frequency_secs: u64,
    pub contact: Option<String>,
    pub last_seen: Option<DateTime<Utc>>,
    pub is_self: bool,
}

/// One auditor's view of one mint.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuditorMintView {
    pub auditor: AuditorRead,
    pub state: MintState,
    pub metrics: MintMetrics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintStats {
    pub total_balance: u64,
    pub total_swaps: u64,
    pub total_swaps_24h: u64,
    pub total_amount_swapped: u64,
    pub total_amount_swapped_24h: u64,
    pub average_swap_time: f64,
    pub average_swap_time_24h: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AppConfig {
    pub app_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceiveResponse {
    pub success: bool,
    pub message: String,
}

/// A NUT-18 payment request with its QR code rendering.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PaymentRequestInfo {
    pub pr: String,
    pub qr_svg: String,
    pub transport_available: bool,
}

/// Aggregate audit metrics for a single mint (swaps in either direction).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MintMetrics {
    pub attempts: u64,
    pub successes: u64,
    pub attempts_24h: u64,
    pub successes_24h: u64,
    /// Average latency of successful swaps, in milliseconds.
    pub avg_latency_ms: f64,
    pub avg_latency_ms_24h: f64,
    pub last_success_at: Option<DateTime<Utc>>,
    /// Most recent error message attributable to this mint, if any.
    pub last_error: Option<String>,
    /// When that error was recorded.
    pub last_error_at: Option<DateTime<Utc>>,
    /// Coarse category slug for the last error (network/mint/timeout/…).
    pub last_error_category: Option<String>,
}

impl MintMetrics {
    pub fn success_rate(&self) -> Option<f64> {
        (self.attempts > 0).then(|| self.successes as f64 / self.attempts as f64)
    }

    pub fn success_rate_24h(&self) -> Option<f64> {
        (self.attempts_24h > 0).then(|| self.successes_24h as f64 / self.attempts_24h as f64)
    }
}

/// One fixed time bucket in the observed audit uptime timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintUptimeBar {
    pub start_at: DateTime<Utc>,
    pub end_at: DateTime<Utc>,
    pub state: MintState,
    pub checks: u64,
    pub failures: u64,
}

/// Observed uptime for one mint, inferred from hourly health probes when
/// available and otherwise from swap attempts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintUptime {
    pub window_hours: u64,
    pub bucket_minutes: u64,
    pub first_seen_at: Option<DateTime<Utc>>,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub current_up_since: Option<DateTime<Utc>>,
    pub checks: u64,
    pub successes: u64,
    pub failures: u64,
    pub uptime_percent: Option<f64>,
    /// Most recent health-check or audit error message, if any.
    pub last_error: Option<String>,
    /// When that error was recorded.
    pub last_error_at: Option<DateTime<Utc>>,
    /// Coarse category slug for the last error (network/mint/timeout/…).
    pub last_error_category: Option<String>,
    pub bars: Vec<MintUptimeBar>,
}

impl Default for MintUptime {
    fn default() -> Self {
        Self {
            window_hours: 24,
            bucket_minutes: 60,
            first_seen_at: None,
            last_checked_at: None,
            current_up_since: None,
            checks: 0,
            successes: 0,
            failures: 0,
            uptime_percent: None,
            last_error: None,
            last_error_at: None,
            last_error_category: None,
            bars: Vec::new(),
        }
    }
}

/// A donation event as shown in the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DonationRead {
    pub id: u64,
    pub mint_id: String,
    pub mint_url: String,
    pub amount: u64,
    pub via: String,
    pub created_at: DateTime<Utc>,
}

/// Donations newer than the requested baseline, plus the new baseline id.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DonationPoll {
    pub latest_id: u64,
    pub donations: Vec<DonationRead>,
}

/// Older serialized observations have no explicit fee-availability marker.
pub fn known_fee() -> bool {
    true
}

/// Public mint identifiers do not depend on local insertion order.
pub fn mint_key(url: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(url)
}
