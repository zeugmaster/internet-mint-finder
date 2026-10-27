use leptos::prelude::*;

/// A mint's face: the icon from its NUT-06 info when it publishes one over
/// https, otherwise its initial on a grey disc. The initial is always drawn
/// under the image, so a slow or broken icon shows the letter, not a gap.
#[component]
pub fn MintAvatar(#[prop(into)] name: String, info: Option<String>) -> impl IntoView {
    let icon = icon_url(info.as_deref());
    view! {
        <span class="mint-avatar" aria-hidden="true">
            <span>{initial(&name)}</span>
            // A plain onerror attribute, not on:error: an icon can fail before
            // hydration, and hiding (not removing) keeps the DOM hydratable.
            {icon.map(|src| view! {
                <img
                    src=src
                    alt=""
                    loading="lazy"
                    decoding="async"
                    referrerpolicy="no-referrer"
                    onerror="this.hidden=true"
                />
            })}
        </span>
    }
}

/// The NUT-06 `icon_url`, only when it is https.
pub fn icon_url(info: Option<&str>) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(info?).ok()?;
    let url = value.get("icon_url")?.as_str()?.trim();
    url.starts_with("https://").then(|| url.to_string())
}

/// The first letter or digit of a name, upper-cased; "?" when there is none.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::{icon_url, initial};

    #[test]
    fn icon_url_is_https_only() {
        let info = |url: &str| format!(r#"{{"name":"x","icon_url":"{url}"}}"#);
        assert_eq!(
            icon_url(Some(&info("https://mint.example/icon.png"))).as_deref(),
            Some("https://mint.example/icon.png")
        );
        assert_eq!(icon_url(Some(&info("http://mint.example/icon.png"))), None);
        assert_eq!(icon_url(Some(&info("javascript:alert(1)"))), None);
        assert_eq!(icon_url(Some(r#"{"name":"x"}"#)), None);
        assert_eq!(icon_url(Some("not json")), None);
        assert_eq!(icon_url(None), None);
    }

    #[test]
    fn initial_skips_punctuation_and_upper_cases() {
        assert_eq!(initial("Northstar"), "N");
        assert_eq!(initial("  ✨minibits"), "M");
        assert_eq!(initial("8333.space"), "8");
        assert_eq!(initial("ärger"), "Ä");
        assert_eq!(initial("---"), "?");
    }
}
