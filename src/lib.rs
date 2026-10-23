#![recursion_limit = "512"]
pub mod api;
pub mod app;
pub mod components;
pub mod format;
pub mod hooks;
pub mod pages;
#[cfg(feature = "ssr")]
pub mod server;
pub mod types;

#[cfg(feature = "hydrate")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn hydrate() {
    use crate::app::*;
    console_error_panic_hook::set_once();
    leptos::mount::hydrate_body(App);
}
