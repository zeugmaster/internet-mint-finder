use crate::api::search_mints;
use crate::components::badge::StateBadge;
use crate::components::icons::{Check, Copy, Star};
use crate::components::mint_avatar::MintAvatar;
use crate::format::{relative_time, short_url};
use crate::hooks::use_poll;
use crate::types::{MintPage, MintRead, MintSort, MintState};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;

const PAGE_SIZE: u64 = 20;

/// Status tabs in order: All, then each health state.
const STATUS_TABS: [(Option<MintState>, &str); 5] = [
    (None, "All"),
    (Some(MintState::Ok), "Healthy"),
    (Some(MintState::Warn), "Degraded"),
    (Some(MintState::Error), "Failing"),
    (Some(MintState::Unknown), "Unchecked"),
];

/// The directory: one row per mint with its rating and plain status. Search,
/// status, sort and page live in the URL (`?q=&status=&sort=&page=`) so Back
/// and shared links keep the view.
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
    let sort = Memo::new(move |_| {
        query
            .get()
            .get("sort")
            .map(|slug| sort_from_slug(&slug))
            .unwrap_or_default()
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
                    let url = view_url(&value, state_filter.get_untracked(), sort.get_untracked(), 0);
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
                sort.get(),
                page.get(),
            )
        },
        |(_, search, state, sort, page)| async move { search_mints(search, state, sort, page).await },
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

    let on_sort = {
        let navigate = navigate.clone();
        move |ev| {
            let sort = sort_from_slug(&event_target_value(&ev));
            navigate(
                &view_url(&applied_search.get_untracked(), state_filter.get_untracked(), sort, 0),
                quiet(),
            );
        }
    };
    // Status tabs are links, so they work before hydration; once hydrated a
    // click replaces the history entry and keeps the scroll position.
    let status_tabs = STATUS_TABS
        .into_iter()
        .map(|(state, label)| {
            let navigate = navigate.clone();
            let href = move || view_url(&applied_search.get(), state, sort.get(), 0);
            view! {
                <a
                    href=href
                    aria-current=move || (state_filter.get() == state).then_some("true")
                    on:click=move |ev| {
                        ev.prevent_default();
                        navigate(&href(), quiet());
                    }
                >
                    {label}
                </a>
            }
        })
        .collect_view();
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
                <div class="directory-filters">
                    <nav class="segmented status-tabs" aria-label="Filter mints by status">{status_tabs}</nav>
                    <label class="sort-field">
                        <span>"Sort"</span>
                        <select class="filter-select" on:change=on_sort prop:value=move || sort_slug(sort.get())>
                            <option value="name">"Name"</option>
                            <option value="top-rated">"Highest rated"</option>
                            <option value="low-rated">"Lowest rated"</option>
                            <option value="most-reviewed">"Most reviews"</option>
                        </select>
                    </label>
                </div>
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
                    let prev_url = view_url(&applied_search.get(), state_filter.get(), sort.get(), current.saturating_sub(1));
                    let next_url = view_url(&applied_search.get(), state_filter.get(), sort.get(), current + 1);
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

/// One mint: avatar, name over its URL, its Nostr rating, status. The name's
/// link is stretched over the row, so the row opens the mint; the URL sits
/// above that link as a button, because copying it is what most visitors
/// come for.
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
            <MintRating avg=mint.review_avg count=mint.review_count/>
            <StateBadge state=mint.state/>
        </div>
    }
}

/// A star, the average and the review count, or nothing for an unrated mint.
#[component]
fn MintRating(avg: Option<f64>, count: u64) -> impl IntoView {
    let reviews = if count == 1 { "review" } else { "reviews" };
    match avg {
        Some(avg) => view! {
            <span class="mint-rating">
                <span class="sr-only">{format!("Rated {avg:.1} out of 5 from {count} {reviews}")}</span>
                <span class="mint-rating-shown" aria-hidden="true">
                    <Star/>
                    <span class="mint-rating-value">{format!("{avg:.1}")}</span>
                    <span class="mint-rating-count">{format!("({count})")}</span>
                </span>
            </span>
        }
        .into_any(),
        None => view! { <span class="mint-rating"></span> }.into_any(),
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
                    <span class="mint-rating"><span class="skeleton" style="width: 48px; height: 14px;"></span></span>
                    <span class="skeleton mint-status-skeleton" style="width: 72px; height: 20px;"></span>
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

/// Sort orders as they appear in the URL; the default (name) is omitted.
fn sort_slug(sort: MintSort) -> &'static str {
    match sort {
        MintSort::Name => "name",
        MintSort::RatingHigh => "top-rated",
        MintSort::RatingLow => "low-rated",
        MintSort::MostReviews => "most-reviewed",
    }
}

fn sort_from_slug(slug: &str) -> MintSort {
    match slug {
        "top-rated" => MintSort::RatingHigh,
        "low-rated" => MintSort::RatingLow,
        "most-reviewed" => MintSort::MostReviews,
        _ => MintSort::Name,
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

/// The directory URL for a view; the first page, the default sort and empty
/// filters are omitted.
fn view_url(search: &str, status: Option<MintState>, sort: MintSort, page: u64) -> String {
    let mut params = Vec::new();
    if !search.trim().is_empty() {
        params.push(format!("q={}", urlencode(search.trim())));
    }
    if let Some(status) = status {
        params.push(format!("status={}", status_slug(status)));
    }
    if sort != MintSort::default() {
        params.push(format!("sort={}", sort_slug(sort)));
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
    use super::{sort_from_slug, sort_slug, status_from_slug, status_slug, view_url};
    use crate::types::{MintSort, MintState};

    #[test]
    fn directory_urls_round_trip_and_omit_defaults() {
        let name = MintSort::Name;
        assert_eq!(view_url("", None, name, 0), "/");
        assert_eq!(view_url(" copper ", None, name, 0), "/?q=copper");
        assert_eq!(
            view_url("paper moon", Some(MintState::Warn), MintSort::RatingHigh, 2),
            "/?q=paper%20moon&status=degraded&sort=top-rated&page=3"
        );
        for sort in [
            MintSort::Name,
            MintSort::RatingHigh,
            MintSort::RatingLow,
            MintSort::MostReviews,
        ] {
            assert_eq!(sort_from_slug(sort_slug(sort)), sort);
        }
        assert_eq!(sort_from_slug("nonsense"), MintSort::Name);
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
