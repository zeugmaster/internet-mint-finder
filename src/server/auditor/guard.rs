//! Per-mint async locks shared across the engine's paid audit work.
//!
//! Paid swaps mutate the *local proof store* of both mints they touch. Running
//! two against the same mint concurrently races proof selection —
//! `prepare_melt` on one task can pick proofs another task is already spending
//! — and at best wastes a swap on a "Token Already Spent" abort.
//!
//! These guards serialise all proof-mutating work *per mint* while letting
//! disjoint mints run in parallel. Locks are always acquired in URL order, so
//! a pair lock can never deadlock against another pair lock or a single lock:
//! every acquisition respects one strict total order over the lock set.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// Registry of per-mint locks, keyed by mint URL. Cheap to clone (shares the
/// underlying map) so concurrent swaps share the same registry.
#[derive(Clone, Default)]
pub struct MintGuards {
    locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

/// Held for the duration of a swap; dropping it releases both mints. Bind it to
/// a named variable (`let _g = ...`) — binding to `_` would drop it instantly.
pub struct PairGuard {
    _first: OwnedMutexGuard<()>,
    _second: Option<OwnedMutexGuard<()>>,
}

impl MintGuards {
    fn handle(&self, url: &str) -> Arc<AsyncMutex<()>> {
        let mut map = self.locks.lock().expect("mint guard registry poisoned");
        map.entry(url.to_string()).or_default().clone()
    }

    /// Exclusive access to one mint's wallet.
    pub async fn lock_one(&self, url: &str) -> OwnedMutexGuard<()> {
        self.handle(url).lock_owned().await
    }

    /// Exclusive access to both mints of a swap, acquired in URL order so the
    /// global lock ordering stays total and concurrent pairs never deadlock.
    /// A swap with two equal URLs is rejected upstream; if the URLs ever do
    /// coincide we still take the single lock exactly once.
    pub async fn lock_pair(&self, a: &str, b: &str) -> PairGuard {
        if a == b {
            return PairGuard {
                _first: self.lock_one(a).await,
                _second: None,
            };
        }
        let (first, second) = if a < b { (a, b) } else { (b, a) };
        // Resolve both handles up front so we never hold the registry lock
        // across an await.
        let first_handle = self.handle(first);
        let second_handle = self.handle(second);
        let first_guard = first_handle.lock_owned().await;
        let second_guard = second_handle.lock_owned().await;
        PairGuard {
            _first: first_guard,
            _second: Some(second_guard),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn same_mint_is_serialised() {
        let guards = MintGuards::default();
        let g = guards.lock_one("https://a.example.com").await;
        // A second lock on the same mint must not be grantable while held.
        let blocked = tokio::time::timeout(
            Duration::from_millis(50),
            guards.lock_one("https://a.example.com"),
        )
        .await;
        assert!(
            blocked.is_err(),
            "second lock should block while first is held"
        );
        drop(g);
        // Once released it is grantable again.
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            guards.lock_one("https://a.example.com"),
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn disjoint_mints_run_in_parallel() {
        let guards = MintGuards::default();
        let _a = guards.lock_one("https://a.example.com").await;
        // A different mint is independent and grantable immediately.
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            guards.lock_one("https://b.example.com"),
        )
        .await
        .is_ok());
    }

    #[tokio::test]
    async fn pair_lock_orders_by_url_regardless_of_argument_order() {
        // Both directions of a pair acquire the same two locks; the second
        // caller must block until the first releases, proving a shared order.
        let guards = MintGuards::default();
        let held = guards
            .lock_pair("https://b.example.com", "https://a.example.com")
            .await;
        let blocked = tokio::time::timeout(
            Duration::from_millis(50),
            guards.lock_pair("https://a.example.com", "https://b.example.com"),
        )
        .await;
        assert!(blocked.is_err(), "overlapping pair should block");
        drop(held);
        assert!(tokio::time::timeout(
            Duration::from_millis(50),
            guards.lock_pair("https://a.example.com", "https://b.example.com"),
        )
        .await
        .is_ok());
    }
}
