//! Reconcile completed wallet work with audit accounting, without re-paying invoices.
use super::{
    error::AuditorError,
    net::{self, NetConfig},
    state::AppState,
    storage::{AuditorDatabase, SwapEvent},
};
use cdk::wallet::{
    types::{TransactionDirection, TransactionId, TransactionStatus},
    Wallet,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingPayment {
    pub id: String,
    pub from_url: String,
    pub to_url: String,
    pub mint_quote: String,
    pub melt_quote: String,
    pub amount: u64,
    pub fee_reserve: u64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl AuditorDatabase {
    pub async fn begin_donation(
        &self,
        receipt: String,
        mint_url: String,
        via: String,
    ) -> Result<Option<u64>, AuditorError> {
        self.run(move |c| {
            c.execute("INSERT OR IGNORE INTO donation_receipts(receipt,mint_url,via,created_at) VALUES(?1,?2,?3,?4)", params![receipt,mint_url,via,chrono::Utc::now().to_rfc3339()])?;
            Ok(c.query_row("SELECT amount FROM donation_receipts WHERE receipt=?1 AND mint_url=?2", params![receipt,mint_url], |r| r.get(0))?)
        }).await
    }

    pub async fn begin_payment(&self, payment: PendingPayment) -> Result<(), AuditorError> {
        self.run(move |c| {
            c.execute("INSERT INTO pending_payments(id,from_url,to_url,mint_quote,melt_quote,amount,fee_reserve,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![payment.id,payment.from_url,payment.to_url,payment.mint_quote,payment.melt_quote,payment.amount,payment.fee_reserve,payment.created_at.to_rfc3339()])?;
            Ok(())
        }).await
    }

    pub async fn pending_payments(&self) -> Result<Vec<PendingPayment>, AuditorError> {
        self.run(|c| {
            let mut statement = c.prepare("SELECT id,from_url,to_url,mint_quote,melt_quote,amount,fee_reserve,created_at FROM pending_payments WHERE swap_id IS NULL ORDER BY created_at")?;
            let rows = statement.query_map([], |r| Ok(PendingPayment {
                id:r.get(0)?, from_url:r.get(1)?, to_url:r.get(2)?, mint_quote:r.get(3)?, melt_quote:r.get(4)?, amount:r.get(5)?, fee_reserve:r.get(6)?,
                created_at: chrono::DateTime::parse_from_rfc3339(&r.get::<_,String>(7)?).map_err(|e| rusqlite::Error::FromSqlConversionFailure(7,rusqlite::types::Type::Text,Box::new(e)))?.with_timezone(&chrono::Utc),
            }))?.collect::<Result<Vec<_>,_>>()?;
            Ok(rows)
        }).await
    }
}

/// The input proof identifiers survive CDK recovery even when user metadata does not.
pub async fn reconcile_donations(
    db: &AuditorDatabase,
    wallet: &Wallet,
) -> Result<(), AuditorError> {
    for transaction in wallet
        .list_transactions(Some(TransactionDirection::Incoming))
        .await?
    {
        if transaction.status != TransactionStatus::Completed || transaction.quote_id.is_some() {
            continue;
        }
        let receipt = receipt_key(
            &wallet.mint_url.to_string(),
            &TransactionId::new(transaction.ys).to_string(),
        );
        let key = receipt.clone();
        let via = db
            .run(move |c| {
                Ok(c.query_row(
                    "SELECT via FROM donation_receipts WHERE receipt=?1 AND amount IS NULL",
                    [key],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
            })
            .await?;
        if let Some(via) = via {
            let amount = u64::from(transaction.amount);
            if amount > 0 {
                db.record_donation_receipt(
                    &wallet.mint_url.to_string(),
                    amount,
                    &via,
                    Some(receipt),
                )
                .await?;
            }
        }
    }
    Ok(())
}

pub async fn paid_fee(wallet: &Wallet, quote: &str) -> Result<Option<u64>, AuditorError> {
    Ok(wallet
        .list_transactions(Some(TransactionDirection::Outgoing))
        .await?
        .into_iter()
        .find(|t| t.quote_id.as_deref() == Some(quote) && t.status == TransactionStatus::Completed)
        .map(|t| t.fee.into()))
}

pub async fn run(state: AppState, mut shutdown: tokio::sync::watch::Receiver<bool>) {
    let net = NetConfig::from_env();
    loop {
        state.workers.beat("wallet recovery");
        for wallet in state.wallet_repository.get_wallets().await {
            if *shutdown.borrow() {
                return;
            }
            let _guard = state
                .wallet_guards
                .lock_one(&wallet.mint_url.to_string())
                .await;
            if let Err(error) = net::with_timeout(
                "recover wallet",
                net.request_timeout.saturating_mul(3),
                wallet.recover_incomplete_sagas(),
            )
            .await
            {
                tracing::warn!(mint = %wallet.mint_url, %error, "Wallet recovery will retry");
            }
            state.workers.beat("wallet recovery");
            if let Err(error) = import_untracked_quotes(&state.auditor_db, &wallet).await {
                tracing::warn!(%error, "Quote recovery will retry");
            }
            if let Err(error) = reconcile_donations(&state.auditor_db, &wallet).await {
                tracing::warn!(%error, "Donation reconciliation will retry");
            }
        }
        match state.auditor_db.pending_payments().await {
            Ok(payments) => {
                for payment in payments {
                    if *shutdown.borrow() {
                        return;
                    }
                    state.workers.beat("wallet recovery");
                    let _guard = state
                        .wallet_guards
                        .lock_pair(&payment.from_url, &payment.to_url)
                        .await;
                    if let Err(error) = net::with_timeout(
                        "recover payment",
                        net.request_timeout.saturating_mul(4),
                        recover_payment(&state, &payment),
                    )
                    .await
                    {
                        tracing::warn!(operation = %payment.id, %error, "Payment recovery will retry without re-paying");
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "Could not read pending payments"),
        }
        state.cache.clear();
        tokio::select! {
            _ = shutdown.changed() => return,
            _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => {},
        }
    }
}

pub fn receipt_key(mint_url: &str, proof_id: &str) -> String {
    format!(
        "{}:{proof_id}",
        super::mint_url::normalize_mint_url(mint_url)
    )
}

// Older versions did not journal payments. Reclaim their ecash without inventing audit history.
async fn import_untracked_quotes(
    db: &AuditorDatabase,
    wallet: &Wallet,
) -> Result<(), AuditorError> {
    if wallet.unit != cdk::nuts::CurrencyUnit::Sat {
        return Ok(());
    }
    for quote in wallet
        .localstore
        .get_unissued_mint_quotes()
        .await
        .map_err(cdk::Error::from)?
    {
        if quote.mint_url != wallet.mint_url || quote.unit != wallet.unit {
            continue;
        }
        let Some(amount) = quote.amount else {
            continue;
        };
        let url = wallet.mint_url.to_string();
        db.run(move |c| {
            c.execute("INSERT INTO pending_payments(id,from_url,to_url,mint_quote,melt_quote,amount,fee_reserve,created_at)
                SELECT ?1,'',?2,?3,'',?4,0,?5 WHERE NOT EXISTS(SELECT 1 FROM pending_payments WHERE to_url=?2 AND mint_quote=?3)",
                params![uuid::Uuid::new_v4().to_string(),url,quote.id,u64::from(amount),chrono::Utc::now().to_rfc3339()])?;
            Ok(())
        }).await?;
    }
    Ok(())
}

async fn received_amount(wallet: &Wallet, quote: &str) -> Result<u64, AuditorError> {
    wallet
        .list_transactions(Some(TransactionDirection::Incoming))
        .await?
        .into_iter()
        .filter(|t| {
            t.quote_id.as_deref() == Some(quote) && t.status == TransactionStatus::Completed
        })
        .try_fold(0u64, |total, t| {
            total
                .checked_add(t.amount.into())
                .ok_or_else(|| AuditorError::Internal("recovered amount overflow".into()))
        })
}

async fn retire_legacy(db: &AuditorDatabase, id: &str) -> Result<(), AuditorError> {
    let id = id.to_owned();
    db.run(move |c| {
        c.execute(
            "UPDATE pending_payments SET swap_id=0 WHERE id=?1 AND swap_id IS NULL",
            [id],
        )?;
        Ok(())
    })
    .await
}

async fn recover_payment(state: &AppState, payment: &PendingPayment) -> Result<(), AuditorError> {
    let to = state.wallet_for_mint(&payment.to_url).await?;
    let mut received = received_amount(&to, &payment.mint_quote).await?;
    let mut expired_unpaid = false;
    // Completed wallet work can be accounted even while the remote mint is offline.
    if received < payment.amount {
        let quote = to.check_mint_quote_status(&payment.mint_quote).await?;
        let mintable = u64::from(quote.amount_mintable());
        if mintable > payment.amount.saturating_sub(received) {
            return Err(AuditorError::Internal(
                "recovery quote amount differs from payment journal".into(),
            ));
        }
        if mintable > 0 {
            to.mint(&payment.mint_quote, cdk::amount::SplitTarget::None, None)
                .await?;
            received = received_amount(&to, &payment.mint_quote).await?;
        }
        expired_unpaid = quote.amount_paid == cdk::Amount::ZERO
            && quote.expiry.saturating_add(300) < chrono::Utc::now().timestamp().max(0) as u64;
    }
    if received > payment.amount {
        return Err(AuditorError::Internal(
            "recovered amount exceeds payment journal".into(),
        ));
    }
    if payment.from_url.is_empty() {
        if received == payment.amount || expired_unpaid {
            retire_legacy(&state.auditor_db, &payment.id).await?;
        }
        return Ok(());
    }
    let from = state.wallet_for_mint(&payment.from_url).await?;
    let a = state
        .auditor_db
        .get_mint_by_url(&payment.from_url)
        .await?
        .ok_or_else(|| AuditorError::NotFound(payment.from_url.clone()))?;
    let b = state
        .auditor_db
        .get_mint_by_url(&payment.to_url)
        .await?
        .ok_or_else(|| AuditorError::NotFound(payment.to_url.clone()))?;
    if received == payment.amount {
        let fee = paid_fee(&from, &payment.melt_quote).await?;
        let mut event = SwapEvent::success(&a, &b, payment.amount, fee.unwrap_or(0), 0.0);
        event.created_at = payment.created_at;
        event.fee_known = fee.is_some();
        event.fee_reserve = Some(payment.fee_reserve);
        event.stage = Some("recovered payment".into());
        state
            .auditor_db
            .complete_payment(&event, Some(payment.id.clone()))
            .await?;
    } else if expired_unpaid {
        let source = from.check_melt_quote_status(&payment.melt_quote).await?;
        if source.state == cdk::nuts::MeltQuoteState::Unpaid && source.used_by_operation.is_none() {
            let mut event = SwapEvent::success(&a, &b, payment.amount, 0, 0.0);
            event.created_at = payment.created_at;
            event.state = crate::types::MintState::Error;
            event.stage = Some("recovered unpaid invoice".into());
            event.error = Some("Invoice expired; both mints confirmed it was not paid".into());
            event.fee_reserve = Some(payment.fee_reserve);
            state
                .auditor_db
                .complete_payment(&event, Some(payment.id.clone()))
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::state::test_state;
    use cdk::nuts::CurrencyUnit;
    use cdk::wallet::types::Transaction;

    fn transaction(wallet: &Wallet, amount: u64, quote: Option<&str>) -> Transaction {
        Transaction {
            mint_url: wallet.mint_url.clone(),
            direction: TransactionDirection::Incoming,
            amount: amount.into(),
            fee: 0.into(),
            unit: CurrencyUnit::Sat,
            ys: vec![],
            timestamp: 1,
            memo: None,
            metadata: Default::default(),
            quote_id: quote.map(str::to_owned),
            payment_request: None,
            payment_proof: None,
            payment_method: None,
            saga_id: Some(uuid::Uuid::new_v4()),
            status: TransactionStatus::Completed,
        }
    }

    #[tokio::test]
    async fn completed_receive_survives_accounting_failure_and_is_counted_once() {
        let state = test_state().await;
        let wallet = state
            .wallet_for_mint("https://offline.example")
            .await
            .unwrap();
        let tx = transaction(&wallet, 19, None);
        let receipt = receipt_key(
            &wallet.mint_url.to_string(),
            &TransactionId::new(tx.ys.clone()).to_string(),
        );
        state
            .auditor_db
            .begin_donation(receipt.clone(), wallet.mint_url.to_string(), "token".into())
            .await
            .unwrap();
        wallet.localstore.add_transaction(tx).await.unwrap();
        state.auditor_db.run(|c| { c.execute_batch("CREATE TRIGGER fail_donation BEFORE INSERT ON donations BEGIN SELECT RAISE(ABORT, 'disk full'); END;")?; Ok(()) }).await.unwrap();
        assert!(reconcile_donations(&state.auditor_db, &wallet)
            .await
            .is_err());
        assert!(state
            .auditor_db
            .get_mint_by_url(&wallet.mint_url.to_string())
            .await
            .unwrap()
            .is_none());
        state
            .auditor_db
            .run(|c| {
                c.execute_batch("DROP TRIGGER fail_donation")?;
                Ok(())
            })
            .await
            .unwrap();
        for _ in 0..2 {
            reconcile_donations(&state.auditor_db, &wallet)
                .await
                .unwrap();
        }
        let mint = state
            .auditor_db
            .get_mint_by_url(&wallet.mint_url.to_string())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mint.sum_donations, 19);
        assert_eq!(
            state
                .auditor_db
                .begin_donation(receipt, wallet.mint_url.to_string(), "token".into())
                .await
                .unwrap(),
            Some(19)
        );
        assert_ne!(
            receipt_key("https://a.example", "same-proofs"),
            receipt_key("https://b.example", "same-proofs")
        );
    }

    #[tokio::test]
    async fn settled_payment_recovers_offline_and_retries_accounting_atomically() {
        let state = test_state().await;
        let a = state
            .auditor_db
            .record_donation("https://sender.invalid", 100, "token")
            .await
            .unwrap();
        let b = state
            .auditor_db
            .record_donation("https://receiver.invalid", 100, "token")
            .await
            .unwrap();
        let to = state.wallet_for_mint(&b.url).await.unwrap();
        let payment = PendingPayment {
            id: "operation".into(),
            from_url: a.url.clone(),
            to_url: b.url.clone(),
            mint_quote: "mint".into(),
            melt_quote: "melt".into(),
            amount: 21,
            fee_reserve: 3,
            created_at: chrono::Utc::now(),
        };
        state
            .auditor_db
            .begin_payment(payment.clone())
            .await
            .unwrap();
        to.localstore
            .add_transaction(transaction(&to, 21, Some("mint")))
            .await
            .unwrap();
        state.auditor_db.run(|c| { c.execute_batch("CREATE TRIGGER fail_swap BEFORE INSERT ON swaps BEGIN SELECT RAISE(ABORT, 'disk full'); END;")?; Ok(()) }).await.unwrap();
        assert!(recover_payment(&state, &payment).await.is_err());
        assert_eq!(state.auditor_db.pending_payments().await.unwrap().len(), 1);
        state
            .auditor_db
            .run(|c| {
                c.execute_batch("DROP TRIGGER fail_swap")?;
                Ok(())
            })
            .await
            .unwrap();
        for _ in 0..2 {
            recover_payment(&state, &payment).await.unwrap();
        }
        assert!(state
            .auditor_db
            .pending_payments()
            .await
            .unwrap()
            .is_empty());
        let swaps = state.auditor_db.get_swaps(0, 10).await.unwrap();
        assert_eq!(swaps.len(), 1);
        assert!(!swaps[0].fee_known);
        assert_eq!(swaps[0].fee_reserve, Some(3));
        assert_eq!(
            state
                .auditor_db
                .get_mint_by_id(a.id)
                .await
                .unwrap()
                .unwrap()
                .n_melts,
            1
        );
        assert_eq!(
            state
                .auditor_db
                .get_mint_by_id(b.id)
                .await
                .unwrap()
                .unwrap()
                .n_mints,
            1
        );
    }

    #[tokio::test]
    async fn legacy_quote_recovery_does_not_invent_audit_history() {
        let state = test_state().await;
        let to = state
            .wallet_for_mint("https://legacy.invalid")
            .await
            .unwrap();
        let quote = cdk::wallet::types::MintQuote {
            id: "legacy".into(),
            mint_url: to.mint_url.clone(),
            payment_method: "bolt11".parse().unwrap(),
            amount: Some(21.into()),
            unit: CurrencyUnit::Sat,
            request: "invoice".into(),
            state: cdk::nuts::MintQuoteState::Unpaid,
            expiry: 1,
            secret_key: None,
            amount_issued: 0.into(),
            amount_paid: 0.into(),
            updated_at: 0,
            estimated_blocks: None,
            used_by_operation: None,
            version: 0,
        };
        to.localstore.add_mint_quote(quote).await.unwrap();
        for _ in 0..2 {
            import_untracked_quotes(&state.auditor_db, &to)
                .await
                .unwrap();
        }
        let payments = state.auditor_db.pending_payments().await.unwrap();
        assert_eq!(payments.len(), 1);
        to.localstore
            .add_transaction(transaction(&to, 21, Some("legacy")))
            .await
            .unwrap();
        recover_payment(&state, &payments[0]).await.unwrap();
        import_untracked_quotes(&state.auditor_db, &to)
            .await
            .unwrap();
        assert!(state
            .auditor_db
            .pending_payments()
            .await
            .unwrap()
            .is_empty());
        assert_eq!(state.auditor_db.get_total_swaps().await.unwrap(), 0);
    }
    #[tokio::test]
    async fn unfinished_receipts_survive_database_reopen() {
        let path = std::env::temp_dir().join(format!("imf-recovery-{}.db", uuid::Uuid::new_v4()));
        let db = AuditorDatabase::open(&path).unwrap();
        let payment = PendingPayment {
            id: "durable".into(),
            from_url: "https://a.example".into(),
            to_url: "https://b.example".into(),
            mint_quote: "quote".into(),
            melt_quote: "melt".into(),
            amount: 10,
            fee_reserve: 1,
            created_at: chrono::Utc::now(),
        };
        db.begin_payment(payment).await.unwrap();
        db.begin_donation("proofs".into(), "https://b.example".into(), "token".into())
            .await
            .unwrap();
        drop(db);
        let db = AuditorDatabase::open(&path).unwrap();
        assert_eq!(db.pending_payments().await.unwrap()[0].id, "durable");
        let receipts: i64 = db
            .run(|c| {
                Ok(c.query_row(
                    "SELECT COUNT(*) FROM donation_receipts WHERE amount IS NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(receipts, 1);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
}
