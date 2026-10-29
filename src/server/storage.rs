//! SQLite-backed auditor database.
//!
//! Relational schema with indexed lookups, time-series queries, and the
//! analytics aggregations needed by the dashboard. All queries run on the
//! blocking thread pool via [`AuditorDatabase::run`] so rusqlite never
//! stalls the async executor.

use crate::server::classify::resolve_category;
use crate::server::error::AuditorError;
use crate::types::{
    AuditorMark, MintRead, MintSort, MintState, MintUptime, MintUptimeBar, SwapEventRead,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// DDL
// ---------------------------------------------------------------------------

const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS mints (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    url             TEXT NOT NULL,
    name            TEXT NOT NULL DEFAULT '',
    info            TEXT,
    sum_donations   INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'UNKNOWN',
    n_errors        INTEGER NOT NULL DEFAULT 0,
    n_mints         INTEGER NOT NULL DEFAULT 0,
    n_melts         INTEGER NOT NULL DEFAULT 0,
    updated_at      TEXT NOT NULL,
    next_update     TEXT,
    latitude        REAL,
    longitude       REAL,
    deleted_at      TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS ix_mints_url ON mints(url);

CREATE TABLE IF NOT EXISTS mint_snapshots (
    ts              TEXT NOT NULL,
    mint_id         INTEGER NOT NULL,
    balance         INTEGER,
    sum_donations   INTEGER,
    state           TEXT,
    n_errors        INTEGER,
    n_mints         INTEGER,
    n_melts         INTEGER,
    updated_at      TEXT,
    PRIMARY KEY (mint_id, ts)
);
CREATE INDEX IF NOT EXISTS ix_mint_snapshots_ts ON mint_snapshots(ts);

CREATE TABLE IF NOT EXISTS swaps (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    from_id         INTEGER,
    to_id           INTEGER,
    from_url        TEXT,
    to_url          TEXT,
    amount          INTEGER NOT NULL DEFAULT 0,
    fee             INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'UNKNOWN',
    error           TEXT,
    failed_mint_id  INTEGER,
    stage           TEXT,
    category        TEXT,
    time_taken      REAL NOT NULL DEFAULT 0.0,
    created_at      TEXT NOT NULL,
    fee_reserve     INTEGER,
    auditor_pubkey  TEXT NOT NULL DEFAULT '',
    event_id        TEXT,
    published_at    TEXT
);
CREATE INDEX IF NOT EXISTS ix_swaps_created_at ON swaps(created_at);
CREATE INDEX IF NOT EXISTS ix_swaps_from_id    ON swaps(from_id);
CREATE INDEX IF NOT EXISTS ix_swaps_to_id      ON swaps(to_id);
CREATE INDEX IF NOT EXISTS ix_swaps_state      ON swaps(state);
CREATE INDEX IF NOT EXISTS ix_swaps_from_url   ON swaps(from_url);
CREATE INDEX IF NOT EXISTS ix_swaps_to_url     ON swaps(to_url);
-- NB: the index on the v4 column `failed_mint_id` is created in
-- `add_missing_columns`, AFTER the ALTER TABLE that adds the column, so this
-- batch stays valid against pre-v4 databases where the column doesn't exist yet.

CREATE TABLE IF NOT EXISTS mint_checks (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mint_id         INTEGER NOT NULL,
    mint_url        TEXT NOT NULL,
    kind            TEXT NOT NULL DEFAULT 'HEALTH',
    state           TEXT NOT NULL DEFAULT 'UNKNOWN',
    error           TEXT,
    category        TEXT,
    latency_ms      REAL,
    created_at      TEXT NOT NULL,
    auditor_pubkey  TEXT NOT NULL DEFAULT '',
    event_id        TEXT,
    published_at    TEXT,
    detail          TEXT
);
CREATE INDEX IF NOT EXISTS ix_mint_checks_mint_id    ON mint_checks(mint_id);
CREATE INDEX IF NOT EXISTS ix_mint_checks_created_at ON mint_checks(created_at);
CREATE INDEX IF NOT EXISTS ix_mint_checks_state      ON mint_checks(state);

CREATE TABLE IF NOT EXISTS donations (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    mint_id         INTEGER NOT NULL,
    mint_url        TEXT NOT NULL,
    amount          INTEGER NOT NULL,
    via             TEXT NOT NULL DEFAULT 'token',
    created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS ix_donations_created_at ON donations(created_at);
CREATE INDEX IF NOT EXISTS ix_donations_mint_id    ON donations(mint_id);

CREATE TABLE IF NOT EXISTS mint_announcements (
    pubkey          TEXT NOT NULL,
    d               TEXT NOT NULL,
    event_id        TEXT NOT NULL,
    mint_url        TEXT,
    nuts            TEXT,
    network         TEXT,
    content         TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL,
    PRIMARY KEY (pubkey, d)
);
CREATE INDEX IF NOT EXISTS ix_announcements_url ON mint_announcements(mint_url);
CREATE INDEX IF NOT EXISTS ix_announcements_d   ON mint_announcements(d);

CREATE TABLE IF NOT EXISTS reviews (
    pubkey          TEXT NOT NULL,
    d               TEXT NOT NULL,
    event_id        TEXT NOT NULL,
    mint_url        TEXT,
    rating          INTEGER,
    content         TEXT NOT NULL DEFAULT '',
    created_at      TEXT NOT NULL,
    PRIMARY KEY (pubkey, d)
);
CREATE INDEX IF NOT EXISTS ix_reviews_url ON reviews(mint_url);
CREATE INDEX IF NOT EXISTS ix_reviews_d   ON reviews(d);

CREATE TABLE IF NOT EXISTS auditors (
    pubkey          TEXT PRIMARY KEY,
    name            TEXT NOT NULL DEFAULT '',
    urls            TEXT NOT NULL DEFAULT '[]',
    donate          TEXT,
    software        TEXT NOT NULL DEFAULT '',
    frequency       INTEGER NOT NULL DEFAULT 0,
    contact         TEXT,
    announced_at    TEXT,
    last_seen       TEXT NOT NULL
);
"#;

// v4: added swaps.{failed_mint_id,stage,category} for correct error attribution
// and mint_checks.{category,latency_ms} for richer uptime detail. `CREATE TABLE
// IF NOT EXISTS` cannot add columns to a pre-existing table, so `init` also runs
// `add_missing_columns` to ALTER older databases in place.
// v5: added mints.deleted_at for soft-deleting (disabling) a mint.
// v6: Nostr identity + outbox on swaps and mint_checks (auditor_pubkey,
// event_id, published_at), swaps.fee_reserve, mint_checks.detail. An empty
// auditor_pubkey means "this instance"; peer rows carry the publishing
// auditor's hex pubkey and the relay event id.
// v7: auditors table (kind 10172 announcements of peer auditors).
// v8: NIP-87 mint_announcements (38172) and reviews (38000), keyed like the
// addressable events they mirror: (author pubkey, d = mint pubkey).
const SCHEMA_VERSION: i64 = 8;

// ---------------------------------------------------------------------------
// Domain structs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Mint {
    pub id: u64,
    pub url: String,
    pub info: Option<String>,
    pub name: String,
    pub sum_donations: u64,
    pub updated_at: DateTime<Utc>,
    pub next_update: Option<DateTime<Utc>>,
    pub state: MintState,
    pub n_errors: u64,
    pub n_mints: u64,
    pub n_melts: u64,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    /// Set when the mint has been soft-deleted (disabled). Deleted mints are
    /// hidden from listings and excluded from the audit, health, and snapshot loops.
    pub deleted_at: Option<DateTime<Utc>>,
}

impl Mint {
    /// The NIP-87 identity of a mint is the `pubkey` in its NUT-06 info.
    pub fn pubkey(&self) -> Option<String> {
        let info: serde_json::Value = serde_json::from_str(self.info.as_deref()?).ok()?;
        info.get("pubkey")?.as_str().map(str::to_string)
    }
}

impl From<Mint> for MintRead {
    fn from(m: Mint) -> Self {
        MintRead {
            id: m.id,
            url: m.url,
            info: m.info,
            name: m.name,
            balance: 0, // filled in by caller from CDK wallet
            balance_available: false,
            sum_donations: m.sum_donations,
            updated_at: m.updated_at,
            next_update: m.next_update,
            state: m.state,
            n_errors: m.n_errors,
            n_mints: m.n_mints,
            n_melts: m.n_melts,
            latitude: m.latitude,
            longitude: m.longitude,
            review_count: 0,
            review_avg: None,
            announced: false,
            auditors_24h: 0,
            auditors_ok_24h: 0,
            success_rate_24h: None,
            auditor_marks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapEvent {
    pub id: u64,
    pub from_id: u64,
    pub to_id: u64,
    pub from_url: String,
    pub to_url: String,
    pub amount: u64,
    pub fee: u64,
    pub created_at: DateTime<Utc>,
    pub time_taken: f64,
    pub state: MintState,
    pub error: Option<String>,
    /// Which mint actually caused a failure (`None` for successes).
    pub failed_mint_id: Option<u64>,
    /// Human label for the swap stage at which a failure occurred.
    pub stage: Option<String>,
    /// Coarse error category slug (network/mint/timeout/…).
    pub category: Option<String>,
    /// Fee reserve quoted by the sending mint (NUT-05), when known.
    #[serde(default)]
    pub fee_reserve: Option<u64>,
    /// Hex pubkey of the auditor that observed this swap; empty for local rows
    /// that have not been published yet.
    #[serde(default)]
    pub auditor_pubkey: String,
    /// Nostr event id once published or ingested.
    #[serde(default)]
    pub event_id: Option<String>,
}

/// One availability/connectivity sample (`mint_checks` row).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MintCheck {
    pub id: u64,
    pub mint_id: u64,
    pub mint_url: String,
    pub kind: String,
    pub state: MintState,
    pub error: Option<String>,
    pub category: Option<String>,
    pub latency_ms: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub auditor_pubkey: String,
    pub event_id: Option<String>,
    /// Check-type specific fields as a JSON object.
    pub detail: Option<String>,
}

/// A NIP-87 Cashu mint announcement (kind 38172), newest per (author, mint).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MintAnnouncement {
    pub pubkey: String,
    /// The mint's pubkey from `/v1/info`, per NIP-87.
    pub d: String,
    pub event_id: String,
    /// First `u` tag, normalized; `None` when it was not an HTTP(S) URL.
    pub mint_url: Option<String>,
    pub nuts: Option<String>,
    pub network: Option<String>,
    /// Kind-0 style JSON (name, description, picture) or empty.
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// A NIP-87 review (kind 38000, `k` = 38172), newest per (author, mint).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Review {
    pub pubkey: String,
    pub d: String,
    pub event_id: String,
    pub mint_url: Option<String>,
    /// Parsed from the `[N/5]` content prefix used across the ecosystem.
    pub rating: Option<u8>,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// Directory columns derived for one mint: social signal plus the 24 h
/// verdict of every auditor that touched it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DirectoryStats {
    pub review_count: u64,
    pub review_avg: Option<f64>,
    pub announced: bool,
    pub auditors_24h: u64,
    pub auditors_ok_24h: u64,
    pub attempts_24h: u64,
    pub successes_24h: u64,
    /// Each auditor's own verdict, local (empty pubkey) first.
    pub marks: Vec<AuditorMark>,
}

/// A peer auditor as last announced (kind 10172) and last seen publishing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Auditor {
    pub pubkey: String,
    pub name: String,
    pub urls: Vec<String>,
    pub donate: Option<String>,
    pub software: String,
    pub frequency_secs: u64,
    pub contact: Option<String>,
    pub announced_at: Option<DateTime<Utc>>,
    pub last_seen: DateTime<Utc>,
}

impl SwapEvent {
    /// A successful swap from `from` to `to`.
    pub fn success(from: &Mint, to: &Mint, amount: u64, fee: u64, time_taken: f64) -> Self {
        Self {
            id: 0,
            from_id: from.id,
            to_id: to.id,
            from_url: from.url.clone(),
            to_url: to.url.clone(),
            amount,
            fee,
            created_at: Utc::now(),
            time_taken,
            state: MintState::Ok,
            error: None,
            failed_mint_id: None,
            stage: None,
            category: None,
            fee_reserve: None,
            auditor_pubkey: String::new(),
            event_id: None,
        }
    }
}

/// A recorded donation (working capital received by the auditor).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Donation {
    pub id: u64,
    pub mint_id: u64,
    pub mint_url: String,
    pub amount: u64,
    pub via: String,
    pub created_at: DateTime<Utc>,
}

impl From<SwapEvent> for SwapEventRead {
    fn from(s: SwapEvent) -> Self {
        SwapEventRead {
            id: s.id,
            from_id: s.from_id,
            to_id: s.to_id,
            from_url: s.from_url,
            to_url: s.to_url,
            amount: s.amount,
            fee: s.fee,
            created_at: s.created_at,
            time_taken: s.time_taken,
            state: s.state,
            error: s.error,
            failed_mint_id: s.failed_mint_id,
            stage: s.stage,
            category: s.category,
            auditor: (!s.auditor_pubkey.is_empty()).then_some(s.auditor_pubkey),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: MintState <-> TEXT
// ---------------------------------------------------------------------------

pub(crate) fn state_to_str(s: &MintState) -> &'static str {
    match s {
        MintState::Ok => "OK",
        MintState::Warn => "WARN",
        MintState::Error => "ERROR",
        MintState::Unknown => "UNKNOWN",
    }
}

pub(crate) fn str_to_state(s: &str) -> MintState {
    match s {
        "OK" => MintState::Ok,
        "WARN" => MintState::Warn,
        "ERROR" => MintState::Error,
        _ => MintState::Unknown,
    }
}

fn insert_swap_row(conn: &Connection, swap: &SwapEvent) -> Result<u64, AuditorError> {
    let amount = i64::try_from(swap.amount)
        .map_err(|_| AuditorError::InvalidInput("swap amount is out of range".into()))?;
    let fee = i64::try_from(swap.fee)
        .map_err(|_| AuditorError::InvalidInput("swap fee is out of range".into()))?;
    conn.execute(
        "INSERT INTO swaps (from_id, to_id, from_url, to_url, amount, fee, state, error,
                            failed_mint_id, stage, category, time_taken, created_at,
                            fee_reserve, auditor_pubkey, event_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            swap.from_id as i64,
            swap.to_id as i64,
            swap.from_url,
            swap.to_url,
            amount,
            fee,
            state_to_str(&swap.state),
            swap.error,
            swap.failed_mint_id.map(|id| id as i64),
            swap.stage,
            swap.category,
            swap.time_taken,
            dt_to_str(&swap.created_at),
            swap.fee_reserve.map(|v| v as i64),
            swap.auditor_pubkey,
            swap.event_id,
        ],
    )?;
    Ok(conn.last_insert_rowid() as u64)
}

fn dt_to_str(dt: &DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn str_to_dt(s: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .or_else(|_| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%SZ")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S"))
                .map(|n| n.and_utc())
        })
        .unwrap_or_else(|_| Utc::now())
}

const MINT_COLUMNS: &str = "id, url, name, info, sum_donations, state, n_errors, n_mints, n_melts,
                            updated_at, next_update, latitude, longitude, deleted_at";

const SWAP_COLUMNS: &str = "id, from_id, to_id, from_url, to_url, amount, fee, state, error,
                            failed_mint_id, stage, category, time_taken, created_at,
                            fee_reserve, auditor_pubkey, event_id";

const MINT_CHECK_COLUMNS: &str = "id, mint_id, mint_url, kind, state, error, category, latency_ms,
                                  created_at, auditor_pubkey, event_id, detail";

/// Add columns introduced after the original schema to an existing table.
/// SQLite has no `ADD COLUMN IF NOT EXISTS`, so each addition is guarded by a
/// `pragma_table_info` lookup. Safe to run on every startup.
fn add_missing_columns(conn: &Connection) -> Result<(), AuditorError> {
    fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, AuditorError> {
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM pragma_table_info(?1) WHERE name = ?2",
            params![table, column],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    let additions: &[(&str, &str, &str)] = &[
        ("swaps", "failed_mint_id", "INTEGER"),
        ("swaps", "stage", "TEXT"),
        ("swaps", "category", "TEXT"),
        ("mint_checks", "category", "TEXT"),
        ("mint_checks", "latency_ms", "REAL"),
        ("mints", "deleted_at", "TEXT"),
        ("swaps", "fee_reserve", "INTEGER"),
        ("swaps", "auditor_pubkey", "TEXT NOT NULL DEFAULT ''"),
        ("swaps", "event_id", "TEXT"),
        ("swaps", "published_at", "TEXT"),
        ("mint_checks", "auditor_pubkey", "TEXT NOT NULL DEFAULT ''"),
        ("mint_checks", "event_id", "TEXT"),
        ("mint_checks", "published_at", "TEXT"),
        ("mint_checks", "detail", "TEXT"),
    ];

    for (table, column, ty) in additions {
        if !column_exists(conn, table, column)? {
            conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {ty}"), [])?;
            info!("Migrated auditor DB: added column {table}.{column}");
        }
    }

    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS ix_swaps_failed_mint ON swaps(failed_mint_id);
         CREATE UNIQUE INDEX IF NOT EXISTS ix_swaps_event_id ON swaps(event_id);
         CREATE UNIQUE INDEX IF NOT EXISTS ix_mint_checks_event_id ON mint_checks(event_id);
         CREATE INDEX IF NOT EXISTS ix_swaps_outbox ON swaps(published_at) WHERE published_at IS NULL;
         CREATE INDEX IF NOT EXISTS ix_mint_checks_outbox ON mint_checks(published_at) WHERE published_at IS NULL;",
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// AuditorDatabase
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AuditorDatabase {
    conn: Arc<Mutex<Connection>>,
}

impl AuditorDatabase {
    /// Open (or create) the auditor SQLite database at `path`.
    pub fn open(path: &Path) -> Result<Self, AuditorError> {
        let conn = Connection::open(path)
            .map_err(|e| AuditorError::Database(format!("failed to open auditor DB: {e}")))?;
        Self::init(conn)
    }

    /// Open an in-memory database. Used by tests.
    pub fn open_in_memory() -> Result<Self, AuditorError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| AuditorError::Database(format!("failed to open in-memory DB: {e}")))?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, AuditorError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-20000;",
        )
        .map_err(|e| AuditorError::Database(format!("failed to set pragmas: {e}")))?;

        conn.execute_batch(SCHEMA_SQL)
            .map_err(|e| AuditorError::Database(format!("failed to create schema: {e}")))?;

        // Additive column migrations for databases created before v4. Each is a
        // no-op when the column already exists.
        add_missing_columns(&conn)?;

        let version: Option<i64> = conn
            .query_row("SELECT version FROM schema_version LIMIT 1", [], |row| {
                row.get(0)
            })
            .optional()?;

        match version {
            None => {
                conn.execute(
                    "INSERT INTO schema_version(version) VALUES (?1)",
                    params![SCHEMA_VERSION],
                )?;
                info!("Initialized auditor DB schema v{}", SCHEMA_VERSION);
            }
            Some(v) if v < SCHEMA_VERSION => {
                // Migrations so far are purely additive; SCHEMA_SQL above has
                // already created any missing tables/indexes.
                conn.execute(
                    "UPDATE schema_version SET version = ?1",
                    params![SCHEMA_VERSION],
                )?;
                info!("Migrated auditor DB schema v{} -> v{}", v, SCHEMA_VERSION);
            }
            Some(v) if v > SCHEMA_VERSION => {
                warn!(
                    "Auditor DB schema version {} is newer than expected {}; \
                     proceeding read-compatibly",
                    v, SCHEMA_VERSION
                );
            }
            _ => {}
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run a closure against the connection on the blocking thread pool.
    ///
    /// This is the single entry point for all SQLite access (including the
    /// analytics module) so queries never block the async executor.
    pub async fn run<T, F>(&self, f: F) -> Result<T, AuditorError>
    where
        F: FnOnce(&Connection) -> Result<T, AuditorError> + Send + 'static,
        T: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| AuditorError::Internal("auditor DB lock poisoned".into()))?;
            f(&guard)
        })
        .await
        .map_err(|e| AuditorError::Internal(format!("blocking task failed: {e}")))?
    }

    // ----- Mints ---------------------------------------------------------------

    /// Search the entire registry, with stable pages and a matching total.
    /// One directory page of 20. Reviews count toward a mint when they name
    /// its URL or its NUT-06 key, as in `directory_stats`; unrated mints sort
    /// after rated ones either way.
    pub async fn search_mints(
        &self,
        search: String,
        state: Option<MintState>,
        sort: MintSort,
        page: u64,
    ) -> Result<(Vec<Mint>, u64), AuditorError> {
        let search = search.trim().to_lowercase();
        let state = state.map(|s| state_to_str(&s).to_string());
        let offset = page.saturating_mul(20).min(i64::MAX as u64) as i64;
        let order = match sort {
            MintSort::Name => "sort_name, id",
            MintSort::RatingHigh => "r_avg IS NULL, r_avg DESC, r_count DESC, sort_name, id",
            MintSort::RatingLow => "r_avg IS NULL, r_avg ASC, r_count DESC, sort_name, id",
            MintSort::MostReviews => "r_count DESC, r_avg IS NULL, r_avg DESC, sort_name, id",
        };
        self.run(move |conn| {
            let condition = "deleted_at IS NULL AND (?1 = '' OR instr(lower(name), ?1) > 0 OR instr(lower(url), ?1) > 0) AND (?2 IS NULL OR state = ?2)";
            let total = conn.query_row(&format!("SELECT COUNT(*) FROM mints WHERE {condition}"), params![search, state], |row| row.get(0))?;
            let mut stmt = conn.prepare(&format!(
                "WITH m AS (
                    SELECT *, CASE WHEN json_valid(info) THEN json_extract(info, '$.pubkey') END AS pk
                    FROM mints WHERE {condition}),
                 s AS (
                    SELECT m.*,
                        (SELECT AVG(rating) FROM reviews r WHERE r.mint_url = m.url OR (m.pk IS NOT NULL AND r.d = m.pk)) AS r_avg,
                        (SELECT COUNT(*) FROM reviews r WHERE r.mint_url = m.url OR (m.pk IS NOT NULL AND r.d = m.pk)) AS r_count,
                        lower(COALESCE(NULLIF(trim(m.name), ''), replace(replace(m.url, 'https://', ''), 'http://', ''))) AS sort_name
                    FROM m)
                 SELECT {MINT_COLUMNS} FROM s ORDER BY {order} LIMIT 20 OFFSET ?3"
            ))?;
            let mints = stmt.query_map(params![search, state, offset], row_to_mint)?.collect::<Result<Vec<_>, _>>()?;
            Ok((mints, total))
        }).await
    }

    pub async fn get_mints(&self, skip: u64, limit: u64) -> Result<Vec<Mint>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MINT_COLUMNS} FROM mints WHERE deleted_at IS NULL
                 ORDER BY id LIMIT ?1 OFFSET ?2"
            ))?;
            let mints = stmt
                .query_map(
                    params![limit.min(500) as i64, skip.min(i64::MAX as u64) as i64],
                    row_to_mint,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(mints)
        })
        .await
    }

    /// Every non-deleted mint, unbounded. The audit loops must see all mints,
    /// not a paginated slice — a hardcoded `LIMIT` would silently drop mints
    /// once the registry grew past it.
    pub async fn get_all_mints(&self) -> Result<Vec<Mint>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MINT_COLUMNS} FROM mints WHERE deleted_at IS NULL ORDER BY id"
            ))?;
            let mints = stmt
                .query_map([], row_to_mint)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(mints)
        })
        .await
    }

    /// Includes disabled entries: an empty active list is not an empty registry.
    pub async fn has_mint_history(&self) -> Result<bool, AuditorError> {
        self.run(|conn| {
            Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM mints)", [], |row| row.get(0))?)
        })
        .await
    }

    pub async fn get_mint_by_id(&self, id: u64) -> Result<Option<Mint>, AuditorError> {
        self.run(move |conn| {
            let mint = conn
                .query_row(
                    &format!(
                        "SELECT {MINT_COLUMNS} FROM mints WHERE id = ?1 AND deleted_at IS NULL"
                    ),
                    params![id as i64],
                    row_to_mint,
                )
                .optional()?;
            Ok(mint)
        })
        .await
    }

    pub async fn get_mint_by_url(&self, url: &str) -> Result<Option<Mint>, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            let mint = conn
                .query_row(
                    &format!(
                        "SELECT {MINT_COLUMNS} FROM mints WHERE url = ?1 AND deleted_at IS NULL"
                    ),
                    params![url],
                    row_to_mint,
                )
                .optional()?;
            Ok(mint)
        })
        .await
    }

    /// Insert or update a mint. Returns the row id.
    pub async fn upsert_mint(&self, mint: &Mint) -> Result<u64, AuditorError> {
        let mint = mint.clone();
        self.run(move |conn| {
            let now = dt_to_str(&Utc::now());
            let state_str = state_to_str(&mint.state);
            let next_update_str = mint.next_update.as_ref().map(dt_to_str);

            conn.execute(
                "INSERT INTO mints (url, name, info, sum_donations, state, n_errors, n_mints, n_melts,
                                    updated_at, next_update, latitude, longitude)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(url) DO UPDATE SET
                    name = excluded.name,
                    info = excluded.info,
                    sum_donations = excluded.sum_donations,
                    state = excluded.state,
                    n_errors = excluded.n_errors,
                    n_mints = excluded.n_mints,
                    n_melts = excluded.n_melts,
                    updated_at = excluded.updated_at,
                    next_update = excluded.next_update,
                    latitude = excluded.latitude,
                    longitude = excluded.longitude",
                params![
                    mint.url,
                    mint.name,
                    mint.info,
                    mint.sum_donations as i64,
                    state_str,
                    mint.n_errors as i64,
                    mint.n_mints as i64,
                    mint.n_melts as i64,
                    now,
                    next_update_str,
                    mint.latitude,
                    mint.longitude,
                ],
            )?;

            let id: i64 = conn.query_row(
                "SELECT id FROM mints WHERE url = ?1",
                params![mint.url],
                |row| row.get(0),
            )?;

            Ok(id as u64)
        })
        .await
    }

    /// Soft-delete (disable) a mint by URL. Sets `deleted_at` so the mint is
    /// hidden from listings and skipped by the audit, health, and snapshot loops,
    /// while its historical swaps and checks are preserved. Returns `true` if a
    /// live mint was deleted, `false` if no live mint with that URL exists.
    pub async fn soft_delete_mint_by_url(&self, url: &str) -> Result<bool, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            let changed = conn.execute(
                "UPDATE mints SET deleted_at = ?2, updated_at = ?2
                 WHERE url = ?1 AND deleted_at IS NULL",
                params![url, dt_to_str(&Utc::now())],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    // ----- Mint Snapshots ------------------------------------------------------

    pub async fn insert_snapshot(
        &self,
        mint_id: u64,
        balance: u64,
        mint: &Mint,
    ) -> Result<(), AuditorError> {
        let mint = mint.clone();
        self.run(move |conn| {
            let ts = dt_to_str(&Utc::now());
            let state_str = state_to_str(&mint.state);

            conn.execute(
                "INSERT OR IGNORE INTO mint_snapshots
                    (ts, mint_id, balance, sum_donations, state, n_errors, n_mints, n_melts, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    ts,
                    mint_id as i64,
                    balance as i64,
                    mint.sum_donations as i64,
                    state_str,
                    mint.n_errors as i64,
                    mint.n_mints as i64,
                    mint.n_melts as i64,
                    dt_to_str(&mint.updated_at),
                ],
            )?;
            Ok(())
        })
        .await
    }

    // ----- Atomic mint counter updates ------------------------------------------
    //
    // These use targeted UPDATEs instead of read-modify-write through
    // `upsert_mint`, so concurrent writers (swap loop, donations) can never
    // clobber each other's counter bumps.

    /// Record a failure: bump the error counter, mark the mint as Error, and
    /// bench it until `retry_at`.
    pub async fn record_mint_failure(
        &self,
        mint_id: u64,
        retry_at: DateTime<Utc>,
    ) -> Result<(), AuditorError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE mints
                 SET n_errors = n_errors + 1, state = 'ERROR', next_update = ?2, updated_at = ?3
                 WHERE id = ?1",
                params![mint_id as i64, dt_to_str(&retry_at), dt_to_str(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    /// Record a successful outgoing swap (melt) and mark the mint healthy.
    pub async fn record_mint_melt_ok(&self, mint_id: u64) -> Result<(), AuditorError> {
        self.run(move |conn| {
            let now = dt_to_str(&Utc::now());
            conn.execute(
                "UPDATE mints
                 SET n_melts = n_melts + 1, state = 'OK', next_update = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![mint_id as i64, now],
            )?;
            Ok(())
        })
        .await
    }

    /// Record a successful incoming swap (mint) and mark the mint healthy.
    pub async fn record_mint_mint_ok(&self, mint_id: u64) -> Result<(), AuditorError> {
        self.run(move |conn| {
            let now = dt_to_str(&Utc::now());
            conn.execute(
                "UPDATE mints
                 SET n_mints = n_mints + 1, state = 'OK', next_update = ?2, updated_at = ?2
                 WHERE id = ?1",
                params![mint_id as i64, now],
            )?;
            Ok(())
        })
        .await
    }

    /// Record a no-fee health probe result for uptime charts and diagnostics.
    // Mirrors a health-check row; keeping named arguments avoids a second storage DTO.
    #[allow(clippy::too_many_arguments)]
    pub async fn insert_mint_check(
        &self,
        mint_id: u64,
        mint_url: &str,
        kind: &str,
        state: MintState,
        error: Option<String>,
        category: Option<String>,
        latency_ms: Option<f64>,
    ) -> Result<u64, AuditorError> {
        let mint_url = mint_url.to_string();
        let kind = kind.to_string();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO mint_checks (mint_id, mint_url, kind, state, error, category,
                                          latency_ms, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    mint_id as i64,
                    mint_url,
                    kind,
                    state_to_str(&state),
                    error,
                    category,
                    latency_ms,
                    dt_to_str(&Utc::now()),
                ],
            )?;
            Ok(conn.last_insert_rowid() as u64)
        })
        .await
    }

    /// Whether the most recent `required` dedicated health samples are all
    /// errors. WARN samples (including rate limiting) and successful samples
    /// reset the failure streak.
    pub async fn has_consecutive_health_errors(
        &self,
        mint_id: u64,
        required: u64,
    ) -> Result<bool, AuditorError> {
        let required = required.max(1);
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT state FROM mint_checks
                 WHERE mint_id = ?1 AND kind = 'HEALTH' AND auditor_pubkey = ''
                 ORDER BY id DESC LIMIT ?2",
            )?;
            let states = stmt
                .query_map(params![mint_id as i64, required as i64], |row| {
                    row.get::<_, String>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;

            Ok(states.len() == required as usize
                && states
                    .iter()
                    .all(|state| str_to_state(state) == MintState::Error))
        })
        .await
    }

    /// Mark a mint failed from a free health probe. The error counter only
    /// increments when the mint transitions into Error, so a disabled mint
    /// checked every hour does not accrue one user-visible error per probe.
    pub async fn record_mint_health_failure(
        &self,
        mint_id: u64,
        retry_at: DateTime<Utc>,
    ) -> Result<(), AuditorError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE mints
                 SET n_errors = n_errors + CASE WHEN state <> 'ERROR' THEN 1 ELSE 0 END,
                     state = 'ERROR',
                     next_update = CASE
                         WHEN next_update IS NOT NULL AND next_update > ?2 THEN next_update
                         ELSE ?2
                     END,
                     updated_at = ?3
                 WHERE id = ?1",
                params![mint_id as i64, dt_to_str(&retry_at), dt_to_str(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    /// Record reachability without clearing an unresolved paid-swap failure.
    pub async fn record_mint_health_ok(&self, mint_id: u64) -> Result<(), AuditorError> {
        self.record_mint_health_recovery(mint_id, MintState::Ok)
            .await
    }

    /// Capability warnings likewise cannot clear a paid-swap failure.
    pub async fn record_mint_health_warn(&self, mint_id: u64) -> Result<(), AuditorError> {
        self.record_mint_health_recovery(mint_id, MintState::Warn)
            .await
    }

    /// Dedicated health samples remain independent in `mint_checks`. The
    /// overall mint state also retains the latest attributed payment failure
    /// until a settled swap supersedes it. A counterparty's failure and local
    /// wallet faults provide no evidence that this mint's payments are broken.
    /// Read and update in one statement so a concurrent settlement cannot be
    /// overwritten by a probe using stale payment history.
    /// Text exclusions cover historical wallet faults recorded before their
    /// internal category was preserved, matching the swap engine's exclusions.
    async fn record_mint_health_recovery(
        &self,
        mint_id: u64,
        state: MintState,
    ) -> Result<(), AuditorError> {
        self.run(move |conn| {
            let now = dt_to_str(&Utc::now());
            conn.execute(
                "WITH latest_payment AS (
                     SELECT state FROM swaps
                     WHERE (state = 'OK' AND (from_id = ?1 OR to_id = ?1))
                        OR (state = 'ERROR' AND failed_mint_id = ?1
                            AND COALESCE(category, '') <> 'internal'
                            AND LOWER(COALESCE(error, '')) NOT LIKE '%insufficient funds%'
                            AND LOWER(COALESCE(error, '')) NOT LIKE '%already spent%'
                            AND LOWER(COALESCE(error, '')) NOT LIKE '%token not verified%'
                            AND LOWER(COALESCE(error, '')) NOT LIKE '%quote already in use%')
                     ORDER BY id DESC LIMIT 1
                 ), payment_status AS (
                     SELECT COALESCE((SELECT state FROM latest_payment), '') = 'ERROR' AS failed
                 )
                 UPDATE mints
                 SET state = CASE WHEN (SELECT failed FROM payment_status) THEN 'ERROR' ELSE ?3 END,
                     next_update = CASE WHEN (SELECT failed FROM payment_status) THEN next_update ELSE ?2 END,
                     updated_at = ?2
                 WHERE id = ?1
                   AND (state <> 'ERROR' OR next_update IS NULL OR next_update <= ?2)",
                params![mint_id as i64, now, state_to_str(&state)],
            )?;
            Ok(())
        })
        .await
    }

    /// Atomically add a donation to an existing mint. Returns the mint id, or
    /// `None` if no mint with this URL is tracked yet (caller inserts then).
    pub async fn add_donation(&self, url: &str, amount: u64) -> Result<Option<u64>, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            // A fresh donation revives a soft-deleted mint: donating from a mint
            // is the canonical way to put it (back) under audit.
            let changed = conn.execute(
                "UPDATE mints
                 SET sum_donations = sum_donations + ?2, updated_at = ?3, deleted_at = NULL
                 WHERE url = ?1",
                params![url, amount as i64, dt_to_str(&Utc::now())],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            let id: i64 =
                conn.query_row("SELECT id FROM mints WHERE url = ?1", params![url], |row| {
                    row.get(0)
                })?;
            Ok(Some(id as u64))
        })
        .await
    }

    // ----- Donations -------------------------------------------------------------

    /// Register/revive the mint, increment its target and record the event in
    /// one transaction. Concurrent first donations must add, never overwrite.
    pub async fn record_donation(
        &self,
        mint_url: &str,
        amount: u64,
        via: &str,
    ) -> Result<Mint, AuditorError> {
        let amount = i64::try_from(amount)
            .ok()
            .filter(|amount| *amount > 0)
            .ok_or_else(|| AuditorError::InvalidInput("donation amount is out of range".into()))?;
        let mint_url = mint_url.to_string();
        let via = via.to_string();
        self.run(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let now = dt_to_str(&Utc::now());
            tx.execute(
                "INSERT INTO mints (url, sum_donations, updated_at, next_update)
                 VALUES (?1, ?2, ?3, ?3)
                 ON CONFLICT(url) DO UPDATE SET
                    sum_donations = mints.sum_donations + excluded.sum_donations,
                    updated_at = excluded.updated_at,
                    deleted_at = NULL
                 WHERE mints.sum_donations <= ?4",
                params![mint_url, amount, now, i64::MAX - amount],
            )
            .and_then(|changed| {
                if changed == 0 {
                    Err(rusqlite::Error::InvalidQuery)
                } else {
                    Ok(changed)
                }
            })?;
            let mint = tx.query_row(
                &format!("SELECT {MINT_COLUMNS} FROM mints WHERE url = ?1"),
                params![mint_url],
                row_to_mint,
            )?;
            tx.execute(
                "INSERT INTO donations (mint_id, mint_url, amount, via, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![mint.id as i64, mint_url, amount, via, now],
            )?;
            tx.commit()?;
            Ok(mint)
        })
        .await
    }

    /// Update descriptive metadata without overwriting audit counters or state.
    pub async fn update_mint_metadata(
        &self,
        id: u64,
        name: String,
        info: String,
    ) -> Result<(), AuditorError> {
        self.run(move |conn| {
            conn.execute(
                "UPDATE mints SET name = ?2, info = ?3 WHERE id = ?1",
                params![id as i64, name, info],
            )?;
            Ok(())
        })
        .await
    }

    /// Record a donation event. Returns the new row id.
    pub async fn insert_donation(
        &self,
        mint_id: u64,
        mint_url: &str,
        amount: u64,
        via: &str,
    ) -> Result<u64, AuditorError> {
        let mint_url = mint_url.to_string();
        let via = via.to_string();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO donations (mint_id, mint_url, amount, via, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    mint_id as i64,
                    mint_url,
                    amount as i64,
                    via,
                    dt_to_str(&Utc::now())
                ],
            )?;
            Ok(conn.last_insert_rowid() as u64)
        })
        .await
    }

    /// Donations newer than `after_id` (ascending), capped at `limit`.
    /// Used by the donate page to acknowledge incoming payments live.
    pub async fn get_donations_after(
        &self,
        after_id: u64,
        limit: u64,
    ) -> Result<Vec<Donation>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, mint_id, mint_url, amount, via, created_at
                 FROM donations WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
            )?;
            let donations = stmt
                .query_map(
                    params![after_id.min(i64::MAX as u64) as i64, limit.min(500) as i64],
                    row_to_donation,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(donations)
        })
        .await
    }

    /// Most recent donations for one mint (descending).
    pub async fn get_donations_by_mint(
        &self,
        mint_id: u64,
        limit: u64,
    ) -> Result<Vec<Donation>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT id, mint_id, mint_url, amount, via, created_at
                 FROM donations WHERE mint_id = ?1 ORDER BY id DESC LIMIT ?2",
            )?;
            let donations = stmt
                .query_map(
                    params![mint_id as i64, limit.min(500) as i64],
                    row_to_donation,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(donations)
        })
        .await
    }

    /// Highest donation id, or 0 when none exist. Baseline for ack polling.
    pub async fn get_latest_donation_id(&self) -> Result<u64, AuditorError> {
        self.run(|conn| {
            let id: i64 =
                conn.query_row("SELECT COALESCE(MAX(id), 0) FROM donations", [], |row| {
                    row.get(0)
                })?;
            Ok(id as u64)
        })
        .await
    }

    // ----- Per-mint reliability metrics -------------------------------------------

    /// Aggregate audit metrics for one mint (swaps in either direction).
    /// Audit metrics for one mint. `auditor` scopes the rows: `None` pools every
    /// auditor, `Some("")` is this instance only, `Some(hex)` one peer.
    pub async fn get_mint_metrics(
        &self,
        mint_id: u64,
        auditor: Option<String>,
    ) -> Result<crate::types::MintMetrics, AuditorError> {
        self.run(move |conn| {
            let id = mint_id as i64;

            let agg =
                |conn: &Connection, cutoff: Option<String>| -> rusqlite::Result<(i64, i64, f64)> {
                    conn.query_row(
                        "SELECT COUNT(*),
                                COALESCE(SUM(CASE WHEN state = 'OK' THEN 1 ELSE 0 END), 0),
                                COALESCE(AVG(CASE WHEN state = 'OK' THEN time_taken END), 0.0)
                         FROM swaps
                         WHERE (from_id = ?1 OR to_id = ?1)
                           AND (?2 IS NULL OR auditor_pubkey = ?2)
                           AND (?3 IS NULL OR created_at >= ?3)",
                        params![id, auditor, cutoff],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                    )
                };

            let (attempts, successes, avg_latency_ms) = agg(conn, None)?;
            let cutoff = dt_to_str(&(Utc::now() - chrono::Duration::hours(24)));
            let (attempts_24h, successes_24h, avg_latency_ms_24h) = agg(conn, Some(cutoff))?;

            let last_success_at: Option<String> = conn.query_row(
                "SELECT MAX(created_at) FROM swaps
                 WHERE (from_id = ?1 OR to_id = ?1) AND state = 'OK'
                   AND (?2 IS NULL OR auditor_pubkey = ?2)",
                params![id, auditor],
                |row| row.get(0),
            )?;

            // Attribute the last error to the mint that actually caused it.
            // New rows carry `failed_mint_id`; legacy rows (NULL) fall back to
            // "this mint was involved" so historical data still surfaces.
            let last_error: Option<(String, Option<String>, String)> = conn
                .query_row(
                    "SELECT error, category, created_at FROM swaps
                     WHERE state <> 'OK' AND error IS NOT NULL
                       AND (failed_mint_id = ?1
                            OR (failed_mint_id IS NULL AND (from_id = ?1 OR to_id = ?1)))
                       AND (?2 IS NULL OR auditor_pubkey = ?2)
                     ORDER BY created_at DESC, id DESC LIMIT 1",
                    params![id, auditor],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;

            let (last_error, last_error_category, last_error_at) = match last_error {
                Some((err, cat, at)) => {
                    let cat = resolve_category(cat.as_deref(), Some(&err)).map(str::to_string);
                    (Some(err), cat, Some(str_to_dt(&at)))
                }
                None => (None, None, None),
            };

            Ok(crate::types::MintMetrics {
                attempts: attempts as u64,
                successes: successes as u64,
                attempts_24h: attempts_24h as u64,
                successes_24h: successes_24h as u64,
                avg_latency_ms,
                avg_latency_ms_24h,
                last_success_at: last_success_at.map(|s| str_to_dt(&s)),
                last_error,
                last_error_at,
                last_error_category,
            })
        })
        .await
    }

    /// Observed uptime for one mint, inferred from hourly health probes when
    /// available and falling back to recent swap attempts.
    pub async fn get_mint_uptime(&self, mint_id: u64) -> Result<MintUptime, AuditorError> {
        self.run(move |conn| {
            const WINDOW_HOURS: u64 = 24;
            const BUCKET_COUNT: usize = 24;

            #[derive(Clone)]
            struct UptimeRow {
                created_at: DateTime<Utc>,
                state: MintState,
            }

            let id = mint_id as i64;
            let now = Utc::now();
            let window_start = now - chrono::Duration::hours(WINDOW_HOURS as i64);
            let window_start_str = dt_to_str(&window_start);
            let window_secs = WINDOW_HOURS as i64 * 60 * 60;
            let bucket_secs = (window_secs / BUCKET_COUNT as i64).max(1);

            let check_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM mint_checks WHERE mint_id = ?1",
                params![id],
                |row| row.get(0),
            )?;

            let (first_seen_at, last_checked_at, current_up_since, last_error, rows) =
                if check_count > 0 {
                    let first_seen_at: Option<String> = conn.query_row(
                        "SELECT MIN(created_at) FROM mint_checks WHERE mint_id = ?1",
                        params![id],
                        |row| row.get(0),
                    )?;

                    let last_check: Option<(i64, String, String)> = conn
                        .query_row(
                            "SELECT id, created_at, state FROM mint_checks
                             WHERE mint_id = ?1
                             ORDER BY created_at DESC, id DESC LIMIT 1",
                            params![id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?;

                    let latest_error_id: Option<i64> = conn.query_row(
                        "SELECT MAX(id) FROM mint_checks
                         WHERE mint_id = ?1 AND state <> 'OK'",
                        params![id],
                        |row| row.get(0),
                    )?;

                    let last_state = last_check.as_ref().map(|(_, _, state)| str_to_state(state));
                    let current_up_since = if matches!(last_state, Some(MintState::Ok)) {
                        let since: Option<String> = conn.query_row(
                            "SELECT MIN(created_at) FROM mint_checks
                             WHERE mint_id = ?1 AND state = 'OK' AND id > ?2",
                            params![id, latest_error_id.unwrap_or(0)],
                            |row| row.get(0),
                        )?;
                        since.map(|ts| str_to_dt(&ts))
                    } else {
                        None
                    };

                    let last_error: Option<(String, Option<String>, String)> = conn
                        .query_row(
                            "SELECT error, category, created_at FROM mint_checks
                             WHERE mint_id = ?1 AND state <> 'OK' AND error IS NOT NULL
                             ORDER BY created_at DESC, id DESC LIMIT 1",
                            params![id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?;

                    let mut stmt = conn.prepare(
                        "SELECT id, created_at, state FROM mint_checks
                         WHERE mint_id = ?1 AND created_at >= ?2
                         ORDER BY created_at ASC, id ASC",
                    )?;
                    let rows = stmt
                        .query_map(params![id, window_start_str], |row| {
                            let created_at: String = row.get(1)?;
                            let state: String = row.get(2)?;
                            Ok(UptimeRow {
                                created_at: str_to_dt(&created_at),
                                state: str_to_state(&state),
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?;

                    (
                        first_seen_at.map(|ts| str_to_dt(&ts)),
                        last_check.map(|(_, ts, _)| str_to_dt(&ts)),
                        current_up_since,
                        last_error,
                        rows,
                    )
                } else {
                    let first_seen_at: Option<String> = conn.query_row(
                        "SELECT MIN(created_at) FROM swaps WHERE from_id = ?1 OR to_id = ?1",
                        params![id],
                        |row| row.get(0),
                    )?;

                    let last_check: Option<(i64, String, String)> = conn
                        .query_row(
                            "SELECT id, created_at, state FROM swaps
                             WHERE from_id = ?1 OR to_id = ?1
                             ORDER BY created_at DESC, id DESC LIMIT 1",
                            params![id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?;

                    let latest_error_id: Option<i64> = conn.query_row(
                        "SELECT MAX(id) FROM swaps
                         WHERE (from_id = ?1 OR to_id = ?1) AND state <> 'OK'",
                        params![id],
                        |row| row.get(0),
                    )?;

                    let last_state = last_check.as_ref().map(|(_, _, state)| str_to_state(state));
                    let current_up_since = if matches!(last_state, Some(MintState::Ok)) {
                        let since: Option<String> = conn.query_row(
                            "SELECT MIN(created_at) FROM swaps
                             WHERE (from_id = ?1 OR to_id = ?1) AND state = 'OK' AND id > ?2",
                            params![id, latest_error_id.unwrap_or(0)],
                            |row| row.get(0),
                        )?;
                        since.map(|ts| str_to_dt(&ts))
                    } else {
                        None
                    };

                    let last_error: Option<(String, Option<String>, String)> = conn
                        .query_row(
                            "SELECT error, category, created_at FROM swaps
                             WHERE state <> 'OK' AND error IS NOT NULL
                               AND (failed_mint_id = ?1
                                    OR (failed_mint_id IS NULL AND (from_id = ?1 OR to_id = ?1)))
                             ORDER BY created_at DESC, id DESC LIMIT 1",
                            params![id],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                        )
                        .optional()?;

                    let mut stmt = conn.prepare(
                        "SELECT id, created_at, state FROM swaps
                         WHERE (from_id = ?1 OR to_id = ?1) AND created_at >= ?2
                         ORDER BY created_at ASC, id ASC",
                    )?;
                    let rows = stmt
                        .query_map(params![id, window_start_str], |row| {
                            let created_at: String = row.get(1)?;
                            let state: String = row.get(2)?;
                            Ok(UptimeRow {
                                created_at: str_to_dt(&created_at),
                                state: str_to_state(&state),
                            })
                        })?
                        .collect::<Result<Vec<_>, _>>()?;

                    (
                        first_seen_at.map(|ts| str_to_dt(&ts)),
                        last_check.map(|(_, ts, _)| str_to_dt(&ts)),
                        current_up_since,
                        last_error,
                        rows,
                    )
                };

            let checks = rows.len() as u64;
            let failures = rows.iter().filter(|row| row.state != MintState::Ok).count() as u64;
            let successes = rows.iter().filter(|row| row.state == MintState::Ok).count() as u64;
            let uptime_percent = (checks > 0).then(|| successes as f64 / checks as f64 * 100.0);

            let mut bars = Vec::with_capacity(BUCKET_COUNT);
            for index in 0..BUCKET_COUNT {
                let start_at = window_start + chrono::Duration::seconds(bucket_secs * index as i64);
                let end_at = if index + 1 == BUCKET_COUNT {
                    now
                } else {
                    window_start + chrono::Duration::seconds(bucket_secs * (index as i64 + 1))
                };

                let mut bucket_checks = 0u64;
                let mut bucket_failures = 0u64;
                let mut has_error = false;
                let mut has_warn = false;
                for row in &rows {
                    let in_bucket = if index + 1 == BUCKET_COUNT {
                        row.created_at >= start_at && row.created_at <= end_at
                    } else {
                        row.created_at >= start_at && row.created_at < end_at
                    };
                    if in_bucket {
                        bucket_checks += 1;
                        if row.state != MintState::Ok {
                            bucket_failures += 1;
                        }
                        has_error |= row.state == MintState::Error;
                        has_warn |= row.state == MintState::Warn;
                    }
                }

                let state = if bucket_checks == 0 {
                    MintState::Unknown
                } else if has_error {
                    MintState::Error
                } else if has_warn {
                    MintState::Warn
                } else {
                    MintState::Ok
                };

                bars.push(MintUptimeBar {
                    start_at,
                    end_at,
                    state,
                    checks: bucket_checks,
                    failures: bucket_failures,
                });
            }

            let (last_error, last_error_category, last_error_at) = match last_error {
                Some((err, cat, at)) => {
                    let cat = resolve_category(cat.as_deref(), Some(&err)).map(str::to_string);
                    (Some(err), cat, Some(str_to_dt(&at)))
                }
                None => (None, None, None),
            };

            Ok(MintUptime {
                window_hours: WINDOW_HOURS,
                bucket_minutes: (bucket_secs / 60).max(1) as u64,
                first_seen_at,
                last_checked_at,
                current_up_since,
                checks,
                successes,
                failures,
                uptime_percent,
                last_error,
                last_error_at,
                last_error_category,
                bars,
            })
        })
        .await
    }

    // ----- Swaps ---------------------------------------------------------------

    pub async fn get_swaps(&self, skip: u64, limit: u64) -> Result<Vec<SwapEvent>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SWAP_COLUMNS} FROM swaps ORDER BY created_at DESC, id DESC LIMIT ?1 OFFSET ?2"
            ))?;
            let swaps = stmt
                .query_map(params![limit.min(500) as i64, skip.min(i64::MAX as u64) as i64], row_to_swap)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(swaps)
        })
        .await
    }

    pub async fn get_swaps_by_mint(
        &self,
        mint_id: u64,
        received: Option<bool>,
        skip: u64,
        limit: u64,
    ) -> Result<Vec<SwapEvent>, AuditorError> {
        self.run(move |conn| {
            let condition = match received {
                Some(true) => "to_id = ?1",
                Some(false) => "from_id = ?1",
                None => "from_id = ?1 OR to_id = ?1",
            };
            let mut stmt = conn.prepare(&format!(
                "SELECT {SWAP_COLUMNS} FROM swaps WHERE {condition}
                 ORDER BY created_at DESC, id DESC LIMIT ?2 OFFSET ?3"
            ))?;
            let swaps = stmt
                .query_map(
                    params![
                        mint_id as i64,
                        limit.min(500) as i64,
                        skip.min(i64::MAX as u64) as i64
                    ],
                    row_to_swap,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(swaps)
        })
        .await
    }

    pub async fn insert_swap(&self, swap: &SwapEvent) -> Result<u64, AuditorError> {
        let swap = swap.clone();
        self.run(move |conn| insert_swap_row(conn, &swap)).await
    }

    /// Commit a settled swap and both mint counters as one accounting event.
    /// Wallet settlement has already happened; callers must surface any failure.
    pub async fn record_swap_success(&self, swap: &SwapEvent) -> Result<u64, AuditorError> {
        if swap.state != MintState::Ok || swap.from_id == swap.to_id {
            return Err(AuditorError::InvalidInput(
                "expected a successful swap between distinct mints".into(),
            ));
        }
        let swap = swap.clone();
        self.run(move |conn| {
            let tx = conn.unchecked_transaction()?;
            let id = insert_swap_row(&tx, &swap)?;
            let now = dt_to_str(&swap.created_at);
            for (counter, mint_id) in [("n_melts", swap.from_id), ("n_mints", swap.to_id)] {
                let changed = tx.execute(
                    &format!(
                        "UPDATE mints SET {counter} = {counter} + 1, state = 'OK',
                              next_update = ?2, updated_at = ?2 WHERE id = ?1"
                    ),
                    params![mint_id as i64, now],
                )?;
                if changed != 1 {
                    return Err(AuditorError::NotFound(format!(
                        "mint {mint_id} missing while recording settled swap"
                    )));
                }
            }
            tx.commit()?;
            Ok(id)
        })
        .await
    }

    // ----- Nostr outbox ----------------------------------------------------------
    //
    // A row with `published_at IS NULL` and an empty `auditor_pubkey` is local
    // and not yet on a relay. Peer rows arrive with their pubkey and event id set.

    pub async fn unpublished_swaps(&self, limit: u64) -> Result<Vec<SwapEvent>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {SWAP_COLUMNS} FROM swaps
                 WHERE published_at IS NULL AND auditor_pubkey = ''
                 ORDER BY id LIMIT ?1"
            ))?;
            let rows = stmt
                .query_map(params![limit.min(1000) as i64], row_to_swap)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn unpublished_checks(&self, limit: u64) -> Result<Vec<MintCheck>, AuditorError> {
        self.run(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT {MINT_CHECK_COLUMNS} FROM mint_checks
                 WHERE published_at IS NULL AND auditor_pubkey = ''
                 ORDER BY id LIMIT ?1"
            ))?;
            let rows = stmt
                .query_map(params![limit.min(1000) as i64], row_to_check)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn mark_swap_published(&self, id: u64, event_id: &str) -> Result<(), AuditorError> {
        let event_id = event_id.to_string();
        self.run(move |conn| {
            conn.execute(
                "UPDATE swaps SET event_id = ?2, published_at = ?3 WHERE id = ?1",
                params![id as i64, event_id, dt_to_str(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn mark_check_published(&self, id: u64, event_id: &str) -> Result<(), AuditorError> {
        let event_id = event_id.to_string();
        self.run(move |conn| {
            conn.execute(
                "UPDATE mint_checks SET event_id = ?2, published_at = ?3 WHERE id = ?1",
                params![id as i64, event_id, dt_to_str(&Utc::now())],
            )?;
            Ok(())
        })
        .await
    }

    // ----- Peer auditors (Nostr ingest) ---------------------------------------

    /// The mint row for a URL, created unfunded if unknown. Soft-deleted mints
    /// are returned too, so peer history attaches to them without reviving them.
    pub async fn ensure_mint_for_url(&self, url: &str) -> Result<Mint, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            let existing = conn
                .query_row(
                    &format!("SELECT {MINT_COLUMNS} FROM mints WHERE url = ?1"),
                    params![url],
                    row_to_mint,
                )
                .optional()?;
            if let Some(mint) = existing {
                return Ok(mint);
            }
            let now = dt_to_str(&Utc::now());
            conn.execute(
                "INSERT INTO mints (url, name, state, updated_at) VALUES (?1, '', 'UNKNOWN', ?2)",
                params![url, now],
            )?;
            Ok(conn.query_row(
                &format!("SELECT {MINT_COLUMNS} FROM mints WHERE url = ?1"),
                params![url],
                row_to_mint,
            )?)
        })
        .await
    }

    /// Store a check observed by a peer auditor (pubkey and event id set).
    pub async fn insert_peer_check(&self, check: &MintCheck) -> Result<u64, AuditorError> {
        let check = check.clone();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO mint_checks (mint_id, mint_url, kind, state, error, category, latency_ms,
                                          created_at, auditor_pubkey, event_id, published_at, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?8, ?11)",
                params![
                    check.mint_id as i64,
                    check.mint_url,
                    check.kind,
                    state_to_str(&check.state),
                    check.error,
                    check.category,
                    check.latency_ms,
                    dt_to_str(&check.created_at),
                    check.auditor_pubkey,
                    check.event_id,
                    check.detail,
                ],
            )?;
            Ok(conn.last_insert_rowid() as u64)
        })
        .await
    }

    pub async fn upsert_auditor(&self, auditor: &Auditor) -> Result<(), AuditorError> {
        let a = auditor.clone();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO auditors (pubkey, name, urls, donate, software, frequency, contact,
                                       announced_at, last_seen)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT(pubkey) DO UPDATE SET
                    name = excluded.name, urls = excluded.urls, donate = excluded.donate,
                    software = excluded.software, frequency = excluded.frequency,
                    contact = excluded.contact,
                    announced_at = MAX(COALESCE(auditors.announced_at, ''), excluded.announced_at),
                    last_seen = MAX(auditors.last_seen, excluded.last_seen)",
                params![
                    a.pubkey,
                    a.name,
                    serde_json::to_string(&a.urls).unwrap_or_else(|_| "[]".into()),
                    a.donate,
                    a.software,
                    a.frequency_secs as i64,
                    a.contact,
                    a.announced_at.as_ref().map(dt_to_str),
                    dt_to_str(&a.last_seen),
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Record that a peer published something, creating a bare row if it has
    /// not announced itself yet.
    pub async fn touch_auditor(
        &self,
        pubkey: &str,
        seen_at: DateTime<Utc>,
    ) -> Result<(), AuditorError> {
        let pubkey = pubkey.to_string();
        self.run(move |conn| {
            let seen = dt_to_str(&seen_at);
            conn.execute(
                "INSERT INTO auditors (pubkey, last_seen) VALUES (?1, ?2)
                 ON CONFLICT(pubkey) DO UPDATE SET last_seen = MAX(auditors.last_seen, excluded.last_seen)",
                params![pubkey, seen],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn get_auditors(&self) -> Result<Vec<Auditor>, AuditorError> {
        self.run(|conn| {
            let mut stmt = conn.prepare(
                "SELECT pubkey, name, urls, donate, software, frequency, contact, announced_at, last_seen
                 FROM auditors ORDER BY last_seen DESC",
            )?;
            let rows = stmt
                .query_map([], |row| {
                    let urls: String = row.get(2)?;
                    Ok(Auditor {
                        pubkey: row.get(0)?,
                        name: row.get(1)?,
                        urls: serde_json::from_str(&urls).unwrap_or_default(),
                        donate: row.get(3)?,
                        software: row.get(4)?,
                        frequency_secs: row.get::<_, i64>(5)?.max(0) as u64,
                        contact: row.get(6)?,
                        announced_at: row.get::<_, Option<String>>(7)?.map(|s| str_to_dt(&s)),
                        last_seen: str_to_dt(&row.get::<_, String>(8)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    // ----- NIP-87 directory (announcements and reviews) -----------------------

    pub async fn upsert_mint_announcement(&self, a: &MintAnnouncement) -> Result<(), AuditorError> {
        let a = a.clone();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO mint_announcements (pubkey, d, event_id, mint_url, nuts, network, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(pubkey, d) DO UPDATE SET
                    event_id = excluded.event_id, mint_url = excluded.mint_url,
                    nuts = excluded.nuts, network = excluded.network,
                    content = excluded.content, created_at = excluded.created_at
                 WHERE excluded.created_at > mint_announcements.created_at",
                params![
                    a.pubkey,
                    a.d,
                    a.event_id,
                    a.mint_url,
                    a.nuts,
                    a.network,
                    a.content,
                    dt_to_str(&a.created_at)
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn upsert_review(&self, r: &Review) -> Result<(), AuditorError> {
        let r = r.clone();
        self.run(move |conn| {
            conn.execute(
                "INSERT INTO reviews (pubkey, d, event_id, mint_url, rating, content, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(pubkey, d) DO UPDATE SET
                    event_id = excluded.event_id, mint_url = excluded.mint_url,
                    rating = excluded.rating, content = excluded.content,
                    created_at = excluded.created_at
                 WHERE excluded.created_at > reviews.created_at",
                params![
                    r.pubkey,
                    r.d,
                    r.event_id,
                    r.mint_url,
                    r.rating.map(i64::from),
                    r.content,
                    dt_to_str(&r.created_at)
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// Newest NIP-87 event time: the ingest watermark for the public kinds.
    pub async fn latest_nip87_event_at(&self) -> Result<Option<DateTime<Utc>>, AuditorError> {
        self.run(|conn| {
            let latest: Option<String> = conn.query_row(
                "SELECT MAX(t) FROM (SELECT MAX(created_at) AS t FROM mint_announcements
                                     UNION ALL SELECT MAX(created_at) FROM reviews)",
                [],
                |row| row.get(0),
            )?;
            Ok(latest.map(|s| str_to_dt(&s)))
        })
        .await
    }

    /// The announcement that best identifies a mint: one signed by the mint's
    /// own key first, then the newest matching its URL or pubkey.
    pub async fn announcement_for_mint(
        &self,
        url: &str,
        pubkey: Option<String>,
    ) -> Result<Option<MintAnnouncement>, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT pubkey, d, event_id, mint_url, nuts, network, content, created_at
                     FROM mint_announcements
                     WHERE mint_url = ?1 OR (?2 IS NOT NULL AND d = ?2)
                     ORDER BY (pubkey = d) DESC, created_at DESC LIMIT 1",
                    params![url, pubkey],
                    row_to_announcement,
                )
                .optional()?)
        })
        .await
    }

    pub async fn reviews_for_mint(
        &self,
        url: &str,
        pubkey: Option<String>,
        limit: u64,
    ) -> Result<Vec<Review>, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT pubkey, d, event_id, mint_url, rating, content, created_at FROM reviews
                 WHERE mint_url = ?1 OR (?2 IS NOT NULL AND d = ?2)
                 ORDER BY created_at DESC LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(params![url, pubkey, limit.min(500) as i64], |row| {
                    Ok(Review {
                        pubkey: row.get(0)?,
                        d: row.get(1)?,
                        event_id: row.get(2)?,
                        mint_url: row.get(3)?,
                        rating: row.get::<_, Option<i64>>(4)?.map(|r| r.clamp(0, 5) as u8),
                        content: row.get(5)?,
                        created_at: str_to_dt(&row.get::<_, String>(6)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Social and audit summary for a directory row. The per-auditor verdict
    /// is each auditor's latest attempt in 24 h that is attributable to this
    /// mint (successes, or failures blamed on it or on nobody).
    pub async fn directory_stats(
        &self,
        mint_id: u64,
        url: &str,
        pubkey: Option<String>,
    ) -> Result<DirectoryStats, AuditorError> {
        let url = url.to_string();
        self.run(move |conn| {
            let id = mint_id as i64;
            let (review_count, review_avg): (i64, Option<f64>) = conn.query_row(
                "SELECT COUNT(*), AVG(rating) FROM reviews
                 WHERE mint_url = ?1 OR (?2 IS NOT NULL AND d = ?2)",
                params![url, pubkey],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let announced: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM mint_announcements
                               WHERE mint_url = ?1 OR (?2 IS NOT NULL AND d = ?2))",
                params![url, pubkey],
                |row| row.get(0),
            )?;
            let cutoff = dt_to_str(&(Utc::now() - chrono::Duration::hours(24)));
            let (auditors, auditors_ok, attempts, successes): (i64, i64, i64, i64) = conn
                .query_row(
                    "WITH involved AS (
                        SELECT auditor_pubkey, state, failed_mint_id, created_at, id FROM swaps
                        WHERE (from_id = ?1 OR to_id = ?1) AND created_at >= ?2),
                     attributable AS (
                        SELECT auditor_pubkey, state,
                               ROW_NUMBER() OVER (PARTITION BY auditor_pubkey
                                                  ORDER BY created_at DESC, id DESC) AS rn
                        FROM involved
                        WHERE state = 'OK' OR failed_mint_id IS NULL OR failed_mint_id = ?1)
                     SELECT (SELECT COUNT(*) FROM attributable WHERE rn = 1),
                            (SELECT COUNT(*) FROM attributable WHERE rn = 1 AND state = 'OK'),
                            (SELECT COUNT(*) FROM involved),
                            (SELECT COUNT(*) FROM involved WHERE state = 'OK')",
                    params![id, cutoff],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )?;
            let mut stmt = conn.prepare(
                "WITH attributable AS (
                    SELECT auditor_pubkey, state, created_at,
                           ROW_NUMBER() OVER (PARTITION BY auditor_pubkey
                                              ORDER BY created_at DESC, id DESC) AS rn
                    FROM swaps
                    WHERE (from_id = ?1 OR to_id = ?1) AND created_at >= ?2
                      AND (state = 'OK' OR failed_mint_id IS NULL OR failed_mint_id = ?1))
                 SELECT auditor_pubkey,
                        MAX(CASE WHEN rn = 1 THEN state END),
                        MAX(CASE WHEN rn = 1 THEN created_at END),
                        SUM(CASE WHEN state <> 'OK' THEN 1 ELSE 0 END)
                 FROM attributable GROUP BY auditor_pubkey ORDER BY auditor_pubkey",
            )?;
            let marks = stmt
                .query_map(params![id, cutoff], |row| {
                    let latest: String = row.get(1)?;
                    let at: String = row.get(2)?;
                    let failures: i64 = row.get(3)?;
                    let state = match (latest.as_str(), failures) {
                        ("OK", 0) => MintState::Ok,
                        ("OK", _) => MintState::Warn,
                        _ => MintState::Error,
                    };
                    Ok(AuditorMark {
                        pubkey: row.get(0)?,
                        state,
                        at: str_to_dt(&at),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(DirectoryStats {
                review_count: review_count.max(0) as u64,
                review_avg,
                announced,
                auditors_24h: auditors.max(0) as u64,
                auditors_ok_24h: auditors_ok.max(0) as u64,
                attempts_24h: attempts.max(0) as u64,
                successes_24h: successes.max(0) as u64,
                marks,
            })
        })
        .await
    }

    /// When this instance last probed and last swapped (local rows only); the
    /// health endpoint's liveness signal.
    pub async fn local_activity(
        &self,
    ) -> Result<(Option<DateTime<Utc>>, Option<DateTime<Utc>>), AuditorError> {
        self.run(|conn| {
            let probe: Option<String> = conn.query_row(
                "SELECT MAX(created_at) FROM mint_checks WHERE auditor_pubkey = ''",
                [],
                |row| row.get(0),
            )?;
            let swap: Option<String> = conn.query_row(
                "SELECT MAX(created_at) FROM swaps WHERE auditor_pubkey = ''",
                [],
                |row| row.get(0),
            )?;
            Ok((probe.map(|s| str_to_dt(&s)), swap.map(|s| str_to_dt(&s))))
        })
        .await
    }

    /// Newest peer observation or announcement time: the ingest watermark.
    pub async fn latest_peer_event_at(&self) -> Result<Option<DateTime<Utc>>, AuditorError> {
        self.run(|conn| {
            let latest: Option<String> = conn.query_row(
                "SELECT MAX(t) FROM (
                    SELECT MAX(created_at) AS t FROM swaps WHERE auditor_pubkey <> ''
                    UNION ALL SELECT MAX(created_at) FROM mint_checks WHERE auditor_pubkey <> ''
                    UNION ALL SELECT MAX(last_seen) FROM auditors)",
                [],
                |row| row.get(0),
            )?;
            Ok(latest.map(|s| str_to_dt(&s)))
        })
        .await
    }

    // ----- Stats helpers ---------------------------------------------------------

    pub async fn get_total_swaps(&self) -> Result<u64, AuditorError> {
        self.run(|conn| {
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM swaps", [], |row| row.get(0))?;
            Ok(count as u64)
        })
        .await
    }

    pub async fn get_swaps_24h(&self) -> Result<u64, AuditorError> {
        self.run(|conn| {
            let cutoff = dt_to_str(&(Utc::now() - chrono::Duration::hours(24)));
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM swaps WHERE created_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )?;
            Ok(count as u64)
        })
        .await
    }

    pub async fn get_total_amount_swapped(&self) -> Result<u64, AuditorError> {
        self.run(|conn| {
            let total: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount), 0) FROM swaps WHERE state = 'OK'",
                [],
                |row| row.get(0),
            )?;
            Ok(total as u64)
        })
        .await
    }

    pub async fn get_total_amount_swapped_24h(&self) -> Result<u64, AuditorError> {
        self.run(|conn| {
            let cutoff = dt_to_str(&(Utc::now() - chrono::Duration::hours(24)));
            let total: i64 = conn.query_row(
                "SELECT COALESCE(SUM(amount), 0) FROM swaps WHERE state = 'OK' AND created_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )?;
            Ok(total as u64)
        })
        .await
    }

    pub async fn get_average_swap_time(&self) -> Result<f64, AuditorError> {
        self.run(|conn| {
            let avg: f64 = conn.query_row(
                "SELECT COALESCE(AVG(time_taken), 0.0) FROM swaps WHERE state = 'OK'",
                [],
                |row| row.get(0),
            )?;
            Ok(avg)
        })
        .await
    }

    pub async fn get_average_swap_time_24h(&self) -> Result<f64, AuditorError> {
        self.run(|conn| {
            let cutoff = dt_to_str(&(Utc::now() - chrono::Duration::hours(24)));
            let avg: f64 = conn.query_row(
                "SELECT COALESCE(AVG(time_taken), 0.0) FROM swaps WHERE state = 'OK' AND created_at >= ?1",
                params![cutoff],
                |row| row.get(0),
            )?;
            Ok(avg)
        })
        .await
    }
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn row_to_mint(row: &rusqlite::Row<'_>) -> rusqlite::Result<Mint> {
    Ok(Mint {
        id: row.get::<_, i64>(0)? as u64,
        url: row.get(1)?,
        name: row.get(2)?,
        info: row.get(3)?,
        sum_donations: row.get::<_, i64>(4)? as u64,
        state: str_to_state(&row.get::<_, String>(5)?),
        n_errors: row.get::<_, i64>(6)? as u64,
        n_mints: row.get::<_, i64>(7)? as u64,
        n_melts: row.get::<_, i64>(8)? as u64,
        updated_at: str_to_dt(&row.get::<_, String>(9)?),
        next_update: row.get::<_, Option<String>>(10)?.map(|s| str_to_dt(&s)),
        latitude: row.get(11)?,
        longitude: row.get(12)?,
        deleted_at: row.get::<_, Option<String>>(13)?.map(|s| str_to_dt(&s)),
    })
}

fn row_to_donation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Donation> {
    Ok(Donation {
        id: row.get::<_, i64>(0)? as u64,
        mint_id: row.get::<_, i64>(1)? as u64,
        mint_url: row.get(2)?,
        amount: row.get::<_, i64>(3)? as u64,
        via: row.get(4)?,
        created_at: str_to_dt(&row.get::<_, String>(5)?),
    })
}

fn row_to_swap(row: &rusqlite::Row<'_>) -> rusqlite::Result<SwapEvent> {
    let error: Option<String> = row.get(8)?;
    let category: Option<String> = row.get(11)?;
    let category = resolve_category(category.as_deref(), error.as_deref()).map(str::to_string);
    Ok(SwapEvent {
        id: row.get::<_, i64>(0)? as u64,
        from_id: row.get::<_, i64>(1)? as u64,
        to_id: row.get::<_, i64>(2)? as u64,
        from_url: row.get(3)?,
        to_url: row.get(4)?,
        amount: row.get::<_, i64>(5)? as u64,
        fee: row.get::<_, i64>(6)? as u64,
        state: str_to_state(&row.get::<_, String>(7)?),
        error,
        failed_mint_id: row.get::<_, Option<i64>>(9)?.map(|id| id as u64),
        stage: row.get(10)?,
        category,
        time_taken: row.get(12)?,
        created_at: str_to_dt(&row.get::<_, String>(13)?),
        fee_reserve: row.get::<_, Option<i64>>(14)?.map(|v| v as u64),
        auditor_pubkey: row.get(15)?,
        event_id: row.get(16)?,
    })
}

fn row_to_announcement(row: &rusqlite::Row<'_>) -> rusqlite::Result<MintAnnouncement> {
    Ok(MintAnnouncement {
        pubkey: row.get(0)?,
        d: row.get(1)?,
        event_id: row.get(2)?,
        mint_url: row.get(3)?,
        nuts: row.get(4)?,
        network: row.get(5)?,
        content: row.get(6)?,
        created_at: str_to_dt(&row.get::<_, String>(7)?),
    })
}

fn row_to_check(row: &rusqlite::Row<'_>) -> rusqlite::Result<MintCheck> {
    Ok(MintCheck {
        id: row.get::<_, i64>(0)? as u64,
        mint_id: row.get::<_, i64>(1)? as u64,
        mint_url: row.get(2)?,
        kind: row.get(3)?,
        state: str_to_state(&row.get::<_, String>(4)?),
        error: row.get(5)?,
        category: row.get(6)?,
        latency_ms: row.get(7)?,
        created_at: str_to_dt(&row.get::<_, String>(8)?),
        auditor_pubkey: row.get(9)?,
        event_id: row.get(10)?,
        detail: row.get(11)?,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mint(url: &str) -> Mint {
        Mint {
            id: 0,
            url: url.to_string(),
            info: None,
            name: format!("mint {url}"),
            sum_donations: 1000,
            updated_at: Utc::now(),
            next_update: None,
            state: MintState::Unknown,
            n_errors: 0,
            n_mints: 0,
            n_melts: 0,
            latitude: None,
            longitude: None,
            deleted_at: None,
        }
    }

    fn sample_swap(from: &Mint, to: &Mint, amount: u64, state: MintState) -> SwapEvent {
        SwapEvent {
            id: 0,
            from_id: from.id,
            to_id: to.id,
            from_url: from.url.clone(),
            to_url: to.url.clone(),
            amount,
            fee: 1,
            created_at: Utc::now(),
            time_taken: 1200.0,
            state,
            error: None,
            failed_mint_id: None,
            stage: None,
            category: None,
            fee_reserve: None,
            auditor_pubkey: String::new(),
            event_id: None,
        }
    }

    #[tokio::test]
    async fn outbox_lists_local_rows_until_marked_published() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        db.record_donation("https://a.example", 100, "token")
            .await
            .unwrap();
        db.record_donation("https://b.example", 100, "token")
            .await
            .unwrap();
        let a = db
            .get_mint_by_url("https://a.example")
            .await
            .unwrap()
            .unwrap();
        let b = db
            .get_mint_by_url("https://b.example")
            .await
            .unwrap()
            .unwrap();

        let local = db
            .insert_swap(&sample_swap(&a, &b, 10, MintState::Ok))
            .await
            .unwrap();
        let mut peer = sample_swap(&b, &a, 10, MintState::Ok);
        peer.auditor_pubkey = "ab".repeat(32);
        peer.event_id = Some("cd".repeat(32));
        db.insert_swap(&peer).await.unwrap();
        db.insert_mint_check(
            a.id,
            &a.url,
            "HEALTH",
            MintState::Ok,
            None,
            None,
            Some(50.0),
        )
        .await
        .unwrap();

        let swaps = db.unpublished_swaps(10).await.unwrap();
        assert_eq!(swaps.iter().map(|s| s.id).collect::<Vec<_>>(), vec![local]);
        let checks = db.unpublished_checks(10).await.unwrap();
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].kind, "HEALTH");

        db.mark_swap_published(local, "0011").await.unwrap();
        db.mark_check_published(checks[0].id, "2233").await.unwrap();
        assert!(db.unpublished_swaps(10).await.unwrap().is_empty());
        assert!(db.unpublished_checks(10).await.unwrap().is_empty());
        let published = db.get_swaps(0, 10).await.unwrap();
        let row = published.iter().find(|s| s.id == local).unwrap();
        assert_eq!(
            row.auditor_pubkey, "",
            "local rows stay local after publishing"
        );
        assert_eq!(row.event_id.as_deref(), Some("0011"));
        assert_eq!(
            db.get_mint_metrics(a.id, Some(String::new()))
                .await
                .unwrap()
                .attempts,
            1
        );
        assert_eq!(db.get_mint_metrics(a.id, None).await.unwrap().attempts, 2);

        // A second peer copy of the same event id is rejected by the unique index.
        assert!(db.insert_swap(&peer).await.is_err());
    }

    #[tokio::test]
    async fn directory_marks_keep_each_auditors_own_verdict() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        for url in ["https://a.example", "https://b.example"] {
            db.record_donation(url, 100, "token").await.unwrap();
        }
        let a = db
            .get_mint_by_url("https://a.example")
            .await
            .unwrap()
            .unwrap();
        let b = db
            .get_mint_by_url("https://b.example")
            .await
            .unwrap()
            .unwrap();
        let peer_ok = "11".repeat(32);
        let peer_mixed = "22".repeat(32);
        let peer_failing = "33".repeat(32);
        let at = |mins: i64| Utc::now() - chrono::Duration::minutes(mins);
        let swap = |pubkey: &str, state: MintState, failed: Option<u64>, mins: i64| {
            let mut s = sample_swap(&a, &b, 10, state);
            s.auditor_pubkey = pubkey.to_string();
            s.failed_mint_id = failed;
            s.created_at = at(mins);
            s.event_id = (!pubkey.is_empty()).then(|| format!("{pubkey}{mins}"));
            s
        };
        for s in [
            swap("", MintState::Ok, None, 5),
            swap(&peer_ok, MintState::Ok, None, 30),
            swap(&peer_mixed, MintState::Error, Some(a.id), 60),
            swap(&peer_mixed, MintState::Ok, None, 10),
            swap(&peer_failing, MintState::Ok, None, 90),
            swap(&peer_failing, MintState::Error, None, 20),
            // Blamed on the other mint, so it says nothing about `a`.
            swap(&peer_ok, MintState::Error, Some(b.id), 1),
            // Older than 24 h.
            swap(&peer_ok, MintState::Error, Some(a.id), 25 * 60),
        ] {
            db.insert_swap(&s).await.unwrap();
        }

        let marks = db.directory_stats(a.id, &a.url, None).await.unwrap().marks;
        let summary: Vec<(&str, MintState)> =
            marks.iter().map(|m| (m.pubkey.as_str(), m.state)).collect();
        assert_eq!(
            summary,
            vec![
                ("", MintState::Ok),
                (peer_ok.as_str(), MintState::Ok),
                (peer_mixed.as_str(), MintState::Warn),
                (peer_failing.as_str(), MintState::Error),
            ]
        );
    }

    #[tokio::test]
    async fn concurrent_first_donations_preserve_every_amount_and_event() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut tasks = tokio::task::JoinSet::new();
        for amount in 1..=32 {
            let db = db.clone();
            tasks.spawn(async move {
                db.record_donation("https://new.example", amount, "token")
                    .await
            });
        }
        while let Some(result) = tasks.join_next().await {
            result.unwrap().unwrap();
        }
        let mint = db
            .get_mint_by_url("https://new.example")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mint.sum_donations, 528);
        let events = db.get_donations_after(0, 100).await.unwrap();
        assert_eq!(events.len(), 32);
        assert_eq!(
            events.iter().map(|d| d.amount).sum::<u64>(),
            mint.sum_donations
        );
        assert_eq!(db.get_all_mints().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn failed_event_insert_rolls_back_registration_and_target() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        db.record_donation("https://existing.example", 50, "token")
            .await
            .unwrap();
        db.run(|conn| {
            conn.execute_batch("CREATE TRIGGER reject_donation BEFORE INSERT ON donations BEGIN SELECT RAISE(ABORT, 'simulated failure'); END;")?;
            Ok(())
        }).await.unwrap();
        assert!(db
            .record_donation("https://existing.example", 20, "token")
            .await
            .is_err());
        assert!(db
            .record_donation("https://new.example", 20, "token")
            .await
            .is_err());
        assert_eq!(
            db.get_mint_by_url("https://existing.example")
                .await
                .unwrap()
                .unwrap()
                .sum_donations,
            50
        );
        assert!(db
            .get_mint_by_url("https://new.example")
            .await
            .unwrap()
            .is_none());
        assert_eq!(db.get_donations_after(0, 100).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn disabled_registry_is_not_empty_and_donation_revives_without_resetting_counters() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        assert!(!db.has_mint_history().await.unwrap());
        let mint = db
            .record_donation("https://mint.example", 10, "token")
            .await
            .unwrap();
        db.record_mint_melt_ok(mint.id).await.unwrap();
        db.soft_delete_mint_by_url(&mint.url).await.unwrap();
        assert!(db.get_all_mints().await.unwrap().is_empty());
        assert!(db.has_mint_history().await.unwrap());
        let revived = db.record_donation(&mint.url, 20, "nut18").await.unwrap();
        assert_eq!(revived.id, mint.id);
        assert_eq!(revived.sum_donations, 30);
        assert_eq!(revived.n_melts, 1);
        assert!(revived.deleted_at.is_none());
    }

    #[tokio::test]
    async fn donation_overflow_and_zero_cannot_corrupt_accounting() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        assert!(db
            .record_donation("https://mint.example", 0, "token")
            .await
            .is_err());
        assert!(db
            .record_donation("https://mint.example", u64::MAX, "token")
            .await
            .is_err());
        db.record_donation("https://mint.example", i64::MAX as u64, "token")
            .await
            .unwrap();
        assert!(db
            .record_donation("https://mint.example", 1, "token")
            .await
            .is_err());
        assert_eq!(
            db.get_mint_by_url("https://mint.example")
                .await
                .unwrap()
                .unwrap()
                .sum_donations,
            i64::MAX as u64
        );
        assert_eq!(db.get_donations_after(0, 100).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn registry_sorts_by_rating_with_unrated_mints_last() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mints = Vec::new();
        for name in ["low", "high", "none", "keyed"] {
            let mint = db
                .record_donation(&format!("https://{name}.example"), 1, "token")
                .await
                .unwrap();
            mints.push(mint);
        }
        let urls: Vec<String> = mints.iter().map(|m| m.url.clone()).collect();
        // "keyed" is reviewed by its NUT-06 key, not its URL.
        let mut keyed = mints[3].clone();
        keyed.info = Some(r#"{"pubkey":"02abc"}"#.into());
        db.upsert_mint(&keyed).await.unwrap();
        let review = |author: &str, d: &str, url: Option<&str>, rating: u8| Review {
            pubkey: author.into(),
            d: d.into(),
            event_id: format!("{author}{d}"),
            mint_url: url.map(str::to_string),
            rating: Some(rating),
            content: String::new(),
            created_at: Utc::now(),
        };
        for r in [
            review("a", "low", Some(&urls[0]), 2),
            review("b", "low", Some(&urls[0]), 3),
            review("a", "high", Some(&urls[1]), 5),
            review("a", "02abc", None, 4),
        ] {
            db.upsert_review(&r).await.unwrap();
        }
        let names = |mints: Vec<Mint>| -> Vec<String> {
            mints.into_iter().map(|m| m.url.replace("https://", "").replace(".example", "")).collect()
        };
        let high = db.search_mints(String::new(), None, MintSort::RatingHigh, 0).await.unwrap().0;
        assert_eq!(names(high), ["high", "keyed", "low", "none"]);
        let low = db.search_mints(String::new(), None, MintSort::RatingLow, 0).await.unwrap().0;
        assert_eq!(names(low), ["low", "keyed", "high", "none"]);
        let most = db.search_mints(String::new(), None, MintSort::MostReviews, 0).await.unwrap().0;
        assert_eq!(names(most), ["low", "high", "keyed", "none"]);
        let by_name = db.search_mints(String::new(), None, MintSort::Name, 0).await.unwrap().0;
        assert_eq!(names(by_name), ["high", "keyed", "low", "none"]);
    }

    #[tokio::test]
    async fn registry_search_pages_and_filters_the_full_active_registry() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        for i in 0..22 {
            db.record_donation(&format!("https://alpha-{i}.example"), 1, "token")
                .await
                .unwrap();
        }
        let hidden = db
            .record_donation("https://alpha-hidden.example", 1, "token")
            .await
            .unwrap();
        db.soft_delete_mint_by_url(&hidden.url).await.unwrap();
        let by_name = MintSort::Name;
        let (first, total) = db
            .search_mints(" ALPHA ".into(), None, by_name, 0)
            .await
            .unwrap();
        let (second, _) = db
            .search_mints("alpha".into(), Some(MintState::Unknown), by_name, 1)
            .await
            .unwrap();
        assert_eq!(total, 22);
        assert_eq!(first.len(), 20);
        assert_eq!(second.len(), 2);
        // Unnamed mints sort by their URL without the scheme.
        assert!(first.last().unwrap().url < second[0].url);
        assert_eq!(
            db.search_mints("%".into(), None, by_name, 0)
                .await
                .unwrap()
                .1,
            0
        );
        assert_eq!(
            db.search_mints("".into(), Some(MintState::Ok), by_name, 0)
                .await
                .unwrap()
                .1,
            0
        );
        assert!(db
            .search_mints("".into(), None, by_name, u64::MAX)
            .await
            .unwrap()
            .0
            .is_empty());
        assert!(db.get_mints(u64::MAX, u64::MAX).await.unwrap().is_empty());
        assert!(db
            .get_donations_after(u64::MAX, u64::MAX)
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn upsert_mint_inserts_then_updates() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://mint.example.com");

        let id = db.upsert_mint(&mint).await.unwrap();
        assert!(id > 0);

        mint.sum_donations = 5000;
        mint.state = MintState::Ok;
        let id2 = db.upsert_mint(&mint).await.unwrap();
        assert_eq!(id, id2);

        let stored = db.get_mint_by_id(id).await.unwrap().unwrap();
        assert_eq!(stored.sum_donations, 5000);
        assert_eq!(stored.state, MintState::Ok);

        let by_url = db
            .get_mint_by_url("https://mint.example.com")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(by_url.id, id);
    }

    #[tokio::test]
    async fn swaps_roundtrip_and_pagination() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();

        for i in 0..5 {
            db.insert_swap(&sample_swap(&a, &b, 10 + i, MintState::Ok))
                .await
                .unwrap();
        }
        db.insert_swap(&sample_swap(&b, &a, 99, MintState::Error))
            .await
            .unwrap();

        assert_eq!(db.get_total_swaps().await.unwrap(), 6);
        assert_eq!(db.get_swaps_24h().await.unwrap(), 6);
        // Only OK swaps count toward volume.
        assert_eq!(db.get_total_amount_swapped().await.unwrap(), 60);

        let page = db.get_swaps(0, 4).await.unwrap();
        assert_eq!(page.len(), 4);

        let a_sent = db
            .get_swaps_by_mint(a.id, Some(false), 0, 100)
            .await
            .unwrap();
        assert_eq!(a_sent.len(), 5);
        let a_received = db
            .get_swaps_by_mint(a.id, Some(true), 0, 100)
            .await
            .unwrap();
        assert_eq!(a_received.len(), 1);
        let a_all = db.get_swaps_by_mint(a.id, None, 0, 100).await.unwrap();
        assert_eq!(a_all.len(), 6);
    }

    #[tokio::test]
    async fn settled_swap_accounting_commits_together_or_rolls_back() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let a = db
            .record_donation("https://a.example", 100, "token")
            .await
            .unwrap();
        let b = db
            .record_donation("https://b.example", 200, "token")
            .await
            .unwrap();
        let event = sample_swap(&a, &b, 10, MintState::Ok);
        db.record_swap_success(&event).await.unwrap();
        assert_eq!(db.get_total_swaps().await.unwrap(), 1);
        let stored_a = db.get_mint_by_id(a.id).await.unwrap().unwrap();
        let stored_b = db.get_mint_by_id(b.id).await.unwrap().unwrap();
        assert_eq!(
            (stored_a.n_melts, stored_a.sum_donations, stored_a.state),
            (1, 100, MintState::Ok)
        );
        assert_eq!(
            (stored_b.n_mints, stored_b.sum_donations, stored_b.state),
            (1, 200, MintState::Ok)
        );

        // Fail the second counter write after the event and first counter have
        // already changed inside the transaction. None may remain committed.
        db.run(|conn| {
            conn.execute_batch(
                "CREATE TRIGGER fail_incoming_counter BEFORE UPDATE OF n_mints ON mints
                                BEGIN SELECT RAISE(ABORT, 'simulated storage failure'); END;",
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert!(db.record_swap_success(&event).await.is_err());
        assert_eq!(db.get_total_swaps().await.unwrap(), 1);
        assert_eq!(db.get_mint_by_id(a.id).await.unwrap().unwrap().n_melts, 1);
        assert_eq!(db.get_mint_by_id(b.id).await.unwrap().unwrap().n_mints, 1);
    }

    #[tokio::test]
    async fn average_swap_time_ignores_failures() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();

        let mut ok = sample_swap(&a, &b, 50, MintState::Ok);
        ok.time_taken = 1000.0;
        db.insert_swap(&ok).await.unwrap();

        let mut failed = sample_swap(&a, &b, 50, MintState::Error);
        failed.time_taken = 0.0;
        db.insert_swap(&failed).await.unwrap();

        assert_eq!(db.get_average_swap_time().await.unwrap(), 1000.0);
    }

    #[tokio::test]
    async fn atomic_counter_updates_do_not_clobber_donations() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://a.example.com");
        mint.id = db.upsert_mint(&mint).await.unwrap();

        // Interleave counter bumps and donation increments; both must stick.
        db.record_mint_failure(mint.id, Utc::now() + chrono::Duration::minutes(30))
            .await
            .unwrap();
        let donated = db.add_donation("https://a.example.com", 500).await.unwrap();
        assert_eq!(donated, Some(mint.id));
        db.record_mint_melt_ok(mint.id).await.unwrap();
        db.record_mint_mint_ok(mint.id).await.unwrap();

        let stored = db.get_mint_by_id(mint.id).await.unwrap().unwrap();
        assert_eq!(stored.n_errors, 1);
        assert_eq!(stored.n_melts, 1);
        assert_eq!(stored.n_mints, 1);
        assert_eq!(stored.sum_donations, 1500); // 1000 initial + 500
        assert_eq!(stored.state, MintState::Ok);

        // Unknown mint: no row updated, caller inserts instead.
        let missing = db
            .add_donation("https://nope.example.com", 10)
            .await
            .unwrap();
        assert_eq!(missing, None);
    }

    #[tokio::test]
    async fn soft_delete_hides_mint_and_donation_revives_it() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://a.example.com");
        mint.id = db.upsert_mint(&mint).await.unwrap();

        // First delete succeeds; the mint vanishes from every read path.
        assert!(db
            .soft_delete_mint_by_url("https://a.example.com")
            .await
            .unwrap());
        assert!(db.get_mint_by_id(mint.id).await.unwrap().is_none());
        assert!(db
            .get_mint_by_url("https://a.example.com")
            .await
            .unwrap()
            .is_none());
        assert!(db.get_mints(0, 100).await.unwrap().is_empty());

        // Deleting again is a no-op (already deleted).
        assert!(!db
            .soft_delete_mint_by_url("https://a.example.com")
            .await
            .unwrap());

        // A fresh donation revives the mint and keeps its prior balance.
        let revived = db.add_donation("https://a.example.com", 250).await.unwrap();
        assert_eq!(revived, Some(mint.id));
        let stored = db.get_mint_by_id(mint.id).await.unwrap().unwrap();
        assert!(stored.deleted_at.is_none());
        assert_eq!(stored.sum_donations, 1250); // 1000 initial + 250
    }

    #[tokio::test]
    async fn donations_roundtrip_and_polling() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://a.example.com");
        mint.id = db.upsert_mint(&mint).await.unwrap();

        assert_eq!(db.get_latest_donation_id().await.unwrap(), 0);

        let d1 = db
            .insert_donation(mint.id, &mint.url, 100, "token")
            .await
            .unwrap();
        let d2 = db
            .insert_donation(mint.id, &mint.url, 50, "nut18")
            .await
            .unwrap();
        assert!(d2 > d1);
        assert_eq!(db.get_latest_donation_id().await.unwrap(), d2);

        let new = db.get_donations_after(d1, 10).await.unwrap();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].amount, 50);
        assert_eq!(new[0].via, "nut18");

        let by_mint = db.get_donations_by_mint(mint.id, 10).await.unwrap();
        assert_eq!(by_mint.len(), 2);
        assert_eq!(by_mint[0].id, d2); // newest first
    }

    #[tokio::test]
    async fn mint_metrics_aggregate_both_directions() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();

        let mut ok_out = sample_swap(&a, &b, 50, MintState::Ok);
        ok_out.time_taken = 1000.0;
        db.insert_swap(&ok_out).await.unwrap();

        let mut ok_in = sample_swap(&b, &a, 50, MintState::Ok);
        ok_in.time_taken = 3000.0;
        db.insert_swap(&ok_in).await.unwrap();

        let mut failed = sample_swap(&a, &b, 50, MintState::Error);
        failed.error = Some("connection refused".to_string());
        db.insert_swap(&failed).await.unwrap();

        let metrics = db.get_mint_metrics(a.id, None).await.unwrap();
        assert_eq!(metrics.attempts, 3);
        assert_eq!(metrics.successes, 2);
        assert_eq!(metrics.attempts_24h, 3);
        assert_eq!(metrics.avg_latency_ms, 2000.0);
        assert!(metrics.last_success_at.is_some());
        assert_eq!(metrics.last_error.as_deref(), Some("connection refused"));
        assert_eq!(metrics.success_rate(), Some(2.0 / 3.0));
    }

    #[tokio::test]
    async fn mint_uptime_groups_recent_audits_into_bars() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();

        let now = Utc::now();
        let mut ok_old = sample_swap(&a, &b, 50, MintState::Ok);
        ok_old.created_at = now - chrono::Duration::hours(3);
        db.insert_swap(&ok_old).await.unwrap();

        let mut failed = sample_swap(&b, &a, 50, MintState::Error);
        failed.created_at = now - chrono::Duration::minutes(10);
        failed.error = Some("connection refused".to_string());
        db.insert_swap(&failed).await.unwrap();

        let mut recovered = sample_swap(&a, &b, 50, MintState::Ok);
        recovered.created_at = now - chrono::Duration::minutes(5);
        db.insert_swap(&recovered).await.unwrap();

        let uptime = db.get_mint_uptime(a.id).await.unwrap();
        assert_eq!(uptime.window_hours, 24);
        assert_eq!(uptime.bucket_minutes, 60);
        assert_eq!(uptime.bars.len(), 24);
        assert_eq!(uptime.checks, 3);
        assert_eq!(uptime.successes, 2);
        assert_eq!(uptime.failures, 1);
        assert_eq!(uptime.uptime_percent, Some(2.0 / 3.0 * 100.0));
        assert!(uptime.first_seen_at.is_some());
        assert!(uptime.last_checked_at.is_some());
        assert!(uptime.current_up_since.is_some());
        assert_eq!(uptime.last_error.as_deref(), Some("connection refused"));
        assert!(uptime
            .bars
            .iter()
            .any(|bar| bar.state == MintState::Error && bar.failures == 1));
    }

    #[tokio::test]
    async fn mint_checks_drive_uptime_when_available() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();

        db.insert_swap(&sample_swap(&a, &b, 50, MintState::Ok))
            .await
            .unwrap();
        db.insert_mint_check(
            a.id,
            &a.url,
            "HEALTH",
            MintState::Error,
            Some("connection refused".to_string()),
            Some("network".to_string()),
            None,
        )
        .await
        .unwrap();

        let uptime = db.get_mint_uptime(a.id).await.unwrap();
        assert_eq!(uptime.checks, 1);
        assert_eq!(uptime.successes, 0);
        assert_eq!(uptime.failures, 1);
        assert_eq!(uptime.last_error.as_deref(), Some("connection refused"));
        assert!(uptime.current_up_since.is_none());
        assert!(uptime.bars.iter().any(|bar| bar.state == MintState::Error));
    }

    #[tokio::test]
    async fn health_error_streak_requires_consecutive_error_samples() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://a.example.com");
        mint.id = db.upsert_mint(&mint).await.unwrap();

        db.insert_mint_check(
            mint.id,
            &mint.url,
            "HEALTH",
            MintState::Error,
            Some("connection refused".to_string()),
            Some("network".to_string()),
            None,
        )
        .await
        .unwrap();
        assert!(!db.has_consecutive_health_errors(mint.id, 2).await.unwrap());

        db.insert_mint_check(
            mint.id,
            &mint.url,
            "HEALTH",
            MintState::Error,
            Some("timeout".to_string()),
            Some("timeout".to_string()),
            None,
        )
        .await
        .unwrap();
        assert!(db.has_consecutive_health_errors(mint.id, 2).await.unwrap());

        db.insert_mint_check(
            mint.id,
            &mint.url,
            "HEALTH",
            MintState::Warn,
            Some("HTTP 429 Too Many Requests".to_string()),
            Some("rate_limit".to_string()),
            None,
        )
        .await
        .unwrap();
        assert!(!db.has_consecutive_health_errors(mint.id, 2).await.unwrap());
    }

    #[tokio::test]
    async fn info_probes_recover_from_legacy_wallet_faults() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();
        let mut failure = sample_swap(&a, &b, 5, MintState::Error);
        failure.failed_mint_id = Some(a.id);
        for category in [None, Some(""), Some("unknown"), Some("liquidity")] {
            for raw in [
                "paying the invoice — wallet state error: token already spent (Token Already Spent)",
                "paying the invoice — wallet error: Insufficient Funds",
                "paying the invoice — wallet error: Token not verified",
                "preparing the payment — wallet error: quote already in use",
            ] {
                failure.error = Some(raw.into());
                failure.category = category.map(str::to_string);
                db.insert_swap(&failure).await.unwrap();
                // Simulate the false ERROR set by the previous health query.
                db.record_mint_failure(a.id, Utc::now() - chrono::Duration::minutes(1))
                    .await
                    .unwrap();
                db.record_mint_health_warn(a.id).await.unwrap();
                assert_eq!(
                    db.get_mint_by_id(a.id).await.unwrap().unwrap().state,
                    MintState::Warn,
                    "{category:?}: {raw}"
                );
                db.record_mint_health_ok(a.id).await.unwrap();
                assert_eq!(
                    db.get_mint_by_id(a.id).await.unwrap().unwrap().state,
                    MintState::Ok,
                    "{category:?}: {raw}"
                );
            }
        }
    }

    #[tokio::test]
    async fn info_probes_preserve_payment_failures_until_a_swap_settles() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();
        let mut failure = sample_swap(&a, &b, 5, MintState::Error);
        failure.failed_mint_id = Some(b.id);
        failure.category = Some("mint".into());
        db.insert_swap(&failure).await.unwrap();
        let expired = Utc::now() - chrono::Duration::minutes(1);
        db.record_mint_failure(b.id, expired).await.unwrap();

        // Later failures attributed to the peer or the local wallet must not
        // hide the mint's unresolved payment failure.
        let mut local = failure.clone();
        local.category = Some("internal".into());
        db.insert_swap(&local).await.unwrap();
        // Legacy local errors must also leave the earlier mint failure intact.
        local.category = Some("unknown".into());
        local.error = Some("wallet error: Token Already Spent".into());
        db.insert_swap(&local).await.unwrap();
        let mut peer = failure.clone();
        peer.failed_mint_id = Some(a.id);
        peer.category = Some("liquidity".into());
        peer.error = Some("wallet state error: insufficient funds (Insufficient funds)".into());
        db.insert_swap(&peer).await.unwrap();

        db.insert_mint_check(
            b.id,
            &b.url,
            "HEALTH",
            MintState::Ok,
            None,
            None,
            Some(20.0),
        )
        .await
        .unwrap();
        db.record_mint_health_ok(b.id).await.unwrap();
        db.record_mint_health_warn(b.id).await.unwrap();
        let stored = db.get_mint_by_id(b.id).await.unwrap().unwrap();
        assert_eq!(stored.state, MintState::Error);
        assert_eq!(stored.n_errors, 1);
        assert_eq!(
            stored.next_update.map(|time| dt_to_str(&time)),
            Some(dt_to_str(&expired))
        );
        let uptime = db.get_mint_uptime(b.id).await.unwrap();
        assert_eq!(uptime.uptime_percent, Some(100.0));

        db.record_mint_health_ok(a.id).await.unwrap();
        assert_eq!(
            db.get_mint_by_id(a.id).await.unwrap().unwrap().state,
            MintState::Ok
        );

        db.record_swap_success(&sample_swap(&a, &b, 100, MintState::Ok))
            .await
            .unwrap();
        db.record_mint_health_ok(b.id).await.unwrap();
        assert_eq!(
            db.get_mint_by_id(b.id).await.unwrap().unwrap().state,
            MintState::Ok
        );
    }

    #[tokio::test]
    async fn info_probe_corrects_previously_cleared_payment_failure() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = sample_mint("https://a.example.com");
        let mut b = sample_mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();
        db.record_swap_success(&sample_swap(&a, &b, 5, MintState::Ok))
            .await
            .unwrap();
        let mut failure = sample_swap(&a, &b, 5, MintState::Error);
        failure.failed_mint_id = Some(b.id);
        // Mint-side liquidity failures still count; only local insufficient
        // funds are excluded from payment health.
        failure.category = Some("liquidity".into());
        failure.error = Some("Mint error: insufficient_balance".into());
        db.insert_swap(&failure).await.unwrap();
        // Simulate an existing installation whose info probe reset state to OK.
        db.record_mint_health_ok(b.id).await.unwrap();
        assert_eq!(
            db.get_mint_by_id(b.id).await.unwrap().unwrap().state,
            MintState::Error
        );
        db.record_mint_health_ok(a.id).await.unwrap();
        assert_eq!(
            db.get_mint_by_id(a.id).await.unwrap().unwrap().state,
            MintState::Ok
        );
    }

    #[tokio::test]
    async fn health_failures_do_not_increment_every_probe() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://a.example.com");
        mint.id = db.upsert_mint(&mint).await.unwrap();

        let retry_at = Utc::now() + chrono::Duration::minutes(1);
        let later_retry_at = retry_at + chrono::Duration::minutes(1);
        db.record_mint_health_failure(mint.id, retry_at)
            .await
            .unwrap();
        db.record_mint_health_failure(mint.id, later_retry_at)
            .await
            .unwrap();

        let stored = db.get_mint_by_id(mint.id).await.unwrap().unwrap();
        assert_eq!(stored.state, MintState::Error);
        assert_eq!(stored.n_errors, 1);

        db.record_mint_health_ok(mint.id).await.unwrap();
        let still_benched = db.get_mint_by_id(mint.id).await.unwrap().unwrap();
        assert_eq!(still_benched.state, MintState::Error);

        let mint_id = mint.id;
        db.run(move |conn| {
            conn.execute(
                "UPDATE mints SET next_update = ?2 WHERE id = ?1",
                params![
                    mint_id as i64,
                    dt_to_str(&(Utc::now() - chrono::Duration::seconds(1)))
                ],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.record_mint_health_ok(mint_id).await.unwrap();
        let recovered = db.get_mint_by_id(mint_id).await.unwrap().unwrap();
        assert_eq!(recovered.state, MintState::Ok);
        assert_eq!(recovered.n_errors, 1);
    }

    #[tokio::test]
    async fn migrates_pre_v4_database_in_place() {
        // Simulate a v3 database: swaps/mint_checks predate the new columns.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE swaps (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, from_id INTEGER, to_id INTEGER,
                 from_url TEXT, to_url TEXT, amount INTEGER NOT NULL DEFAULT 0,
                 fee INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL DEFAULT 'UNKNOWN',
                 error TEXT, time_taken REAL NOT NULL DEFAULT 0.0, created_at TEXT NOT NULL);
             CREATE TABLE mint_checks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, mint_id INTEGER NOT NULL,
                 mint_url TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'HEALTH',
                 state TEXT NOT NULL DEFAULT 'UNKNOWN', error TEXT, created_at TEXT NOT NULL);
             CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
             INSERT INTO schema_version(version) VALUES (3);
             INSERT INTO swaps (from_id, to_id, from_url, to_url, amount, fee, state, error,
                                time_taken, created_at)
             VALUES (1, 2, 'a', 'b', 10, 0, 'ERROR', 'legacy boom', 0.0, '2024-01-01T00:00:00Z');",
        )
        .unwrap();

        // Opening must migrate in place without error (the bug this guards).
        let db = AuditorDatabase::init(conn).unwrap();

        // New columns are writable and round-trip.
        let mut a = sample_mint("https://a.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        let mut ev = sample_swap(&a, &a, 5, MintState::Error);
        ev.failed_mint_id = Some(a.id);
        ev.stage = Some("paying the invoice".to_string());
        ev.category = Some("network".to_string());
        db.insert_swap(&ev).await.unwrap();

        let swaps = db.get_swaps(0, 10).await.unwrap();
        // Legacy row survives with NULL attribution columns.
        assert!(swaps
            .iter()
            .any(|s| s.error.as_deref() == Some("legacy boom") && s.failed_mint_id.is_none()));
        // New row carries the v4 attribution.
        assert!(swaps
            .iter()
            .any(|s| s.failed_mint_id == Some(a.id) && s.category.as_deref() == Some("network")));

        // mint_checks gained category + latency.
        db.insert_mint_check(
            a.id,
            &a.url,
            "HEALTH",
            MintState::Error,
            Some("x".to_string()),
            Some("network".to_string()),
            Some(12.0),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn snapshot_insert_is_idempotent_per_timestamp() {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut mint = sample_mint("https://a.example.com");
        mint.id = db.upsert_mint(&mint).await.unwrap();

        db.insert_snapshot(mint.id, 123, &mint).await.unwrap();
        db.insert_snapshot(mint.id, 456, &mint).await.unwrap();

        let count: i64 = db
            .run(
                |conn| Ok(conn.query_row("SELECT COUNT(*) FROM mint_snapshots", [], |r| r.get(0))?),
            )
            .await
            .unwrap();
        // Same second + same mint -> INSERT OR IGNORE dedupes.
        assert_eq!(count, 1);
    }
}
