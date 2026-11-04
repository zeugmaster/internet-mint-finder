use super::{nostr::NostrConfig, storage::Database, workers::Workers};
use axum::extract::FromRef;
use leptos::prelude::LeptosOptions;
use std::{path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct AppState {
    pub leptos_options: LeptosOptions,
    pub app_name: String,
    pub db: Arc<Database>,
    pub nostr: Arc<NostrConfig>,
    pub workers: Arc<Workers>,
    pub base_url: Option<String>,
}

impl AppState {
    pub async fn new(leptos_options: LeptosOptions) -> Result<Self, Box<dyn std::error::Error>> {
        let dir = PathBuf::from(std::env::var("IMF_DATA_DIR").unwrap_or_else(|_| "data".into()));
        std::fs::create_dir_all(&dir)?;
        let db = Database::open(&dir.join("directory.sqlite3"))?;
        let base_url = std::env::var("BASE_URL")
            .ok()
            .filter(|v| !v.trim().is_empty())
            .map(|v| {
                super::mint_url::normalize_mint_url(&v)
                    .ok_or("BASE_URL must be an HTTP(S) URL without credentials, query or fragment")
            })
            .transpose()?;
        Ok(Self {
            leptos_options,
            app_name: std::env::var("APP_NAME").unwrap_or_else(|_| "Internet Mint Finder".into()),
            db: Arc::new(db),
            nostr: Arc::new(NostrConfig::from_env()?),
            workers: Default::default(),
            base_url,
        })
    }
}

impl FromRef<AppState> for LeptosOptions {
    fn from_ref(state: &AppState) -> Self {
        state.leptos_options.clone()
    }
}

#[cfg(test)]
pub(crate) fn test_state() -> AppState {
    AppState {
        leptos_options: leptos::prelude::get_configuration(Some("Cargo.toml"))
            .unwrap()
            .leptos_options,
        app_name: "IMF test".into(),
        db: Arc::new(Database::open_in_memory().unwrap()),
        nostr: Arc::new(NostrConfig::default()),
        workers: Default::default(),
        base_url: None,
    }
}
