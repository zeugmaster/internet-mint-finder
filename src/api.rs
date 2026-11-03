//! Leptos server functions.
//!
//! Thin wrappers around `crate::server::service`; all domain logic lives
//! server-side and is shared with the REST API in `crate::server::http`.

use crate::types::{
    AnnouncementRead, AppConfig, AuditorMintView, AuditorRead, DonationPoll, DonationRead,
    MintMetrics, MintRead, MintReviews, MintStats, MintUptime, PaginationParams,
    PaymentRequestInfo, ReceiveResponse, SwapEventRead,
};
use leptos::prelude::*;

/// A local database check for the UI connection indicator (no mint requests).
#[server]
pub async fn check_connection() -> Result<(), ServerFnError> {
    app_state()?.auditor_db.has_mint_history().await?;
    Ok(())
}

#[server]
pub async fn search_mints(
    search: String,
    state_filter: Option<crate::types::MintState>,
    sort: crate::types::MintSort,
    page: u64,
) -> Result<crate::types::MintPage, ServerFnError> {
    let state = app_state()?;
    let (mints, total) = state
        .auditor_db
        .search_mints(search, state_filter, sort, page)
        .await?;
    let mut reads = Vec::with_capacity(mints.len());
    for mint in mints {
        reads.push(crate::server::service::with_balance(&state, mint).await);
    }
    Ok(crate::types::MintPage {
        mints: reads,
        total,
    })
}

#[cfg(feature = "ssr")]
fn app_state() -> Result<crate::server::state::AppState, ServerFnError> {
    use_context::<crate::server::state::AppState>()
        .ok_or_else(|| ServerFnError::new("AppState not found"))
}

#[server(name = GetAppConfig)]
pub async fn get_app_config() -> Result<AppConfig, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        Ok(AppConfig {
            app_name: state.app_name.clone(),
        })
    }
    #[cfg(not(feature = "ssr"))]
    {
        unreachable!()
    }
}

#[server(name = ReceiveToken)]
pub async fn receive_token(token: String) -> Result<ReceiveResponse, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::server::service;

        let state = app_state()?;
        match service::receive_token(&state, &token).await {
            Ok(outcome) => Ok(ReceiveResponse {
                success: true,
                message: format!(
                    "Received {} sat for {}",
                    outcome.received_amount, outcome.mint.url
                ),
            }),
            Err(e) => Ok(ReceiveResponse {
                success: false,
                message: e.to_string(),
            }),
        }
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = token;
        unreachable!()
    }
}

#[server(name = GetMints)]
pub async fn get_mints(params: PaginationParams) -> Result<Vec<MintRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::server::service;

        let state = app_state()?;
        Ok(service::mints_with_balance(&state, params.skip, params.limit).await?)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = params;
        unreachable!()
    }
}

#[server(name = GetMintById)]
pub async fn get_mint_by_id(id: String) -> Result<Option<MintRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::server::service;

        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        Ok(service::mint_with_balance(&state, id).await?)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = id;
        unreachable!()
    }
}

#[server(name = GetSwaps)]
pub async fn get_swaps(params: PaginationParams) -> Result<Vec<SwapEventRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let swaps = state
            .auditor_db
            .get_swaps(params.skip, params.limit)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        Ok(swaps.into_iter().map(SwapEventRead::from).collect())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = params;
        unreachable!()
    }
}

#[server(name = GetSwapsByMint)]
pub async fn get_swaps_by_mint(
    mint_id: String,
    received: Option<bool>,
    params: PaginationParams,
) -> Result<Vec<SwapEventRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let mint_id = crate::server::service::mint_local_id(&state, &mint_id).await?;
        let swaps = state
            .auditor_db
            .get_swaps_by_mint(mint_id, received, params.skip, params.limit)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        Ok(swaps.into_iter().map(SwapEventRead::from).collect())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (mint_id, received, params);
        unreachable!()
    }
}

#[server(name = GetStats)]
pub async fn get_stats() -> Result<MintStats, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::server::service;

        let state = app_state()?;
        Ok(service::compute_stats(&state).await?)
    }
    #[cfg(not(feature = "ssr"))]
    {
        unreachable!()
    }
}

/// Build a NUT-18 payment request (optionally locked to one mint) together
/// with a QR code SVG for display.
#[server(name = GetPaymentRequest)]
pub async fn get_payment_request(
    mint_url: Option<String>,
) -> Result<PaymentRequestInfo, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::server::service;

        let state = app_state()?;
        if state.base_url.is_none() {
            return Ok(PaymentRequestInfo::default());
        }
        let pr = service::build_payment_request(&state, mint_url.as_deref())?.to_string();
        let qr_svg = service::qr_svg(&pr)?;
        Ok(PaymentRequestInfo {
            pr,
            qr_svg,
            transport_available: true,
        })
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = mint_url;
        unreachable!()
    }
}

/// Aggregate audit metrics for one mint.
#[server(name = GetMintMetrics)]
pub async fn get_mint_metrics(id: String) -> Result<MintMetrics, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        state
            .auditor_db
            .get_mint_metrics(id, None)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = id;
        unreachable!()
    }
}

/// Observed audit uptime for one mint.
#[server(name = GetMintUptime)]
pub async fn get_mint_uptime(id: String) -> Result<MintUptime, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        state
            .auditor_db
            .get_mint_uptime(id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = id;
        unreachable!()
    }
}

/// Recent donations for one mint.
#[server(name = GetMintDonations)]
pub async fn get_mint_donations(
    id: String,
    limit: u64,
) -> Result<Vec<DonationRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        let donations = state
            .auditor_db
            .get_donations_by_mint(id, limit)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))?;
        Ok(donations.into_iter().map(DonationRead::from).collect())
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = (id, limit);
        unreachable!()
    }
}

/// Poll for new donations (live acknowledgement on the donate page).
/// `after_id = None` returns only the current baseline.
#[server(name = PollDonations)]
pub async fn poll_donations(after_id: Option<u64>) -> Result<DonationPoll, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        use crate::server::service;

        let state = app_state()?;
        Ok(service::poll_donations(&state, after_id).await?)
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = after_id;
        unreachable!()
    }
}

/// This instance and every peer auditor known from Nostr.
#[server(name = GetAuditors)]
pub async fn get_auditors() -> Result<Vec<AuditorRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        crate::server::service::list_auditors(&state)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "ssr"))]
    {
        unreachable!()
    }
}

/// Per-auditor metrics for one mint.
#[server(name = GetMintAuditorViews)]
pub async fn get_mint_auditor_views(id: String) -> Result<Vec<AuditorMintView>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        crate::server::service::mint_auditor_views(&state, id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = id;
        unreachable!()
    }
}

/// The mint's NIP-87 announcement, if any.
#[server(name = GetMintAnnouncement)]
pub async fn get_mint_announcement(id: String) -> Result<Option<AnnouncementRead>, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        crate::server::service::mint_announcement(&state, id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = id;
        unreachable!()
    }
}

/// NIP-87 reviews of a mint plus the context for signing a new one.
#[server(name = GetMintReviews)]
pub async fn get_mint_reviews(id: String) -> Result<MintReviews, ServerFnError> {
    #[cfg(feature = "ssr")]
    {
        let state = app_state()?;
        let id = crate::server::service::mint_local_id(&state, &id).await?;
        crate::server::service::mint_reviews(&state, id)
            .await
            .map_err(|e| ServerFnError::new(e.to_string()))
    }
    #[cfg(not(feature = "ssr"))]
    {
        let _ = id;
        unreachable!()
    }
}
