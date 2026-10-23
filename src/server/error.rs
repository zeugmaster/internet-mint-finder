//! Unified error type for the auditor backend.
//!
//! Every fallible server-side operation returns [`AuditorError`], which maps
//! cleanly onto HTTP status codes for the REST API and onto `ServerFnError`
//! for Leptos server functions.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

/// A wallet-state fault that originates in the auditor's own local state — a
/// stale proof store or insufficient local liquidity — rather than a fault of
/// the mint. Detected structurally from the typed [`cdk::Error`], so the
/// classification survives any change to cdk's error wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletStateKind {
    /// The mint rejected our inputs as already spent (NUT-07): the local proof
    /// store is out of sync with the mint ("zombie" proofs).
    AlreadySpent,
    /// The mint could not verify a proof's signature. This can be repaired by
    /// deterministic restore when an older client paired restored signatures
    /// with the wrong secrets.
    InvalidProof,
    /// Local proof selection could not cover the requested amount.
    InsufficientFunds,
}

impl std::fmt::Display for WalletStateKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WalletStateKind::AlreadySpent => write!(f, "token already spent"),
            WalletStateKind::InvalidProof => write!(f, "token not verified"),
            WalletStateKind::InsufficientFunds => write!(f, "insufficient funds"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuditorError {
    /// The request payload or parameters were invalid.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A referenced resource does not exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// The auditor SQLite database failed.
    #[error("database error: {0}")]
    Database(String),

    /// A CDK wallet operation failed (token invalid, quote rejected, ...).
    #[error("wallet error: {0}")]
    Wallet(String),

    /// NUT-04/NUT-05 rejection (11006). Keep it typed so the caller can add
    /// the requested amount and advertised limits after CDK loses the detail.
    #[error("mint amount rejected: {0}")]
    AmountRejected(String),

    /// A mint could not be reached (connection refused/reset, DNS, TLS).
    #[error("mint unreachable: {0}")]
    Unreachable(String),

    /// A mint network call exceeded its deadline.
    #[error("mint timeout: {0}")]
    Timeout(String),

    /// A failure rooted in the auditor's own wallet state (stale local proofs
    /// or local liquidity), not a mint fault. Recognised from the typed cdk
    /// error so it is never confused with a genuine mint-side rejection — and
    /// so it never wrongly benches a healthy mint.
    #[error("wallet state error: {kind} ({detail})")]
    WalletState {
        kind: WalletStateKind,
        detail: String,
    },

    /// Anything else.
    #[error("internal error: {0}")]
    Internal(String),
}

impl AuditorError {
    pub fn status_code(&self) -> StatusCode {
        match self {
            AuditorError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            AuditorError::NotFound(_) => StatusCode::NOT_FOUND,
            AuditorError::Database(_) | AuditorError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
            AuditorError::Wallet(_)
            | AuditorError::AmountRejected(_)
            | AuditorError::Unreachable(_) => StatusCode::BAD_GATEWAY,
            AuditorError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            AuditorError::WalletState { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Short, stable category slug for grouping/coloring in the UI and metrics.
    /// Mirrors [`crate::server::classify`] categories for engine-level errors.
    pub fn category(&self) -> &'static str {
        match self {
            AuditorError::Unreachable(_) => "network",
            AuditorError::Timeout(_) => "timeout",
            AuditorError::InvalidInput(_) => "config",
            AuditorError::NotFound(_) => "config",
            AuditorError::Database(_) | AuditorError::Internal(_) => "internal",
            AuditorError::Wallet(_) => "mint",
            AuditorError::AmountRejected(_) => "config",
            AuditorError::WalletState { .. } => "internal",
        }
    }

    /// These limits are metadata observed by the auditor, not a reconstruction
    /// of a remote error response. They may be stale if a mint changed settings.
    pub fn with_amount_context(self, amount: u64, min: Option<u64>, max: Option<u64>) -> Self {
        let Self::AmountRejected(detail) = self else {
            return self;
        };
        let bound = |value: Option<u64>| {
            value
                .map(|v| format!("{v} sat"))
                .unwrap_or_else(|| "not advertised".into())
        };
        Self::AmountRejected(format!(
            "requested {amount} sat; advertised minimum: {}; advertised maximum: {}; {detail}",
            bound(min),
            bound(max),
        ))
    }
}

impl From<rusqlite::Error> for AuditorError {
    fn from(e: rusqlite::Error) -> Self {
        AuditorError::Database(e.to_string())
    }
}

impl From<cdk::Error> for AuditorError {
    fn from(e: cdk::Error) -> Self {
        // Typed wallet-state faults first. These reflect the auditor's own local
        // state (a stale proof store or local liquidity), not a mint fault, and
        // must be recognised structurally: matching on the error *text* alone
        // would silently misclassify — and wrongly bench a healthy mint — the
        // day cdk changes its wording. A mint returning "token already spent"
        // (NUT-07 code 11001) decodes to `Error::TokenAlreadySpent`, so this
        // also covers the mint-side rejection that the reconcile path repairs.
        match &e {
            cdk::Error::AmountOutofLimitRange(min, max, amount) => {
                let detail =
                    if u64::from(*min) == 0 && u64::from(*max) == 0 && u64::from(*amount) == 0 {
                        "code 11006 (CDK did not retain the mint's limit details)".to_string()
                    } else {
                        format!("code 11006: {e}")
                    };
                return AuditorError::AmountRejected(detail);
            }
            cdk::Error::TokenAlreadySpent => {
                return AuditorError::WalletState {
                    kind: WalletStateKind::AlreadySpent,
                    detail: e.to_string(),
                };
            }
            cdk::Error::DHKE(cdk::dhke::Error::TokenNotVerified) => {
                return AuditorError::WalletState {
                    kind: WalletStateKind::InvalidProof,
                    detail: e.to_string(),
                };
            }
            cdk::Error::InsufficientFunds => {
                return AuditorError::WalletState {
                    kind: WalletStateKind::InsufficientFunds,
                    detail: e.to_string(),
                };
            }
            _ => {}
        }
        let msg = e.to_string();
        let lower = msg.to_lowercase();
        // A killed/unreachable mint surfaces as a transport error from the CDK
        // HTTP client. Route those into the dedicated variants so the UI can
        // say "unreachable" instead of a generic "wallet error", and so a real
        // mint-side rejection is never confused with the mint simply being off.
        if lower.contains("timed out") || lower.contains("timeout") || lower.contains("deadline") {
            AuditorError::Timeout(msg)
        } else if lower.contains("connection refused")
            || lower.contains("connection reset")
            || lower.contains("connection closed")
            || lower.contains("connection aborted")
            || lower.contains("connect error")
            || lower.contains("tcp connect")
            || lower.contains("dns error")
            || lower.contains("failed to lookup")
            || lower.contains("error trying to connect")
            || lower.contains("network is unreachable")
            || lower.contains("no route to host")
            || lower.contains("broken pipe")
        {
            AuditorError::Unreachable(msg)
        } else {
            AuditorError::Wallet(msg)
        }
    }
}

#[derive(Serialize)]
struct ErrorBody {
    error: String,
}

impl IntoResponse for AuditorError {
    fn into_response(self) -> Response {
        let status = self.status_code();
        if status.is_server_error() {
            tracing::error!("request failed: {}", self);
        } else {
            tracing::debug!("request rejected: {}", self);
        }
        (
            status,
            Json(ErrorBody {
                error: self.to_string(),
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_rejections_distinguish_unknown_and_known_bounds() {
        let unknown = AuditorError::from(cdk::Error::AmountOutofLimitRange(
            0.into(),
            0.into(),
            0.into(),
        ))
        .with_amount_context(5, None, None)
        .to_string();
        assert!(unknown.contains("requested 5 sat"));
        assert!(unknown.contains("not advertised"));
        assert!(unknown.contains("code 11006"));
        assert!(!unknown.contains("between `0`"));

        let known = AuditorError::from(cdk::Error::AmountOutofLimitRange(
            100.into(),
            1000.into(),
            5.into(),
        ));
        assert!(known
            .to_string()
            .contains("between `100` and `1000` is `5`"));
        assert_eq!(known.category(), "config");

        let other = AuditorError::Wallet("original mint detail".into()).with_amount_context(
            5,
            Some(100),
            Some(1000),
        );
        assert_eq!(other.to_string(), "wallet error: original mint detail");
    }

    #[test]
    fn converts_token_not_verified_to_invalid_local_proof() {
        let err = AuditorError::from(cdk::Error::DHKE(cdk::dhke::Error::TokenNotVerified));

        assert!(matches!(
            err,
            AuditorError::WalletState {
                kind: WalletStateKind::InvalidProof,
                ..
            }
        ));
    }
}
