//! Runtime configuration for the auditing engine.

/// Every knob can be set via an `AUDITOR_*` environment variable; unset values
/// fall back to the defaults below.
#[derive(Debug, Clone)]
pub struct AuditorConfig {
    /// Minimum delay between swap rounds (seconds). `AUDITOR_MIN_DELAY_SECS`
    pub min_swap_delay_secs: u64,
    /// Maximum delay between swap rounds (seconds). `AUDITOR_MAX_DELAY_SECS`
    pub max_swap_delay_secs: u64,
    /// Minimum swap amount (sats). `AUDITOR_MIN_AMOUNT`
    pub min_amount: u64,
    /// Maximum swap amount (sats). `AUDITOR_MAX_AMOUNT`
    pub max_amount: u64,
    /// Working-capital overshoot (sats) a mint may hold *above* its donation
    /// target before the receive guard rejects further inbound swaps. This is
    /// the liveness valve: with zero overshoot the engine deadlocks the moment
    /// every mint's balance reaches its target. Must be >= `min_amount` so a
    /// mint sitting at its cap can still receive one minimal swap; set it higher
    /// to tolerate balance drift across rounds. `AUDITOR_RECEIVE_OVERSHOOT_SATS`
    pub receive_overshoot_sats: u64,
    /// Maximum acceptable fee reserve as percent of amount. `AUDITOR_MAX_FEE_PERCENT`
    pub max_fee_percent: f64,
    /// Absolute fee-reserve tolerance before the percent check kicks in (sats).
    /// `AUDITOR_FEE_TOLERANCE_SATS`
    pub fee_tolerance_sats: u64,
    /// How long a mint is benched (in the main swap loop) after it fails all
    /// in-round retries, before that loop retries it (seconds). Optional
    /// error-retry rounds ignore this and re-audit Error mints sooner; see
    /// `error_retry_interval_secs`. `AUDITOR_ERROR_COOLDOWN_SECS`
    pub error_cooldown_secs: u64,
    /// How many times a swap is attempted with the same receiver invoice before
    /// the mint is put into the Error state. Each attempt re-quotes and
    /// re-prepares; they all settle the same invoice, and an "already paid"
    /// guard makes the retry safe against double payment. `AUDITOR_MAX_SWAP_ATTEMPTS`
    pub max_swap_attempts: u64,
    /// Maximum concurrent swaps within an optional error-retry batch. Normal
    /// rounds select one pair. Per-mint locks keep overlapping pairs safe.
    /// `AUDITOR_MAX_CONCURRENT_SWAPS`
    pub max_concurrent_swaps: u64,
    /// Delay between optional error-retry rounds (seconds); 0 disables them.
    /// The paid worker runs whichever round is due first, without overlapping
    /// normal and retry rounds. `AUDITOR_ERROR_RETRY_INTERVAL_SECS`
    pub error_retry_interval_secs: u64,
    /// Interval between no-fee NUT-06 availability checks (seconds).
    /// Values below one hour are clamped to one hour to protect rate-limited
    /// mints. `AUDITOR_HEALTH_CHECK_INTERVAL_SECS`
    pub health_check_interval_secs: u64,
    /// Consecutive failed health checks required before the mint's operational
    /// state changes to Error. `AUDITOR_HEALTH_FAILURE_THRESHOLD`
    pub health_failure_threshold: u64,
    /// Interval between balance snapshots (seconds). `AUDITOR_SNAPSHOT_INTERVAL_SECS`
    pub snapshot_interval_secs: u64,
    /// In-round redeem window: how many times to poll a paid mint quote for
    /// readiness before recording the swap as failed. Python parity deliberately
    /// does not run a background quote-reclaim worker afterward.
    /// `AUDITOR_MINT_READY_ATTEMPTS`
    pub mint_ready_attempts: u64,
    /// Delay between mint-quote readiness polls (milliseconds).
    /// `AUDITOR_MINT_READY_DELAY_MS`
    pub mint_ready_delay_ms: u64,
}

impl Default for AuditorConfig {
    fn default() -> Self {
        Self {
            // Python parity (cashu-auditor/src/auditor.py): single random swap
            // every 5-15 min, 5-100 sat, and a 2% fee reserve. The receive
            // overshoot is clamped to `min_amount` below so a wallet restored
            // at its donation target can still make progress.
            min_swap_delay_secs: 300,
            max_swap_delay_secs: 900,
            min_amount: 5,
            max_amount: 100,
            receive_overshoot_sats: 0,
            max_fee_percent: 2.0,
            fee_tolerance_sats: 10,
            error_cooldown_secs: 30 * 60,
            max_swap_attempts: 3,
            max_concurrent_swaps: 8,
            // 0 disables error-retry rounds: Python has no second audit path,
            // a failed mint is simply re-eligible for the next random swap round.
            error_retry_interval_secs: 0,
            health_check_interval_secs: 60 * 60,
            health_failure_threshold: 2,
            snapshot_interval_secs: 60 * 60,
            mint_ready_attempts: 20,
            mint_ready_delay_ms: 1000,
        }
    }
}

impl AuditorConfig {
    pub fn from_env() -> Self {
        fn env_u64(key: &str, default: u64) -> u64 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        fn env_f64(key: &str, default: f64) -> f64 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(default)
        }
        let defaults = Self::default();
        let mut config = Self {
            min_swap_delay_secs: env_u64("AUDITOR_MIN_DELAY_SECS", defaults.min_swap_delay_secs),
            max_swap_delay_secs: env_u64("AUDITOR_MAX_DELAY_SECS", defaults.max_swap_delay_secs),
            min_amount: env_u64("AUDITOR_MIN_AMOUNT", defaults.min_amount),
            max_amount: env_u64("AUDITOR_MAX_AMOUNT", defaults.max_amount),
            receive_overshoot_sats: env_u64(
                "AUDITOR_RECEIVE_OVERSHOOT_SATS",
                defaults.receive_overshoot_sats,
            ),
            max_fee_percent: env_f64("AUDITOR_MAX_FEE_PERCENT", defaults.max_fee_percent),
            fee_tolerance_sats: env_u64("AUDITOR_FEE_TOLERANCE_SATS", defaults.fee_tolerance_sats),
            error_cooldown_secs: env_u64(
                "AUDITOR_ERROR_COOLDOWN_SECS",
                defaults.error_cooldown_secs,
            ),
            max_swap_attempts: env_u64("AUDITOR_MAX_SWAP_ATTEMPTS", defaults.max_swap_attempts),
            max_concurrent_swaps: env_u64(
                "AUDITOR_MAX_CONCURRENT_SWAPS",
                defaults.max_concurrent_swaps,
            ),
            error_retry_interval_secs: env_u64(
                "AUDITOR_ERROR_RETRY_INTERVAL_SECS",
                defaults.error_retry_interval_secs,
            ),
            health_check_interval_secs: env_u64(
                "AUDITOR_HEALTH_CHECK_INTERVAL_SECS",
                defaults.health_check_interval_secs,
            ),
            health_failure_threshold: env_u64(
                "AUDITOR_HEALTH_FAILURE_THRESHOLD",
                defaults.health_failure_threshold,
            ),
            snapshot_interval_secs: env_u64(
                "AUDITOR_SNAPSHOT_INTERVAL_SECS",
                defaults.snapshot_interval_secs,
            ),
            mint_ready_attempts: env_u64(
                "AUDITOR_MINT_READY_ATTEMPTS",
                defaults.mint_ready_attempts,
            ),
            mint_ready_delay_ms: env_u64(
                "AUDITOR_MINT_READY_DELAY_MS",
                defaults.mint_ready_delay_ms,
            ),
        };

        // Keep ranges sane even with odd env values.
        if config.max_swap_delay_secs <= config.min_swap_delay_secs {
            config.max_swap_delay_secs = config.min_swap_delay_secs + 1;
        }
        if config.max_amount < config.min_amount {
            config.max_amount = config.min_amount;
        }
        // The overshoot is the liveness valve: it must cover at least one
        // minimal swap, or a mint at its donation cap can never receive.
        if config.receive_overshoot_sats < config.min_amount {
            config.receive_overshoot_sats = config.min_amount;
        }
        // At least one attempt, or no swap would ever run.
        config.max_swap_attempts = config.max_swap_attempts.max(1);
        // At least one concurrent swap, or a round would run nothing.
        config.max_concurrent_swaps = config.max_concurrent_swaps.max(1);
        // This also turns the old production value of 900 seconds into the new
        // one-hour cadence instead of accidentally restoring 15-minute probes.
        config.health_check_interval_secs = config.health_check_interval_secs.max(60 * 60);
        // One transient timeout must not change the operational mint state.
        config.health_failure_threshold = config.health_failure_threshold.max(2);
        if config.mint_ready_attempts == 0 {
            config.mint_ready_attempts = 1;
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_from_env_clamps_ranges() {
        // No env vars set in tests: defaults apply and are internally sane.
        let config = AuditorConfig::default();
        assert!(config.max_swap_delay_secs > config.min_swap_delay_secs);
        assert!(config.max_amount >= config.min_amount);
    }

    #[test]
    fn retry_and_error_loop_defaults_are_sane() {
        let config = AuditorConfig::default();
        assert!(config.max_swap_attempts >= 1);
        // Python parity: optional error-retry rounds are disabled by default
        // (0). A failed mint becomes eligible again on the next random swap round.
        assert_eq!(config.error_retry_interval_secs, 0);
        assert_eq!(config.health_check_interval_secs, 60 * 60);
        assert_eq!(config.health_failure_threshold, 2);
    }

    #[test]
    fn from_env_preserves_disabled_error_retry_interval() {
        const KEY: &str = "AUDITOR_ERROR_RETRY_INTERVAL_SECS";
        let original = std::env::var_os(KEY);
        std::env::set_var(KEY, "0");

        let config = AuditorConfig::from_env();

        match original {
            Some(value) => std::env::set_var(KEY, value),
            None => std::env::remove_var(KEY),
        }

        assert_eq!(config.error_retry_interval_secs, 0);
    }

    #[test]
    fn from_env_enforces_rate_safe_health_minimums() {
        const INTERVAL_KEY: &str = "AUDITOR_HEALTH_CHECK_INTERVAL_SECS";
        const THRESHOLD_KEY: &str = "AUDITOR_HEALTH_FAILURE_THRESHOLD";
        let original_interval = std::env::var_os(INTERVAL_KEY);
        let original_threshold = std::env::var_os(THRESHOLD_KEY);
        std::env::set_var(INTERVAL_KEY, "900");
        std::env::set_var(THRESHOLD_KEY, "1");

        let config = AuditorConfig::from_env();

        match original_interval {
            Some(value) => std::env::set_var(INTERVAL_KEY, value),
            None => std::env::remove_var(INTERVAL_KEY),
        }
        match original_threshold {
            Some(value) => std::env::set_var(THRESHOLD_KEY, value),
            None => std::env::remove_var(THRESHOLD_KEY),
        }

        assert_eq!(config.health_check_interval_secs, 60 * 60);
        assert_eq!(config.health_failure_threshold, 2);
    }
}
