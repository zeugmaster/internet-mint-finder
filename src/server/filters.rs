//! Filter system for dashboard analytics queries.
//!
//! A `Filters` struct is built from query parameters and produces a
//! parameterized SQL WHERE clause that can be applied to the `swaps` table.
//! Ported from the Python scraper's `filters.py`.

use serde::Deserialize;

/// Date-only ranges include the entire last day, including fractional seconds.
/// Full timestamps retain their explicitly requested inclusive bound.
fn end_bound(end: &str) -> (&'static str, String) {
    if end.len() == 10 {
        if let Some(next_day) = chrono::NaiveDate::parse_from_str(end, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.succ_opt())
        {
            return ("<", format!("{next_day}T00:00:00"));
        }
    }
    ("<=", end.to_string())
}

/// Dashboard filter parameters, parsed from query string.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Filters {
    /// ISO date start bound (inclusive). `YYYY-MM-DD` or full timestamp.
    pub start: Option<String>,
    /// ISO date end bound (inclusive). `YYYY-MM-DD` or full timestamp.
    pub end: Option<String>,
    /// Exact-match from_url filters. Empty = no restriction.
    #[serde(default)]
    pub from_url: Vec<String>,
    /// Exact-match to_url filters. Empty = no restriction.
    #[serde(default)]
    pub to_url: Vec<String>,
    /// Exact-match from-mint version filters.
    #[serde(default)]
    pub from_version: Vec<String>,
    /// Exact-match to-mint version filters.
    #[serde(default)]
    pub to_version: Vec<String>,
    /// Auditor scope: `local` for this instance, else a peer's hex pubkey.
    /// Empty = every auditor.
    #[serde(default)]
    pub auditor: Vec<String>,
}

impl Filters {
    pub fn validate(&self) -> Result<(), crate::server::error::AuditorError> {
        for value in [&self.start, &self.end].into_iter().flatten() {
            if chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_err()
                && chrono::DateTime::parse_from_rfc3339(value).is_err()
            {
                return Err(crate::server::error::AuditorError::InvalidInput(
                    "dates must be YYYY-MM-DD or RFC3339 timestamps".into(),
                ));
            }
        }
        Ok(())
    }

    /// Parse dashboard query parameters.
    ///
    /// Axum's stock `Query` extractor uses `serde_urlencoded`, which does not
    /// handle repeated multi-select params in the shape produced by
    /// `URLSearchParams` (`from_url=a&from_url=b`). The analytics dashboard uses
    /// that repeated-key format, so keep parsing explicit here.
    pub fn from_query(query: &str) -> Self {
        let mut filters = Self::default();

        for (key, value) in form_urlencoded::parse(query.as_bytes()) {
            let value = value.into_owned();
            if value.is_empty() {
                continue;
            }

            match key.as_ref() {
                "start" => filters.start = Some(value),
                "end" => filters.end = Some(value),
                "from_url" => filters.from_url.push(value),
                "to_url" => filters.to_url.push(value),
                "from_version" => filters.from_version.push(value),
                "to_version" => filters.to_version.push(value),
                "auditor" => filters.auditor.push(value),
                _ => {}
            }
        }

        filters
    }

    /// Produce a `(sql_fragment, params)` pair for use in a WHERE clause
    /// against the `swaps` table.
    ///
    /// Returns `("", vec![])` if no filters are set.
    pub fn where_swaps(&self) -> (String, Vec<String>) {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<String> = Vec::new();

        if let Some(ref start) = self.start {
            clauses.push(format!("created_at >= ?{}", params.len() + 1));
            params.push(start.clone());
        }

        if let Some(ref end) = self.end {
            let (operator, end_val) = end_bound(end);
            clauses.push(format!("created_at {operator} ?{}", params.len() + 1));
            params.push(end_val);
        }

        if !self.from_url.is_empty() {
            let placeholders: Vec<String> = self
                .from_url
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", params.len() + i + 1))
                .collect();
            clauses.push(format!("from_url IN ({})", placeholders.join(",")));
            params.extend(self.from_url.iter().cloned());
        }

        if !self.to_url.is_empty() {
            let placeholders: Vec<String> = self
                .to_url
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", params.len() + i + 1))
                .collect();
            clauses.push(format!("to_url IN ({})", placeholders.join(",")));
            params.extend(self.to_url.iter().cloned());
        }

        if !self.from_version.is_empty() {
            let placeholders: Vec<String> = self
                .from_version
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", params.len() + i + 1))
                .collect();
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM mints AS _fm WHERE _fm.url = swaps.from_url \
                 AND json_extract(_fm.info, '$.version') IN ({}))",
                placeholders.join(",")
            ));
            params.extend(self.from_version.iter().cloned());
        }

        if !self.to_version.is_empty() {
            let placeholders: Vec<String> = self
                .to_version
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", params.len() + i + 1))
                .collect();
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM mints AS _tm WHERE _tm.url = swaps.to_url \
                 AND json_extract(_tm.info, '$.version') IN ({}))",
                placeholders.join(",")
            ));
            params.extend(self.to_version.iter().cloned());
        }

        self.push_auditor_clause(&mut clauses, &mut params);

        if clauses.is_empty() {
            return (String::new(), vec![]);
        }

        (format!("WHERE {}", clauses.join(" AND ")), params)
    }

    /// `auditor_pubkey IN (...)`, with `local` mapped to the empty string that
    /// marks this instance's own rows.
    fn push_auditor_clause(&self, clauses: &mut Vec<String>, params: &mut Vec<String>) {
        if self.auditor.is_empty() {
            return;
        }
        let placeholders: Vec<String> = (0..self.auditor.len())
            .map(|i| format!("?{}", params.len() + i + 1))
            .collect();
        clauses.push(format!("auditor_pubkey IN ({})", placeholders.join(",")));
        params.extend(self.auditor.iter().map(|a| {
            if a == "local" {
                String::new()
            } else {
                a.clone()
            }
        }));
    }

    /// Union of explicitly selected mint URLs (source + destination), used to
    /// scope the non-directional tables (`mint_checks`, `mint_snapshots`).
    /// Empty = no mint restriction.
    fn selected_mint_urls(&self) -> Vec<String> {
        let mut urls: Vec<String> = self
            .from_url
            .iter()
            .chain(self.to_url.iter())
            .cloned()
            .collect();
        urls.sort();
        urls.dedup();
        urls
    }

    /// Produce a `(sql_fragment, params)` pair scoped to the `mint_checks`
    /// table: the date range applies to `created_at`, and any selected mints
    /// restrict `mint_url`. Returns `("", vec![])` when unfiltered.
    pub fn where_checks(&self) -> (String, Vec<String>) {
        self.where_dated("created_at", "mint_url", true)
    }

    /// Donation receipts belong to this instance; peers never share wallets.
    pub fn where_donations(&self) -> (String, Vec<String>) {
        let (mut clause, params) = self.where_dated("created_at", "mint_url", false);
        if !self.auditor.is_empty() && !self.auditor.iter().any(|a| a == "local") {
            clause.push_str(if clause.is_empty() {
                "WHERE 0"
            } else {
                " AND 0"
            });
        }
        (clause, params)
    }

    /// Produce a `(sql_fragment, params)` pair scoped to the `mint_snapshots`
    /// table: the date range applies to `ts`; selected mints restrict the
    /// joined mint URL. The caller must join `mints` as `m`, since the
    /// snapshot table only stores `mint_id`.
    pub fn where_snapshots(&self) -> (String, Vec<String>) {
        self.where_dated("ts", "m.url", false)
    }

    /// Shared builder for the date-scoped tables. `ts_col` is the timestamp
    /// column; `url_col` (when non-empty) is restricted to the selected mints;
    /// `has_auditor` tables also take the auditor scope.
    fn where_dated(&self, ts_col: &str, url_col: &str, has_auditor: bool) -> (String, Vec<String>) {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<String> = Vec::new();

        if let Some(ref start) = self.start {
            clauses.push(format!("{ts_col} >= ?{}", params.len() + 1));
            params.push(start.clone());
        }

        if let Some(ref end) = self.end {
            let (operator, end_val) = end_bound(end);
            clauses.push(format!("{ts_col} {operator} ?{}", params.len() + 1));
            params.push(end_val);
        }

        let urls = self.selected_mint_urls();
        if !url_col.is_empty() && !urls.is_empty() {
            let placeholders: Vec<String> = urls
                .iter()
                .enumerate()
                .map(|(i, _)| format!("?{}", params.len() + i + 1))
                .collect();
            clauses.push(format!("{url_col} IN ({})", placeholders.join(",")));
            params.extend(urls);
        }

        if has_auditor {
            self.push_auditor_clause(&mut clauses, &mut params);
        }

        if clauses.is_empty() {
            return (String::new(), vec![]);
        }

        (format!("WHERE {}", clauses.join(" AND ")), params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auditor_scope_maps_local_to_the_empty_pubkey() {
        let f = Filters {
            auditor: vec!["local".into(), "ab".repeat(32)],
            ..Default::default()
        };
        let (sql, params) = f.where_swaps();
        assert_eq!(sql, "WHERE auditor_pubkey IN (?1,?2)");
        assert_eq!(params, vec![String::new(), "ab".repeat(32)]);
        assert_eq!(f.where_checks().0, "WHERE auditor_pubkey IN (?1,?2)");
        assert!(
            f.where_snapshots().0.is_empty(),
            "snapshots have no auditor column"
        );
    }

    #[test]
    fn empty_filters_produce_no_where_clause() {
        let f = Filters::default();
        let (sql, params) = f.where_swaps();
        assert!(sql.is_empty());
        assert!(params.is_empty());
    }

    #[test]
    fn date_only_end_includes_the_entire_day_for_every_table() {
        let f = Filters {
            end: Some("2026-12-31".to_string()),
            ..Default::default()
        };
        for (sql, params) in [f.where_swaps(), f.where_checks(), f.where_snapshots()] {
            assert!(sql.ends_with(" < ?1"));
            assert_eq!(params, vec!["2027-01-01T00:00:00".to_string()]);
        }
    }

    #[test]
    fn explicit_timestamp_end_remains_inclusive() {
        let f = Filters {
            end: Some("2026-06-01T12:34:56.789Z".to_string()),
            ..Default::default()
        };
        let (sql, params) = f.where_swaps();
        assert_eq!(sql, "WHERE created_at <= ?1");
        assert_eq!(params, vec!["2026-06-01T12:34:56.789Z"]);
    }

    #[test]
    fn url_filters_use_positional_placeholders() {
        let f = Filters {
            start: Some("2026-01-01".to_string()),
            from_url: vec!["a".into(), "b".into()],
            to_url: vec!["c".into()],
            ..Default::default()
        };
        let (sql, params) = f.where_swaps();
        assert_eq!(
            sql,
            "WHERE created_at >= ?1 AND from_url IN (?2,?3) AND to_url IN (?4)"
        );
        assert_eq!(params, vec!["2026-01-01", "a", "b", "c"]);
    }

    #[test]
    fn snapshot_url_filters_reference_the_joined_mints_alias() {
        let f = Filters {
            from_url: vec!["https://a.example.com".into()],
            ..Default::default()
        };
        let (sql, params) = f.where_snapshots();
        assert_eq!(sql, "WHERE m.url IN (?1)");
        assert_eq!(params, vec!["https://a.example.com".to_string()]);
    }

    #[test]
    fn dashboard_query_parser_accepts_repeated_filter_params() {
        let filters = Filters::from_query(
            "start=2026-06-01&end=2026-06-17\
             &from_url=https%3A%2F%2Fa.example.com\
             &from_url=https%3A%2F%2Fb.example.com\
             &to_url=https%3A%2F%2Fc.example.com\
             &from_version=Nutshell%2F0.16.0\
             &to_version=cdk-mintd%2F0.11.0\
             &group_by=to_url",
        );

        assert_eq!(filters.start.as_deref(), Some("2026-06-01"));
        assert_eq!(filters.end.as_deref(), Some("2026-06-17"));
        assert_eq!(
            filters.from_url,
            vec![
                "https://a.example.com".to_string(),
                "https://b.example.com".to_string()
            ]
        );
        assert_eq!(filters.to_url, vec!["https://c.example.com".to_string()]);
        assert_eq!(filters.from_version, vec!["Nutshell/0.16.0".to_string()]);
        assert_eq!(filters.to_version, vec!["cdk-mintd/0.11.0".to_string()]);
    }
}
