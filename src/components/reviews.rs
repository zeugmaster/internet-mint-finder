use crate::components::icons::{Star, User};
use crate::format::{absolute_time, short_pubkey};
use crate::types::{MintReviews, ReviewContext, ReviewRead};
use leptos::prelude::*;

/// How many reviews show before "Show all".
const FIRST_REVIEWS: usize = 4;

// Reviews are kind 38000 events read from relays; a new one is signed in the
// browser (`public/review.js`) and never posted to us. The mint page loads
// reviews once, not on the poll, so a review being typed survives. The whole
// section uses three text styles: its heading, body text, and small grey meta
// (plus the average as the one large figure).

/// The reviews section body, in one card like the rest of the mint page.
/// With reviews: the average on the left, the reviews on the right, and the
/// write-a-review row across the card's foot. Without: that row alone, so
/// nothing says "none yet" twice.
#[component]
pub fn ReviewsBlock(data: MintReviews) -> impl IntoView {
    let MintReviews { reviews, context } = data;
    if reviews.is_empty() {
        return view! {
            <div class="card reviews-card"><ReviewCompose context empty=true/></div>
        }
        .into_any();
    }
    let for_summary = reviews.clone();
    view! {
        <div class="card reviews-card">
            <div class="reviews">
                <div class="reviews-summary"><RatingSummary reviews=for_summary/></div>
                <div class="reviews-main"><ReviewList reviews/></div>
            </div>
            <ReviewCompose context/>
        </div>
    }
    .into_any()
}

/// The overall rating: the average, its stars, and how many ratings it rests on.
#[component]
fn RatingSummary(reviews: Vec<ReviewRead>) -> impl IntoView {
    let ratings: Vec<u8> = reviews.iter().filter_map(|r| r.rating).collect();
    view! {
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
            // Reviews that carry no star rating.
            None => view! { <p class="rating-count">"No star ratings yet"</p> }.into_any(),
        }}
    }
}

/// The first reviews and a "Show all" for the rest. The write-a-review row
/// follows across the card's foot: read what others say before adding your
/// own.
#[component]
fn ReviewList(reviews: Vec<ReviewRead>) -> impl IntoView {
    let mut first = reviews;
    let rest = if first.len() > FIRST_REVIEWS {
        first.split_off(FIRST_REVIEWS)
    } else {
        Vec::new()
    };
    let total = first.len() + rest.len();
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
            {(!review.content.trim().is_empty()).then(|| view! { <p class="review-text" dir="auto">{review.content}</p> })}
        </li>
    }
}

/// The write-a-review row: one line of text and a button. The form stays in a
/// details element, closed until asked for; `public/review.js` finds its
/// fields by id either way. With no reviews yet the line says so itself.
#[component]
fn ReviewCompose(context: ReviewContext, #[prop(optional)] empty: bool) -> impl IntoView {
    let can_review = context.mint_pubkey.is_some();
    let relays = context.relays.join(",");
    let line = if empty {
        "No reviews yet. Used this mint? Rate it with your Nostr key."
    } else {
        "Used this mint? Rate it with your Nostr key."
    };
    view! {
        <details class=if empty { "review-compose is-empty" } else { "review-compose" }>
            <summary>
                <span class="review-compose-line">{line}</span>
                <span class="btn">
                    <span class="when-closed">"Write a review"</span>
                    <span class="when-open">"Close"</span>
                </span>
            </summary>
            <div class="review-compose-body">
                <div class="field" data-signer="out">
                    <label class="field-label" for="review-nsec">"Your Nostr key (nsec)"</label>
                    <div class="flex gap-2">
                        <input id="review-nsec" type="password" class="field-input" placeholder="nsec1…" autocomplete="off" spellcheck="false"/>
                        <button type="button" id="review-signin" class="btn">"Sign in"</button>
                    </div>
                    <p class="field-hint">"It stays in this browser and is only used to sign reviews. With a Nostr extension like Alby or nos2x, you can skip this."</p>
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
                    <StarInput disabled=!can_review/>
                    <div class="field">
                        <label class="field-label" for="review-content">"Your experience"</label>
                        <textarea id="review-content" name="content" class="field-textarea" rows="3" maxlength="1000" disabled=!can_review></textarea>
                    </div>
                    <button type="submit" class="btn btn-primary" disabled=!can_review>"Post review"</button>
                    <p id="review-status" class="field-hint mt-2">
                        {if can_review { "Posted to Nostr, not to this site." } else { "Reviews open once this mint shares its public key, which it hasn't yet." }}
                    </p>
                </form>
            </div>
        </details>
    }
}

/// The rating as five stars to pick from: hovering fills the stars up to the
/// pointer, clicking keeps them. Underneath it is a plain radio group named
/// `rating` (values 1–5), so arrow keys, screen readers, `required` and
/// `public/review.js` all work, and the stars work before hydration.
#[component]
fn StarInput(disabled: bool) -> impl IntoView {
    const WORDS: [&str; 5] = ["avoid", "poor", "okay", "good", "excellent"];
    view! {
        <fieldset class="field star-field" disabled=disabled>
            <legend class="field-label">"Rating"</legend>
            <div class="star-input">
                {(1..=5u8).map(|n| {
                    let id = format!("review-rating-{n}");
                    let word = WORDS[usize::from(n - 1)];
                    let name = format!("{n} {}, {word}", if n == 1 { "star" } else { "stars" });
                    view! {
                        <input type="radio" name="rating" id=id.clone() value=n.to_string() required=true/>
                        <label for=id title=word>
                            <Star/>
                            <span class="sr-only">{name}</span>
                        </label>
                    }
                }).collect_view()}
            </div>
        </fieldset>
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
