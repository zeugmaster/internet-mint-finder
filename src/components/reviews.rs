use crate::components::icons::{Star, User};
use crate::format::{absolute_time, short_pubkey};
use crate::types::{MintReviews, ReviewContext, ReviewRead};
use leptos::prelude::*;

/// How many reviews show before "Show all".
const FIRST_REVIEWS: usize = 4;

// The mint page lays the reviews out side by side: the overall rating on the
// left; the write-a-review row and the reviews on the right. Reviews are kind
// 38000 events read from relays; a new one is signed in the browser
// (`public/review.js`) and never posted to us. The page loads reviews once,
// not on the poll, so a review being typed survives.

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
            None => view! { <p class="rating-none">"No ratings yet"</p> }.into_any(),
        }}
    }
}

/// The first reviews, a "Show all" for the rest, then the write-a-review row:
/// read what others say before adding your own.
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
        <ReviewCompose context=data.context/>
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

/// The write-a-review row. The form stays in a details element, closed until
/// asked for; `public/review.js` finds its fields by id either way.
#[component]
fn ReviewCompose(context: ReviewContext) -> impl IntoView {
    let can_review = context.mint_pubkey.is_some();
    let relays = context.relays.join(",");
    view! {
        <details class="review-compose">
            <summary>
                <span>
                    <span class="review-compose-title">"Used this mint?"</span>
                    <span class="review-compose-hint">"Rate it with your Nostr key. Reviews go straight to the relays; this server never receives them."</span>
                </span>
                <span class="btn">
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
