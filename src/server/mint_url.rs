//! Canonical mint URL spelling, shared by the registry, token receive, the
//! Nostr publisher/ingester and the CLIs. Delegates to CDK's `MintUrl` so a
//! URL stored here always equals the one CDK prints for the same mint
//! (lower-case scheme and host, no trailing slash, path case preserved).

use std::str::FromStr;

pub fn normalize_mint_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    cdk::mint_url::MintUrl::from_str(trimmed)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_mint_url;

    #[test]
    fn matches_cdk_spelling() {
        assert_eq!(
            normalize_mint_url(" https://Mint.Example.COM/ "),
            "https://mint.example.com"
        );
        assert_eq!(
            normalize_mint_url("http://localhost:8085/"),
            "http://localhost:8085"
        );
        assert_eq!(
            normalize_mint_url("https://a.example/Path/"),
            "https://a.example/Path"
        );
        // Not a URL: trimmed, otherwise untouched, never panics.
        assert_eq!(normalize_mint_url(" junk/ "), "junk");
    }
}
