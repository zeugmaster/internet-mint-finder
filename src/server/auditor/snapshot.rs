//! Periodic balance snapshots for the time-series charts.

use crate::server::error::AuditorError;
use crate::server::state::local_balance;
use crate::server::storage::AuditorDatabase;
use cdk::wallet::WalletRepository;
use std::sync::Arc;

/// Record one balance snapshot per tracked mint. Balances are read from the
/// local wallet store, so this never touches the network.
pub(super) async fn run(
    wallet_repository: &Arc<WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
) -> Result<(), AuditorError> {
    let mints = auditor_db.get_audited_mints().await?;
    for mint in mints {
        let balance = local_balance(wallet_repository, &mint.url).await?;
        auditor_db.insert_snapshot(mint.id, balance, &mint).await?;
    }
    Ok(())
}
