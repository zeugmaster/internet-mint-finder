//! Periodic balance snapshots for the time-series charts.

use super::balance_for;
use crate::server::error::AuditorError;
use crate::server::storage::AuditorDatabase;
use cdk::wallet::WalletRepository;
use std::sync::Arc;

/// Record one balance snapshot per tracked mint. Balances are read from the
/// local wallet store, so this never touches the network.
pub(super) async fn run(
    wallet_repository: &Arc<WalletRepository>,
    auditor_db: &Arc<AuditorDatabase>,
) -> Result<(), AuditorError> {
    let mints = auditor_db.get_all_mints().await?;
    // Directory-only mints hold nothing here; a snapshot per hour each would
    // only grow the table.
    for mint in mints.into_iter().filter(|m| m.sum_donations > 0) {
        let balance = balance_for(wallet_repository, &mint.url).await;
        auditor_db.insert_snapshot(mint.id, balance, &mint).await?;
    }
    Ok(())
}
