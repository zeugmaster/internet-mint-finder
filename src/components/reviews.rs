use crate::components::brand::FoilStar;
use crate::components::icons::Star;
use crate::format::{absolute_time, format_count, short_npub};
use crate::types::{MintReviews, ReviewContext, ReviewRead};
use leptos::prelude::*;
// Safari's autocorrect switch, which the view macro doesn't know by name.
use leptos::tachys::html::attribute::custom::custom_attribute;

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
    let MintReviews {
        reviews,
        total,
        rating_count,
        rating_avg,
        context,
    } = data;
    if reviews.is_empty() {
        return view! {
            <div class="card reviews-card"><ReviewCompose context empty=true/></div>
        }
        .into_any();
    }
    view! {
        <div class="card reviews-card">
            <div class="reviews">
                <div class="reviews-summary"><RatingSummary avg=rating_avg count=rating_count/></div>
                <div class="reviews-main"><ReviewList reviews total/></div>
            </div>
            <ReviewCompose context/>
        </div>
    }
    .into_any()
}

/// The overall rating: the average, its stars, and how many ratings it rests
/// on, over every stored review, as the directory row counts them.
#[component]
fn RatingSummary(avg: Option<f64>, count: u64) -> impl IntoView {
    view! {
        {match avg {
            Some(avg) => view! {
                <div class="rating-big">
                    <span class="rating-value">{format!("{avg:.1}")}</span>
                    <span class="rating-scale">"/5"</span>
                </div>
                <Stars value=avg large=true/>
                <p class="sr-only">{format!("Rated {avg:.1} out of 5")}</p>
                <p class="rating-count">{format!("Based on {}", count_label(count, "rating", "ratings"))}</p>
            }.into_any(),
            // Reviews that carry no star rating.
            None => view! { <p class="rating-count">"No star ratings yet"</p> }.into_any(),
        }}
    }
}

/// The first reviews and a "Show N more" for the rest of those loaded, the
/// latest `REVIEWS_LOADED`; past that a line under them says how many there
/// are in all. The write-a-review row follows across the card's foot: read
/// what others say before adding your own.
#[component]
fn ReviewList(reviews: Vec<ReviewRead>, total: u64) -> impl IntoView {
    let mut first = reviews;
    let rest = if first.len() > FIRST_REVIEWS {
        first.split_off(FIRST_REVIEWS)
    } else {
        Vec::new()
    };
    let loaded = (first.len() + rest.len()) as u64;
    let more = rest.len() as u64;
    view! {
        <ol class="review-list" aria-label="Reviews">
            {first.into_iter().map(|r| view! { <ReviewItem review=r/> }).collect_view()}
        </ol>
        {(!rest.is_empty()).then(|| view! {
            // A details element, so "Show more" works before hydration.
            <details class="reviews-more">
                <summary class="btn">{format!("Show {} more", format_count(more))}</summary>
                <ol class="review-list">
                    {rest.into_iter().map(|r| view! { <ReviewItem review=r/> }).collect_view()}
                </ol>
                {(total > loaded).then(|| view! {
                    <p class="reviews-capped">
                        {format!("The latest {} of {} reviews.", format_count(loaded), format_count(total))}
                    </p>
                })}
            </details>
        })}
    }
}

/// One review as a line: its stars, the author's npub cut in the middle (the
/// whole key on hover), and the date at the right, with the text under it
/// when there is any, its line breaks kept. A rating without words is that
/// line alone. A long review shows its first lines and a "Read more" (a
/// details element, so it works before hydration).
#[component]
fn ReviewItem(review: ReviewRead) -> impl IntoView {
    let long = is_long(&review.content);
    let full = if review.npub.is_empty() {
        review.pubkey.clone()
    } else {
        review.npub.clone()
    };
    let who = short_npub(&full);
    view! {
        <li class="review">
            {review.rating.map(|n| view! {
                <span class="review-rating">
                    <Stars value=f64::from(n)/>
                    <span class="sr-only">{format!("{n} out of 5, ")}</span>
                </span>
            })}
            <span class="review-who" title=full>{who}</span>
            <span class="review-date" title=absolute_time(review.created_at)>
                {review.created_at.format("%b %-d, %Y").to_string()}
            </span>
            {(!review.content.trim().is_empty()).then(|| view! {
                <p class="review-text" class:is-long=long dir="auto">{review.content.trim().to_string()}</p>
            })}
            {long.then(|| view! {
                <details class="review-read-more"><summary class="text-link">"Read more"</summary></details>
            })}
        </li>
    }
}

/// The write-a-review row: one line of text and a button. The form stays in a
/// details element, closed until asked for; `public/review.js` finds its
/// fields by id either way. With no reviews yet the line says so itself. A
/// review needs the mint's public key, so a mint without one gets a line and
/// no form: nothing asks for a Nostr key that couldn't be used.
#[component]
fn ReviewCompose(context: ReviewContext, #[prop(optional)] empty: bool) -> impl IntoView {
    if context.mint_pubkey.is_none() {
        let line = if empty {
            "No reviews yet. They open once this mint shares its public key."
        } else {
            "New reviews open once this mint shares its public key."
        };
        return view! {
            <p class=if empty { "review-compose is-empty" } else { "review-compose" }>
                <span class="review-compose-line">{line}</span>
            </p>
        }
        .into_any();
    }
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
                <div class="signer" data-signer="out">
                    // Shown by review.js once it finds a Nostr extension
                    // (NIP-07): one click, and the key never leaves it.
                    <div class="signer-extension" data-extension="on" hidden=true>
                        <button type="button" id="review-extension" class="btn">"Sign in with your Nostr extension"</button>
                        <p class="field-hint">"It asks you to approve, and your key stays in the extension."</p>
                        <p class="signer-or" aria-hidden="true">"or"</p>
                    </div>
                    <div class="field">
                        <label class="field-label" for="review-nsec">"Your Nostr key (nsec)"</label>
                        <div class="flex gap-2">
                            <input id="review-nsec" type="password" class="field-input" placeholder="nsec1…" autocomplete="off" autocapitalize="none" {..custom_attribute("autocorrect", "off")} spellcheck="false"/>
                            <button type="button" id="review-signin" class="btn">"Sign in"</button>
                        </div>
                        // A failed sign-in says so here, by the field, not at
                        // the form's foot under the on-screen keyboard.
                        <p id="review-signin-error" class="field-hint is-error" role="alert" hidden=true></p>
                        <p class="field-hint">
                            "It stays in this browser and is only used to sign reviews."
                            <span data-extension="off">" With a Nostr extension like Alby or nos2x, you won't need to paste it."</span>
                        </p>
                    </div>
                </div>
                <p class="field-hint signer-in" data-signer="in" hidden=true>
                    <span>
                        "Signed in as " <span id="review-npub" class="mono"></span>
                        <span id="review-via"></span>
                    </span>
                    <button type="button" id="review-signout" class="btn btn-ghost btn-small">"Sign out"</button>
                </p>
                <form id="review-form" class="mt-4"
                    data-mint-url=context.mint_url.clone()
                    data-mint-pubkey=context.mint_pubkey.clone().unwrap_or_default()
                    data-announcer=context.announcer.clone().unwrap_or_default()
                    data-relays=relays>
                    <StarInput/>
                    <div class="field">
                        <label class="field-label" for="review-content">"Your experience"</label>
                        <textarea id="review-content" name="content" class="field-textarea" rows="3" maxlength="1000"></textarea>
                    </div>
                    <div class="review-submit">
                        <p id="review-status" class="field-hint" aria-live="polite">"Posted to Nostr, not to this site."</p>
                        <button type="submit" class="btn btn-primary">"Post review"</button>
                    </div>
                </form>
            </div>
        </details>
    }
    .into_any()
}

/// The rating as five stars to pick from: hovering fills the stars up to the
/// pointer, clicking keeps them. Underneath it is a plain radio group named
/// `rating` (values 1–5), so arrow keys, screen readers, `required` and
/// `public/review.js` all work, and the stars work before hydration.
#[component]
fn StarInput() -> impl IntoView {
    const WORDS: [&str; 5] = ["avoid", "poor", "okay", "good", "excellent"];
    view! {
        <fieldset class="field star-field">
            <legend class="field-label">"Rating"</legend>
            <div class="star-input">
                {(1..=5u8).map(|n| {
                    let id = format!("review-rating-{n}");
                    let word = WORDS[usize::from(n - 1)];
                    let name = format!("{n} {}, {word}", if n == 1 { "star" } else { "stars" });
                    view! {
                        <input type="radio" name="rating" id=id.clone() value=n.to_string() required=true/>
                        // The grey star, with the metal one over it shown once
                        // the star is picked or previewed.
                        <label for=id title=word>
                            <span class="star-pick"><Star/><FoilStar/></span>
                            <span class="sr-only">{name}</span>
                        </label>
                    }
                }).collect_view()}
            </div>
        </fieldset>
    }
}

/// Five stars filled to `value` (0–5): grey stars under a row of metal ones
/// clipped to the rating. The fill is measured in whole stars plus a fraction
/// of one, so 4.4 fills four stars and 40% of the fifth.
#[component]
pub fn Stars(value: f64, #[prop(optional)] large: bool) -> impl IntoView {
    let value = value.clamp(0.0, 5.0);
    let full = value.floor();
    let style = format!("--full: {full}; --frac: {:.2}", value - full);
    let track = (0..5).map(|_| view! { <Star/> }).collect_view();
    let fill = (0..5).map(|_| view! { <FoilStar/> }).collect_view();
    view! {
        <span class=if large { "stars stars-lg" } else { "stars" } aria-hidden="true">
            <span class="stars-track">{track}</span>
            <span class="stars-fill" style=style>{fill}</span>
        </span>
    }
}

fn count_label(n: u64, one: &str, many: &str) -> String {
    format!("{} {}", format_count(n), if n == 1 { one } else { many })
}

/// More than about eight lines of the review column: over 480 characters or
/// eight lines of its own.
fn is_long(text: &str) -> bool {
    let text = text.trim();
    text.chars().count() > 480 || text.lines().count() > 8
}

#[cfg(test)]
mod tests {
    use super::{count_label, is_long};

    #[test]
    fn count_labels_and_long_reviews() {
        assert_eq!(count_label(1, "rating", "ratings"), "1 rating");
        assert_eq!(count_label(7, "rating", "ratings"), "7 ratings");
        assert_eq!(
            count_label(1284, "rating", "ratings"),
            "1\u{202F}284 ratings"
        );
        assert!(!is_long("Fast and cheap."));
        assert!(is_long(&"word ".repeat(100)));
        assert!(is_long(&"line\n".repeat(9)));
    }
}
