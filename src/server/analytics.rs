//! Analytics query engine for the dashboard.
//!
//! Each function takes a `rusqlite::Connection` reference and a `Filters`
//! object, returning data structures ready for JSON serialization.
//! Ported from the Python scraper's `queries.py`.

use crate::server::classify::{resolve_category, shorten_error};
use crate::server::error::AuditorError;
use crate::server::filters::Filters;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct FilterOptions {
    pub from_urls: Vec<String>,
    pub to_urls: Vec<String>,
    pub from_versions: Vec<String>,
    pub to_versions: Vec<String>,
    pub date: DateBounds,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DateBounds {
    pub min: Option<String>,
    pub max: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Kpis {
    pub total_sats_swapped: i64,
    pub total_fees_paid: i64,
    pub total_swaps: i64,
    pub total_errors: i64,
    /// Donations received in the filter window, in sats.
    pub total_donations: i64,
    /// Fraction of swaps that succeeded, `0.0..=1.0`.
    pub success_rate: f64,
    /// 95th-percentile latency of successful swaps, in ms.
    pub p95_latency_ms: f64,
    /// Momentum: rolling last-24h figures and their % change vs the prior 24h.
    /// Independent of the date filter — always relative to "now".
    pub swaps_24h: i64,
    pub swaps_delta_pct: Option<f64>,
    pub sats_24h: i64,
    pub sats_delta_pct: Option<f64>,
    pub errors_24h: i64,
    pub errors_delta_pct: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ScatterPoint {
    pub mint: String,
    pub attempts: i64,
    pub errors: i64,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct QualityMint {
    pub mint: String,
    pub attempts: i64,
    pub errors: i64,
    pub error_rate: f64,
    pub avg_latency_ms: f64,
    pub score: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TimeseriesPoint {
    pub bucket: String,
    pub swaps: i64,
    pub errors: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LatencyPoint {
    pub bucket: String,
    pub avg_ms: f64,
    pub n_ok: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Histogram {
    pub edges: Vec<f64>,
    pub counts: Vec<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorCount {
    pub label: String,
    pub count: i64,
    pub example: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: i64,
    pub url: String,
    pub name: String,
    pub state: String,
    pub n_errors: i64,
    pub n_mints: i64,
    pub n_melts: i64,
    pub sum_donations: i64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GraphEdge {
    pub from_id: i64,
    pub to_id: i64,
    pub count: i64,
    pub total_amount: i64,
    pub total_fee: i64,
    pub last_swap: String,
    pub state: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MintGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
}

// ---------- Reliability -----------------------------------------------------

/// One time-bucket cell in a mint's uptime row.
#[derive(Debug, Serialize, Deserialize)]
pub struct UptimeCell {
    pub bucket: String,
    pub checks: i64,
    pub failures: i64,
    /// Fraction of checks in this bucket that were healthy, `0.0..=1.0`.
    pub uptime: f64,
    /// Worst state observed in the bucket: `OK` / `WARN` / `ERROR`, or `NONE`
    /// when no probe landed in the window.
    pub state: String,
}

/// One mint's row in the SLA grid, plus its rollup figures.
#[derive(Debug, Serialize, Deserialize)]
pub struct UptimeRow {
    pub mint_id: i64,
    pub mint: String,
    pub checks: i64,
    pub failures: i64,
    pub uptime: f64,
    /// State of the most recent probe (`OK` / `WARN` / `ERROR` / `NONE`).
    pub current_state: String,
    pub last_checked_at: Option<String>,
    pub cells: Vec<UptimeCell>,
}

/// Status-page style grid: shared bucket axis, one row per mint.
#[derive(Debug, Serialize, Deserialize)]
pub struct UptimeGrid {
    pub buckets: Vec<String>,
    pub rows: Vec<UptimeRow>,
}

/// Where in the swap pipeline failures occur (funnel).
#[derive(Debug, Serialize, Deserialize)]
pub struct FailureStage {
    pub stage: String,
    pub count: i64,
}

/// Which mint is blamed for failures (even as a swap destination).
#[derive(Debug, Serialize, Deserialize)]
pub struct BlameMint {
    pub mint_id: i64,
    pub mint: String,
    pub failures: i64,
    /// Dominant coarse category for this mint's failures.
    pub top_category: String,
}

/// Failure count for one coarse error category (network/mint/timeout/…).
#[derive(Debug, Serialize, Deserialize)]
pub struct CategoryCount {
    pub category: String,
    pub count: i64,
}

// ---------- Money flow / TVL ------------------------------------------------

/// One mint's balance series, aligned to the shared bucket axis.
#[derive(Debug, Serialize, Deserialize)]
pub struct TvlMintSeries {
    pub mint: String,
    pub values: Vec<i64>,
}

/// Total-value-locked over time, as a stackable set of per-mint balance series.
#[derive(Debug, Serialize, Deserialize)]
pub struct TvlSeries {
    pub buckets: Vec<String>,
    pub total: Vec<i64>,
    pub mints: Vec<TvlMintSeries>,
}

/// A node in the sats-flow Sankey diagram.
#[derive(Debug, Serialize, Deserialize)]
pub struct FlowNode {
    pub name: String,
}

/// A weighted link between two mints (indices into [`FlowData::nodes`]).
#[derive(Debug, Serialize, Deserialize)]
pub struct FlowLink {
    pub source: usize,
    pub target: usize,
    pub value: i64,
    pub fee: i64,
}

/// Sankey-ready flow of sats between mints.
#[derive(Debug, Serialize, Deserialize)]
pub struct FlowData {
    pub nodes: Vec<FlowNode>,
    pub links: Vec<FlowLink>,
}

/// Effective fee economics for one source→destination corridor.
#[derive(Debug, Serialize, Deserialize)]
pub struct FeeRoute {
    pub from: String,
    pub to: String,
    pub swaps: i64,
    pub total_amount: i64,
    pub total_fee: i64,
    /// Effective fee as a fraction of amount routed, `0.0..`.
    pub fee_pct: f64,
}

/// Donations received in one time bucket.
#[derive(Debug, Serialize, Deserialize)]
pub struct DonationPoint {
    pub bucket: String,
    pub amount: i64,
    pub count: i64,
}

// ---------------------------------------------------------------------------
// Helper: bind params
// ---------------------------------------------------------------------------

/// Bind a `Vec<String>` into a rusqlite params_from_iter.
/// We use `params_from_iter` with `&[&dyn rusqlite::types::ToSql]`.
fn bind_and_query<T, F>(
    conn: &Connection,
    sql: &str,
    params: &[String],
    map_fn: F,
) -> Result<Vec<T>, AuditorError>
where
    F: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(param_refs.as_slice(), map_fn)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn bind_and_query_one<T, F>(
    conn: &Connection,
    sql: &str,
    params: &[String],
    map_fn: F,
) -> Result<T, AuditorError>
where
    F: FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
{
    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();
    Ok(conn.query_row(sql, param_refs.as_slice(), map_fn)?)
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// All available filter options for populating dropdowns.
pub fn get_filter_options(conn: &Connection) -> Result<FilterOptions, AuditorError> {
    let from_urls = bind_and_query(
        conn,
        "SELECT DISTINCT from_url FROM swaps WHERE from_url IS NOT NULL ORDER BY from_url",
        &[],
        |row| row.get(0),
    )?;

    let to_urls = bind_and_query(
        conn,
        "SELECT DISTINCT to_url FROM swaps WHERE to_url IS NOT NULL ORDER BY to_url",
        &[],
        |row| row.get(0),
    )?;

    let versions: Vec<String> = bind_and_query(
        conn,
        "SELECT DISTINCT json_extract(info, '$.version') AS ver \
         FROM mints WHERE info IS NOT NULL AND json_extract(info, '$.version') IS NOT NULL \
         ORDER BY ver",
        &[],
        |row| row.get(0),
    )?;

    let date = {
        let mut stmt = conn.prepare("SELECT MIN(created_at), MAX(created_at) FROM swaps")?;
        let (min, max): (Option<String>, Option<String>) =
            stmt.query_row([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        DateBounds { min, max }
    };

    Ok(FilterOptions {
        from_urls,
        to_urls,
        from_versions: versions.clone(),
        to_versions: versions,
        date,
    })
}

/// KPI summary tiles, plus 24h momentum deltas.
pub fn kpis(conn: &Connection, filters: &Filters) -> Result<Kpis, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let sql = format!(
        "SELECT \
            COALESCE(SUM(CASE WHEN state = 'OK' THEN amount ELSE 0 END), 0), \
            COALESCE(SUM(fee), 0), \
            COUNT(*), \
            SUM(CASE WHEN state <> 'OK' THEN 1 ELSE 0 END) \
         FROM swaps {where_clause}"
    );

    let (total_sats_swapped, total_fees_paid, total_swaps, total_errors) =
        bind_and_query_one(conn, &sql, &params, |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
            ))
        })?;

    let success_rate = if total_swaps > 0 {
        (total_swaps - total_errors) as f64 / total_swaps as f64
    } else {
        0.0
    };

    // Donations in the same window (reuses the date/mint scope helper).
    let (don_where, don_params) = filters.where_checks();
    let total_donations: i64 = bind_and_query_one(
        conn,
        &format!("SELECT COALESCE(SUM(amount), 0) FROM donations {don_where}"),
        &don_params,
        |row| row.get(0),
    )?;

    // p95 latency of successful swaps in the window.
    let extra = and_or_where(&where_clause);
    let lat_sql = format!(
        "SELECT time_taken FROM swaps {where_clause} {extra} state = 'OK' AND time_taken > 0 \
         ORDER BY time_taken"
    );
    let latencies: Vec<f64> = bind_and_query(conn, &lat_sql, &params, |row| row.get(0))?;
    let p95_latency_ms = percentile(&latencies, 0.95);

    // 24h momentum, relative to now and independent of the filter.
    let now = chrono::Utc::now();
    let bound = |h: i64| {
        (now - chrono::Duration::hours(h))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    };
    let (t0, t24, t48) = (bound(0), bound(24), bound(48));

    let window = |from: &str, to: &str| -> Result<(i64, i64, i64), AuditorError> {
        bind_and_query_one(
            conn,
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN state = 'OK' THEN amount ELSE 0 END), 0), \
                SUM(CASE WHEN state <> 'OK' THEN 1 ELSE 0 END) \
             FROM swaps WHERE created_at > ?1 AND created_at <= ?2",
            &[from.to_string(), to.to_string()],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                ))
            },
        )
    };
    let (swaps_24h, sats_24h, errors_24h) = window(&t24, &t0)?;
    let (swaps_prev, sats_prev, errors_prev) = window(&t48, &t24)?;

    let delta = |cur: i64, prev: i64| -> Option<f64> {
        (prev > 0).then(|| (cur - prev) as f64 / prev as f64 * 100.0)
    };

    Ok(Kpis {
        total_sats_swapped,
        total_fees_paid,
        total_swaps,
        total_errors,
        total_donations,
        success_rate,
        p95_latency_ms,
        swaps_24h,
        swaps_delta_pct: delta(swaps_24h, swaps_prev),
        sats_24h,
        sats_delta_pct: delta(sats_24h, sats_prev),
        errors_24h,
        errors_delta_pct: delta(errors_24h, errors_prev),
    })
}

/// Linear-interpolation-free percentile of a pre-sorted slice (`q` in 0..=1).
fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() as f64 * q).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[idx]
}

/// Scatter data: one point per mint, x=error_rate, y=avg_latency.
pub fn latency_error_scatter(
    conn: &Connection,
    filters: &Filters,
    group_by: &str,
) -> Result<Vec<ScatterPoint>, AuditorError> {
    if group_by != "from_url" && group_by != "to_url" {
        return Err(AuditorError::InvalidInput(format!(
            "invalid group_by: {group_by}"
        )));
    }

    let (where_clause, params) = filters.where_swaps();
    let extra_and = if where_clause.is_empty() {
        "WHERE"
    } else {
        "AND"
    };
    let sql = format!(
        "SELECT \
            {group_by} AS mint, \
            COUNT(*) AS attempts, \
            SUM(CASE WHEN state <> 'OK' THEN 1 ELSE 0 END) AS errors, \
            AVG(CASE WHEN state = 'OK' THEN time_taken END) AS avg_latency_ok, \
            AVG(time_taken) AS avg_latency_all \
         FROM swaps {where_clause} {extra_and} {group_by} IS NOT NULL \
         GROUP BY {group_by} \
         HAVING attempts > 0 \
         ORDER BY attempts DESC"
    );

    bind_and_query(conn, &sql, &params, |row| {
        let attempts: i64 = row.get(1)?;
        let errors: i64 = row.get::<_, Option<i64>>(2)?.unwrap_or(0);
        let avg_ok: Option<f64> = row.get(3)?;
        let avg_all: Option<f64> = row.get(4)?;
        Ok(ScatterPoint {
            mint: row.get(0)?,
            attempts,
            errors,
            error_rate: if attempts > 0 {
                errors as f64 / attempts as f64
            } else {
                0.0
            },
            avg_latency_ms: avg_ok.or(avg_all).unwrap_or(0.0),
        })
    })
}

/// Top quality mints by normalized (latency, error_rate) score.
pub fn top_quality_mints(
    conn: &Connection,
    filters: &Filters,
    group_by: &str,
    limit: usize,
    min_attempts: i64,
) -> Result<Vec<QualityMint>, AuditorError> {
    let rows = latency_error_scatter(conn, filters, group_by)?;
    let qualified: Vec<_> = rows
        .into_iter()
        .filter(|r| r.attempts >= min_attempts)
        .collect();

    if qualified.is_empty() {
        return Ok(vec![]);
    }

    let lat: Vec<f64> = qualified.iter().map(|r| r.avg_latency_ms).collect();
    let err: Vec<f64> = qualified.iter().map(|r| r.error_rate).collect();
    let lat_lo = lat.iter().cloned().fold(f64::INFINITY, f64::min);
    let lat_hi = lat.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let err_lo = err.iter().cloned().fold(f64::INFINITY, f64::min);
    let err_hi = err.iter().cloned().fold(f64::NEG_INFINITY, f64::max);

    let norm = |v: f64, lo: f64, hi: f64| -> f64 {
        if (hi - lo).abs() < f64::EPSILON {
            0.0
        } else {
            (v - lo) / (hi - lo)
        }
    };

    let mut result: Vec<QualityMint> = qualified
        .into_iter()
        .map(|r| {
            let nlat = norm(r.avg_latency_ms, lat_lo, lat_hi);
            let nerr = norm(r.error_rate, err_lo, err_hi);
            QualityMint {
                mint: r.mint,
                attempts: r.attempts,
                errors: r.errors,
                error_rate: r.error_rate,
                avg_latency_ms: r.avg_latency_ms,
                score: (nlat + nerr) / 2.0,
            }
        })
        .collect();

    result.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result.truncate(limit);

    Ok(result)
}

/// Auto-bucketing expression for time series over the `swaps.created_at` column.
fn bucket_expr(start: Option<&str>, end: Option<&str>) -> String {
    bucket_expr_col(start, end, "created_at")
}

/// Auto-bucketing expression for an arbitrary timestamp column, so the
/// reliability (`mint_checks.created_at`) and TVL (`mint_snapshots.ts`) series
/// share the same hour/day/week granularity rules as the swap series.
fn bucket_expr_col(start: Option<&str>, end: Option<&str>, col: &str) -> String {
    let days = match (start, end) {
        (Some(s), Some(e)) => {
            // Parse just enough to get day difference
            let s_date = &s[..10.min(s.len())];
            let e_date = &e[..10.min(e.len())];
            chrono::NaiveDate::parse_from_str(e_date, "%Y-%m-%d")
                .and_then(|ed| {
                    chrono::NaiveDate::parse_from_str(s_date, "%Y-%m-%d")
                        .map(|sd| (ed - sd).num_days())
                })
                .unwrap_or(30)
        }
        _ => 30,
    };

    if days <= 2 {
        format!("strftime('%Y-%m-%dT%H:00', {col})")
    } else if days <= 90 {
        format!("strftime('%Y-%m-%d', {col})")
    } else {
        // Weekly: snap to Monday
        format!("strftime('%Y-%m-%d', {col}, '-' || ((strftime('%w', {col}) + 6) % 7) || ' days')")
    }
}

/// Swaps & errors over time, auto-bucketed.
pub fn swaps_errors_timeseries(
    conn: &Connection,
    filters: &Filters,
) -> Result<Vec<TimeseriesPoint>, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let bucket = bucket_expr(filters.start.as_deref(), filters.end.as_deref());
    let sql = format!(
        "SELECT \
            {bucket} AS bucket, \
            COUNT(*) AS swaps, \
            SUM(CASE WHEN state <> 'OK' THEN 1 ELSE 0 END) AS errors \
         FROM swaps {where_clause} \
         GROUP BY bucket ORDER BY bucket"
    );

    bind_and_query(conn, &sql, &params, |row| {
        Ok(TimeseriesPoint {
            bucket: row.get(0)?,
            swaps: row.get(1)?,
            errors: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
        })
    })
}

/// Average latency over time, auto-bucketed.
pub fn latency_timeseries(
    conn: &Connection,
    filters: &Filters,
) -> Result<Vec<LatencyPoint>, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let bucket = bucket_expr(filters.start.as_deref(), filters.end.as_deref());
    let sql = format!(
        "SELECT \
            {bucket} AS bucket, \
            AVG(CASE WHEN state = 'OK' THEN time_taken END) AS avg_ms, \
            COUNT(CASE WHEN state = 'OK' THEN 1 END) AS n_ok \
         FROM swaps {where_clause} \
         GROUP BY bucket ORDER BY bucket"
    );

    bind_and_query(conn, &sql, &params, |row| {
        Ok(LatencyPoint {
            bucket: row.get(0)?,
            avg_ms: row.get::<_, Option<f64>>(1)?.unwrap_or(0.0),
            n_ok: row.get(2)?,
        })
    })
}

/// Histogram of successful-swap latency, clipped at p99.
pub fn latency_histogram(
    conn: &Connection,
    filters: &Filters,
    bins: usize,
) -> Result<Histogram, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let extra_and = if where_clause.is_empty() {
        "WHERE"
    } else {
        "AND"
    };
    let sql = format!(
        "SELECT time_taken FROM swaps \
         {where_clause} {extra_and} state = 'OK' AND time_taken > 0 \
         ORDER BY time_taken"
    );

    let values: Vec<f64> = bind_and_query(conn, &sql, &params, |row| row.get(0))?;

    if values.is_empty() {
        return Ok(Histogram {
            edges: vec![],
            counts: vec![],
        });
    }

    let p99_idx = ((values.len() as f64 * 0.99) as usize).max(1) - 1;
    let mut upper = values[p99_idx.min(values.len() - 1)];
    if upper <= 0.0 {
        upper = *values.last().unwrap();
    }
    let width = if bins > 0 { upper / bins as f64 } else { upper };

    let edges: Vec<f64> = (0..=bins)
        .map(|i| (width * i as f64 * 1000.0).round() / 1000.0)
        .collect();
    let mut counts = vec![0i64; bins];
    for v in &values {
        let idx = if width > 0.0 {
            ((*v / width) as usize).min(bins - 1)
        } else {
            0
        };
        counts[idx] += 1;
    }

    Ok(Histogram { edges, counts })
}

/// Error counts grouped by classified reason.
pub fn errors_by_reason(
    conn: &Connection,
    filters: &Filters,
    limit: usize,
) -> Result<Vec<ErrorCount>, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let extra_and = if where_clause.is_empty() {
        "WHERE"
    } else {
        "AND"
    };
    let sql = format!(
        "SELECT error FROM swaps \
         {where_clause} {extra_and} state <> 'OK' AND error IS NOT NULL AND error <> ''"
    );

    let raw_errors: Vec<String> = bind_and_query(conn, &sql, &params, |row| row.get(0))?;

    let mut counts: HashMap<String, i64> = HashMap::new();
    let mut examples: HashMap<String, String> = HashMap::new();

    for raw in &raw_errors {
        let label = shorten_error(Some(raw)).unwrap_or("Other").to_string();
        *counts.entry(label.clone()).or_insert(0) += 1;
        examples
            .entry(label)
            .or_insert_with(|| raw.chars().take(200).collect());
    }

    let mut result: Vec<ErrorCount> = counts
        .into_iter()
        .map(|(label, count)| ErrorCount {
            example: examples.get(&label).cloned().unwrap_or_default(),
            label,
            count,
        })
        .collect();

    result.sort_by_key(|e| std::cmp::Reverse(e.count));
    result.truncate(limit);

    Ok(result)
}

/// Graph of mints and their swap relationships.
pub fn mint_graph(conn: &Connection) -> Result<MintGraph, AuditorError> {
    let nodes: Vec<GraphNode> = bind_and_query(
        conn,
        "SELECT id, url, name, state, n_errors, n_mints, n_melts, sum_donations FROM mints ORDER BY id",
        &[],
        |row| {
            Ok(GraphNode {
                id: row.get(0)?,
                url: row.get(1)?,
                name: row.get(2)?,
                state: row.get(3)?,
                n_errors: row.get(4)?,
                n_mints: row.get(5)?,
                n_melts: row.get(6)?,
                sum_donations: row.get(7)?,
            })
        },
    )?;

    let edges: Vec<GraphEdge> = bind_and_query(
        conn,
        "SELECT from_id, to_id, COUNT(*) AS cnt, \
                SUM(amount) AS total_amount, SUM(fee) AS total_fee, \
                MAX(created_at) AS last_swap, \
                (SELECT s2.state FROM swaps s2 WHERE s2.from_id = swaps.from_id AND s2.to_id = swaps.to_id ORDER BY s2.created_at DESC LIMIT 1) AS last_state \
         FROM swaps \
         GROUP BY from_id, to_id \
         ORDER BY cnt DESC",
        &[],
        |row| {
            Ok(GraphEdge {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                count: row.get(2)?,
                total_amount: row.get(3)?,
                total_fee: row.get(4)?,
                last_swap: row.get(5)?,
                state: row.get::<_, Option<String>>(6)?.unwrap_or_else(|| "UNKNOWN".to_string()),
            })
        },
    )?;

    Ok(MintGraph { nodes, edges })
}

// ---------------------------------------------------------------------------
// Reliability queries
// ---------------------------------------------------------------------------

/// Derive a cell/row state slug from its check tally.
fn tally_state(checks: i64, failures: i64) -> &'static str {
    if checks <= 0 {
        "NONE"
    } else if failures <= 0 {
        "OK"
    } else if failures >= checks {
        "ERROR"
    } else {
        "WARN"
    }
}

/// Helper: turn a (possibly empty) where-clause into the `extra` keyword needed
/// to append a further predicate (`WHERE` vs `AND`).
fn and_or_where(where_clause: &str) -> &'static str {
    if where_clause.is_empty() {
        "WHERE"
    } else {
        "AND"
    }
}

/// Status-page uptime grid built from health probes (`mint_checks`).
///
/// One row per mint, aligned to a shared, auto-bucketed time axis. Empty when
/// no probes fall in the window.
pub fn uptime_grid(conn: &Connection, filters: &Filters) -> Result<UptimeGrid, AuditorError> {
    use std::collections::{BTreeSet, HashMap};

    let (where_clause, params) = filters.where_checks();
    let bucket = bucket_expr_col(
        filters.start.as_deref(),
        filters.end.as_deref(),
        "created_at",
    );

    let sql = format!(
        "SELECT mint_id, mint_url, {bucket} AS bucket, \
            COUNT(*) AS checks, \
            SUM(CASE WHEN state <> 'OK' THEN 1 ELSE 0 END) AS failures \
         FROM mint_checks {where_clause} \
         GROUP BY mint_id, bucket \
         ORDER BY mint_url, bucket"
    );

    struct Agg {
        mint_id: i64,
        mint: String,
        bucket: String,
        checks: i64,
        failures: i64,
    }
    let aggs = bind_and_query(conn, &sql, &params, |row| {
        Ok(Agg {
            mint_id: row.get(0)?,
            mint: row.get(1)?,
            bucket: row.get(2)?,
            checks: row.get(3)?,
            failures: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
        })
    })?;

    if aggs.is_empty() {
        return Ok(UptimeGrid {
            buckets: vec![],
            rows: vec![],
        });
    }

    // Latest probe state per mint within the same window.
    let last_sql = format!(
        "SELECT mc.mint_id, mc.state, mc.created_at \
         FROM mint_checks mc \
         JOIN (SELECT mint_id, MAX(id) AS mid FROM mint_checks {where_clause} GROUP BY mint_id) last \
           ON last.mid = mc.id"
    );
    let mut last_state: HashMap<i64, (String, String)> = HashMap::new();
    for (id, state, at) in bind_and_query(conn, &last_sql, &params, |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        last_state.insert(id, (state, at));
    }

    let buckets: Vec<String> = aggs
        .iter()
        .map(|a| a.bucket.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    struct RowAcc {
        mint_id: i64,
        mint: String,
        checks: i64,
        failures: i64,
        cells: HashMap<String, (i64, i64)>,
    }
    let mut rows_map: HashMap<i64, RowAcc> = HashMap::new();
    for a in aggs {
        let acc = rows_map.entry(a.mint_id).or_insert_with(|| RowAcc {
            mint_id: a.mint_id,
            mint: a.mint.clone(),
            checks: 0,
            failures: 0,
            cells: HashMap::new(),
        });
        acc.checks += a.checks;
        acc.failures += a.failures;
        acc.cells.insert(a.bucket, (a.checks, a.failures));
    }

    let mut rows: Vec<UptimeRow> = rows_map
        .into_values()
        .map(|acc| {
            let cells = buckets
                .iter()
                .map(|b| {
                    let (checks, failures) = acc.cells.get(b).copied().unwrap_or((0, 0));
                    UptimeCell {
                        bucket: b.clone(),
                        checks,
                        failures,
                        uptime: if checks > 0 {
                            (checks - failures) as f64 / checks as f64
                        } else {
                            0.0
                        },
                        state: tally_state(checks, failures).to_string(),
                    }
                })
                .collect();
            let (current_state, last_checked_at) = last_state
                .get(&acc.mint_id)
                .map(|(s, t)| (s.clone(), Some(t.clone())))
                .unwrap_or_else(|| ("NONE".to_string(), None));
            UptimeRow {
                mint_id: acc.mint_id,
                mint: acc.mint,
                checks: acc.checks,
                failures: acc.failures,
                uptime: if acc.checks > 0 {
                    (acc.checks - acc.failures) as f64 / acc.checks as f64
                } else {
                    0.0
                },
                current_state,
                last_checked_at,
                cells,
            }
        })
        .collect();

    // Worst uptime first — that's what an operator wants to triage.
    rows.sort_by(|a, b| {
        a.uptime
            .partial_cmp(&b.uptime)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.mint.cmp(&b.mint))
    });

    Ok(UptimeGrid { buckets, rows })
}

/// Failure counts by swap pipeline stage (funnel data).
pub fn failure_stages(
    conn: &Connection,
    filters: &Filters,
) -> Result<Vec<FailureStage>, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let extra = and_or_where(&where_clause);
    let sql = format!(
        "SELECT COALESCE(NULLIF(stage, ''), 'unknown') AS stage, COUNT(*) AS n \
         FROM swaps {where_clause} {extra} state <> 'OK' \
         GROUP BY stage ORDER BY n DESC"
    );
    bind_and_query(conn, &sql, &params, |row| {
        Ok(FailureStage {
            stage: row.get(0)?,
            count: row.get(1)?,
        })
    })
}

/// Which mint is blamed for failures, ranked, with its dominant category.
pub fn blame_mints(
    conn: &Connection,
    filters: &Filters,
    limit: usize,
) -> Result<Vec<BlameMint>, AuditorError> {
    use std::collections::HashMap;

    let (where_clause, params) = filters.where_swaps();
    let extra = and_or_where(&where_clause);
    let sql = format!(
        "SELECT swaps.failed_mint_id, \
            COALESCE(NULLIF(m.name, ''), m.url, 'mint ' || swaps.failed_mint_id) AS mint, \
            swaps.category, swaps.error \
         FROM swaps LEFT JOIN mints m ON m.id = swaps.failed_mint_id \
         {where_clause} {extra} swaps.failed_mint_id IS NOT NULL AND swaps.state <> 'OK'"
    );

    struct Raw {
        id: i64,
        mint: String,
        category: Option<String>,
        error: Option<String>,
    }
    let raw = bind_and_query(conn, &sql, &params, |row| {
        Ok(Raw {
            id: row.get(0)?,
            mint: row.get(1)?,
            category: row.get(2)?,
            error: row.get(3)?,
        })
    })?;

    struct Acc {
        mint: String,
        failures: i64,
        cats: HashMap<String, i64>,
    }
    let mut by_mint: HashMap<i64, Acc> = HashMap::new();
    for r in raw {
        let cat = resolve_category(r.category.as_deref(), r.error.as_deref())
            .unwrap_or("unknown")
            .to_string();
        let acc = by_mint.entry(r.id).or_insert_with(|| Acc {
            mint: r.mint.clone(),
            failures: 0,
            cats: HashMap::new(),
        });
        acc.failures += 1;
        *acc.cats.entry(cat).or_insert(0) += 1;
    }

    let mut result: Vec<BlameMint> = by_mint
        .into_iter()
        .map(|(id, acc)| {
            let top_category = acc
                .cats
                .into_iter()
                .max_by_key(|(_, n)| *n)
                .map(|(c, _)| c)
                .unwrap_or_else(|| "unknown".to_string());
            BlameMint {
                mint_id: id,
                mint: acc.mint,
                failures: acc.failures,
                top_category,
            }
        })
        .collect();

    result.sort_by_key(|b| std::cmp::Reverse(b.failures));
    result.truncate(limit);
    Ok(result)
}

/// Failure counts grouped by coarse error category, falling back to the
/// classifier when the stored category is missing or still unknown.
pub fn errors_by_category(
    conn: &Connection,
    filters: &Filters,
) -> Result<Vec<CategoryCount>, AuditorError> {
    use std::collections::HashMap;

    let (where_clause, params) = filters.where_swaps();
    let extra = and_or_where(&where_clause);
    let sql = format!(
        "SELECT category, error FROM swaps \
         {where_clause} {extra} state <> 'OK'"
    );

    let rows = bind_and_query(conn, &sql, &params, |row| {
        Ok((
            row.get::<_, Option<String>>(0)?,
            row.get::<_, Option<String>>(1)?,
        ))
    })?;

    let mut counts: HashMap<String, i64> = HashMap::new();
    for (category, error) in rows {
        let cat = resolve_category(category.as_deref(), error.as_deref())
            .unwrap_or("unknown")
            .to_string();
        *counts.entry(cat).or_insert(0) += 1;
    }

    let mut result: Vec<CategoryCount> = counts
        .into_iter()
        .map(|(category, count)| CategoryCount { category, count })
        .collect();
    result.sort_by_key(|c| std::cmp::Reverse(c.count));
    Ok(result)
}

// ---------------------------------------------------------------------------
// Money flow / TVL queries
// ---------------------------------------------------------------------------

/// Total-value-locked over time, from balance snapshots. For each mint and
/// bucket the *latest* snapshot balance in that bucket is taken (a level, not a
/// flow), then per-mint series are aligned to a shared bucket axis.
pub fn tvl_series(conn: &Connection, filters: &Filters) -> Result<TvlSeries, AuditorError> {
    use std::collections::{BTreeSet, HashMap};

    let (where_clause, params) = filters.where_snapshots();
    let bucket = bucket_expr_col(filters.start.as_deref(), filters.end.as_deref(), "ts");

    // Last snapshot per (mint, bucket); join mints for a display name.
    let sql = format!(
        "SELECT mint, bucket, balance FROM ( \
            SELECT COALESCE(NULLIF(m.name, ''), m.url, 'mint ' || s.mint_id) AS mint, \
                {bucket} AS bucket, s.balance AS balance, \
                ROW_NUMBER() OVER (PARTITION BY s.mint_id, {bucket} ORDER BY s.ts DESC, s.rowid DESC) AS rn \
            FROM mint_snapshots s LEFT JOIN mints m ON m.id = s.mint_id \
            {where_clause} \
         ) WHERE rn = 1 ORDER BY mint, bucket"
    );

    struct Row {
        mint: String,
        bucket: String,
        balance: i64,
    }
    let rows = bind_and_query(conn, &sql, &params, |row| {
        Ok(Row {
            mint: row.get(0)?,
            bucket: row.get(1)?,
            balance: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
        })
    })?;

    if rows.is_empty() {
        return Ok(TvlSeries {
            buckets: vec![],
            total: vec![],
            mints: vec![],
        });
    }

    let buckets: Vec<String> = rows
        .iter()
        .map(|r| r.bucket.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let bucket_idx: HashMap<&str, usize> = buckets
        .iter()
        .enumerate()
        .map(|(i, b)| (b.as_str(), i))
        .collect();

    let mut by_mint: HashMap<String, Vec<i64>> = HashMap::new();
    for r in &rows {
        let series = by_mint
            .entry(r.mint.clone())
            .or_insert_with(|| vec![0; buckets.len()]);
        if let Some(&i) = bucket_idx.get(r.bucket.as_str()) {
            series[i] = r.balance;
        }
    }

    // Carry the last known balance forward across empty buckets so the stack
    // reads as a continuous level rather than dropping to zero between probes.
    let mut mints: Vec<TvlMintSeries> = by_mint
        .into_iter()
        .map(|(mint, mut values)| {
            let mut last = 0;
            for v in values.iter_mut() {
                if *v == 0 {
                    *v = last;
                } else {
                    last = *v;
                }
            }
            TvlMintSeries { mint, values }
        })
        .collect();
    mints.sort_by(|a, b| a.mint.cmp(&b.mint));

    let total: Vec<i64> = (0..buckets.len())
        .map(|i| mints.iter().map(|m| m.values[i]).sum())
        .collect();

    Ok(TvlSeries {
        buckets,
        total,
        mints,
    })
}

/// Sankey-ready flow of sats between mints (successful swaps only).
pub fn flow_data(conn: &Connection, filters: &Filters) -> Result<FlowData, AuditorError> {
    use std::collections::HashMap;

    let (where_clause, params) = filters.where_swaps();
    let extra = and_or_where(&where_clause);
    let sql = format!(
        "SELECT from_url, to_url, SUM(amount) AS amt, SUM(fee) AS fee \
         FROM swaps {where_clause} {extra} state = 'OK' \
            AND from_url IS NOT NULL AND to_url IS NOT NULL \
         GROUP BY from_url, to_url HAVING amt > 0 ORDER BY amt DESC"
    );

    struct Edge {
        from: String,
        to: String,
        amount: i64,
        fee: i64,
    }
    let edges = bind_and_query(conn, &sql, &params, |row| {
        Ok(Edge {
            from: row.get(0)?,
            to: row.get(1)?,
            amount: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            fee: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
        })
    })?;

    let strip = |u: &str| u.replace("https://", "").replace("http://", "");
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut nodes: Vec<FlowNode> = Vec::new();
    let mut idx_of = |name: String, nodes: &mut Vec<FlowNode>| -> usize {
        *index.entry(name.clone()).or_insert_with(|| {
            nodes.push(FlowNode { name });
            nodes.len() - 1
        })
    };

    let mut links = Vec::with_capacity(edges.len());
    for e in edges {
        let source = idx_of(strip(&e.from), &mut nodes);
        let target = idx_of(strip(&e.to), &mut nodes);
        links.push(FlowLink {
            source,
            target,
            value: e.amount,
            fee: e.fee,
        });
    }

    Ok(FlowData { nodes, links })
}

/// Effective fee per source→destination corridor, priciest first.
pub fn fee_routes(
    conn: &Connection,
    filters: &Filters,
    limit: usize,
) -> Result<Vec<FeeRoute>, AuditorError> {
    let (where_clause, params) = filters.where_swaps();
    let extra = and_or_where(&where_clause);
    let sql = format!(
        "SELECT from_url, to_url, COUNT(*) AS n, \
            COALESCE(SUM(amount), 0) AS amt, COALESCE(SUM(fee), 0) AS fee \
         FROM swaps {where_clause} {extra} state = 'OK' \
            AND from_url IS NOT NULL AND to_url IS NOT NULL \
         GROUP BY from_url, to_url HAVING amt > 0 \
         ORDER BY (CAST(SUM(fee) AS REAL) / SUM(amount)) DESC, from_url, to_url LIMIT {limit}"
    );

    let strip = |u: String| u.replace("https://", "").replace("http://", "");
    bind_and_query(conn, &sql, &params, move |row| {
        let amount: i64 = row.get(3)?;
        let fee: i64 = row.get(4)?;
        Ok(FeeRoute {
            from: strip(row.get(0)?),
            to: strip(row.get(1)?),
            swaps: row.get(2)?,
            total_amount: amount,
            total_fee: fee,
            fee_pct: if amount > 0 {
                fee as f64 / amount as f64
            } else {
                0.0
            },
        })
    })
}

/// Donations received over time, auto-bucketed.
pub fn donations_timeseries(
    conn: &Connection,
    filters: &Filters,
) -> Result<Vec<DonationPoint>, AuditorError> {
    let (where_clause, params) = filters.where_checks();
    let bucket = bucket_expr_col(
        filters.start.as_deref(),
        filters.end.as_deref(),
        "created_at",
    );
    let sql = format!(
        "SELECT {bucket} AS bucket, COALESCE(SUM(amount), 0) AS amount, COUNT(*) AS n \
         FROM donations {where_clause} \
         GROUP BY bucket ORDER BY bucket"
    );
    bind_and_query(conn, &sql, &params, |row| {
        Ok(DonationPoint {
            bucket: row.get(0)?,
            amount: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
            count: row.get(2)?,
        })
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::storage::{AuditorDatabase, Mint, SwapEvent};
    use crate::types::MintState;
    use chrono::Utc;

    async fn seeded_db() -> AuditorDatabase {
        let db = AuditorDatabase::open_in_memory().unwrap();

        let mut a = mint("https://a.example.com");
        let mut b = mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();

        for i in 0..8 {
            db.insert_swap(&swap(&a, &b, 100, 1500.0 + i as f64, MintState::Ok, None))
                .await
                .unwrap();
        }
        db.insert_swap(&swap(
            &a,
            &b,
            100,
            0.0,
            MintState::Error,
            Some("connection refused".to_string()),
        ))
        .await
        .unwrap();
        db.insert_swap(&swap(&b, &a, 50, 900.0, MintState::Ok, None))
            .await
            .unwrap();

        db
    }

    fn mint(url: &str) -> Mint {
        Mint {
            id: 0,
            url: url.to_string(),
            info: Some(r#"{"version":"Nutshell/0.16.0"}"#.to_string()),
            name: url.to_string(),
            sum_donations: 0,
            updated_at: Utc::now(),
            next_update: None,
            state: MintState::Ok,
            n_errors: 0,
            n_mints: 0,
            n_melts: 0,
            latitude: None,
            longitude: None,
            deleted_at: None,
        }
    }

    fn swap(
        from: &Mint,
        to: &Mint,
        amount: u64,
        time_taken: f64,
        state: MintState,
        error: Option<String>,
    ) -> SwapEvent {
        SwapEvent {
            id: 0,
            from_id: from.id,
            to_id: to.id,
            from_url: from.url.clone(),
            to_url: to.url.clone(),
            amount,
            fee: 2,
            created_at: Utc::now(),
            time_taken,
            state,
            error,
            failed_mint_id: None,
            stage: None,
            category: None,
            fee_reserve: None,
            auditor_pubkey: String::new(),
            event_id: None,
        }
    }

    #[tokio::test]
    async fn kpis_count_totals_and_errors() {
        let db = seeded_db().await;
        let kpis = db.run(|c| kpis(c, &Filters::default())).await.unwrap();
        assert_eq!(kpis.total_swaps, 10);
        assert_eq!(kpis.total_errors, 1);
        assert_eq!(kpis.total_sats_swapped, 8 * 100 + 50);
        assert_eq!(
            kpis.total_sats_swapped as u64,
            db.get_total_amount_swapped().await.unwrap()
        );
    }

    #[tokio::test]
    async fn date_filter_keeps_last_second_and_excludes_next_day() {
        let (db, a, b) = two_mints().await;
        for timestamp in [
            "2026-06-01T23:59:59Z",
            "2026-06-01T23:59:59.999Z",
            "2026-06-02T00:00:00Z",
        ] {
            let mut event = swap(&a, &b, 100, 1200.0, MintState::Ok, None);
            event.created_at = timestamp.parse().unwrap();
            db.insert_swap(&event).await.unwrap();
        }
        let result = db
            .run(|c| {
                kpis(
                    c,
                    &Filters {
                        start: Some("2026-06-01".into()),
                        end: Some("2026-06-01".into()),
                        ..Default::default()
                    },
                )
            })
            .await
            .unwrap();
        assert_eq!(result.total_swaps, 2);
        assert_eq!(result.total_sats_swapped, 200);
    }

    #[tokio::test]
    async fn scatter_groups_by_from_url() {
        let db = seeded_db().await;
        let points = db
            .run(|c| latency_error_scatter(c, &Filters::default(), "from_url"))
            .await
            .unwrap();
        assert_eq!(points.len(), 2);
        let a = points
            .iter()
            .find(|p| p.mint == "https://a.example.com")
            .unwrap();
        assert_eq!(a.attempts, 9);
        assert_eq!(a.errors, 1);
        assert!((a.error_rate - 1.0 / 9.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn scatter_rejects_invalid_group_by() {
        let db = seeded_db().await;
        let result = db
            .run(|c| latency_error_scatter(c, &Filters::default(), "amount; DROP TABLE swaps"))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn histogram_covers_all_ok_swaps() {
        let db = seeded_db().await;
        let hist = db
            .run(|c| latency_histogram(c, &Filters::default(), 10))
            .await
            .unwrap();
        assert_eq!(hist.counts.iter().sum::<i64>(), 9);
        assert_eq!(hist.edges.len(), 11);
    }

    #[tokio::test]
    async fn errors_by_reason_classifies() {
        let db = seeded_db().await;
        let errors = db
            .run(|c| errors_by_reason(c, &Filters::default(), 10))
            .await
            .unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].label, "Connection error");
        assert_eq!(errors[0].count, 1);
    }

    #[tokio::test]
    async fn graph_has_nodes_and_edges() {
        let db = seeded_db().await;
        let graph = db.run(mint_graph).await.unwrap();
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 2);
    }

    #[tokio::test]
    async fn filter_options_include_versions() {
        let db = seeded_db().await;
        let options = db.run(get_filter_options).await.unwrap();
        assert_eq!(options.from_urls.len(), 2);
        assert_eq!(options.from_versions, vec!["Nutshell/0.16.0".to_string()]);
        assert!(options.date.min.is_some());
    }

    /// Build two mints and return the db plus their assigned ids.
    async fn two_mints() -> (AuditorDatabase, Mint, Mint) {
        let db = AuditorDatabase::open_in_memory().unwrap();
        let mut a = mint("https://a.example.com");
        let mut b = mint("https://b.example.com");
        a.id = db.upsert_mint(&a).await.unwrap();
        b.id = db.upsert_mint(&b).await.unwrap();
        (db, a, b)
    }

    #[tokio::test]
    async fn kpis_expose_success_rate_and_p95() {
        let db = seeded_db().await;
        let k = db.run(|c| kpis(c, &Filters::default())).await.unwrap();
        assert_eq!(k.total_swaps, 10);
        // 1 of 10 failed.
        assert!((k.success_rate - 0.9).abs() < 1e-9);
        assert!(k.p95_latency_ms > 0.0);
    }

    #[tokio::test]
    async fn uptime_grid_aggregates_checks() {
        let (db, a, b) = two_mints().await;
        for state in [MintState::Ok, MintState::Error] {
            db.insert_mint_check(a.id, &a.url, "HEALTH", state, None, None, Some(120.0))
                .await
                .unwrap();
        }
        db.insert_mint_check(
            b.id,
            &b.url,
            "HEALTH",
            MintState::Ok,
            None,
            None,
            Some(80.0),
        )
        .await
        .unwrap();

        let grid = db
            .run(|c| uptime_grid(c, &Filters::default()))
            .await
            .unwrap();
        assert_eq!(grid.rows.len(), 2);
        assert!(!grid.buckets.is_empty());
        // Worst uptime first: mint a (50%) ahead of mint b (100%).
        let first = &grid.rows[0];
        assert_eq!(first.mint, "https://a.example.com");
        assert_eq!(first.checks, 2);
        assert_eq!(first.failures, 1);
        assert!((first.uptime - 0.5).abs() < 1e-9);
        assert_eq!(first.cells.len(), grid.buckets.len());
    }

    #[tokio::test]
    async fn failure_stages_blame_and_category() {
        let (db, a, b) = two_mints().await;
        db.insert_swap(&swap(&a, &b, 100, 1200.0, MintState::Ok, None))
            .await
            .unwrap();
        let mut e = swap(
            &a,
            &b,
            100,
            0.0,
            MintState::Error,
            Some("mint timeout: did not respond within 10s".to_string()),
        );
        e.stage = Some("paying invoice".to_string());
        e.failed_mint_id = Some(b.id);
        db.insert_swap(&e).await.unwrap();

        let stages = db
            .run(|c| failure_stages(c, &Filters::default()))
            .await
            .unwrap();
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].stage, "paying invoice");
        assert_eq!(stages[0].count, 1);

        let blame = db
            .run(|c| blame_mints(c, &Filters::default(), 10))
            .await
            .unwrap();
        assert_eq!(blame.len(), 1);
        assert_eq!(blame[0].mint_id, b.id as i64);
        assert_eq!(blame[0].failures, 1);
        assert_eq!(blame[0].top_category, "timeout");

        // Correlated version filters must keep referencing the swaps table
        // when the blame query joins mint metadata.
        let filtered = db
            .run(|c| {
                blame_mints(
                    c,
                    &Filters {
                        from_version: vec!["Nutshell/0.16.0".into()],
                        to_version: vec!["Nutshell/0.16.0".into()],
                        ..Default::default()
                    },
                    10,
                )
            })
            .await
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].mint_id, b.id as i64);

        let cats = db
            .run(|c| errors_by_category(c, &Filters::default()))
            .await
            .unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0].category, "timeout");
    }

    #[tokio::test]
    async fn historical_unknown_categories_are_reclassified_across_readers() {
        let (db, a, b) = two_mints().await;
        let mut failure = swap(
            &a,
            &b,
            10,
            0.0,
            MintState::Error,
            Some("paying the invoice — wallet error: Payment failed".into()),
        );
        failure.failed_mint_id = Some(b.id);
        for category in [None, Some(""), Some("mint"), Some("unknown")] {
            failure.category = category.map(str::to_string);
            db.insert_swap(&failure).await.unwrap();
        }

        let (categories, blame) = db
            .run(|conn| {
                Ok((
                    errors_by_category(conn, &Filters::default())?,
                    blame_mints(conn, &Filters::default(), 10)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(categories.len(), 1);
        assert_eq!(categories[0].category, "mint");
        assert_eq!(categories[0].count, 4);
        assert_eq!(blame[0].top_category, "mint");
        assert_eq!(blame[0].failures, 4);
        assert_eq!(
            db.get_mint_metrics(b.id, None)
                .await
                .unwrap()
                .last_error_category
                .as_deref(),
            Some("mint")
        );
        assert_eq!(
            db.get_mint_uptime(b.id)
                .await
                .unwrap()
                .last_error_category
                .as_deref(),
            Some("mint")
        );
        assert!(db
            .get_swaps(0, 10)
            .await
            .unwrap()
            .iter()
            .all(|s| s.category.as_deref() == Some("mint")));

        // An explicit category from a typed fault takes precedence over raw text.
        failure.failed_mint_id = Some(a.id);
        failure.category = Some("internal".into());
        db.insert_swap(&failure).await.unwrap();
        let (categories, blame) = db
            .run(|conn| {
                Ok((
                    errors_by_category(conn, &Filters::default())?,
                    blame_mints(conn, &Filters::default(), 10)?,
                ))
            })
            .await
            .unwrap();
        assert_eq!(categories.len(), 2);
        assert!(categories
            .iter()
            .any(|c| c.category == "internal" && c.count == 1));
        assert!(blame
            .iter()
            .any(|m| m.mint_id == a.id as i64 && m.top_category == "internal"));
        assert_eq!(
            db.get_mint_metrics(a.id, None)
                .await
                .unwrap()
                .last_error_category
                .as_deref(),
            Some("internal")
        );
        assert_eq!(
            db.get_mint_uptime(a.id)
                .await
                .unwrap()
                .last_error_category
                .as_deref(),
            Some("internal")
        );
        assert_eq!(
            db.get_swaps(0, 1).await.unwrap()[0].category.as_deref(),
            Some("internal")
        );
    }

    #[tokio::test]
    async fn flow_and_fees_from_successful_swaps() {
        let db = seeded_db().await;
        let flow = db.run(|c| flow_data(c, &Filters::default())).await.unwrap();
        // a->b and b->a corridors, two nodes.
        assert_eq!(flow.nodes.len(), 2);
        assert_eq!(flow.links.len(), 2);
        assert!(flow.links.iter().all(|l| l.value > 0));

        let fees = db
            .run(|c| fee_routes(c, &Filters::default(), 10))
            .await
            .unwrap();
        assert_eq!(fees.len(), 2);
        assert!(fees[0].fee_pct >= fees[1].fee_pct);
    }

    #[tokio::test]
    async fn fee_routes_rank_by_aggregate_rate_not_one_swap_fee() {
        let (db, a, b) = two_mints().await;
        for _ in 0..3 {
            let mut event = swap(&a, &b, 100, 1200.0, MintState::Ok, None);
            event.fee = 10;
            db.insert_swap(&event).await.unwrap();
        }
        let mut event = swap(&b, &a, 100, 1200.0, MintState::Ok, None);
        event.fee = 5;
        db.insert_swap(&event).await.unwrap();
        let routes = db
            .run(|c| fee_routes(c, &Filters::default(), 10))
            .await
            .unwrap();
        assert_eq!(routes[0].from, "a.example.com");
        assert!((routes[0].fee_pct - 0.1).abs() < 1e-9);
        assert!((routes[1].fee_pct - 0.05).abs() < 1e-9);
    }

    #[tokio::test]
    async fn tvl_carries_balance_forward() {
        let (db, a, _b) = two_mints().await;
        let aid = a.id as i64;
        db.run(move |conn| {
            for (ts, bal) in [
                ("2026-06-01T00:00:00", 500i64),
                ("2026-06-03T00:00:00", 800),
            ] {
                conn.execute(
                    "INSERT INTO mint_snapshots (ts, mint_id, balance) VALUES (?1, ?2, ?3)",
                    rusqlite::params![ts, aid, bal],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let filters = Filters {
            start: Some("2026-06-01".to_string()),
            end: Some("2026-06-04".to_string()),
            ..Default::default()
        };
        let tvl = db.run(move |c| tvl_series(c, &filters)).await.unwrap();
        assert_eq!(tvl.mints.len(), 1);
        let series = &tvl.mints[0].values;
        // Two snapshot days, plus the carried-forward day in between.
        assert_eq!(series.first().copied(), Some(500));
        assert_eq!(series.last().copied(), Some(800));
        assert!(series.contains(&500)); // gap day carried forward
    }

    #[tokio::test]
    async fn tvl_respects_mint_filters() {
        let (db, a, b) = two_mints().await;
        let (aid, bid) = (a.id as i64, b.id as i64);
        db.run(move |conn| {
            for (mint_id, bal) in [(aid, 500i64), (bid, 900)] {
                conn.execute(
                    "INSERT INTO mint_snapshots (ts, mint_id, balance) VALUES (?1, ?2, ?3)",
                    rusqlite::params!["2026-06-01T00:00:00", mint_id, bal],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let filters = Filters {
            start: Some("2026-06-01".to_string()),
            end: Some("2026-06-02".to_string()),
            from_url: vec![a.url.clone()],
            ..Default::default()
        };
        let tvl = db.run(move |c| tvl_series(c, &filters)).await.unwrap();
        assert_eq!(tvl.mints.len(), 1);
        assert_eq!(tvl.mints[0].mint, a.url);
        assert!(tvl.mints[0].values.iter().all(|v| *v == 500));
        assert!(tvl.total.iter().all(|v| *v == 500));
    }

    #[tokio::test]
    async fn donations_timeseries_sums_amounts() {
        let (db, a, _b) = two_mints().await;
        db.insert_donation(a.id, &a.url, 21, "token").await.unwrap();
        db.insert_donation(a.id, &a.url, 79, "token").await.unwrap();
        let points = db
            .run(|c| donations_timeseries(c, &Filters::default()))
            .await
            .unwrap();
        assert_eq!(points.iter().map(|p| p.amount).sum::<i64>(), 100);
        assert_eq!(points.iter().map(|p| p.count).sum::<i64>(), 2);
    }
}
