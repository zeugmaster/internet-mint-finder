//! The paid swap audit: mint an invoice on one mint, pay it by melting ecash
//! on another, and record the outcome.
//!
//! Unlike the previous implementation, **every** failure stage now writes a
//! `swaps` row tagged with the stage, the mint that actually failed
//! (`failed_mint_id`), and a coarse error category. That is what lets the mint
//! detail page show the real reason a mint is failing ("connecting to mint —
//! mint unreachable") instead of a stale, misattributed "Amount undefined".

use super::config::AuditorConfig;
use super::guard::MintGuards;
use super::{balance_for, swap_support_for_round, SwapSupport, SwapSupportCache};
use crate::server::classify;
use crate::server::error::{AuditorError, WalletStateKind};
use crate::server::net::{self, NetConfig};
use crate::server::storage::{AuditorDatabase, Mint, SwapEvent};
use crate::types::MintState;
use cdk::nuts::nut00::ProofsMethods;
use cdk::nuts::{nut12, CurrencyUnit, Id, Keys, PaymentMethod, Proofs, PublicKey};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rand::seq::SliceRandom;
use rand::Rng;
use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::{sleep, Duration};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Stages
// ---------------------------------------------------------------------------

/// The step of a swap at which a failure occurred. The label is shown verbatim
/// in the UI, so it reads as a plain phrase.
#[derive(Debug, Clone, Copy)]
pub(super) enum SwapStage {
    Connect,
    RequestInvoice,
    PaymentAmount,
    QuoteMelt,
    FeeCheck,
    PreparePayment,
    PayInvoice,
    RedeemEcash,
}

impl SwapStage {
    fn label(self) -> &'static str {
        match self {
            SwapStage::Connect => "connecting to mint",
            SwapStage::RequestInvoice => "requesting invoice",
            SwapStage::PaymentAmount => "checking payment amount",
            SwapStage::QuoteMelt => "quoting the payment",
            SwapStage::FeeCheck => "checking the fee",
            SwapStage::PreparePayment => "preparing the payment",
            SwapStage::PayInvoice => "paying the invoice",
            SwapStage::RedeemEcash => "redeeming ecash",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct MintWithBalance {
    pub mint: Mint,
    pub balance: u64,
    pub swap_support: SwapSupport,
}

#[derive(Debug, Clone)]
pub(super) struct SwapSelection {
    pub from: MintWithBalance,
    pub to: MintWithBalance,
    pub amount: u64,
}

// ---------------------------------------------------------------------------
// Swap round
// ---------------------------------------------------------------------------

pub(super) async fn run_round(
    wallet_repository: &Arc<cdk::wallet::WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
    config: &AuditorConfig,
    net: &NetConfig,
    guards: &MintGuards,
    support_cache: &SwapSupportCache,
) -> Result<(), AuditorError> {
    let mints = auditor_db.get_all_mints().await?;

    if mints.len() < 2 {
        info!("Not enough mints for swap, need at least 2");
        return Ok(());
    }

    let mints_with_balance =
        build_mints_with_balance(wallet_repository, net, support_cache, mints).await;

    // Python parity: each round audits exactly ONE swap, with the receiver and
    // sender picked at random from the eligible mints (mirrors `choose_to_mint`
    // / `choose_from_mint_and_amount` in cashu-auditor/src/auditor.py). The
    // round-robin "audit every pair" scheme was intentionally dropped so the
    // Rust engine matches the reference's throughput and selection semantics.
    let Some(selection) = choose_swap(&mints_with_balance, config) else {
        info!(
            "No suitable mint pair found for paid swap; checked: {}",
            mint_selection_summary(&mints_with_balance)
        );
        return Ok(());
    };

    info!(
        "Auditing paid swap {} -> {} ({} sat)",
        selection.from.mint.url, selection.to.mint.url, selection.amount
    );

    run_one_swap(
        wallet_repository,
        auditor_db,
        config,
        net,
        guards,
        &selection,
    )
    .await
}

/// Pick one random receiver, then one compatible sender and an amount within
/// that pair's limits. Both sides must be past their error cooldown. A failed
/// mint remains eligible after cooldown even though an info probe cannot prove
/// that its payments have recovered.
///
/// The pair's range intersects the configured limits, NUT-04 receive limits,
/// NUT-05 spend limits, receiver headroom and the sender's 20% balance buffer.
fn choose_swap(mints: &[MintWithBalance], config: &AuditorConfig) -> Option<SwapSelection> {
    let mut rng = rand::thread_rng();
    let now = Utc::now();

    let to_candidates: Vec<&MintWithBalance> = mints
        .iter()
        .filter(|m| {
            mints
                .iter()
                .any(|from| compatible_amount_window(from, m, now, config).is_some())
        })
        .collect();
    let to = *to_candidates.choose(&mut rng)?;

    let from_candidates: Vec<_> = mints
        .iter()
        .filter_map(|from| {
            compatible_amount_window(from, to, now, config).map(|(min, max)| (from, min, max))
        })
        .collect();
    let (from, min, max) = *from_candidates.choose(&mut rng)?;
    let amount = rng.gen_range(min..=max);

    Some(SwapSelection {
        from: from.clone(),
        to: to.clone(),
        amount,
    })
}

/// Run a batch of pre-selected swaps concurrently, bounded by
/// `config.max_concurrent_swaps`. Each swap takes its own per-mint pair lock, so
/// overlapping mints serialise while disjoint mints proceed in parallel. Used by
/// both the main round and the error-retry round.
async fn run_selections_concurrently(
    wallet_repository: &Arc<cdk::wallet::WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
    config: &AuditorConfig,
    net: &NetConfig,
    guards: &MintGuards,
    selections: Vec<SwapSelection>,
) {
    let permits = Arc::new(Semaphore::new(config.max_concurrent_swaps.max(1) as usize));
    let mut set: JoinSet<()> = JoinSet::new();
    for selection in selections {
        let wallet_repository = wallet_repository.clone();
        let auditor_db = auditor_db.clone();
        let config = config.clone();
        let net = net.clone();
        let guards = guards.clone();
        let permits = permits.clone();
        set.spawn(async move {
            // Semaphore is never closed, so acquire cannot fail.
            let _permit = permits
                .acquire_owned()
                .await
                .expect("swap semaphore closed");
            if let Err(e) = run_one_swap(
                &wallet_repository,
                &auditor_db,
                &config,
                &net,
                &guards,
                &selection,
            )
            .await
            {
                warn!(
                    "Paid swap {} -> {} ended with error: {}",
                    selection.from.mint.url, selection.to.mint.url, e
                );
            }
        });
    }
    while set.join_next().await.is_some() {}
}

/// Re-audit every mint currently in the Error state, far more often than the
/// main swap loop. For each Error mint it runs one swap pairing it with the best
/// available partner, **ignoring the bench cooldown** — that is the point: we
/// want to retry benched mints so one that has recovered flips back to OK within
/// a short interval instead of waiting for the slow main cadence. A successful
/// swap marks the mint OK via the usual melt/mint success path.
pub(super) async fn run_error_retry_round(
    wallet_repository: &Arc<cdk::wallet::WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
    config: &AuditorConfig,
    net: &NetConfig,
    guards: &MintGuards,
    support_cache: &SwapSupportCache,
) -> Result<(), AuditorError> {
    let mints = auditor_db.get_all_mints().await?;
    if mints.len() < 2 {
        return Ok(());
    }

    let mints_with_balance =
        build_mints_with_balance(wallet_repository, net, support_cache, mints).await;
    if !mints_with_balance
        .iter()
        .any(|m| m.mint.state == MintState::Error)
    {
        return Ok(());
    }

    let mut selections = Vec::new();
    for i in 0..mints_with_balance.len() {
        if mints_with_balance[i].mint.state != MintState::Error {
            continue;
        }
        let Some(selection) = pick_error_retry_pair(&mints_with_balance, i, config) else {
            info!(
                "Error-retry: no viable partner to re-audit {}",
                mints_with_balance[i].mint.url
            );
            continue;
        };
        info!(
            "Error-retry: re-auditing {} via {} -> {} ({} sat)",
            mints_with_balance[i].mint.url,
            selection.from.mint.url,
            selection.to.mint.url,
            selection.amount
        );
        selections.push(selection);
    }

    if selections.is_empty() {
        return Ok(());
    }

    // Distinct Error mints may pick overlapping partners; the per-mint locks in
    // `run_one_swap` serialise those while disjoint pairs retry in parallel.
    run_selections_concurrently(
        wallet_repository,
        auditor_db,
        config,
        net,
        guards,
        selections,
    )
    .await;
    Ok(())
}

/// Choose a swap that re-audits `target` (an Error mint). Tries the target as
/// receiver and as sender against every other mint — ignoring the bench gate —
/// and picks the pairing with the widest amount window. `None` if no compatible
/// partner exists (no headroom, unsupported method, etc.).
fn pick_error_retry_pair(
    mints: &[MintWithBalance],
    target: usize,
    config: &AuditorConfig,
) -> Option<SwapSelection> {
    let t = &mints[target];
    // (partner index, min, max, target_is_receiver)
    let mut best: Option<(usize, u64, u64, bool)> = None;
    let consider = |cand: (usize, u64, u64, bool), best: &mut Option<(usize, u64, u64, bool)>| {
        if best.map(|(_, _, bmx, _)| cand.2 > bmx).unwrap_or(true) {
            *best = Some(cand);
        }
    };

    for (j, other) in mints.iter().enumerate() {
        if j == target {
            continue;
        }
        // target as receiver: other -> target
        if let Some((mn, mx)) = amount_window(other, t, config) {
            consider((j, mn, mx, true), &mut best);
        }
        // target as sender: target -> other
        if let Some((mn, mx)) = amount_window(t, other, config) {
            consider((j, mn, mx, false), &mut best);
        }
    }

    let (j, min_amount, max_amount, target_is_receiver) = best?;
    let amount = rand::thread_rng().gen_range(min_amount..=max_amount);
    let (from, to) = if target_is_receiver {
        (mints[j].clone(), t.clone())
    } else {
        (t.clone(), mints[j].clone())
    };
    Some(SwapSelection { from, to, amount })
}

/// Run one sender -> receiver paid swap and record the outcome: a success row,
/// or an attributed, stage-tagged failure row. Returns `Err` only for hard
/// local errors (bad mint URL, wallet open failure); audit failures that have
/// already been recorded return `Ok`.
async fn run_one_swap(
    wallet_repository: &Arc<cdk::wallet::WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
    config: &AuditorConfig,
    net: &NetConfig,
    guards: &MintGuards,
    selection: &SwapSelection,
) -> Result<(), AuditorError> {
    let from_mint = selection.from.mint.clone();
    let to_mint = selection.to.mint.clone();
    let amount = selection.amount;

    // Serialise every proof-mutating step on either mint for the whole swap:
    // concurrent swaps touching the same wallet would race proof selection.
    // Held until this function returns. Acquired in URL order so overlapping
    // pairs can never deadlock.
    let _swap_guard = guards.lock_pair(&from_mint.url, &to_mint.url).await;

    let to_wallet = wallet_repository
        .get_wallet(
            &cdk::mint_url::MintUrl::from_str(&to_mint.url)
                .map_err(|e| AuditorError::InvalidInput(e.to_string()))?,
            &CurrencyUnit::Sat,
        )
        .await?;
    let from_wallet = wallet_repository
        .get_wallet(
            &cdk::mint_url::MintUrl::from_str(&from_mint.url)
                .map_err(|e| AuditorError::InvalidInput(e.to_string()))?,
            &CurrencyUnit::Sat,
        )
        .await?;

    if let Err(e) =
        ensure_live_receive_headroom(&to_wallet, &to_mint, amount, config.receive_overshoot_sats)
            .await
    {
        // The receiver is at/near its donation cap: the auditor's own balance
        // state, not a mint fault. Skip silently rather than recording a failure
        // that would wrongly blame the mint. Balances can drift past the cap
        // between the round's snapshot and this live check (other swaps in the
        // round or an external wallet operation).
        info!("Skipping swap into {}: {}", to_mint.url, e);
        return Ok(());
    }

    // Stage: connect to both mints (NUT-06). This is where a killed mint is
    // first detected, and it now produces a recorded, attributed failure.
    if let Err(e) = net::with_timeout(
        &format!("mint info {}", to_mint.url),
        net.request_timeout,
        to_wallet.load_mint_info(),
    )
    .await
    {
        record_failure(
            auditor_db,
            &from_mint,
            &to_mint,
            amount,
            0,
            0.0,
            SwapStage::Connect,
            &to_mint,
            &e,
        )
        .await;
        bench_mint(auditor_db, to_mint.id, config).await;
        return Ok(());
    }
    if let Err(e) = net::with_timeout(
        &format!("mint info {}", from_mint.url),
        net.request_timeout,
        from_wallet.load_mint_info(),
    )
    .await
    {
        record_failure(
            auditor_db,
            &from_mint,
            &to_mint,
            amount,
            0,
            0.0,
            SwapStage::Connect,
            &from_mint,
            &e,
        )
        .await;
        bench_mint(auditor_db, from_mint.id, config).await;
        return Ok(());
    }

    info!(
        "Swapping from {} to {} amount: {} sat",
        from_mint.url, to_mint.url, amount
    );

    // Stage: request an invoice from the receiving mint.
    let mint_quote = match net::with_timeout(
        &format!("mint quote {}", to_mint.url),
        net.request_timeout,
        to_wallet.mint_quote(
            PaymentMethod::BOLT11,
            Some(cdk::Amount::from(amount)),
            None,
            None,
        ),
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            let e = e.with_amount_context(
                amount,
                selection.to.swap_support.receive_min_amount,
                selection.to.swap_support.receive_max_amount,
            );
            record_failure(
                auditor_db,
                &from_mint,
                &to_mint,
                amount,
                0,
                0.0,
                SwapStage::RequestInvoice,
                &to_mint,
                &e,
            )
            .await;
            bench_mint(auditor_db, to_mint.id, config).await;
            return Ok(());
        }
    };

    if let Err(e) = ensure_optional_quote_amount(&to_mint.url, mint_quote.amount, amount) {
        record_failure(
            auditor_db,
            &from_mint,
            &to_mint,
            amount,
            0,
            0.0,
            SwapStage::PaymentAmount,
            &to_mint,
            &e,
        )
        .await;
        bench_mint(auditor_db, to_mint.id, config).await;
        return Ok(());
    }

    info!(
        "Mint quote details: to_mint={}, quote_id={}, requested_amount={} sat",
        to_mint.url, mint_quote.id, amount
    );
    log_wallet_debug_state(&from_wallet, &from_mint.url, "before_melt_quote").await;

    // Stage: quote the melt (payment) on the sending mint.
    let melt_quote = match net::with_timeout(
        &format!("melt quote {}", from_mint.url),
        net.request_timeout,
        from_wallet.melt_quote(PaymentMethod::BOLT11, &mint_quote.request, None, None),
    )
    .await
    {
        Ok(q) => q,
        Err(e) => {
            let e = e.with_amount_context(
                amount,
                selection.from.swap_support.spend_min_amount,
                selection.from.swap_support.spend_max_amount,
            );
            record_failure(
                auditor_db,
                &from_mint,
                &to_mint,
                amount,
                0,
                0.0,
                SwapStage::QuoteMelt,
                &from_mint,
                &e,
            )
            .await;
            bench_mint(auditor_db, from_mint.id, config).await;
            return Ok(());
        }
    };

    let invoice_amount_sats: u64 = melt_quote.amount.into();
    let fee_reserve_sats: u64 = melt_quote.fee_reserve.into();
    if let Err(e) = ensure_exact_payment_amount(&to_mint.url, invoice_amount_sats, amount) {
        record_failure(
            auditor_db,
            &from_mint,
            &to_mint,
            amount,
            fee_reserve_sats,
            0.0,
            SwapStage::PaymentAmount,
            &to_mint,
            &e,
        )
        .await;
        bench_mint(auditor_db, to_mint.id, config).await;
        return Ok(());
    }

    let total_amount_sats = invoice_amount_sats.saturating_add(fee_reserve_sats);
    let amount_difference = total_amount_sats.saturating_sub(amount);
    let balance_before_melt: u64 = from_wallet
        .total_balance()
        .await
        .map(u64::from)
        .unwrap_or(0);

    info!(
        "Melt quote details: from_mint={}, quote_id={}, invoice_amount={} sat, fee_reserve={} sat, total_required={} sat, quoted_delta={} sat, balance_before_melt={} sat",
        from_mint.url, melt_quote.id, invoice_amount_sats, fee_reserve_sats, total_amount_sats, amount_difference, balance_before_melt
    );
    log_wallet_debug_state(&from_wallet, &from_mint.url, "before_prepare_melt").await;

    // Stage: reject swaps where the mint's fee reserve is unreasonable.
    if fee_reserve_too_high(
        amount,
        amount_difference,
        total_amount_sats,
        fee_reserve_sats,
        config,
    ) {
        let msg = format!(
            "Fee reserve of {:.1}% is too high. Mint wants to charge {} sat for invoice of {} sat.",
            fee_reserve_sats as f64 / amount as f64 * 100.0,
            total_amount_sats,
            amount
        );
        let err = AuditorError::Internal(msg);
        record_failure(
            auditor_db,
            &from_mint,
            &to_mint,
            amount,
            fee_reserve_sats,
            0.0,
            SwapStage::FeeCheck,
            &from_mint,
            &err,
        )
        .await;
        bench_mint(auditor_db, from_mint.id, config).await;
        return Ok(());
    }

    // Re-check the target balance after quote/fee discovery and before
    // preparing the melt. `prepare_melt` may reserve proofs locally; the guard
    // must run before that point so a stale selection cannot create either a
    // payment or stranded reservations.
    if let Err(e) =
        ensure_live_receive_headroom(&to_wallet, &to_mint, amount, config.receive_overshoot_sats)
            .await
    {
        // Cap reached since the quote was issued: skip silently (not a mint
        // fault). The unpaid mint/melt quotes simply expire; no funds moved.
        info!("Skipping swap into {} after quote: {}", to_mint.url, e);
        return Ok(());
    }

    let start_time = Instant::now();

    // Stage: pay the invoice by melting on the sender, retrying the value-moving
    // melt up to `max_swap_attempts` times before the mint drops to Error. Every
    // attempt re-prepares (fresh proof selection) and confirms against the SAME
    // receiver invoice (`mint_quote`); before each attempt after the first, and
    // after any melt error, we check whether that invoice is already paid — so a
    // melt that succeeded at the mint but errored on our side is never paid a
    // second time.
    let attempts = config.max_swap_attempts.max(1);
    let mut fee_paid: Option<u64> = None;
    let mut last_failure: Option<(SwapStage, AuditorError)> = None;

    for attempt in 1..=attempts {
        if attempt > 1 {
            match invoice_already_paid(net, &to_wallet, &mint_quote.id).await {
                Ok(true) => {
                    info!(
                        "Invoice {} on {} already paid by a previous attempt; not re-melting",
                        mint_quote.id, to_mint.url
                    );
                    fee_paid = Some(0);
                    break;
                }
                Ok(false) => {}
                Err(e) => {
                    // Can't determine the invoice's payment state — stop rather
                    // than risk paying it twice. Recorded + benched below.
                    warn!(
                        "Could not confirm whether invoice {} is already paid before retry ({}); not retrying",
                        mint_quote.id, e
                    );
                    last_failure = Some((SwapStage::PayInvoice, e));
                    break;
                }
            }
        }

        match attempt_pay_invoice(
            &from_wallet,
            &from_mint.url,
            &melt_quote.id,
            amount,
            total_amount_sats,
            balance_before_melt,
            net,
        )
        .await
        {
            Ok(paid) => {
                let time_taken = start_time.elapsed().as_secs_f64() * 1000.0;
                info!(
                    "Melt successful on attempt {}/{}: time: {}ms, amount: {} sat, fee: {} sat",
                    attempt, attempts, time_taken as u64, amount, paid
                );
                fee_paid = Some(paid);
                break;
            }
            Err((stage, e)) => {
                // If the melt actually went through despite the error, the
                // invoice is now paid: take it as success and redeem.
                if let Ok(true) = invoice_already_paid(net, &to_wallet, &mint_quote.id).await {
                    info!(
                        "Melt from {} errored but invoice {} is paid; treating as success",
                        from_mint.url, mint_quote.id
                    );
                    fee_paid = Some(0);
                    break;
                }
                warn!(
                    "Swap attempt {}/{} {} -> {} failed at stage '{}': {}",
                    attempt,
                    attempts,
                    from_mint.url,
                    to_mint.url,
                    stage.label(),
                    e
                );
                last_failure = Some((stage, e));
            }
        }
    }

    let fee_paid = match fee_paid {
        Some(fee) => fee,
        None => {
            // Out of attempts (or we stopped early): record the last failure at
            // the stage it occurred and drop the sender to Error so the
            // error-retry loop re-audits it soon. Wallet-state faults
            // (already-spent / insufficient funds) are ours, not the mint's, so
            // they are recorded but never benched.
            let (stage, e) = last_failure.unwrap_or((
                SwapStage::PayInvoice,
                AuditorError::Internal("melt failed".to_string()),
            ));
            record_failure(
                auditor_db,
                &from_mint,
                &to_mint,
                amount,
                fee_reserve_sats,
                0.0,
                stage,
                &from_mint,
                &e,
            )
            .await;
            if !is_wallet_state_error(&e) {
                bench_mint(auditor_db, from_mint.id, config).await;
            }
            return Ok(());
        }
    };

    let time_taken = start_time.elapsed().as_secs_f64() * 1000.0;

    // Stage: redeem the ecash on the receiving mint.
    if let Err(e) = mint_paid_quote_with_retry(
        net,
        &to_wallet,
        &to_mint.url,
        &mint_quote.id,
        amount,
        config.mint_ready_attempts,
        config.mint_ready_delay_ms,
    )
    .await
    {
        record_failure(
            auditor_db,
            &from_mint,
            &to_mint,
            amount,
            fee_paid,
            time_taken,
            SwapStage::RedeemEcash,
            &to_mint,
            &e,
        )
        .await;
        bench_mint(auditor_db, to_mint.id, config).await;
        return Ok(());
    }

    let mut event = SwapEvent::success(&from_mint, &to_mint, amount, fee_paid, time_taken);
    event.fee_reserve = Some(fee_reserve_sats);
    auditor_db.record_swap_success(&event).await.map_err(|e| {
        error!(from = %from_mint.url, to = %to_mint.url, amount, fee_paid, %e,
               "Swap settled but audit accounting could not be persisted");
        e
    })?;
    info!(
        "Swap from {} to {} of {} sat successful",
        from_mint.url, to_mint.url, amount
    );
    Ok(())
}

/// Whether the receiving mint's invoice (mint quote) has already been paid —
/// i.e. it now has a mintable amount. This is the double-payment guard for the
/// melt retry: a melt that succeeded at the mint but errored on our side leaves
/// the invoice paid, and we must not pay it again.
async fn invoice_already_paid(
    net: &NetConfig,
    wallet: &cdk::Wallet,
    quote_id: &str,
) -> Result<bool, AuditorError> {
    let status = net::with_timeout(
        &format!("mint quote status {quote_id}"),
        net.request_timeout,
        wallet.check_mint_quote_status(quote_id),
    )
    .await?;
    Ok(status.amount_mintable() > cdk::Amount::ZERO)
}

/// One melt attempt against an already-issued receiver invoice: (re)prepare the
/// proofs on the sender and confirm the payment. Returns the fee paid on
/// success. On failure it first runs saga recovery and (for an already-spent
/// rejection) reconciles the local proof store, so the *next* attempt starts
/// from clean state, then returns the error.
async fn attempt_pay_invoice(
    from_wallet: &cdk::Wallet,
    from_mint_url: &str,
    melt_quote_id: &str,
    amount: u64,
    total_amount_sats: u64,
    balance_before_melt: u64,
    net: &NetConfig,
) -> Result<u64, (SwapStage, AuditorError)> {
    // Stage: prepare the melt (proof selection / swap).
    let prepared_melt = match net::with_timeout(
        &format!("prepare melt {from_mint_url}"),
        net.request_timeout,
        from_wallet.prepare_melt(melt_quote_id, HashMap::new()),
    )
    .await
    {
        Ok(p) => p,
        Err(e) => {
            error!(
                "Could not prepare melt for mint {}: requested={} sat, total_required={} sat, balance_before_melt={} sat, error={}",
                from_mint_url, amount, total_amount_sats, balance_before_melt, e
            );
            // CDK's prepare_melt reserves the melt quote in the DB before
            // selecting proofs. If the saga fails or is dropped before CDK runs
            // its own compensations, the reservation stays set and every retry
            // of the same quote_id fails with "Quote already in use". Calling
            // recover_incomplete_sagas cleans up both orphaned reservations (no
            // matching saga) and incomplete sagas, so the next attempt can
            // reserve the quote again.
            match from_wallet.recover_incomplete_sagas().await {
                Ok(report) => info!(
                    "Post-prepare-failure recovery for {}: recovered={}, compensated={}, skipped={}, failed={}",
                    from_mint_url, report.recovered, report.compensated, report.skipped, report.failed
                ),
                Err(recovery_error) => warn!(
                    "Post-prepare-failure recovery failed for {}: {}",
                    from_mint_url, recovery_error
                ),
            }
            // A stale local proof store can surface here (an internal swap inside
            // prepare_melt may hit proofs the mint already spent). Reconcile so
            // those zombie proofs are not reselected on the next attempt.
            if is_already_spent_error(&e) {
                reconcile_spent_proofs(from_wallet, from_mint_url, net).await;
            }
            return Err((SwapStage::PreparePayment, e));
        }
    };

    let fee: u64 = prepared_melt.total_fee().into();
    info!(
        "Prepared melt details: from_mint={}, melt_quote_id={}, requested={} sat, total_required={} sat, prepared_fee={} sat, requires_swap={}, proofs_total={} sat, proofs_to_swap_total={} sat",
        from_mint_url,
        melt_quote_id,
        amount,
        total_amount_sats,
        fee,
        prepared_melt.requires_swap(),
        prepared_melt.proofs().total_amount().map(u64::from).unwrap_or(0),
        prepared_melt.proofs_to_swap().total_amount().map(u64::from).unwrap_or(0),
    );
    info!(
        "Prepared melt proof split: from_mint={}, proofs={}, proofs_to_swap={}",
        from_mint_url,
        proof_summary(Some(prepared_melt.proofs().clone())),
        proof_summary(Some(prepared_melt.proofs_to_swap().clone()))
    );

    // Capture the proofs this melt will have the mint verify, before `confirm()`
    // consumes the prepared melt. If the mint rejects the batch as
    // cryptographically invalid ("Token not verified"), these are exactly the
    // proofs to inspect and remove — no guessing, no wiping the whole store.
    let melt_input_proofs: Proofs = prepared_melt
        .proofs()
        .iter()
        .chain(prepared_melt.proofs_to_swap().iter())
        .cloned()
        .collect();

    // Stage: pay the invoice. Value-moving, so it gets a generous deadline and a
    // post-failure saga recovery pass rather than a hard short timeout.
    match net::with_timeout(
        &format!("melt {from_mint_url}"),
        net.request_timeout.saturating_mul(3),
        prepared_melt.confirm(),
    )
    .await
    {
        Ok(confirmed) => Ok(confirmed.fee_paid().into()),
        Err(e) => {
            log_wallet_debug_state(from_wallet, from_mint_url, "after_melt_failure").await;
            error!("Error melting from {}: {}", from_mint_url, e);

            match from_wallet.recover_incomplete_sagas().await {
                Ok(report) => info!(
                    "Post-failure recovery for {}: recovered={}, compensated={}, skipped={}, failed={}",
                    from_mint_url, report.recovered, report.compensated, report.skipped, report.failed
                ),
                Err(recovery_error) => error!(
                    "Post-failure recovery failed for {}: {}",
                    from_mint_url, recovery_error
                ),
            }

            // Older cdk restore code could pair an out-of-order signature with
            // the wrong deterministic secret, leaving a cryptographically
            // invalid proof in the local store. A restore alone can't fix it:
            // restore upserts by Y and never *removes* the poisoned row, so
            // `prepare_melt` keeps reselecting it and every melt fails forever.
            // The mint just told us this exact batch is invalid — DLEQ-verify it
            // offline, delete the proofs that fail, then restore valid ones.
            if is_token_not_verified_error(&e) {
                let invalid_ys = dleq_invalid_proof_ys(from_wallet, &melt_input_proofs).await;
                remove_invalid_proofs_and_restore(from_wallet, from_mint_url, invalid_ys, net)
                    .await;
            } else if is_already_spent_error(&e) {
                // The mint rejected our inputs as already spent: reconcile
                // after saga recovery so they are not selected again.
                reconcile_spent_proofs(from_wallet, from_mint_url, net).await;
            }
            Err((SwapStage::PayInvoice, e))
        }
    }
}

/// Build the per-mint balance + support view concurrently, preserving input
/// order. One slow mint-info read can't hold up the others, and the fan-out is
/// bounded by `net.max_concurrent_probes` so a large mint set can't open
/// hundreds of outbound connections at once. Capability metadata normally
/// comes from the hourly health worker's cache; only a cache miss probes here.
async fn build_mints_with_balance(
    wallet_repository: &Arc<cdk::wallet::WalletRepository>,
    net: &NetConfig,
    support_cache: &SwapSupportCache,
    mints: Vec<Mint>,
) -> Vec<MintWithBalance> {
    let permits = Arc::new(Semaphore::new(net.max_concurrent_probes.max(1)));
    let mut set: JoinSet<(usize, MintWithBalance)> = JoinSet::new();
    for (idx, mint) in mints.into_iter().enumerate() {
        let wallet_repository = wallet_repository.clone();
        let net = net.clone();
        let support_cache = support_cache.clone();
        let permits = permits.clone();
        set.spawn(async move {
            let _permit = permits
                .acquire_owned()
                .await
                .expect("probe semaphore closed");
            let balance = balance_for(&wallet_repository, &mint.url).await;
            let swap_support =
                swap_support_for_round(&wallet_repository, &net, &support_cache, &mint.url).await;
            (
                idx,
                MintWithBalance {
                    mint,
                    balance,
                    swap_support,
                },
            )
        });
    }

    let mut indexed: Vec<(usize, MintWithBalance)> = Vec::new();
    while let Some(joined) = set.join_next().await {
        if let Ok(pair) = joined {
            indexed.push(pair);
        }
    }
    indexed.sort_by_key(|(idx, _)| *idx);
    indexed.into_iter().map(|(_, m)| m).collect()
}

fn is_insufficient_funds_error(e: &AuditorError) -> bool {
    matches!(
        e,
        AuditorError::WalletState {
            kind: WalletStateKind::InsufficientFunds,
            ..
        }
    ) || e.to_string().to_lowercase().contains("insufficient funds")
}

/// The mint rejected our inputs as already spent (NUT-07 `TokenAlreadySpent`).
/// Prefers the typed [`WalletStateKind`] (set at `cdk::Error` conversion time)
/// and falls back to a substring match only for an error that arrives wrapped
/// inside another variant's message.
fn is_already_spent_error(e: &AuditorError) -> bool {
    matches!(
        e,
        AuditorError::WalletState {
            kind: WalletStateKind::AlreadySpent,
            ..
        }
    ) || e.to_string().to_lowercase().contains("already spent")
}

/// A cryptographic proof mismatch reported by the mint. Prefer the typed cdk
/// conversion, retaining a text fallback for errors wrapped by another layer.
fn is_token_not_verified_error(e: &AuditorError) -> bool {
    matches!(
        e,
        AuditorError::WalletState {
            kind: WalletStateKind::InvalidProof,
            ..
        }
    ) || e.to_string().to_lowercase().contains("token not verified")
}

/// CDK reserved the melt quote for a prior prepare_melt attempt that did not
/// release its reservation. This is an auditor-side concurrency/retry artefact
/// — the mint is not at fault. Detected by string match because the error
/// propagates through `cdk::Error::Database` which has no dedicated
/// `AuditorError` variant.
fn is_quote_in_use_error(e: &AuditorError) -> bool {
    e.to_string()
        .to_lowercase()
        .contains("quote already in use")
}

/// Errors that reflect the auditor's own wallet state — local liquidity or a
/// stale local proof store — rather than a fault of the mint. These must not
/// bench the mint; benching a healthy mint over our own desync is exactly what
/// kept a sender stuck failing every round.
fn is_wallet_state_error(e: &AuditorError) -> bool {
    is_insufficient_funds_error(e)
        || is_already_spent_error(e)
        || is_token_not_verified_error(e)
        || is_quote_in_use_error(e)
}

/// Re-run cdk's deterministic restore after a proof-verification failure.
/// Restore is idempotent and the wallet database upserts proofs by Y, so a
/// correctly paired signature replaces a bad row without duplicating value.
async fn restore_unverified_proofs(wallet: &cdk::Wallet, mint_url: &str, net: &NetConfig) {
    warn!(
        "Mint {} could not verify a local proof; running deterministic wallet restore before retry",
        mint_url
    );
    match net::with_timeout(
        &format!("restore invalid proofs {mint_url}"),
        net.request_timeout.saturating_mul(6),
        wallet.restore(),
    )
    .await
    {
        Ok(restored) => info!(
            "Proof restore completed for {}: unspent={} sat, pending={} sat, spent={} sat",
            mint_url,
            u64::from(restored.unspent),
            u64::from(restored.pending),
            u64::from(restored.spent)
        ),
        Err(restore_error) => warn!(
            "Could not repair unverified proofs for {}: {}",
            mint_url, restore_error
        ),
    }
}

/// DLEQ-verify a set of local proofs against the mint's keyset keys (offline,
/// NUT-12) and return the `Y` of every proof whose DLEQ proof is *present but
/// invalid* — a cryptographically bad local proof, e.g. one an older restore
/// paired with the wrong secret. Proofs that carry no DLEQ proof are left alone:
/// absence is not evidence of invalidity, and we never risk deleting good value
/// we cannot verify offline.
async fn dleq_invalid_proof_ys(
    wallet: &cdk::Wallet,
    proofs: &[cdk::nuts::Proof],
) -> Vec<PublicKey> {
    let mut keys_cache: HashMap<Id, Keys> = HashMap::new();
    let mut invalid = Vec::new();

    for proof in proofs {
        if let std::collections::hash_map::Entry::Vacant(entry) = keys_cache.entry(proof.keyset_id)
        {
            match wallet
                .keyset_with_policy(
                    proof.keyset_id,
                    cdk::wallet::types::KeysetLoadPolicy::CacheOnly,
                )
                .await
            {
                Ok(keyset) => {
                    entry.insert(keyset.keys);
                }
                Err(e) => {
                    warn!(
                        "Invalid-proof scan: could not load keys for keyset {}: {}",
                        proof.keyset_id, e
                    );
                    continue;
                }
            }
        }
        let Some(keys) = keys_cache.get(&proof.keyset_id) else {
            continue;
        };
        let Some(mint_pubkey) = keys.amount_key(proof.amount) else {
            continue;
        };

        match proof.verify_dleq(mint_pubkey) {
            Ok(()) => {}
            // No usable DLEQ to judge by — leave it for restore/retry.
            Err(nut12::Error::MissingDleqProof) | Err(nut12::Error::IncompleteDleqProof) => {}
            Err(e) => match proof.y() {
                Ok(y) => {
                    warn!(
                        "Local proof failed DLEQ verification ({} sat, keyset {}): {} — scheduling removal",
                        u64::from(proof.amount),
                        proof.keyset_id,
                        e
                    );
                    invalid.push(y);
                }
                Err(ye) => warn!("Could not compute Y for a DLEQ-invalid proof: {}", ye),
            },
        }
    }

    invalid
}

/// Delete cryptographically-invalid local proofs by `Y`, then restore. Deletion
/// is the half the old restore-only recovery was missing: the wallet store
/// upserts proofs by `Y` (`update_proofs(added, removed)`), so a restore can
/// never purge a poisoned row — only removing it stops `prepare_melt` from
/// reselecting the same bad proof and failing every melt forever. Restore runs
/// afterwards regardless, so a non-DLEQ mint still gets the previous best-effort
/// repair.
async fn remove_invalid_proofs_and_restore(
    wallet: &cdk::Wallet,
    mint_url: &str,
    invalid_ys: Vec<PublicKey>,
    net: &NetConfig,
) {
    if !invalid_ys.is_empty() {
        let count = invalid_ys.len();
        match wallet.localstore.update_proofs(vec![], invalid_ys).await {
            Ok(()) => warn!(
                "Removed {} cryptographically-invalid local proof(s) from {} before restore",
                count, mint_url
            ),
            Err(e) => warn!(
                "Could not remove {} invalid local proof(s) for {}: {}",
                count, mint_url, e
            ),
        }
    }
    restore_unverified_proofs(wallet, mint_url, net).await;
}

/// A melt that fails with "Token Already Spent" means the sending wallet's local
/// proof store is out of sync with the mint: it still holds proofs the mint has
/// already spent ("zombie" proofs — Unspent locally, Spent at the mint). cdk's
/// failure compensation only reverts proofs to Unspent by *local* state, so it
/// never clears these; `prepare_melt` then keeps reselecting them and every swap
/// from this mint aborts with the same error forever. Reconcile via NUT-07:
/// check the wallet's unspent proofs against the mint and let cdk mark the spent
/// ones Spent locally.
///
/// Called reactively after an "already spent" failure. Best-effort — a failed
/// reconcile is logged and retried by a later affected swap. The NUT-07 call
/// is bounded by the net deadline so a hung mint can't stall the caller.
pub(super) async fn reconcile_spent_proofs(wallet: &cdk::Wallet, mint_url: &str, net: &NetConfig) {
    let unspent = match wallet.get_unspent_proofs().await {
        Ok(proofs) => proofs,
        Err(e) => {
            warn!(
                "Could not load unspent proofs to reconcile {}: {}",
                mint_url, e
            );
            return;
        }
    };
    if unspent.is_empty() {
        return;
    }

    let total = unspent.len();
    match net::with_timeout(
        &format!("check proof state {mint_url}"),
        net.request_timeout,
        wallet.check_proofs_spent(unspent),
    )
    .await
    {
        Ok(states) => {
            let spent = states
                .iter()
                .filter(|s| s.state == cdk::nuts::State::Spent)
                .count();
            if spent > 0 {
                info!(
                    "Reconciled proof state for {}: {} of {} unspent proofs were already spent at the mint and marked spent locally",
                    mint_url, spent, total
                );
            }
        }
        Err(e) => warn!("Proof-state reconcile failed for {}: {}", mint_url, e),
    }
}

async fn mint_paid_quote_with_retry(
    net: &NetConfig,
    wallet: &cdk::Wallet,
    mint_url: &str,
    quote_id: &str,
    expected_amount: u64,
    attempts: u64,
    retry_delay_ms: u64,
) -> Result<Proofs, AuditorError> {
    for attempt in 1..=attempts {
        let status = net::with_timeout(
            &format!("mint quote status {mint_url}"),
            net.request_timeout,
            wallet.check_mint_quote_status(quote_id),
        )
        .await;

        match status {
            Ok(quote) if quote.amount_mintable() > cdk::Amount::ZERO => {
                let mintable: u64 = quote.amount_mintable().into();
                ensure_exact_payment_amount(mint_url, mintable, expected_amount)?;
                return net::with_timeout(
                    &format!("mint {mint_url}"),
                    net.request_timeout.saturating_mul(3),
                    wallet.mint(quote_id, cdk::amount::SplitTarget::None, None),
                )
                .await
                .map_err(|e| {
                    error!("Error minting on {}: {}", mint_url, e);
                    e
                });
            }
            Ok(_) if attempt < attempts => {
                warn!(
                    "Mint quote not mintable yet: mint={}, quote_id={}, attempt={}/{}",
                    mint_url, quote_id, attempt, attempts
                );
                sleep(Duration::from_millis(retry_delay_ms)).await;
            }
            Ok(_) => {
                return Err(AuditorError::Wallet(format!(
                    "mint quote {quote_id} on {mint_url} never became mintable"
                )));
            }
            Err(e) => {
                if attempt < attempts {
                    warn!(
                        "Could not check mint quote status yet: mint={}, quote_id={}, attempt={}/{}, error={}",
                        mint_url, quote_id, attempt, attempts, e
                    );
                    sleep(Duration::from_millis(retry_delay_ms)).await;
                    continue;
                }
                error!("Error checking mint quote on {}: {}", mint_url, e);
                return Err(e);
            }
        }
    }

    Err(AuditorError::Wallet(format!(
        "mint quote {quote_id} on {mint_url} never became mintable"
    )))
}

fn mint_selection_summary(mints: &[MintWithBalance]) -> String {
    mints
        .iter()
        .map(|m| {
            format!(
                "{} (state: {:?}, balance: {}, target: {}, can_receive_sat_bolt11: {}, can_spend_sat_bolt11: {})",
                m.mint.url, m.mint.state, m.balance, m.mint.sum_donations, m.swap_support.can_receive, m.swap_support.can_spend
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// Mint selection
// ---------------------------------------------------------------------------

fn fee_reserve_too_high(
    amount: u64,
    amount_difference: u64,
    total_amount_sats: u64,
    fee_reserve_sats: u64,
    config: &AuditorConfig,
) -> bool {
    amount_difference > config.fee_tolerance_sats
        && (fee_reserve_sats as f64 > amount as f64 * config.max_fee_percent / 100.0
            || total_amount_sats as f64 > amount as f64 * (1.0 + config.max_fee_percent / 100.0))
}

/// Remaining ecash a mint may receive: its donation target plus a fixed
/// working-capital `overshoot`, minus what it already holds. The overshoot is
/// the liveness valve that lets a mint sitting exactly at its donation cap
/// still accept a minimal test swap; see `compatible_amount_window` for the
/// safety rationale.
fn receive_capacity(balance: u64, donation_target: u64, overshoot: u64) -> u64 {
    donation_target
        .saturating_add(overshoot)
        .saturating_sub(balance)
}

fn ensure_receive_headroom(
    mint_url: &str,
    balance: u64,
    donation_target: u64,
    amount: u64,
    overshoot: u64,
) -> Result<(), AuditorError> {
    let headroom = receive_capacity(balance, donation_target, overshoot);
    if amount <= headroom {
        return Ok(());
    }

    Err(AuditorError::Internal(format!(
        "refusing to receive {amount} sat on {mint_url}: wallet balance {balance} sat, donation target {donation_target} sat (+{overshoot} overshoot), available headroom {headroom} sat"
    )))
}

fn ensure_optional_quote_amount(
    mint_url: &str,
    quoted_amount: Option<cdk::Amount>,
    expected_amount: u64,
) -> Result<(), AuditorError> {
    if let Some(quoted_amount) = quoted_amount {
        ensure_exact_payment_amount(mint_url, quoted_amount.into(), expected_amount)?;
    }
    Ok(())
}

fn ensure_exact_payment_amount(
    mint_url: &str,
    actual_amount: u64,
    expected_amount: u64,
) -> Result<(), AuditorError> {
    if actual_amount == expected_amount {
        return Ok(());
    }

    Err(AuditorError::Internal(format!(
        "refusing payment amount from {mint_url}: expected {expected_amount} sat, got {actual_amount} sat"
    )))
}

async fn ensure_live_receive_headroom(
    wallet: &cdk::Wallet,
    to_mint: &Mint,
    amount: u64,
    overshoot: u64,
) -> Result<(), AuditorError> {
    let balance = wallet.total_balance().await.map(u64::from)?;
    ensure_receive_headroom(
        &to_mint.url,
        balance,
        to_mint.sum_donations,
        amount,
        overshoot,
    )
}

/// A mint can join a paid swap unless it is benched (Error state with a
/// cooldown that has not yet expired).
fn is_available_for_paid_swap(m: &MintWithBalance, now: DateTime<Utc>) -> bool {
    match m.mint.state {
        MintState::Error => m.mint.next_update.map(|t| t <= now).unwrap_or(true),
        _ => true,
    }
}

/// The amount window for a directed pair, gated by the bench/cooldown check.
fn compatible_amount_window(
    from_mint: &MintWithBalance,
    to_mint: &MintWithBalance,
    now: DateTime<Utc>,
    config: &AuditorConfig,
) -> Option<(u64, u64)> {
    if !is_available_for_paid_swap(from_mint, now) || !is_available_for_paid_swap(to_mint, now) {
        return None;
    }
    amount_window(from_mint, to_mint, config)
}

/// The amount window for a directed pair *ignoring* the bench/cooldown gate:
/// URL/support/amount limits/headroom only. The error-retry loop uses this so it can
/// deliberately re-audit a mint that is currently benched.
fn amount_window(
    from_mint: &MintWithBalance,
    to_mint: &MintWithBalance,
    config: &AuditorConfig,
) -> Option<(u64, u64)> {
    if from_mint.mint.url == to_mint.mint.url {
        return None;
    }
    if !from_mint.swap_support.can_spend || !to_mint.swap_support.can_receive {
        return None;
    }

    // Safety invariant: the auditor must not hold more ecash on a mint than was
    // donated for it, plus a small fixed working-capital overshoot
    // (`balance <= sum_donations + min_amount`). Summed over all mints this
    // still bounds total holdings close to total donations, which is what stops
    // a malicious mint from being fed real Lightning value it never intends to
    // honour.
    //
    // The overshoot is the liveness valve. A donation credits balance and
    // sum_donations by the same amount, so a freshly funded mint is born
    // exactly at its cap. Swaps conserve the total balance, so a hard cap pins
    // every mint at its cap with zero headroom and the engine deadlocks — no
    // mint can ever receive (this is why a new auditor logs "No suitable mint
    // pair found" forever). Allowing each receiver `min_amount` of overshoot
    // lets a minimal test swap start; per-mint extra exposure is bounded by
    // that constant. Mirrors the Python reference's `max(headroom,
    // MINIMUM_AMOUNT)` floor, extended to cover the exact-at-cap boundary.
    let receive_headroom = receive_capacity(
        to_mint.balance,
        to_mint.mint.sum_donations,
        config.receive_overshoot_sats,
    );
    if receive_headroom == 0 {
        return None;
    }

    let min_amount = config
        .min_amount
        .max(to_mint.swap_support.receive_min_amount.unwrap_or(0))
        .max(from_mint.swap_support.spend_min_amount.unwrap_or(0));
    let mut max_amount = config
        .max_amount
        .min(receive_headroom)
        .min(from_mint.balance.saturating_mul(8) / 10);

    if let Some(receive_max) = to_mint.swap_support.receive_max_amount {
        max_amount = max_amount.min(receive_max);
    }
    if let Some(spend_max) = from_mint.swap_support.spend_max_amount {
        max_amount = max_amount.min(spend_max);
    }

    (max_amount >= min_amount).then_some((min_amount, max_amount))
}

/// Directed pairs for one round under a rotating step `k in [1, n-1]` derived
/// from `round`: `(i, (i + k) % n)` for every mint `i`. Because `k != 0` and
/// `k < n`, each index appears exactly once as a sender and once as a receiver
/// (no self-swaps), so one round audits every mint in both roles. `k` is
/// `(j - i) mod n`, unique per directed pair, so walking `round` over `n-1`
/// consecutive values covers all `n*(n-1)` directed pairs exactly once; the
/// schedule then repeats with period `n-1`. Empty for `n < 2`.
///
/// Test-only: the round-robin scheme was replaced by Python-style single random
/// swap selection (`choose_swap`); retained to document/verify the old algorithm.
#[cfg(test)]
fn round_pairs(n: usize, round: u64) -> Vec<(usize, usize)> {
    if n < 2 {
        return Vec::new();
    }
    let step = (round % (n as u64 - 1)) as usize + 1;
    (0..n).map(|i| (i, (i + step) % n)).collect()
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

async fn log_wallet_debug_state(wallet: &cdk::Wallet, mint_url: &str, stage: &str) {
    let unspent_balance = wallet.total_balance().await.map(u64::from);
    let reserved_balance = wallet.total_reserved_balance().await.map(u64::from);
    let pending_balance = wallet.total_pending_balance().await.map(u64::from);

    info!(
        "Wallet debug state: mint={}, stage={}, unspent_balance={:?}, reserved_balance={:?}, pending_balance={:?}, unspent_proofs={}, reserved_proofs={}, pending_proofs={}",
        mint_url,
        stage,
        unspent_balance,
        reserved_balance,
        pending_balance,
        proof_summary(wallet.get_unspent_proofs().await.ok()),
        proof_summary(wallet.get_reserved_proofs().await.ok()),
        proof_summary(wallet.get_pending_proofs().await.ok()),
    );
}

fn proof_summary(proofs: Option<Proofs>) -> String {
    let Some(proofs) = proofs else {
        return "unavailable".to_string();
    };

    let total = proofs.total_amount().map(u64::from).unwrap_or(0);
    let mut histogram: BTreeMap<u64, usize> = BTreeMap::new();
    for proof in proofs.iter() {
        *histogram.entry(u64::from(proof.amount)).or_default() += 1;
    }

    let histogram = if histogram.is_empty() {
        "none".to_string()
    } else {
        histogram
            .into_iter()
            .map(|(amount, count)| format!("{}x{}", amount, count))
            .collect::<Vec<_>>()
            .join(",")
    };

    format!(
        "count={},total={},histogram=[{}]",
        proofs.len(),
        total,
        histogram
    )
}

// ---------------------------------------------------------------------------
// Persistence helpers
// ---------------------------------------------------------------------------

/// Bench a mint for the configured cooldown so the main swap loop retries it
/// later rather than excluding it forever. The dedicated error-retry loop
/// re-audits Error mints far sooner regardless of this cooldown. Uses an atomic
/// UPDATE so concurrent writes can't clobber.
async fn bench_mint(db: &Arc<AuditorDatabase>, mint_id: u64, config: &AuditorConfig) {
    let retry_at = Utc::now() + ChronoDuration::seconds(config.error_cooldown_secs as i64);
    if let Err(error) = db.record_mint_failure(mint_id, retry_at).await {
        error!(mint_id, %error, "Could not persist mint cooldown after failed swap");
    }
}

/// Record a failed swap, attributed to the mint that actually failed and
/// tagged with the stage and a coarse error category. This is the single
/// choke point that makes every failure visible and correctly blamed.
#[allow(clippy::too_many_arguments)]
async fn record_failure(
    db: &Arc<AuditorDatabase>,
    from_mint: &Mint,
    to_mint: &Mint,
    amount: u64,
    fee: u64,
    time_taken: f64,
    stage: SwapStage,
    failed_mint: &Mint,
    err: &AuditorError,
) {
    let raw = err.to_string();
    let detail = format!("{} — {}", stage.label(), classify::enrich_raw(&raw));
    // Use the same wallet-fault decision as benching, including wrapped CDK
    // errors. Text classification must not turn our local fault into mint blame.
    let category = if is_wallet_state_error(err) {
        Some("internal".to_string())
    } else {
        classify::categorize(Some(&detail)).map(str::to_string)
    };

    error!(
        "Swap {} -> {} failed at stage '{}' (blaming {}): {}",
        from_mint.url,
        to_mint.url,
        stage.label(),
        failed_mint.url,
        raw
    );

    let event = SwapEvent {
        id: 0,
        from_id: from_mint.id,
        to_id: to_mint.id,
        from_url: from_mint.url.clone(),
        to_url: to_mint.url.clone(),
        amount,
        fee,
        created_at: Utc::now(),
        time_taken,
        state: MintState::Error,
        error: Some(detail),
        failed_mint_id: Some(failed_mint.id),
        stage: Some(stage.label().to_string()),
        category,
        fee_reserve: None,
        auditor_pubkey: String::new(),
        event_id: None,
    };

    if let Err(e) = db.insert_swap(&event).await {
        error!(
            "Could not record swap failure for {}: {}",
            failed_mint.url, e
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_proof_scan_uses_inactive_cached_keys_and_preserves_unverifiable_proofs() {
        use cdk::nuts::{KeySet, KeySetInfo, MintInfo, Proof, ProofDleq, SecretKey};
        use cdk::secret::Secret;

        let db = Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(":memory:")
                .await
                .unwrap(),
        );
        let wallet = cdk::Wallet::new(
            "https://offline-mint.example",
            CurrencyUnit::Sat,
            db,
            [0; 64],
            None,
        )
        .unwrap();
        let secret_key = SecretKey::from_slice(&[1; 32]).unwrap();
        let public_key = secret_key.public_key();
        let keys: Keys = serde_json::from_value(serde_json::json!({
            "1": public_key.to_string(),
        }))
        .unwrap();
        let keyset_id = Id::v1_from_keys(&keys);
        wallet
            .localstore
            .add_mint(wallet.mint_url.clone(), Some(MintInfo::default()))
            .await
            .unwrap();
        wallet
            .localstore
            .add_mint_keysets(
                wallet.mint_url.clone(),
                vec![KeySetInfo {
                    id: keyset_id,
                    unit: CurrencyUnit::Sat,
                    active: false,
                    input_fee_ppk: 0,
                    final_expiry: None,
                }],
            )
            .await
            .unwrap();
        wallet
            .localstore
            .add_keys(KeySet {
                id: keyset_id,
                unit: CurrencyUnit::Sat,
                active: Some(false),
                keys,
                input_fee_ppk: 0,
                final_expiry: None,
            })
            .await
            .unwrap();

        let unsigned = Proof::new(1.into(), keyset_id, Secret::new("unsigned"), public_key);
        let mut invalid = unsigned.clone();
        invalid.secret = Secret::new("invalid");
        invalid.dleq = Some(ProofDleq::new(
            secret_key.clone(),
            secret_key.clone(),
            secret_key,
        ));
        let mut unknown = invalid.clone();
        unknown.keyset_id = Id::from_str("009a1f293253e41e").unwrap();
        let expected_y = invalid.y().unwrap();

        let invalid_ys = tokio::time::timeout(
            Duration::from_secs(1),
            dleq_invalid_proof_ys(&wallet, &[unsigned, invalid, unknown]),
        )
        .await
        .expect("local proof inspection must not wait for a mint");
        assert_eq!(invalid_ys, vec![expected_y]);
    }

    fn full_support() -> SwapSupport {
        SwapSupport {
            can_receive: true,
            can_spend: true,
            ..SwapSupport::default()
        }
    }

    fn mint(url: &str, state: MintState, balance: u64, target: u64) -> MintWithBalance {
        mint_with_support(url, state, balance, target, full_support())
    }

    fn mint_with_support(
        url: &str,
        state: MintState,
        balance: u64,
        target: u64,
        swap_support: SwapSupport,
    ) -> MintWithBalance {
        MintWithBalance {
            mint: Mint {
                id: 0,
                url: url.to_string(),
                info: None,
                name: url.to_string(),
                sum_donations: target,
                updated_at: Utc::now(),
                next_update: None,
                state,
                n_errors: 0,
                n_mints: 0,
                n_melts: 0,
                latitude: None,
                longitude: None,
                deleted_at: None,
            },
            balance,
            swap_support,
        }
    }

    // Test-only convenience wrappers mirroring the previous public helpers.
    fn choose_to_mint(mints: &[MintWithBalance], now: DateTime<Utc>) -> Option<MintWithBalance> {
        let candidates: Vec<&MintWithBalance> = mints
            .iter()
            .filter(|m| is_available_for_paid_swap(m, now))
            .filter(|m| m.swap_support.can_receive)
            .filter(|m| m.balance < m.mint.sum_donations)
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let mut rng = rand::thread_rng();
        Some(candidates[rng.gen_range(0..candidates.len())].clone())
    }

    fn choose_from_mint_and_amount<'a>(
        mints: &'a [MintWithBalance],
        to_mint: &MintWithBalance,
        config: &AuditorConfig,
    ) -> Option<(&'a MintWithBalance, u64)> {
        let now = Utc::now();
        let mut suitable = Vec::new();
        for m in mints.iter().filter(|m| m.mint.url != to_mint.mint.url) {
            if !m.swap_support.can_spend {
                continue;
            }
            let Some((min_amount, max_amount)) = compatible_amount_window(m, to_mint, now, config)
            else {
                continue;
            };
            suitable.push((m, min_amount, max_amount));
        }
        let (selected, min_amount, max_amount) = suitable
            .iter()
            .max_by_key(|(m, _, max)| (m.balance, *max))?;
        let amount = rand::thread_rng().gen_range(*min_amount..=*max_amount);
        Some((*selected, amount))
    }

    #[test]
    fn wallet_state_errors_are_not_blamed_on_the_mint() {
        // Typed path: a cdk `TokenAlreadySpent` / `InsufficientFunds` converts to
        // `AuditorError::WalletState`, recognised structurally regardless of the
        // error text.
        let typed_spent = AuditorError::WalletState {
            kind: WalletStateKind::AlreadySpent,
            detail: "anything".to_string(),
        };
        assert!(is_already_spent_error(&typed_spent));
        assert!(is_wallet_state_error(&typed_spent));
        assert!(!is_insufficient_funds_error(&typed_spent));

        let typed_insufficient = AuditorError::WalletState {
            kind: WalletStateKind::InsufficientFunds,
            detail: "anything".to_string(),
        };
        assert!(is_insufficient_funds_error(&typed_insufficient));
        assert!(is_wallet_state_error(&typed_insufficient));
        assert!(!is_already_spent_error(&typed_insufficient));

        let typed_invalid = AuditorError::WalletState {
            kind: WalletStateKind::InvalidProof,
            detail: "anything".to_string(),
        };
        assert!(is_token_not_verified_error(&typed_invalid));
        assert!(is_wallet_state_error(&typed_invalid));
        assert!(!is_already_spent_error(&typed_invalid));

        // Substring fallback: an error that arrives wrapped in another variant's
        // message is still caught.
        let wrapped_spent = AuditorError::Wallet("swap failed: Token Already Spent".to_string());
        assert!(is_already_spent_error(&wrapped_spent));
        assert!(is_wallet_state_error(&wrapped_spent));

        let wrapped_insufficient = AuditorError::Wallet("Insufficient Funds".to_string());
        assert!(is_insufficient_funds_error(&wrapped_insufficient));
        assert!(is_wallet_state_error(&wrapped_insufficient));

        let wrapped_invalid = AuditorError::Wallet("swap failed: Token not verified".to_string());
        assert!(is_token_not_verified_error(&wrapped_invalid));
        assert!(is_wallet_state_error(&wrapped_invalid));

        // A genuine mint/transport fault must still bench the mint.
        let mint_fault = AuditorError::Unreachable("connection refused".to_string());
        assert!(!is_wallet_state_error(&mint_fault));
    }

    #[tokio::test]
    async fn recorded_wallet_faults_preserve_internal_category_and_health() {
        let db = Arc::new(AuditorDatabase::open_in_memory().unwrap());
        let mut from = mint("https://from.example.com", MintState::Ok, 500, 500).mint;
        let mut to = mint("https://to.example.com", MintState::Ok, 0, 500).mint;
        from.id = db.upsert_mint(&from).await.unwrap();
        to.id = db.upsert_mint(&to).await.unwrap();
        let errors = [
            AuditorError::WalletState {
                kind: WalletStateKind::AlreadySpent,
                detail: "Payment failed".into(),
            },
            AuditorError::WalletState {
                kind: WalletStateKind::InvalidProof,
                detail: "Payment failed".into(),
            },
            AuditorError::WalletState {
                kind: WalletStateKind::InsufficientFunds,
                detail: "Payment failed".into(),
            },
            AuditorError::Wallet("swap failed: Token Already Spent".into()),
            AuditorError::Wallet("Insufficient Funds".into()),
            AuditorError::Wallet("swap failed: Token not verified".into()),
            AuditorError::Wallet("database error: quote already in use".into()),
        ];
        for error in errors {
            record_failure(
                &db,
                &from,
                &to,
                10,
                0,
                0.0,
                SwapStage::PayInvoice,
                &from,
                &error,
            )
            .await;
            // Check the stored value, before read-time classification can fix it.
            let category: String = db
                .run(|conn| {
                    Ok(conn.query_row(
                        "SELECT category FROM swaps ORDER BY id DESC LIMIT 1",
                        [],
                        |row| row.get(0),
                    )?)
                })
                .await
                .unwrap();
            assert_eq!(category, "internal", "{error}");
            db.record_mint_health_ok(from.id).await.unwrap();
            assert_eq!(
                db.get_mint_by_id(from.id).await.unwrap().unwrap().state,
                MintState::Ok,
                "{error}"
            );
        }
    }

    #[test]
    fn error_retry_pairs_a_benched_mint_ignoring_cooldown() {
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            ..AuditorConfig::default()
        };
        let now = Utc::now();
        let mut benched = mint(
            "https://bad.example.com",
            MintState::Error,
            100_000,
            100_000,
        );
        benched.mint.next_update = Some(now + ChronoDuration::minutes(10)); // cooldown active
        let healthy = mint("https://good.example.com", MintState::Ok, 100, 100_000);
        let mints = vec![benched.clone(), healthy];

        // The main loop refuses the pair while the bench cooldown is active...
        assert!(compatible_amount_window(&mints[0], &mints[1], now, &config).is_none());

        // ...but the error-retry path pairs the benched mint anyway.
        let selection =
            pick_error_retry_pair(&mints, 0, &config).expect("benched mint still gets a partner");
        assert!(
            selection.from.mint.url == benched.mint.url
                || selection.to.mint.url == benched.mint.url
        );
        assert!(selection.amount >= config.min_amount && selection.amount <= config.max_amount);
    }

    #[test]
    fn fee_guard_uses_fractional_percent_config() {
        let config = AuditorConfig {
            max_fee_percent: 1.5,
            fee_tolerance_sats: 0,
            ..AuditorConfig::default()
        };
        // 1.5% of 1000 = 15 sat. Both the fee-reserve branch and the
        // total-amount branch scale with the configured percent; stay off the
        // exact 1.5% boundary so f64 rounding of 1.015 can't flip the strict `>`.
        assert!(!fee_reserve_too_high(1000, 14, 1014, 14, &config));
        assert!(fee_reserve_too_high(1000, 16, 1016, 16, &config));
    }

    #[test]
    fn receive_headroom_guard_rejects_overshoot() {
        // No overshoot: behaves as a hard cap at the donation target.
        assert!(ensure_receive_headroom("https://mint.example.com", 90, 100, 10, 0).is_ok());

        let err = ensure_receive_headroom("https://mint.example.com", 91, 100, 10, 0)
            .expect_err("amount would exceed target");
        assert!(err.to_string().contains("available headroom 9 sat"));

        let err = ensure_receive_headroom("https://mint.example.com", 120, 100, 1, 0)
            .expect_err("already over target");
        assert!(err.to_string().contains("available headroom 0 sat"));

        // Overshoot extends the cap by a fixed working-capital allowance, so a
        // mint sitting exactly at its donation target can still receive up to
        // `overshoot` more — but no further.
        assert!(ensure_receive_headroom("https://mint.example.com", 100, 100, 10, 10).is_ok());
        let err = ensure_receive_headroom("https://mint.example.com", 100, 100, 11, 10)
            .expect_err("amount exceeds target + overshoot");
        assert!(err.to_string().contains("available headroom 10 sat"));
    }

    #[test]
    fn payment_amount_guard_rejects_mismatched_quotes() {
        assert!(ensure_exact_payment_amount("https://mint.example.com", 10, 10).is_ok());
        assert!(ensure_optional_quote_amount("https://mint.example.com", None, 10).is_ok());
        assert!(ensure_optional_quote_amount(
            "https://mint.example.com",
            Some(cdk::Amount::from(10)),
            10,
        )
        .is_ok());

        let err = ensure_exact_payment_amount("https://mint.example.com", 11, 10)
            .expect_err("payment amount exceeds selected amount");
        assert!(err.to_string().contains("expected 10 sat, got 11 sat"));

        let err = ensure_optional_quote_amount(
            "https://mint.example.com",
            Some(cdk::Amount::from(9)),
            10,
        )
        .expect_err("quote amount differs from selected amount");
        assert!(err.to_string().contains("expected 10 sat, got 9 sat"));
    }

    #[test]
    fn to_mint_prefers_below_target() {
        let now = Utc::now();
        let mints = vec![
            mint("https://full.example.com", MintState::Ok, 1000, 500),
            mint("https://hungry.example.com", MintState::Ok, 100, 500),
        ];
        for _ in 0..20 {
            let chosen = choose_to_mint(&mints, now).unwrap();
            assert_eq!(chosen.mint.url, "https://hungry.example.com");
        }
    }

    #[test]
    fn to_mint_rejects_all_receivers_at_donation_cap() {
        let now = Utc::now();
        let mints = vec![
            mint("https://full.example.com", MintState::Ok, 500, 500),
            mint("https://over.example.com", MintState::Ok, 600, 500),
        ];
        assert!(choose_to_mint(&mints, now).is_none());
    }

    #[test]
    fn benched_mint_is_skipped_until_cooldown_expires() {
        let now = Utc::now();
        let mut benched = mint("https://bad.example.com", MintState::Error, 0, 500);
        benched.mint.next_update = Some(now + ChronoDuration::minutes(10));
        let ok = mint("https://good.example.com", MintState::Ok, 100, 500);

        let mints = vec![benched.clone(), ok];
        for _ in 0..20 {
            let chosen = choose_to_mint(&mints, now).unwrap();
            assert_eq!(chosen.mint.url, "https://good.example.com");
        }

        let later = now + ChronoDuration::minutes(11);
        let only_benched = vec![benched];
        let chosen = choose_to_mint(&only_benched, later).unwrap();
        assert_eq!(chosen.mint.url, "https://bad.example.com");
    }

    #[test]
    fn error_mint_without_cooldown_is_retried() {
        let now = Utc::now();
        let benched = mint("https://bad.example.com", MintState::Error, 0, 500);
        assert!(choose_to_mint(&[benched], now).is_some());
    }

    #[test]
    fn to_mint_requires_receive_support() {
        let now = Utc::now();
        let unsupported = mint_with_support(
            "https://unsupported.example.com",
            MintState::Ok,
            0,
            500,
            SwapSupport {
                can_receive: false,
                can_spend: true,
                ..SwapSupport::default()
            },
        );
        let supported = mint("https://supported.example.com", MintState::Ok, 100, 500);
        let chosen = choose_to_mint(&[unsupported, supported.clone()], now).unwrap();
        assert_eq!(chosen.mint.url, supported.mint.url);
    }

    #[test]
    fn from_mint_needs_balance_and_buffer() {
        let config = AuditorConfig::default();
        let to = mint("https://to.example.com", MintState::Ok, 0, 500);
        // balance 5 -> 0.8 * 5 = 4 sat, below the 5 sat minimum, so no buffer.
        let poor = mint("https://poor.example.com", MintState::Ok, 5, 500);
        assert!(choose_from_mint_and_amount(&[poor, to.clone()], &to, &config).is_none());

        let rich = mint("https://rich.example.com", MintState::Ok, 100_000, 500);
        let candidates = [rich, to.clone()];
        let (chosen, amount) = choose_from_mint_and_amount(&candidates, &to, &config).unwrap();
        assert_eq!(chosen.mint.url, "https://rich.example.com");
        assert!(amount >= config.min_amount && amount <= config.max_amount);
    }

    #[test]
    fn from_mint_skips_active_error_cooldown() {
        let config = AuditorConfig::default();
        let now = Utc::now();
        let to = mint("https://to.example.com", MintState::Ok, 0, 500);
        let mut benched = mint(
            "https://benched.example.com",
            MintState::Error,
            100_000,
            500,
        );
        benched.mint.next_update = Some(now + ChronoDuration::minutes(10));
        assert!(choose_from_mint_and_amount(&[benched, to.clone()], &to, &config).is_none());
    }

    #[test]
    fn from_mint_requires_spend_support_and_amount_window() {
        let config = AuditorConfig::default();
        let to = mint_with_support(
            "https://to.example.com",
            MintState::Ok,
            0,
            500,
            SwapSupport {
                can_receive: true,
                can_spend: true,
                receive_min_amount: Some(50),
                receive_max_amount: Some(70),
                ..SwapSupport::default()
            },
        );
        let no_melt = mint_with_support(
            "https://no-melt.example.com",
            MintState::Ok,
            100_000,
            500,
            SwapSupport {
                can_receive: true,
                can_spend: false,
                ..SwapSupport::default()
            },
        );
        let incompatible_range = mint_with_support(
            "https://range.example.com",
            MintState::Ok,
            100_000,
            500,
            SwapSupport {
                can_receive: true,
                can_spend: true,
                spend_max_amount: Some(40),
                ..SwapSupport::default()
            },
        );
        let good = mint_with_support(
            "https://good.example.com",
            MintState::Ok,
            100_000,
            500,
            SwapSupport {
                can_receive: true,
                can_spend: true,
                spend_min_amount: Some(60),
                spend_max_amount: Some(65),
                ..SwapSupport::default()
            },
        );

        let candidates = [no_melt, incompatible_range, good.clone(), to.clone()];
        let (chosen, amount) = choose_from_mint_and_amount(&candidates, &to, &config).unwrap();
        assert_eq!(chosen.mint.url, good.mint.url);
        assert!((60..=65).contains(&amount));
    }

    #[test]
    fn from_mint_never_equals_to_mint() {
        let config = AuditorConfig::default();
        let to = mint("https://to.example.com", MintState::Ok, 100_000, 500);
        assert!(choose_from_mint_and_amount(std::slice::from_ref(&to), &to, &config).is_none());
    }

    #[test]
    fn choose_swap_intersects_receiver_and_sender_limits() {
        let config = AuditorConfig {
            max_amount: 150,
            ..AuditorConfig::default()
        };
        let mut to = mint("https://to.example.com", MintState::Ok, 0, 500);
        to.swap_support.receive_min_amount = Some(100);
        to.swap_support.receive_max_amount = Some(115);
        let mut from = mint("https://from.example.com", MintState::Ok, 500, 0);
        from.swap_support.spend_min_amount = Some(110);
        from.swap_support.spend_max_amount = Some(120);
        let mut incompatible = mint("https://small.example.com", MintState::Ok, 500, 0);
        incompatible.swap_support.spend_max_amount = Some(80);

        for _ in 0..50 {
            let sel =
                choose_swap(&[to.clone(), from.clone(), incompatible.clone()], &config).unwrap();
            assert_eq!(sel.to.mint.url, to.mint.url);
            assert_eq!(sel.from.mint.url, from.mint.url);
            assert!((110..=115).contains(&sel.amount));
        }
        assert!(choose_swap(&[to.clone(), incompatible], &config).is_none());
        // The sender's balance buffer and receiver's headroom still bind.
        from.balance = 137; // 109 sats after the buffer, below the sender minimum.
        assert!(choose_swap(&[to.clone(), from.clone()], &config).is_none());
        from.balance = 500;
        to.mint.sum_donations = 109;
        assert!(choose_swap(&[to, from], &config).is_none());
    }

    #[test]
    fn choose_swap_handles_100_sat_minimum_with_default_limits() {
        let mut to = mint("https://to.example.com", MintState::Ok, 0, 500);
        to.swap_support.receive_min_amount = Some(100);
        let from = mint("https://from.example.com", MintState::Ok, 500, 0);
        let mints = [to, from];
        for _ in 0..20 {
            assert_eq!(
                choose_swap(&mints, &AuditorConfig::default())
                    .unwrap()
                    .amount,
                100
            );
        }
        let config = AuditorConfig {
            max_amount: 99,
            ..AuditorConfig::default()
        };
        assert!(choose_swap(&mints, &config).is_none());
    }

    #[test]
    fn choose_swap_handles_disjoint_sender_ranges() {
        let to = mint("https://to.example.com", MintState::Ok, 0, 500);
        let mut small = mint("https://small.example.com", MintState::Ok, 500, 0);
        small.swap_support.spend_min_amount = Some(10);
        small.swap_support.spend_max_amount = Some(20);
        let mut large = mint("https://large.example.com", MintState::Ok, 500, 0);
        large.swap_support.spend_min_amount = Some(80);
        large.swap_support.spend_max_amount = Some(90);
        for _ in 0..50 {
            let sel = choose_swap(
                &[to.clone(), small.clone(), large.clone()],
                &AuditorConfig::default(),
            )
            .unwrap();
            let (min, max) = amount_window(&sel.from, &sel.to, &AuditorConfig::default()).unwrap();
            assert!((min..=max).contains(&sel.amount));
            assert!((10..=20).contains(&sel.amount) || (80..=90).contains(&sel.amount));
        }
    }

    #[test]
    fn choose_swap_retries_failed_mints_only_after_cooldown() {
        let config = AuditorConfig::default();
        let mut to = mint("https://to.example.com", MintState::Error, 100, 500);
        let mut from = mint("https://from.example.com", MintState::Error, 500, 0);
        let past = Some(Utc::now() - ChronoDuration::minutes(1));
        let future = Some(Utc::now() + ChronoDuration::minutes(30));
        to.mint.next_update = past;
        from.mint.next_update = past;
        assert!(choose_swap(&[to.clone(), from.clone()], &config).is_some());
        from.mint.next_update = future;
        assert!(choose_swap(&[to.clone(), from.clone()], &config).is_none());
        from.mint.next_update = past;
        to.mint.next_update = future;
        to.balance = 0; // Empty wallets must respect cooldown too.
        assert!(choose_swap(&[to, from], &config).is_none());
    }

    #[tokio::test]
    async fn invoice_limit_failure_records_amount_context_and_attribution() {
        let db = Arc::new(AuditorDatabase::open_in_memory().unwrap());
        let mut from = mint("https://from.example.com", MintState::Ok, 500, 0).mint;
        let mut to = mint("https://to.example.com", MintState::Ok, 0, 500).mint;
        from.id = db.upsert_mint(&from).await.unwrap();
        to.id = db.upsert_mint(&to).await.unwrap();
        let error = AuditorError::from(cdk::Error::AmountOutofLimitRange(
            0.into(),
            0.into(),
            0.into(),
        ))
        .with_amount_context(23, Some(100), Some(1_000_000));
        record_failure(
            &db,
            &from,
            &to,
            23,
            0,
            0.0,
            SwapStage::RequestInvoice,
            &to,
            &error,
        )
        .await;
        let event = db.get_swaps(0, 1).await.unwrap().remove(0);
        assert_eq!(event.failed_mint_id, Some(to.id));
        assert_eq!(event.category.as_deref(), Some("config"));
        let detail = event.error.unwrap();
        assert!(detail.contains("requesting invoice"));
        assert!(detail.contains("requested 23 sat"));
        assert!(detail.contains("advertised minimum: 100 sat"));
        assert!(detail.contains("advertised maximum: 1000000 sat"));
        assert!(!detail.contains("between `0`"));
    }

    #[test]
    fn choose_swap_matches_python_selection() {
        // With overshoot disabled, production selection retains the Python
        // strict cap on the receiver and 0.8-balance buffer on the sender.
        let config = AuditorConfig::default(); // min 5, max 100, overshoot 0
        let at_cap = mint("https://atcap.example.com", MintState::Ok, 500, 500);
        let hungry = mint("https://hungry.example.com", MintState::Ok, 0, 500);
        let rich = mint("https://rich.example.com", MintState::Ok, 100_000, 500);
        let mints = vec![at_cap, hungry.clone(), rich];
        for _ in 0..50 {
            let sel = choose_swap(&mints, &config).expect("a swap is available");
            // strict balance < sum_donations: only the under-cap mint receives.
            assert_eq!(sel.to.mint.url, hungry.mint.url);
            assert_ne!(sel.from.mint.url, sel.to.mint.url);
            assert!(sel.amount >= config.min_amount && sel.amount <= config.max_amount);
            assert!(sel.from.balance.saturating_mul(8) / 10 >= sel.amount);
        }

        // Every mint exactly at its donation cap -> no eligible receiver -> no
        // swap (the accepted Python strict-cap behavior, overshoot disabled).
        let all_full = vec![
            mint("https://a.example.com", MintState::Ok, 500, 500),
            mint("https://b.example.com", MintState::Ok, 500, 500),
        ];
        assert!(choose_swap(&all_full, &config).is_none());
    }

    #[test]
    fn choose_swap_uses_overshoot_for_wallet_backed_recovery() {
        // Startup recovery restores donation targets from the current wallet
        // balances. Both mints therefore start exactly at cap; the configured
        // working-capital overshoot must still make the first swap possible.
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            receive_overshoot_sats: 100,
            ..AuditorConfig::default()
        };
        let mints = vec![
            mint("https://a.example.com", MintState::Ok, 30, 30),
            mint("https://b.example.com", MintState::Ok, 21, 21),
        ];

        for _ in 0..50 {
            let selection = choose_swap(&mints, &config).expect("overshoot opens a swap");
            assert!(selection.amount >= config.min_amount);
            assert!(selection.amount <= config.receive_overshoot_sats);
            assert!(selection.from.balance.saturating_mul(8) / 10 >= selection.amount);
        }
    }

    #[test]
    fn receiver_at_cap_gets_bounded_overshoot() {
        // Liveness + safety: a mint exactly at its donation target may still
        // receive up to `config.min_amount` of working-capital overshoot (so a
        // fresh auditor can start swapping), but a mint already past
        // target + overshoot has no capacity and is rejected.
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            receive_overshoot_sats: 10,
            ..AuditorConfig::default()
        };
        let now = Utc::now();
        let from = mint("https://from.example.com", MintState::Ok, 100_000, 100_000);
        let at_target = mint("https://at.example.com", MintState::Ok, 500, 500);
        let over_target = mint("https://over.example.com", MintState::Ok, 900, 500);

        let (min_amount, max_amount) =
            compatible_amount_window(&from, &at_target, now, &config).expect("overshoot window");
        assert_eq!(min_amount, 10);
        assert_eq!(max_amount, 10); // exactly the overshoot allowance
        assert!(compatible_amount_window(&from, &over_target, now, &config).is_none());
    }

    #[test]
    fn receiver_window_never_exceeds_donation_headroom() {
        // Capacity (sum_donations + overshoot - balance) is a hard ceiling on
        // the swap amount, even when it is smaller than config.max_amount.
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            ..AuditorConfig::default()
        };
        let now = Utc::now();
        let from = mint("https://from.example.com", MintState::Ok, 100_000, 100_000);
        // 30 sat below target + 10 overshoot = 40 sat of capacity, below max_amount.
        let to = mint("https://to.example.com", MintState::Ok, 470, 500);

        let (min_amount, max_amount) =
            compatible_amount_window(&from, &to, now, &config).expect("window exists");
        assert!(
            max_amount <= 40,
            "max_amount {max_amount} exceeded capacity of 40"
        );
        assert!(min_amount <= max_amount);
    }

    #[test]
    fn tiny_headroom_floored_by_overshoot() {
        // Headroom smaller than the swap minimum used to be skipped, which (at
        // the exact-at-cap extreme) deadlocked the engine. The working-capital
        // overshoot now floors capacity so such a mint can accept a minimal
        // test swap instead.
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            receive_overshoot_sats: 10,
            ..AuditorConfig::default()
        };
        let now = Utc::now();
        let from = mint("https://from.example.com", MintState::Ok, 100_000, 100_000);
        let to = mint("https://to.example.com", MintState::Ok, 495, 500); // 5 below cap
        let (min_amount, max_amount) =
            compatible_amount_window(&from, &to, now, &config).expect("overshoot window");
        // cap 500 + overshoot 10 - balance 495 = 15 of capacity.
        assert_eq!(min_amount, 10);
        assert_eq!(max_amount, 15);
    }

    #[test]
    fn fresh_auditor_at_cap_can_still_swap() {
        // Regression: a freshly funded auditor has balance == sum_donations on
        // every mint (a donation credits both equally). A hard donation cap
        // deadlocked here — every mint had zero receive headroom and the round
        // logged "No suitable mint pair found" forever. The working-capital
        // overshoot must let a minimal test swap start.
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            receive_overshoot_sats: 10,
            ..AuditorConfig::default()
        };
        let now = Utc::now();
        let from = mint("https://one.example.com", MintState::Ok, 500, 500);
        let to = mint("https://two.example.com", MintState::Ok, 100, 100);
        let (min_amount, _max_amount) = compatible_amount_window(&from, &to, now, &config)
            .expect("at-cap mints can still swap via overshoot");
        assert!(min_amount >= config.min_amount);
    }

    #[test]
    fn larger_overshoot_unblocks_a_mint_drifted_over_target() {
        // Regression for the saturation deadlock: when balances drift above the
        // donation target (e.g. through rounding), an overshoot
        // equal to min_amount leaves headroom below min_amount and every pair is
        // skipped ("No suitable mint pair found"). A larger overshoot restores a
        // usable window.
        let now = Utc::now();
        let to = mint("https://recv.example.com", MintState::Ok, 405, 400); // 5 over target
        let from = mint("https://send.example.com", MintState::Ok, 500, 100_000);

        // Overshoot == min_amount (10): cap 410, balance 405 => headroom 5 < 10.
        let tight = AuditorConfig {
            min_amount: 10,
            max_amount: 100,
            receive_overshoot_sats: 10,
            ..AuditorConfig::default()
        };
        assert!(compatible_amount_window(&from, &to, now, &tight).is_none());

        // Overshoot 100: cap 500, balance 405 => headroom 95 >= min_amount.
        let loose = AuditorConfig {
            receive_overshoot_sats: 100,
            ..tight
        };
        let (min_amount, max_amount) =
            compatible_amount_window(&from, &to, now, &loose).expect("overshoot opens a window");
        assert!(min_amount >= loose.min_amount);
        assert!(max_amount >= min_amount);
    }

    #[test]
    fn rotating_step_audits_each_mint_and_covers_all_pairs() {
        let n = 5usize;

        // Each round audits every mint once as sender and once as receiver, with
        // no self-swaps.
        for round in 0..(n as u64 - 1) {
            let pairs = round_pairs(n, round);
            assert_eq!(pairs.len(), n);
            let mut senders: Vec<usize> = pairs.iter().map(|(f, _)| *f).collect();
            let mut receivers: Vec<usize> = pairs.iter().map(|(_, t)| *t).collect();
            senders.sort_unstable();
            receivers.sort_unstable();
            assert_eq!(senders, (0..n).collect::<Vec<_>>());
            assert_eq!(receivers, (0..n).collect::<Vec<_>>());
            assert!(pairs.iter().all(|(f, t)| f != t));
        }

        // Over n-1 rounds every directed pair appears exactly once.
        let mut seen = std::collections::HashSet::new();
        for round in 0..(n as u64 - 1) {
            for pair in round_pairs(n, round) {
                assert!(
                    seen.insert(pair),
                    "directed pair {pair:?} repeated within a sweep"
                );
            }
        }
        assert_eq!(seen.len(), n * (n - 1));

        // The schedule repeats with period n-1.
        assert_eq!(round_pairs(n, 0), round_pairs(n, n as u64 - 1));

        // Fewer than two mints yields no swaps.
        assert!(round_pairs(1, 0).is_empty());
        assert!(round_pairs(0, 7).is_empty());
    }

    #[test]
    fn benched_mint_is_skipped_as_sender_or_receiver() {
        let config = AuditorConfig {
            min_amount: 10,
            max_amount: 10,
            ..AuditorConfig::default()
        };
        let now = Utc::now();
        let mut benched = mint("https://bad.example.com", MintState::Error, 100_000, 500);
        benched.mint.next_update = Some(now + ChronoDuration::minutes(10));
        let healthy = mint("https://a.example.com", MintState::Ok, 100, 100_000);

        // The benched mint is rejected in either role; its cooldown has not expired.
        assert!(compatible_amount_window(&benched, &healthy, now, &config).is_none());
        assert!(compatible_amount_window(&healthy, &benched, now, &config).is_none());
    }
}
