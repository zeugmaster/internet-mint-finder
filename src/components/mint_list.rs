use crate::api::search_mints;
use crate::components::icons::{Check, Copy, Cross};
use crate::components::mint_avatar::MintAvatar;
use crate::components::reviews::Stars;
use crate::format::{format_count, relative_time, relative_time_from, short_url};
use crate::hooks::use_poll;
use crate::types::{MintPage, MintRead, MintSort};
use chrono::{DateTime, Utc};
use leptos::prelude::*;
// Safari's autocorrect switch, which the view macro doesn't know by name.
use leptos::tachys::html::attribute::custom::custom_attribute;
use leptos_router::hooks::{use_navigate, use_query_map};
use leptos_router::NavigateOptions;

const PAGE_SIZE: u64 = 20;

/// What the directory remembers while a mint is open, so going back, or up the
/// breadcrumb, lands on the same filters at the same place in the list.
#[derive(Clone, Copy)]
pub struct DirectoryMemory {
    /// The directory's URL when last seen; the mint page's breadcrumb uses it.
    pub view: RwSignal<String>,
    /// How far down the directory the reader has scrolled.
    pub scroll: StoredValue<f64>,
    /// Where to land on coming back: set when a mint opens, used once.
    pub return_to: StoredValue<Option<f64>>,
}

impl DirectoryMemory {
    pub fn new() -> Self {
        Self {
            view: RwSignal::new("/".to_string()),
            scroll: StoredValue::new(0.0),
            return_to: StoredValue::new(None),
        }
    }
}

impl Default for DirectoryMemory {
    fn default() -> Self {
        Self::new()
    }
}

/// The directory: one row per mint with its rating. Search,
/// sort and page live in the URL (`?q=&sort=&page=`) so Back
/// and shared links keep the view.
#[component]
pub fn MintList() -> impl IntoView {
    let tick = use_poll(15);
    let query = use_query_map();
    let navigate = use_navigate();

    let applied_search = Memo::new(move |_| query.get().get("q").unwrap_or_default());
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
                    let url = view_url(&value, sort.get_untracked(), 0);
                    navigate(&url, quiet());
                },
                std::time::Duration::from_millis(250),
            ) {
                on_cleanup(move || handle.clear());
            }
        }
    });

    let mints = Resource::new(
        move || (tick.get(), applied_search.get(), sort.get(), page.get()),
        |(_, search, sort, page)| async move { search_mints(search, sort, page).await },
    );

    // A failed background refresh keeps the last good list on screen.
    let last_good = RwSignal::new(None::<MintPage>);
    let updated_at = RwSignal::new(None::<DateTime<Utc>>);
    // The view the list on screen answers. Until the list for a new search,
    // filter, sort or page arrives, the old one dims (`.is-updating`); the
    // refresh keeps the view, so it never dims.
    let view = Memo::new(move |_| (applied_search.get(), sort.get(), page.get()));
    let shown_view = RwSignal::new(None::<(String, MintSort, u64)>);
    let updating = move || shown_view.get().is_some_and(|shown| shown != view.get());

    // Back on the view that was open, the list takes the reader to where they
    // left it. The browser can't do this itself: the page is shorter than
    // that until the list arrives.
    let memory = expect_context::<DirectoryMemory>();
    let here = view_url(
        &applied_search.get_untracked(),
        sort.get_untracked(),
        page.get_untracked(),
    );
    let same_view = memory.view.get_untracked() == here;
    let return_to = StoredValue::new(memory.return_to.get_value().filter(|_| same_view));
    memory.return_to.set_value(None);
    Effect::new(move |_| {
        memory
            .view
            .set(view_url(&applied_search.get(), sort.get(), page.get()));
    });

    Effect::new(move |_| {
        if let Some(Ok(data)) = mints.get() {
            last_good.set(Some(data));
            updated_at.set(Some(Utc::now()));
            shown_view.set(Some(view.get_untracked()));
            #[cfg(feature = "hydrate")]
            if let Some(y) = return_to.get_value() {
                return_to.set_value(None);
                // Two frames, so the list is in the page before it scrolls.
                request_animation_frame(move || {
                    request_animation_frame(move || window().scroll_to_with_x_and_y(0.0, y));
                });
            }
        }
    });
    #[cfg(not(feature = "hydrate"))]
    let _ = return_to;
    let freshness = move || {
        tick.track();
        let stale = matches!(mints.get(), Some(Err(_)));
        match (updated_at.get(), stale) {
            (Some(at), false) => format!("Updated {}", relative_time(at)),
            (Some(at), true) => {
                format!(
                    "Couldn't refresh. Showing results from {}, retrying.",
                    relative_time_from(at)
                )
            }
            (None, _) => String::new(),
        }
    };

    let on_sort = {
        let navigate = navigate.clone();
        move |ev| {
            let sort = sort_from_slug(&event_target_value(&ev));
            navigate(&view_url(&applied_search.get_untracked(), sort, 0), quiet());
        }
    };
    let clear = {
        let navigate = navigate.clone();
        move |_| {
            search.set(String::new());
            navigate("/", quiet());
        }
    };
    // Once hydrated, Enter applies the search at once instead of reloading,
    // and puts the keyboard away so the results aren't under it.
    let search_input = NodeRef::<leptos::html::Input>::new();
    let on_search = {
        let navigate = navigate.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            if let Some(input) = search_input.get_untracked() {
                let _ = input.blur();
            }
            let url = view_url(&search.get_untracked(), sort.get_untracked(), 0);
            navigate(&url, quiet());
        }
    };

    view! {
        <div class="directory">
            <div class="directory-tools">
                // A plain GET form, so search works before (and without)
                // hydration; the hidden fields keep the status and sort.
                <form class="search-field" role="search" action="/" method="get" on:submit=on_search>
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="10.5" cy="10.5" r="6.5"/><path d="m16 16 4 4"/></svg>
                    // The attribute carries the search into the server's HTML; the
                    // property keeps it live once hydrated.
                    // Mint names and URLs, not prose: no capitals, corrections
                    // or spelling marks, and the keyboard's key says Search.
                    <input type="search" name="q" aria-label="Search mints by name or URL" placeholder="Search name or URL"
                        node_ref=search_input
                        autocapitalize="none" {..custom_attribute("autocorrect", "off")} spellcheck="false" enterkeyhint="search"
                        value=search.get_untracked()
                        prop:value=move || search.get() on:input=move |ev| search.set(event_target_value(&ev))/>
                    {move || (sort.get() != MintSort::default()).then(|| view! { <input type="hidden" name="sort" value=sort_slug(sort.get())/> })}
                </form>
                <div class="directory-filters">
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
                <div id="mints" class="directory-results arrives" class:is-updating=updating>
                {move || mints.get().map(|result| {
                    let data = match result {
                        Ok(data) => Some(data),
                        Err(_) => last_good.get(),
                    };
                    let Some(data) = data else {
                        return view! {
                            <div class="card card-pad empty-state" role="alert">
                                <h2>"Couldn't load mints"</h2>
                                <p>"Check your connection. This retries every 15 seconds."</p>
                                <button type="button" class="btn btn-secondary mt-4" on:click=move |_| mints.refetch()>"Try again"</button>
                            </div>
                        }.into_any();
                    };
                    let total = data.total;
                    let current = page.get();
                    let pages = total.div_ceil(PAGE_SIZE).max(1);
                    let filtered = !applied_search.get().is_empty();
                    // A page number past the list's end, from an old link.
                    let past_end = data.mints.is_empty() && total > 0;
                    let first_url = view_url(&applied_search.get(), sort.get(), 0);
                    let count = format!("{} {}", format_count(total), if total == 1 { "mint" } else { "mints" });
                    // Previous and Next land on the list (#mints), where the
                    // reader was, rather than on the page's head.
                    let prev_url = format!("{}#mints", view_url(&applied_search.get(), sort.get(), current.saturating_sub(1)));
                    let next_url = format!("{}#mints", view_url(&applied_search.get(), sort.get(), current + 1));
                    view! {
                        <div class="directory-meta">
                            // Read out when a search or filter changes the count.
                            <span role="status">{count}</span>
                            <span>{freshness}</span>
                        </div>
                        {if past_end {
                            view! {
                                <div class="card card-pad empty-state">
                                    <h2>"No mints on this page"</h2>
                                    <p>{format!("This list ends at page {pages}.")}</p>
                                    <a class="btn btn-secondary mt-4" href=first_url>"Go to the first page"</a>
                                </div>
                            }.into_any()
                        } else if data.mints.is_empty() {
                            if filtered {
                                view! {
                                    <div class="card card-pad empty-state">
                                        <h2>"No mints match"</h2>
                                        <p>"Try another name, or clear the filters."</p>
                                        <button type="button" class="btn btn-secondary mt-4" on:click=clear.clone()>"Clear filters"</button>
                                    </div>
                                }.into_any()
                            } else {
                                view! {
                                    <div class="card card-pad empty-state">
                                        <h2>"No mints yet"</h2>
                                        <p>"Mints show up here once they're listed on Nostr."</p>
                                    </div>
                                }.into_any()
                            }
                        } else {
                            view! {
                                <ul class="card mint-list" aria-label="Mints">
                                    {data.mints.into_iter().map(|mint| view! { <li><MintRow mint/></li> }).collect_view()}
                                </ul>
                            }.into_any()
                        }}
                        <Show when=move || { pages > 1 && !past_end }>
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
                </div>
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
    let rating_id = format!("mint-{}-rating", mint.id);
    let described_by = mint.review_avg.map(|_| rating_id.clone());
    view! {
        <div class="mint-row">
            <MintAvatar name=name.clone() info=mint.info.clone()/>
            <span class="mint-id">
                <a class="mint-name mint-link" href=href dir="auto" aria-describedby=described_by>{name}</a>
                <CopyUrl url=mint.url.clone()/>
            </span>
            <MintRating avg=mint.review_avg count=mint.review_count id=rating_id/>
        </div>
    }
}

/// Five stars filled to the average and the review count in words, or
/// nothing for an unrated mint. The average itself is on the mint page.
#[component]
fn MintRating(avg: Option<f64>, count: u64, id: String) -> impl IntoView {
    let reviews = if count == 1 { "review" } else { "reviews" };
    match avg {
        Some(avg) => view! {
            <span class="mint-rating">
                <span class="sr-only" id=id>{format!("Rated {avg:.1} out of 5 from {} {reviews}", format_count(count))}</span>
                <span class="mint-rating-shown" aria-hidden="true">
                    <Stars value=avg/>
                    <span class="mint-rating-count">{format!("{} {reviews}", format_count(count))}</span>
                </span>
            </span>
        }
        .into_any(),
        None => view! { <span class="mint-rating"></span> }.into_any(),
    }
}

/// The mint's address: shows the short form, copies the full URL.
#[component]
pub fn CopyUrl(url: String) -> impl IntoView {
    let shown = short_url(&url);
    view! { <CopyText text=url shown/> }
}

/// A value as a quiet button: shows `shown`, copies the full `text`, and
/// turns to a check for a moment once it is on the clipboard, or says
/// "Couldn't copy" with a cross when the browser refuses.
#[component]
pub fn CopyText(text: String, shown: String) -> impl IntoView {
    // None: idle. Some(true): copied. Some(false): the browser refused.
    let copied = RwSignal::new(None::<bool>);
    // Set with the first answer, so the icon animates only when it changes,
    // not when the row is drawn.
    let used = RwSignal::new(false);
    let title = format!("Copy {text}");
    // Named as a whole: the head and tail are separate boxes, which a screen
    // reader would otherwise read with a space between them.
    let name = format!("Copy {shown}");
    // The pending return to idle; a second copy clears it and starts its own,
    // so the check holds its full moment instead of ending early.
    #[cfg(feature = "hydrate")]
    let timer = StoredValue::new(None::<TimeoutHandle>);
    let on_click = move |_| {
        #[cfg(feature = "hydrate")]
        {
            let text = text.clone();
            leptos::task::spawn_local(async move {
                let result = wasm_bindgen_futures::JsFuture::from(
                    window().navigator().clipboard().write_text(&text),
                )
                .await;
                used.try_set(true);
                copied.try_set(Some(result.is_ok()));
                if let Some(pending) = timer.try_get_value().flatten() {
                    pending.clear();
                }
                let handle = set_timeout_with_handle(
                    move || {
                        copied.try_set(None);
                    },
                    std::time::Duration::from_millis(1600),
                )
                .ok();
                timer.try_set_value(handle);
            });
        }
        #[cfg(not(feature = "hydrate"))]
        let _ = &text;
    };
    view! {
        <button
            type="button"
            class="copy-url"
            class:is-copied=move || copied.get() == Some(true)
            class:is-failed=move || copied.get() == Some(false)
            class:is-used=move || used.get()
            title=title
            aria-label=move || if copied.get() == Some(false) { "Couldn't copy".to_string() } else { name.clone() }
            on:click=on_click
        >
            <span class="copy-url-text">
                {move || if copied.get() == Some(false) {
                    view! { <span class="copy-url-head">"Couldn't copy"</span> }.into_any()
                } else {
                    let (head, tail) = split_tail(&shown);
                    view! {
                        <span class="copy-url-head">{head}</span>
                        <span class="copy-url-tail">{tail}</span>
                    }.into_any()
                }}
            </span>
            {move || match copied.get() {
                Some(true) => view! { <Check/> }.into_any(),
                Some(false) => view! { <Cross/> }.into_any(),
                None => view! { <Copy/> }.into_any(),
            }}
        </button>
        <span class="sr-only" role="status">
            {move || match copied.get() {
                Some(true) => "Copied",
                Some(false) => "Couldn't copy. Your browser blocked the clipboard.",
                None => "",
            }}
        </span>
    }
}

/// A value and its last six characters, which stay in view when the rest is
/// cut short ("legend.lnbits.c…BiQGoC").
fn split_tail(shown: &str) -> (String, String) {
    let chars: Vec<char> = shown.chars().collect();
    let at = chars.len().saturating_sub(6);
    (chars[..at].iter().collect(), chars[at..].iter().collect())
}

/// Loading rows shaped like the real ones: an avatar, a name over a URL, and
/// a status.
#[component]
fn DirectorySkeleton() -> impl IntoView {
    view! {
        <div class="directory-meta"><span class="skeleton" style="width: 72px; height: 12px;"></span></div>
        <ul class="card mint-list" role="status" aria-label="Loading mints">
            {[46, 32, 40, 28, 36, 44].into_iter().map(|w| view! {
                <li><div class="mint-row is-skeleton">
                    <span class="skeleton mint-avatar"></span>
                    <span class="mint-id">
                        <span class="skeleton" style=format!("width: {w}%; height: 14px;")></span>
                        <span class="skeleton" style=format!("width: {}%; height: 12px; margin-top: 6px;", w / 2 + 12)></span>
                    </span>
                    <span class="mint-rating"><span class="skeleton" style="width: 78px; height: 14px;"></span></span>
                    <span class="skeleton mint-status-skeleton" style="width: 72px; height: 20px;"></span>
                </div></li>
            }).collect_view()}
        </ul>
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

/// The directory URL for a view; the first page, the default sort and empty
/// filters are omitted.
fn view_url(search: &str, sort: MintSort, page: u64) -> String {
    let mut params = Vec::new();
    if !search.trim().is_empty() {
        params.push(format!("q={}", urlencode(search.trim())));
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
    use super::{sort_from_slug, sort_slug, split_tail, view_url};
    use crate::types::MintSort;

    #[test]
    fn the_tail_keeps_the_last_six_characters() {
        assert_eq!(
            split_tail("legend.lnbits.com/cashu/api/v1/4gr9Xcmz3XEkUNwiBiQGoC"),
            (
                "legend.lnbits.com/cashu/api/v1/4gr9Xcmz3XEkUNwi".into(),
                "BiQGoC".into()
            )
        );
        assert_eq!(split_tail("mint.ü"), (String::new(), "mint.ü".into()));
    }

    #[test]
    fn directory_urls_round_trip_and_omit_defaults() {
        let name = MintSort::Name;
        assert_eq!(view_url("", name, 0), "/");
        assert_eq!(view_url(" copper ", name, 0), "/?q=copper");
        assert_eq!(
            view_url("paper moon", MintSort::RatingHigh, 2),
            "/?q=paper%20moon&sort=top-rated&page=3"
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
    }
}
