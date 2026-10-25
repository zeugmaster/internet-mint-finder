use crate::format::{absolute_time, relative_time, short_pubkey};
use crate::types::{AnnouncementRead, MintReviews};
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

/// Reviews (kind 38000) read from relays, plus a form that signs a new one in
/// the browser with a pasted nsec or NIP-07 (`public/review.js`). Nothing is
/// posted to us.
#[component]
pub fn ReviewsPanel(data: MintReviews) -> impl IntoView {
    let ctx = data.context.clone();
    let can_review = ctx.mint_pubkey.is_some();
    let relays = ctx.relays.join(",");
    let count = data.reviews.len();
    view! {
        <div class="card card-pad">
            <div class="section-head"><h3>"Reviews"</h3><span>{format!("{count} on Nostr")}</span></div>
            {if data.reviews.is_empty() {
                view! { <p class="text-dim">"No reviews yet. Be the first; your Nostr key signs it."</p> }.into_any()
            } else {
                view! {
                    <div class="donation-list">
                        {data.reviews.into_iter().map(|r| {
                            let stars = r.rating.map(|n| format!("{}{}", "★".repeat(n as usize), "☆".repeat(5usize.saturating_sub(n as usize))));
                            view! {
                                <div class="donation-row" style="flex-direction: column; align-items: flex-start; gap: 4px;">
                                    <div class="flex items-baseline gap-2">
                                        {stars.map(|s| view! { <span class="amount" aria-label="rating">{s}</span> })}
                                        <span class="meta mono" title=r.npub.clone()>{short_pubkey(&r.npub)}</span>
                                        <span class="meta" title=absolute_time(r.created_at)>{relative_time(r.created_at)}</span>
                                    </div>
                                    <p>{r.content}</p>
                                </div>
                            }
                        }).collect_view()}
                    </div>
                }.into_any()
            }}
            <div class="field mt-6" data-signer="out">
                <label class="field-label" for="review-nsec">"Sign in to review"</label>
                <div class="flex gap-2">
                    <input id="review-nsec" type="password" class="field-input" placeholder="nsec1…" autocomplete="off" spellcheck="false"/>
                    <button type="button" id="review-signin" class="btn btn-secondary">"Sign in"</button>
                </div>
                <p class="field-hint">"Your key stays in this browser and only signs reviews. A NIP-07 extension (Alby, nos2x) works without signing in."</p>
            </div>
            <p class="field-hint mt-6" data-signer="in" hidden=true>
                "Signed in as " <span id="review-npub" class="mono"></span> " · "
                <button type="button" id="review-signout" class="btn btn-ghost btn-small">"Sign out"</button>
            </p>
            <form id="review-form" class="mt-6"
                data-mint-url=ctx.mint_url.clone()
                data-mint-pubkey=ctx.mint_pubkey.clone().unwrap_or_default()
                data-announcer=ctx.announcer.clone().unwrap_or_default()
                data-relays=relays>
                <h4 class="field-label">"Write a review"</h4>
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
    }
}
