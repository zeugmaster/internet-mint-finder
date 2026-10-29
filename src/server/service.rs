//! Domain services shared by the REST API and the Leptos server functions.
//!
//! Everything that touches both the CDK wallet and the auditor database in
//! one logical operation lives here, so the HTTP layer and the server
//! functions stay thin and never drift apart.

use crate::server::error::AuditorError;
use crate::server::state::AppState;
use crate::server::storage::{Donation, Mint};
use crate::types::{
    AnnouncementRead, AuditorMintView, AuditorRead, DonationPoll, DonationRead, MintRead,
    MintReviews, MintState, MintStats, ReviewContext, ReviewRead,
};
use cdk::nuts::nut00::Token;
use cdk::nuts::nut18::{PaymentRequest, PaymentRequestPayload, Transport, TransportType};
use cdk::nuts::CurrencyUnit;
use cdk::wallet::ReceiveOptions;
use std::str::FromStr;

/// Result of receiving a Cashu token.
pub struct ReceiveOutcome {
    pub mint: MintRead,
    pub received_amount: u64,
}

/// Receive a serialized Cashu token into the auditor wallet and register or
/// update the issuing mint in the auditor database.
///
/// The received amount is added to the mint's donation target
/// (`sum_donations`), which the auditor uses to decide how much liquidity it
/// keeps on each mint.
pub async fn receive_token(
    state: &AppState,
    token_str: &str,
) -> Result<ReceiveOutcome, AuditorError> {
    let token_str = token_str.trim();
    let token = Token::from_str(token_str)
        .map_err(|e| AuditorError::InvalidInput(format!("invalid token: {e}")))?;
    validate_donation_token(&token)?;
    let mint_url = token
        .mint_url()
        .map_err(|e| AuditorError::InvalidInput(format!("invalid mint URL: {e}")))?
        .to_string();

    if let Some(aliased_url) = state.mint_url_alias(&mint_url) {
        let token_str = rewrite_token_mint_url(token, &aliased_url)?.to_string();
        receive_for_mint(state, &aliased_url, &token_str, "token").await
    } else {
        receive_for_mint(state, &mint_url, token_str, "token").await
    }
}

/// Receive a NUT-18 payment payload (donation) and register the mint.
pub async fn receive_donation(
    state: &AppState,
    payload: PaymentRequestPayload,
) -> Result<ReceiveOutcome, AuditorError> {
    let token = Token::new(
        payload.mint.clone(),
        payload.proofs,
        payload.memo,
        payload.unit.clone(),
    );
    validate_donation_token(&token)?;
    let mint_url = payload.mint.to_string();
    let mint_url = state.mint_url_alias(&mint_url).unwrap_or(mint_url);
    let token = rewrite_token_mint_url(token, &mint_url)?;
    receive_for_mint(state, &mint_url, &token.to_string(), "nut18").await
}

fn validate_donation_token(token: &Token) -> Result<(), AuditorError> {
    if token.unit().unwrap_or_default() != CurrencyUnit::Sat {
        return Err(AuditorError::InvalidInput(
            "only sat tokens are accepted".into(),
        ));
    }
    if let Token::TokenV3(token) = token {
        if token.is_multi_mint() {
            return Err(AuditorError::InvalidInput(
                "donate one mint at a time".into(),
            ));
        }
    }
    let amount = token
        .value()
        .map_err(|e| AuditorError::InvalidInput(format!("invalid token amount: {e}")))?;
    if u64::from(amount) == 0 {
        return Err(AuditorError::InvalidInput("token contains no ecash".into()));
    }
    Ok(())
}

async fn receive_for_mint(
    state: &AppState,
    mint_url: &str,
    token_str: &str,
    via: &str,
) -> Result<ReceiveOutcome, AuditorError> {
    // Every donation path ends here, so this one check keeps a directory-only
    // instance from ever holding ecash.
    if !state.auditor_config.paid_swaps {
        return Err(AuditorError::InvalidInput(
            "this server doesn't accept donations".into(),
        ));
    }
    let wallet = state.wallet_for_mint(mint_url).await?;

    // CDK returns the amount actually received, after fees. A balance delta
    // also includes concurrent swaps and donations and can even underflow.
    let received_amount = u64::from(
        wallet
            .receive(token_str, ReceiveOptions::default())
            .await
            .map_err(|e| AuditorError::Wallet(format!("failed to receive token: {e}")))?,
    );

    // Reject empty receives (already-spent / zero-value tokens) before they can
    // register a junk mint with no working capital.
    if received_amount == 0 {
        return Err(AuditorError::InvalidInput(format!(
            "received 0 sat from {mint_url}: token is empty or already spent"
        )));
    }

    // Persist immediately; optional network metadata must never stand between
    // a successful receive and its accounting.
    let mut mint = state
        .auditor_db
        .record_donation(mint_url, received_amount, via)
        .await?;
    state.cache.clear();
    // Receiving already populated CDK's local mint info. Reuse it without
    // making another network request or replacing any audit counters.
    if let Ok(Some(info)) = wallet.localstore.get_mint(wallet.mint_url.clone()).await {
        if let Ok(json) = serde_json::to_string(&info) {
            let name = info.name.unwrap_or_default();
            match state
                .auditor_db
                .update_mint_metadata(mint.id, name.clone(), json.clone())
                .await
            {
                Ok(()) => {
                    mint.name = name;
                    mint.info = Some(json);
                }
                Err(error) => tracing::warn!(%error, "could not cache donation mint metadata"),
            }
        }
    }

    Ok(ReceiveOutcome {
        mint: with_balance(state, mint).await,
        received_amount,
    })
}

fn rewrite_token_mint_url(mut token: Token, mint_url: &str) -> Result<Token, AuditorError> {
    let mint_url = cdk::mint_url::MintUrl::from_str(mint_url)
        .map_err(|e| AuditorError::InvalidInput(format!("invalid mint URL alias: {e}")))?;

    match &mut token {
        Token::TokenV3(token) => {
            for entry in &mut token.token {
                entry.mint = mint_url.clone();
            }
        }
        Token::TokenV4(token) => {
            token.mint_url = mint_url;
        }
    }

    Ok(token)
}

/// Convert a stored mint into its API representation, enriched with the live
/// CDK wallet balance. Reads never create wallets or contact external mints.
/// An unavailable balance is explicitly marked rather than presented as zero.
pub async fn with_balance(state: &AppState, mint: Mint) -> MintRead {
    let pubkey = mint.pubkey();
    let mut read = MintRead::from(mint);
    // Peer-only and announced mints have no local wallet to ask.
    if read.sum_donations > 0 {
        match local_balance(state, &read.url).await {
            Ok(balance) => {
                read.balance = balance;
                read.balance_available = true;
            }
            Err(error) => tracing::warn!(mint = %read.url, %error, "balance unavailable"),
        }
    }
    match state
        .auditor_db
        .directory_stats(read.id, &read.url, pubkey)
        .await
    {
        Ok(stats) => {
            read.review_count = stats.review_count;
            read.review_avg = stats.review_avg;
            read.announced = stats.announced;
            read.auditors_24h = stats.auditors_24h;
            read.auditors_ok_24h = stats.auditors_ok_24h;
            read.success_rate_24h = (stats.attempts_24h > 0)
                .then(|| stats.successes_24h as f64 / stats.attempts_24h as f64);
            // Local rows carry an empty pubkey until published; name them by
            // this instance's key so the hallmark matches the auditors list.
            let self_hex = state.nostr.self_pubkey_hex().unwrap_or_default();
            read.auditor_marks = stats
                .marks
                .into_iter()
                .map(|mut mark| {
                    if mark.pubkey.is_empty() {
                        mark.pubkey = self_hex.clone();
                    }
                    mark
                })
                .collect();
        }
        Err(error) => tracing::warn!(mint = %read.url, %error, "directory stats unavailable"),
    }
    read
}

fn npub_of(hex: &str) -> String {
    use nostr_sdk::prelude::{PublicKey, ToBech32};
    PublicKey::from_hex(hex)
        .ok()
        .and_then(|pk| pk.to_bech32().ok())
        .unwrap_or_default()
}

/// The mint's NIP-87 announcement, with the signer checked against the key
/// the mint reports in NUT-06.
pub async fn mint_announcement(
    state: &AppState,
    mint_id: u64,
) -> Result<Option<AnnouncementRead>, AuditorError> {
    let Some(mint) = state.auditor_db.get_mint_by_id(mint_id).await? else {
        return Err(AuditorError::NotFound(format!("mint {mint_id} not found")));
    };
    let mint_pubkey = mint.pubkey();
    let Some(a) = state
        .auditor_db
        .announcement_for_mint(&mint.url, mint_pubkey.clone())
        .await?
    else {
        return Ok(None);
    };
    let content: serde_json::Value = serde_json::from_str(&a.content).unwrap_or_default();
    let field = |key: &str| {
        content
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    Ok(Some(AnnouncementRead {
        npub: npub_of(&a.pubkey),
        verified: mint_pubkey.map(|pk| pk == a.pubkey && pk == a.d),
        name: field("name"),
        description: field("description").or_else(|| field("about")),
        picture: field("picture"),
        nuts: a
            .nuts
            .as_deref()
            .map(|n| {
                n.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        pubkey: a.pubkey,
        d: a.d,
        mint_url: a.mint_url,
        network: a.network,
        created_at: a.created_at,
    }))
}

/// Reviews from relays plus what the browser needs to sign a new one.
pub async fn mint_reviews(state: &AppState, mint_id: u64) -> Result<MintReviews, AuditorError> {
    let Some(mint) = state.auditor_db.get_mint_by_id(mint_id).await? else {
        return Err(AuditorError::NotFound(format!("mint {mint_id} not found")));
    };
    let mint_pubkey = mint.pubkey();
    let announcer = state
        .auditor_db
        .announcement_for_mint(&mint.url, mint_pubkey.clone())
        .await?
        .map(|a| a.pubkey);
    let reviews = state
        .auditor_db
        .reviews_for_mint(&mint.url, mint_pubkey.clone(), 100)
        .await?
        .into_iter()
        .map(|r| ReviewRead {
            npub: npub_of(&r.pubkey),
            pubkey: r.pubkey,
            rating: r.rating,
            content: r.content,
            created_at: r.created_at,
        })
        .collect();
    Ok(MintReviews {
        reviews,
        context: ReviewContext {
            mint_url: mint.url,
            mint_pubkey,
            announcer,
            relays: state.nostr.relays.iter().map(|r| r.to_string()).collect(),
        },
    })
}

async fn local_balance(state: &AppState, url: &str) -> Result<u64, AuditorError> {
    let mint_url = cdk::mint_url::MintUrl::from_str(url)
        .map_err(|e| AuditorError::Internal(format!("invalid stored mint URL: {e}")))?;
    let wallet = state
        .wallet_repository
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await?;
    Ok(wallet.total_balance().await?.into())
}
/// List mints with live balances. Balances are resolved concurrently so one
/// slow wallet can't serialize the whole page load; input order is preserved.
pub async fn mints_with_balance(
    state: &AppState,
    skip: u64,
    limit: u64,
) -> Result<Vec<MintRead>, AuditorError> {
    let mints = state.auditor_db.get_mints(skip, limit).await?;

    let mut set: tokio::task::JoinSet<(usize, MintRead)> = tokio::task::JoinSet::new();
    for (idx, mint) in mints.into_iter().enumerate() {
        let state = state.clone();
        set.spawn(async move { (idx, with_balance(&state, mint).await) });
    }

    let mut indexed: Vec<(usize, MintRead)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        indexed
            .push(joined.map_err(|e| AuditorError::Internal(format!("balance task failed: {e}")))?);
    }
    indexed.sort_by_key(|(idx, _)| *idx);
    Ok(indexed.into_iter().map(|(_, read)| read).collect())
}

/// Fetch a single mint with live balance.
pub async fn mint_with_balance(
    state: &AppState,
    id: u64,
) -> Result<Option<MintRead>, AuditorError> {
    match state.auditor_db.get_mint_by_id(id).await? {
        Some(mint) => Ok(Some(with_balance(state, mint).await)),
        None => Ok(None),
    }
}

const STATS_CACHE_KEY: &str = "stats";

/// Aggregate auditor statistics. Cached briefly because the total-balance
/// computation walks every mint wallet.
pub async fn compute_stats(state: &AppState) -> Result<MintStats, AuditorError> {
    if let Some(cached) = state.cache.get(STATS_CACHE_KEY) {
        if let Ok(stats) = serde_json::from_str::<MintStats>(&cached) {
            return Ok(stats);
        }
    }

    let db = &state.auditor_db;

    // Sum live balances concurrently across all mint wallets.
    let mut set: tokio::task::JoinSet<Result<u64, AuditorError>> = tokio::task::JoinSet::new();
    for mint in db.get_all_mints().await? {
        let state = state.clone();
        set.spawn(async move { local_balance(&state, &mint.url).await });
    }
    let mut total_balance = 0u64;
    while let Some(joined) = set.join_next().await {
        let balance =
            joined.map_err(|e| AuditorError::Internal(format!("balance task failed: {e}")))??;
        total_balance = total_balance
            .checked_add(balance)
            .ok_or_else(|| AuditorError::Internal("total balance overflow".into()))?;
    }

    let stats = MintStats {
        total_balance,
        total_swaps: db.get_total_swaps().await?,
        total_swaps_24h: db.get_swaps_24h().await?,
        total_amount_swapped: db.get_total_amount_swapped().await?,
        total_amount_swapped_24h: db.get_total_amount_swapped_24h().await?,
        average_swap_time: db.get_average_swap_time().await?,
        average_swap_time_24h: db.get_average_swap_time_24h().await?,
    };

    state.cache.set(STATS_CACHE_KEY.to_string(), &stats);
    Ok(stats)
}

/// Build a NUT-18 payment request for donations. If `mint_url` is given the
/// request is locked to that mint; otherwise any audited mint is acceptable.
pub fn build_payment_request(
    state: &AppState,
    mint_url: Option<&str>,
) -> Result<PaymentRequest, AuditorError> {
    let mut builder = PaymentRequest::builder()
        .unit(CurrencyUnit::Sat)
        .description("Cashu mint auditor donation");

    if let Some(ref base_url) = state.base_url {
        let donate_url = format!("{}/api/donate", base_url.trim_end_matches('/'));
        builder = builder.add_transport(Transport {
            _type: TransportType::HttpPost,
            target: donate_url,
            tags: vec![],
        });
    }

    if let Some(url) = mint_url {
        let mint_url = cdk::mint_url::MintUrl::from_str(url)
            .map_err(|e| AuditorError::InvalidInput(format!("invalid mint URL: {e}")))?;
        builder = builder.add_mint(mint_url);
    }

    Ok(builder.build())
}

/// Render arbitrary data as a QR code SVG (used for payment requests).
pub fn qr_svg(data: &str) -> Result<String, AuditorError> {
    use qrcode::render::svg;
    use qrcode::QrCode;

    let code = QrCode::new(data.as_bytes())
        .map_err(|e| AuditorError::Internal(format!("QR encoding failed: {e}")))?;
    Ok(code
        .render::<svg::Color<'_>>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .build())
}

impl From<Donation> for DonationRead {
    fn from(d: Donation) -> Self {
        DonationRead {
            id: d.id,
            mint_id: d.mint_id,
            mint_url: d.mint_url,
            amount: d.amount,
            via: d.via,
            created_at: d.created_at,
        }
    }
}

/// Poll for donations. With `after_id = None` only the current baseline id is
/// returned (no events); with `Some(id)` every donation newer than `id` is
/// included so the donate page can acknowledge payments as they arrive.
pub async fn poll_donations(
    state: &AppState,
    after_id: Option<u64>,
) -> Result<DonationPoll, AuditorError> {
    match after_id {
        None => Ok(DonationPoll {
            latest_id: state.auditor_db.get_latest_donation_id().await?,
            donations: vec![],
        }),
        Some(id) => {
            let donations = state.auditor_db.get_donations_after(id, 20).await?;
            let latest_id = donations.last().map(|d| d.id).unwrap_or(id);
            Ok(DonationPoll {
                latest_id,
                donations: donations.into_iter().map(DonationRead::from).collect(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{rewrite_token_mint_url, validate_donation_token};
    use cdk::nuts::nut00::Token;
    use std::str::FromStr;

    #[test]
    fn invalid_units_and_empty_tokens_are_rejected_before_wallet_access() {
        let mint = "https://mint.example".parse().unwrap();
        let usd = Token::new(mint, vec![], None, cdk::nuts::CurrencyUnit::Usd);
        assert!(validate_donation_token(&usd)
            .unwrap_err()
            .to_string()
            .contains("only sat"));
        let mint = "https://mint.example".parse().unwrap();
        let empty = Token::new(mint, vec![], None, cdk::nuts::CurrencyUnit::Sat);
        assert!(validate_donation_token(&empty).is_err());
    }

    #[test]
    fn rewrites_v4_token_mint_url_for_local_aliases() {
        let token = Token::from_str(
            "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=",
        )
        .unwrap();

        let rewritten = rewrite_token_mint_url(token, "http://mint-three:8085").unwrap();

        assert_eq!(
            rewritten.mint_url().unwrap().to_string(),
            "http://mint-three:8085"
        );
    }
}

/// This instance first, then every peer announced or seen on Nostr.
pub async fn list_auditors(state: &AppState) -> Result<Vec<AuditorRead>, AuditorError> {
    let self_hex = state.nostr.self_pubkey_hex().unwrap_or_default();
    let cfg = &state.auditor_config;
    let mut list = vec![AuditorRead {
        pubkey: self_hex.clone(),
        npub: npub_of(&self_hex),
        name: state.app_name.clone(),
        urls: state.nostr.public_urls.clone(),
        donate: build_payment_request(state, None)
            .ok()
            .map(|pr| pr.to_string()),
        software: format!("imf {}", env!("CARGO_PKG_VERSION")),
        frequency_secs: (cfg.min_swap_delay_secs + cfg.max_swap_delay_secs) / 2,
        contact: state.nostr.contact.clone(),
        last_seen: None,
        is_self: true,
    }];
    for a in state.auditor_db.get_auditors().await? {
        if a.pubkey == self_hex {
            continue;
        }
        list.push(AuditorRead {
            npub: npub_of(&a.pubkey),
            pubkey: a.pubkey,
            name: a.name,
            urls: a.urls,
            donate: a.donate,
            software: a.software,
            frequency_secs: a.frequency_secs,
            contact: a.contact,
            last_seen: Some(a.last_seen),
            is_self: false,
        });
    }
    // Configured peers are ingested before (or without) announcing
    // themselves; list them anyway so every hallmark has a row.
    for key in &state.nostr.auditors {
        let hex = key.to_hex();
        if list.iter().any(|a| a.pubkey == hex) {
            continue;
        }
        list.push(AuditorRead {
            npub: npub_of(&hex),
            pubkey: hex,
            name: String::new(),
            urls: Vec::new(),
            donate: None,
            software: String::new(),
            frequency_secs: 0,
            contact: None,
            last_seen: None,
            is_self: false,
        });
    }
    Ok(list)
}

/// Every auditor's own numbers for one mint, side by side and never merged.
pub async fn mint_auditor_views(
    state: &AppState,
    mint_id: u64,
) -> Result<Vec<AuditorMintView>, AuditorError> {
    let Some(mint) = state.auditor_db.get_mint_by_id(mint_id).await? else {
        return Err(AuditorError::NotFound(format!("mint {mint_id} not found")));
    };
    let mut views = Vec::new();
    for auditor in list_auditors(state).await? {
        let scope = if auditor.is_self {
            String::new()
        } else {
            auditor.pubkey.clone()
        };
        let metrics = state
            .auditor_db
            .get_mint_metrics(mint_id, Some(scope))
            .await?;
        let state = if auditor.is_self {
            mint.state
        } else {
            peer_state(&metrics)
        };
        views.push(AuditorMintView {
            auditor,
            state,
            metrics,
        });
    }
    Ok(views)
}

/// A peer's state for a mint is whatever its latest attempt said.
fn peer_state(metrics: &crate::types::MintMetrics) -> MintState {
    match (metrics.last_success_at, metrics.last_error_at) {
        (None, None) => MintState::Unknown,
        (Some(ok), Some(err)) if err > ok => MintState::Error,
        (None, Some(_)) => MintState::Error,
        _ => MintState::Ok,
    }
}
