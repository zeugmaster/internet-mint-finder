//! The same URL spelling for legacy reviews, announcements and new reviews.
pub fn normalize_mint_url(raw: &str) -> Option<String> {
    if raw.len() > 2048 {
        return None;
    }
    let url = url::Url::parse(raw.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_without_changing_case_sensitive_paths() {
        assert_eq!(
            normalize_mint_url(" HTTPS://Mint.Example:443/Path/ ").as_deref(),
            Some("https://mint.example/Path")
        );
        for value in [
            "javascript:alert(1)",
            "https://u:p@mint.example",
            "https://mint.example?x=1",
            "https://mint.example#x",
            "junk",
        ] {
            assert!(normalize_mint_url(value).is_none());
        }
    }
}
