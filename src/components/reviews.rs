use crate::components::icons::{Star, User};
use crate::format::{absolute_time, relative_time, short_pubkey};
use crate::types::{
    AnnouncementRead, MintMetrics, MintRead, MintReviews, MintState, MintUptime, ReviewContext,
    ReviewRead,
};
use leptos::prelude::*;

/// The mint's NIP-87 announcement (kind 38172), if any relay had one.
#[component]
pub fn AnnouncementCard(announcement: Option<AnnouncementRead>) -> impl IntoView {
    match announcement {
        None => view! {
            <div class="card card-pad">
                <h3 class="section-title">"Nostr announcement"</h3>
                <p class="text-dim">"No NIP-87 announcement found on the relays this instance reads."</p>
            </div>
        }
        .into_any(),
        Some(a) => {
            let verified = match a.verified {
                Some(true) => ("badge badge-ok", "signed by the mint"),
                Some(false) => ("badge badge-warn", "signer differs from mint key"),
                None => ("badge badge-muted", "mint key not probed yet"),
            };
            let nuts = (!a.nuts.is_empty()).then(|| a.nuts.join(", "));
            view! {
                <div class="card card-pad">
                    <h3 class="section-title">"Nostr announcement"</h3>
                    <div class="kv-list">
                        <div><span class="text-dim">"Announced by "</span><span class="mono" title=a.npub.clone()>{short_pubkey(&a.npub)}</span>" "<span class=verified.0>{verified.1}</span></div>
                        {a.description.clone().map(|d| view! { <p class="mt-2">{d}</p> })}
                        {a.network.clone().map(|n| view! { <div><span class="text-dim">"Network "</span>{n}</div> })}
                        {nuts.map(|n| view! { <div><span class="text-dim">"NUTs "</span><span class="mono">{n}</span></div> })}
                        <div class="text-dim" title=absolute_time(a.created_at)>{format!("Last updated {}", relative_time(a.created_at))}</div>
                    </div>
                </div>
            }
            .into_any()
        }
    }
}

/// How many reviews show before "Show all".
const FIRST_REVIEWS: usize = 4;

// The reviews section has three parts that the mint page lays out side by
// side: the overall rating and what the audits measured on the left; the
// write-a-review card and the reviews on the right. Reviews are kind 38000
// events read from relays; a new one is signed in the browser
// (`public/review.js`) and never posted to us. The page loads reviews once
// and refreshes only the measured figures, so a review being typed survives.

/// The overall rating: the average, its stars, and how many ratings it rests on.
#[component]
pub fn RatingSummary(reviews: Vec<ReviewRead>) -> impl IntoView {
    let ratings: Vec<u8> = reviews.iter().filter_map(|r| r.rating).collect();
    view! {
        <h3 class="reviews-label">"Overall rating"</h3>
        {match rating_average(&ratings) {
            Some(avg) => view! {
                <div class="rating-big">
                    <span class="rating-value">{format!("{avg:.1}")}</span>
                    <span class="rating-scale">"/5"</span>
                </div>
                <Stars value=avg large=true/>
                <p class="sr-only">{format!("Rated {avg:.1} out of 5")}</p>
                <p class="rating-count">{format!("Based on {}", count_label(ratings.len(), "rating", "ratings"))}</p>
            }.into_any(),
            None => view! {
                <div class="rating-big rating-none">"No ratings yet"</div>
                <Stars value=0.0 large=true/>
            }.into_any(),
        }}
    }
}

/// The three audit figures beside the rating, with what they rest on.
#[component]
pub fn MeasuredSummary(
    mint: Option<MintRead>,
    metrics: Option<MintMetrics>,
    uptime: Option<MintUptime>,
) -> impl IntoView {
    let rows = measured_rows(mint.as_ref(), metrics.as_ref(), uptime.as_ref());
    let caption = measured_caption(mint.as_ref(), metrics.as_ref(), uptime.as_ref());
    view! {
        <h3 class="reviews-label measured-head">"Measured by audits"</h3>
        <dl class="measured">
            {rows.into_iter().map(|row| view! {
                <dt>{row.label}</dt>
                <dd class="measured-value">{row.value}</dd>
                <dd class="measured-bar" aria-hidden="true">
                    <span style=format!("width: {:.1}%", row.ratio.unwrap_or(0.0) * 100.0)></span>
                </dd>
            }).collect_view()}
        </dl>
        <p class="measured-caption">{caption}</p>
    }
}

/// The write-a-review card, then the first reviews and a "Show all" for the rest.
#[component]
pub fn ReviewList(data: MintReviews) -> impl IntoView {
    let mut first = data.reviews;
    let rest = if first.len() > FIRST_REVIEWS {
        first.split_off(FIRST_REVIEWS)
    } else {
        Vec::new()
    };
    let total = first.len() + rest.len();
    view! {
        <ReviewCompose context=data.context/>
        {if total == 0 {
            view! {
                <p class="reviews-empty">"No reviews yet. Reviews are signed with a Nostr key; yours would be the first."</p>
            }.into_any()
        } else {
            view! {
                <ol class="review-list" aria-label="Reviews">
                    {first.into_iter().map(|r| view! { <ReviewItem review=r/> }).collect_view()}
                </ol>
                {(!rest.is_empty()).then(|| view! {
                    // A details element, so "Show all" works before hydration.
                    <details class="reviews-more">
                        <summary class="btn">{format!("Show all {total} reviews")}</summary>
                        <ol class="review-list">
                            {rest.into_iter().map(|r| view! { <ReviewItem review=r/> }).collect_view()}
                        </ol>
                    </details>
                })}
            }.into_any()
        }}
    }
}

/// One review: its stars, the author's key and date, and the text.
#[component]
fn ReviewItem(review: ReviewRead) -> impl IntoView {
    let who = if review.npub.is_empty() {
        short_pubkey(&review.pubkey)
    } else {
        short_pubkey(&review.npub)
    };
    let full = if review.npub.is_empty() { review.pubkey.clone() } else { review.npub.clone() };
    view! {
        <li class="review">
            {review.rating.map(|n| view! {
                <div class="review-rating">
                    <Stars value=f64::from(n)/>
                    <span>{n}</span>
                    <span class="sr-only">" out of 5"</span>
                </div>
            })}
            <div class="review-author">
                <span class="mint-avatar review-avatar" aria-hidden="true"><User/></span>
                <span>
                    <span class="review-who" title=full>{who}</span>
                    <span class="review-date" title=absolute_time(review.created_at)>
                        {review.created_at.format("%b %-d, %Y").to_string()}
                    </span>
                </span>
            </div>
            {(!review.content.trim().is_empty()).then(|| view! { <p class="review-text">{review.content}</p> })}
        </li>
    }
}

/// The write-a-review card. The form stays in a details element, closed until
/// asked for; `public/review.js` finds its fields by id either way.
#[component]
fn ReviewCompose(context: ReviewContext) -> impl IntoView {
    let can_review = context.mint_pubkey.is_some();
    let relays = context.relays.join(",");
    view! {
        <details class="card card-pad review-compose">
            <summary>
                <span>
                    <span class="review-compose-title">"Used this mint?"</span>
                    <span class="review-compose-hint">"Rate it with your Nostr key. Reviews go straight to the relays; this server never receives them."</span>
                </span>
                <span class="btn btn-primary">
                    <span class="when-closed">"Write a review"</span>
                    <span class="when-open">"Close"</span>
                </span>
            </summary>
            <div class="review-compose-body">
                <div class="field" data-signer="out">
                    <label class="field-label" for="review-nsec">"Sign in to review"</label>
                    <div class="flex gap-2">
                        <input id="review-nsec" type="password" class="field-input" placeholder="nsec1…" autocomplete="off" spellcheck="false"/>
                        <button type="button" id="review-signin" class="btn">"Sign in"</button>
                    </div>
                    <p class="field-hint">"Your key stays in this browser and only signs reviews. A NIP-07 extension (Alby, nos2x) works without signing in."</p>
                </div>
                <p class="field-hint" data-signer="in" hidden=true>
                    "Signed in as " <span id="review-npub" class="mono"></span> " · "
                    <button type="button" id="review-signout" class="btn btn-ghost btn-small">"Sign out"</button>
                </p>
                <form id="review-form" class="mt-4"
                    data-mint-url=context.mint_url.clone()
                    data-mint-pubkey=context.mint_pubkey.clone().unwrap_or_default()
                    data-announcer=context.announcer.clone().unwrap_or_default()
                    data-relays=relays>
                    <div class="field">
                        <label class="field-label" for="review-rating">"Rating"</label>
                        <select id="review-rating" name="rating" class="filter-select" disabled=!can_review>
                            <option value="5">"5 · excellent"</option><option value="4">"4 · good"</option>
                            <option value="3" selected>"3 · okay"</option><option value="2">"2 · poor"</option><option value="1">"1 · avoid"</option>
                        </select>
                    </div>
                    <div class="field">
                        <label class="field-label" for="review-content">"Your experience"</label>
                        <textarea id="review-content" name="content" class="field-textarea" rows="3" maxlength="1000" disabled=!can_review></textarea>
                    </div>
                    <button type="submit" class="btn btn-primary" disabled=!can_review>"Sign with Nostr and publish"</button>
                    <p id="review-status" class="field-hint mt-2">
                        {if can_review { "Signed in your browser and sent to the relays; this server never receives it." } else { "The mint's key is not known yet (no NUT-06 probe answered), so a review cannot be addressed to it." }}
                    </p>
                </form>
            </div>
        </details>
    }
}

/// Five stars filled to `value` (0–5). The fill is measured in whole stars
/// plus a fraction of one, so 4.4 fills four stars and 40% of the fifth.
#[component]
fn Stars(value: f64, #[prop(optional)] large: bool) -> impl IntoView {
    let value = value.clamp(0.0, 5.0);
    let full = value.floor();
    let style = format!("--full: {full}; --frac: {:.2}", value - full);
    let stars = || (0..5).map(|_| view! { <Star/> }).collect_view();
    view! {
        <span class=if large { "stars stars-lg" } else { "stars" } aria-hidden="true">
            <span class="stars-track">{stars()}</span>
            <span class="stars-fill" style=style>{stars()}</span>
        </span>
    }
}

/// One measured row: a label, a share for the bar, and the figure shown.
struct MeasuredRow {
    label: &'static str,
    ratio: Option<f64>,
    value: String,
}

/// The three audit figures that matter most when choosing a mint: is it up,
/// do payments through it settle, and do the independent auditors agree.
fn measured_rows(
    mint: Option<&MintRead>,
    metrics: Option<&MintMetrics>,
    uptime: Option<&MintUptime>,
) -> Vec<MeasuredRow> {
    let percent = |ratio: Option<f64>| {
        ratio
            .map(|r| format!("{:.0}%", r * 100.0))
            .unwrap_or_else(|| "—".to_string())
    };
    let up = uptime
        .and_then(|u| u.uptime_percent.map(|p| p / 100.0))
        .or_else(|| uptime.and_then(|u| (u.checks > 0).then(|| u.successes as f64 / u.checks as f64)));
    let settled = metrics.and_then(|m| m.success_rate_24h().or_else(|| m.success_rate()));
    let marks = mint.map(|m| m.auditor_marks.as_slice()).unwrap_or_default();
    let clean = marks.iter().filter(|m| m.state == MintState::Ok).count();
    let verdicts = (!marks.is_empty()).then(|| clean as f64 / marks.len() as f64);
    vec![
        MeasuredRow { label: "Uptime", ratio: up, value: percent(up) },
        MeasuredRow { label: "Payments settled", ratio: settled, value: percent(settled) },
        MeasuredRow {
            label: "Auditors with no failures",
            ratio: verdicts,
            value: if marks.is_empty() { "—".to_string() } else { format!("{clean}/{}", marks.len()) },
        },
    ]
}

/// What the three figures rest on, so a reader can judge them.
fn measured_caption(
    mint: Option<&MintRead>,
    metrics: Option<&MintMetrics>,
    uptime: Option<&MintUptime>,
) -> String {
    let mut parts = Vec::new();
    if let Some(u) = uptime.filter(|u| u.checks > 0) {
        parts.push(count_label(u.checks as usize, "health check", "health checks"));
    }
    if let Some(m) = metrics {
        if m.attempts_24h > 0 {
            parts.push(count_label(m.attempts_24h as usize, "test payment", "test payments"));
        } else if m.attempts > 0 {
            parts.push(format!("{} (all time)", count_label(m.attempts as usize, "test payment", "test payments")));
        }
    }
    if let Some(n) = mint.map(|m| m.auditor_marks.len()).filter(|n| *n > 0) {
        parts.push(count_label(n, "auditor", "auditors"));
    }
    if parts.is_empty() {
        "Nothing measured in the last 24 hours yet.".to_string()
    } else {
        format!("Last 24 hours: {}.", parts.join(", "))
    }
}

/// Mean of the given 1–5 ratings, or None when there are none.
fn rating_average(ratings: &[u8]) -> Option<f64> {
    (!ratings.is_empty())
        .then(|| ratings.iter().map(|&r| f64::from(r)).sum::<f64>() / ratings.len() as f64)
}

fn count_label(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::{count_label, rating_average};

    #[test]
    fn average_and_count_labels() {
        assert_eq!(rating_average(&[]), None);
        assert_eq!(rating_average(&[5, 4, 4]), Some(13.0 / 3.0));
        assert_eq!(count_label(1, "rating", "ratings"), "1 rating");
        assert_eq!(count_label(7, "rating", "ratings"), "7 ratings");
    }
}
