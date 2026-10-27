use crate::api::search_mints;
use crate::components::badge::StateBadge;
use crate::components::icons::{Check, Copy};
use crate::components::mint_avatar::MintAvatar;
use crate::format::{relative_time, short_url};
use crate::hooks::use_poll;
use crate::types::{MintPage, MintRead, MintState};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;

const PAGE_SIZE: u64 = 20;

/// The directory: one row per mint with its plain status. Search, status and
/// page live in the URL (`?q=&status=&page=`) so Back and shared links keep
/// the view.
#[component]
pub fn MintList() -> impl IntoView {
    let tick = use_poll(15);
    let query = use_query_map();
    let navigate = use_navigate();

    let applied_search = Memo::new(move |_| query.get().get("q").unwrap_or_default());
    let state_filter = Memo::new(move |_| {
        query
            .get()
            .get("status")
            .and_then(|slug| status_from_slug(&slug))
    });
    let page = Memo::new(move |_| {
        query
            .get()
            .get("page")
            .and_then(|p| p.parse::<u64>().ok())
            .map(|p| p.saturating_sub(1))
            .unwrap_or(0)
    });

    // Typing updates the URL after a short pause; replacing keeps history clean.
    let search = RwSignal::new(applied_search.get_untracked());
    Effect::new({
        let navigate = navigate.clone();
        move |_| {
            let value = search.get();
            if value == applied_search.get_untracked() {
                return;
            }
            let navigate = navigate.clone();
            if let Ok(handle) = set_timeout_with_handle(
                move || {
                    let url = view_url(&value, state_filter.get_untracked(), 0);
                    navigate(&url, quiet());
                },
                std::time::Duration::from_millis(250),
            ) {
                on_cleanup(move || handle.clear());
            }
        }
    });

    let mints = Resource::new(
        move || {
            (
                tick.get(),
                applied_search.get(),
                state_filter.get(),
                page.get(),
            )
        },
        |(_, search, state, page)| async move { search_mints(search, state, page).await },
    );

    // A failed background refresh keeps the last good list on screen.
    let last_good = RwSignal::new(None::<MintPage>);
    let updated_at = RwSignal::new(None::<DateTime<Utc>>);
    Effect::new(move |_| {
        if let Some(Ok(data)) = mints.get() {
            last_good.set(Some(data));
            updated_at.set(Some(Utc::now()));
        }
    });
    let freshness = move || {
        tick.track();
        let stale = matches!(mints.get(), Some(Err(_)));
        match (updated_at.get(), stale) {
            (Some(at), false) => format!("Updated {}", relative_time(at)),
            (Some(at), true) => {
                format!("Couldn't refresh, retrying · updated {}", relative_time(at))
            }
            (None, _) => String::new(),
        }
    };

    let on_status = {
        let navigate = navigate.clone();
        move |ev| {
            let status = status_from_slug(&event_target_value(&ev));
            navigate(
                &view_url(&applied_search.get_untracked(), status, 0),
                quiet(),
            );
        }
    };
    let clear = {
        let navigate = navigate.clone();
        move |_| {
            search.set(String::new());
            navigate("/", quiet());
        }
    };

    view! {
        <div class="directory">
            <div class="directory-tools">
                <div class="search-field">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/></svg>
                    <input type="search" aria-label="Search mints by name or URL" placeholder="Search name or URL"
                        prop:value=move || search.get() on:input=move |ev| search.set(event_target_value(&ev))/>
                </div>
                <select class="filter-select" aria-label="Filter mints by status" on:change=on_status
                    prop:value=move || state_filter.get().map(status_slug).unwrap_or("all")>
                    <option value="all">"All statuses"</option>
                    <option value="healthy">"Healthy"</option>
                    <option value="degraded">"Degraded"</option>
                    <option value="failing">"Failing"</option>
                    <option value="unchecked">"Not checked yet"</option>
                </select>
            </div>
            <Transition fallback=|| view! { <DirectorySkeleton/> }>
                {move || mints.get().map(|result| {
                    let data = match result {
                        Ok(data) => Some(data),
                        Err(_) => last_good.get(),
                    };
                    let Some(data) = data else {
                        return view! {
                            <div class="empty-state" role="alert">
                                <h3>"Couldn't load mints"</h3>
                                <p>"Check your connection. This retries every 15 seconds."</p>
                                <button type="button" class="btn btn-secondary mt-4" on:click=move |_| mints.refetch()>"Try again"</button>
                            </div>
                        }.into_any();
                    };
                    let total = data.total;
                    let current = page.get();
                    let pages = total.div_ceil(PAGE_SIZE).max(1);
                    let filtered = !applied_search.get().is_empty() || state_filter.get().is_some();
                    let count = format!("{} {}", total, if total == 1 { "mint" } else { "mints" });
                    let prev_url = view_url(&applied_search.get(), state_filter.get(), current.saturating_sub(1));
                    let next_url = view_url(&applied_search.get(), state_filter.get(), current + 1);
                    view! {
                        <div class="directory-meta">
                            <span>{count}</span>
                            <span>{freshness}</span>
                        </div>
                        {if data.mints.is_empty() {
                            if filtered {
                                view! {
                                    <div class="empty-state">
                                        <h3>"No mints match"</h3>
                                        <p>"Nothing matches this search and status."</p>
                                        <button type="button" class="btn btn-secondary mt-4" on:click=clear.clone()>"Clear search"</button>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="empty-state">
                                        <h3>"No mints yet"</h3>
                                        <p>"Mints appear once they are announced on Nostr, or when someone donates ecash from one."</p>
                                        <a href="/receive" class="text-link">"Donate ecash"</a>
                                    </div>
                                }.into_any()
                            }
                        } else {
                            view! {
                                <ul class="mint-list" aria-label="Mints">
                                    {data.mints.into_iter().map(|mint| view! { <li><MintRow mint/></li> }).collect_view()}
                                </ul>
                            }.into_any()
                        }}
                        <Show when=move || { pages > 1 }>
                            <nav class="pagination" aria-label="Pages">
                                {if current > 0 {
                                    view! { <a class="btn btn-ghost btn-small" href=prev_url.clone()>"Previous"</a> }.into_any()
                                } else {
                                    view! { <span class="btn btn-ghost btn-small" aria-disabled="true">"Previous"</span> }.into_any()
                                }}
                                <span class="page-number">{format!("{} / {}", current + 1, pages)}</span>
                                {if current + 1 < pages {
                                    view! { <a class="btn btn-ghost btn-small" href=next_url.clone()>"Next"</a> }.into_any()
                                } else {
                                    view! { <span class="btn btn-ghost btn-small" aria-disabled="true">"Next"</span> }.into_any()
                                }}
                            </nav>
                        </Show>
                    }.into_any()
                })}
            </Transition>
        </div>
    }
}

/// One mint: avatar, name over its URL, status. The name's link is stretched
/// over the row, so the row opens the mint; the URL sits above that link as a
/// button, because copying it is what most visitors come for.
#[component]
fn MintRow(mint: MintRead) -> impl IntoView {
    let href = format!("/mints/{}", mint.id);
    let name = if mint.name.trim().is_empty() {
        short_url(&mint.url)
    } else {
        mint.name.clone()
    };
    view! {
        <div class="mint-row">
            <MintAvatar name=name.clone() info=mint.info.clone()/>
            <span class="mint-id">
                <a class="mint-name mint-link" href=href>{name}</a>
                <CopyUrl url=mint.url.clone()/>
            </span>
            <StateBadge state=mint.state/>
        </div>
    }
}

/// The mint's address: shows the short form, copies the full URL, and turns
/// to a check for a moment once it is on the clipboard.
#[component]
pub fn CopyUrl(url: String) -> impl IntoView {
    // None: idle. Some(true): copied. Some(false): the browser refused.
    let copied = RwSignal::new(None::<bool>);
    let title = format!("Copy {url}");
    let short = short_url(&url);
    let on_click = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let url = url.clone();
            leptos::task::spawn_local(async move {
                let result = wasm_bindgen_futures::JsFuture::from(
                    window().navigator().clipboard().write_text(&url),
                )
                .await;
                copied.try_set(Some(result.is_ok()));
                set_timeout(
                    move || {
                        copied.try_set(None);
                    },
                    std::time::Duration::from_millis(1600),
                );
            });
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = &url;
    };
    view! {
        <button
            type="button"
            class="copy-url"
            class:is-copied=move || copied.get() == Some(true)
            title=title
            on:click=on_click
        >
            <span class="sr-only">"Copy "</span>
            <span class="copy-url-text">{short}</span>
            {move || if copied.get() == Some(true) {
                view! { <Check/> }.into_any()
            } else {
                view! { <Copy/> }.into_any()
            }}
        </button>
        <span class="sr-only" role="status">
            {move || match copied.get() {
                Some(true) => "Copied",
                Some(false) => "Couldn't copy. The full URL is in the button's tooltip.",
                None => "",
            }}
        </span>
    }
}

/// Loading rows shaped like the real ones: an avatar, a name over a URL, and
/// a status.
#[component]
fn DirectorySkeleton() -> impl IntoView {
    view! {
        <div class="directory-meta"><span class="skeleton" style="width: 72px; height: 12px;"></span></div>
        <ul class="mint-list" role="status" aria-label="Loading mints">
            {[46, 32, 40, 28, 36, 44].into_iter().map(|w| view! {
                <li><div class="mint-row is-skeleton">
                    <span class="skeleton mint-avatar"></span>
                    <span class="mint-id">
                        <span class="skeleton" style=format!("width: {w}%; height: 14px;")></span>
                        <span class="skeleton" style=format!("width: {}%; height: 12px; margin-top: 6px;", w / 2 + 12)></span>
                    </span>
                    <span class="skeleton" style="width: 72px; height: 20px;"></span>
                </div></li>
            }).collect_view()}
        </ul>
    }
}

/// Status filters as they appear in the URL.
fn status_slug(state: MintState) -> &'static str {
    match state {
        MintState::Ok => "healthy",
        MintState::Warn => "degraded",
        MintState::Error => "failing",
        MintState::Unknown => "unchecked",
    }
}

fn status_from_slug(slug: &str) -> Option<MintState> {
    match slug {
        "healthy" => Some(MintState::Ok),
        "degraded" => Some(MintState::Warn),
        "failing" => Some(MintState::Error),
        "unchecked" => Some(MintState::Unknown),
        _ => None,
    }
}

/// The directory URL for a view; the first page and empty filters are omitted.
fn view_url(search: &str, status: Option<MintState>, page: u64) -> String {
    let mut params = Vec::new();
    if !search.trim().is_empty() {
        params.push(format!("q={}", urlencode(search.trim())));
    }
    if let Some(status) = status {
        params.push(format!("status={}", status_slug(status)));
    }
    if page > 0 {
        params.push(format!("page={}", page + 1));
    }
    if params.is_empty() {
        "/".to_string()
    } else {
        format!("/?{}", params.join("&"))
    }
}

/// Filter changes replace the history entry and keep the scroll position.
fn quiet() -> NavigateOptions {
    NavigateOptions {
        replace: true,
        scroll: false,
        ..Default::default()
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{status_from_slug, status_slug, view_url};
    use crate::types::MintState;

    #[test]
    fn directory_urls_round_trip_and_omit_defaults() {
        assert_eq!(view_url("", None, 0), "/");
        assert_eq!(view_url(" copper ", None, 0), "/?q=copper");
        assert_eq!(
            view_url("paper moon", Some(MintState::Warn), 2),
            "/?q=paper%20moon&status=degraded&page=3"
        );
        for state in [
            MintState::Ok,
            MintState::Warn,
            MintState::Error,
            MintState::Unknown,
        ] {
            assert_eq!(status_from_slug(status_slug(state)), Some(state));
        }
        assert_eq!(status_from_slug("all"), None);
    }
}
