use crate::api::{get_payment_request, poll_donations, ReceiveToken};
use crate::components::button::{Button, ButtonFormat};
use crate::components::input::Textarea;
use crate::format::{format_sats, short_url};
use crate::hooks::use_poll;
use crate::types::DonationRead;
use leptos::ev;
use leptos::prelude::*;
use leptos_meta::Title;
use leptos_router::hooks::use_query_map;

#[component]
pub fn ReceivePage() -> impl IntoView {
    let query = use_query_map();
    let mint_filter = Signal::derive(move || query.get().get("mint"));

    view! {
        <Title text="Donate — Internet Mint Finder"/>
        <div class="shell">
            <div class="page-head rise">
                <div>
                    <div class="eyebrow"><span class="eyebrow-line"></span>"Community funded"</div>
                    <h1 class="page-title">"Keep the audits moving."</h1>
                    <p class="page-subtitle">
                        "Donations are working capital: every sat you contribute keeps the "
                        "swap engine running against that mint. Donating ecash from a new "
                        "mint automatically puts it under audit."
                    </p>
                </div>
            </div>

            <DonationAcks mint_filter/>
            <TokenDonationResult/>

            <div class="donate-grid">
                <section class="rise rise-1">
                    <h2 class="section-title">"Scan with your wallet"</h2>
                    <DonatePanel mint_filter=mint_filter/>
                </section>

                <section class="rise rise-2">
                    <h2 class="section-title">"Or paste a Cashu token"</h2>
                    <TokenPanel/>
                </section>
            </div>
        </div>
    }
}

/// Live acknowledgement of incoming donations. Polls the server and shows a
/// banner for every donation that arrives while the page is open — this is
/// how NUT-18 payments (which complete out-of-band) get confirmed to the
/// donor.
#[component]
fn DonationAcks(mint_filter: Signal<Option<String>>) -> impl IntoView {
    let last_seen: RwSignal<Option<u64>> = RwSignal::new(None);
    let acks: RwSignal<Vec<DonationRead>> = RwSignal::new(Vec::new());

    let tick = use_poll(5);
    let poll = Resource::new(
        move || tick.get(),
        move |_| {
            let after = last_seen.get_untracked();
            async move { poll_donations(after).await }
        },
    );

    Effect::new(move |_| {
        if let Some(Ok(result)) = poll.get() {
            last_seen.set(Some(result.latest_id));
            if !result.donations.is_empty() {
                acks.update(|list| {
                    list.extend(
                        result
                            .donations
                            .iter()
                            .filter(|d| {
                                mint_filter
                                    .get_untracked()
                                    .as_ref()
                                    .is_none_or(|url| url == &d.mint_url)
                            })
                            .cloned(),
                    );
                    // Keep the stack short; newest stay visible.
                    let overflow = list.len().saturating_sub(3);
                    if overflow > 0 {
                        list.drain(0..overflow);
                    }
                });
            }
        }
    });

    view! {
        <Show when=move || !acks.get().is_empty()>
            <div class="flex flex-col gap-2 mb-6" role="status" aria-live="polite">
                <p class="field-hint">"Recent donations to the auditor · includes other visitors"</p>
                <For
                    each=move || acks.get()
                    key=|d| d.id
                    children=|donation| {
                        let via = if donation.via == "nut18" {
                            "payment request"
                        } else {
                            "token"
                        };
                        view! {
                            <div class="ack-banner">
                                <span class="check">"✓"</span>
                                <span class="amount">
                                    {format!("+{} sat", format_sats(donation.amount))}
                                </span>
                                <span class="detail">
                                    {format!(
                                        "received via {} on {}",
                                        via,
                                        short_url(&donation.mint_url)
                                    )}
                                </span>
                            </div>
                        }
                    }
                />
            </div>
        </Show>
    }
}

/// Confirmation for the no-JS token donation POST flow. This keeps token
/// donations usable even if the WASM hydration bundle fails to attach the
/// client-side form handlers.
#[component]
fn TokenDonationResult() -> impl IntoView {
    let query = use_query_map();

    view! {
        <Show when=move || query.get().get("token_result").is_some()>
            <div class=move || {
                let is_ok = query
                    .get()
                    .get("token_result")
                    .as_deref()
                    == Some("ok");
                format!(
                    "callout mb-6 {}",
                    if is_ok { "callout-ok" } else { "callout-err" },
                )
            }>
                {move || {
                    let params = query.get();
                    match params.get("token_result").as_deref() {
                        Some("ok") => {
                            let amount = params
                                .get("amount")
                                .unwrap_or_else(|| "ecash".to_string());
                            let mint = params
                                .get("mint")
                                .map(|m| short_url(&m))
                                .unwrap_or_else(|| "the mint".to_string());
                            format!("Donation confirmed: received {amount} sat for {mint}. Thank you!")
                        }
                        _ => {
                            let message = params
                                .get("message")
                                .unwrap_or_else(|| "Could not receive the token.".to_string());
                            format!("Donation failed: {message}")
                        }
                    }
                }}
            </div>
        </Show>
    }
}

#[component]
fn DonatePanel(mint_filter: Signal<Option<String>>) -> impl IntoView {
    let request = Resource::new(
        move || mint_filter.get(),
        |mint| async move { get_payment_request(mint).await },
    );

    let copy_status = RwSignal::new("");
    let copying = RwSignal::new(false);

    view! {
        <div class="card card-pad">
            <Suspense fallback=|| {
                view! {
                    <div class="qr-panel">
                        <div class="skeleton" style="width: 248px; height: 248px;"></div>
                        <div class="skeleton" style="width: 100%; height: 48px;"></div>
                    </div>
                }
            }>
                {move || {
                    request
                        .get()
                        .map(|res| match res {
                            Ok(info) if !info.transport_available => view! {
                                <div class="empty-state">
                                    <h3>"Donate with a token"</h3>
                                    <p>"Payment requests are unavailable on this instance. Paste ecash from your wallet to fund the auditor."</p>
                                    <a href="#token" class="text-link">"Go to token donation →"</a>
                                </div>
                            }.into_any(),
                            Ok(info) => {
                                let pr_for_copy = info.pr.clone();
                                let on_copy = move |_| {
                                    #[cfg(feature = "hydrate")]
                                    {
                                        let request = pr_for_copy.clone();
                                        copying.set(true);
                                        leptos::task::spawn_local(async move {
                                            let result = wasm_bindgen_futures::JsFuture::from(
                                                window().navigator().clipboard().write_text(&request)
                                            ).await;
                                            copy_status.try_set(if result.is_ok() { "Payment request copied." } else { "Copy was blocked. Select and copy the request above." });
                                            copying.try_set(false);
                                        });
                                    }
                                    #[cfg(not(feature = "hydrate"))]
                                    {
                                        let _ = &pr_for_copy;
                                    }
                                };

                                view! {
                                    <div class="qr-panel">
                                        <Show when=move || mint_filter.get().is_some()>
                                            <p class="text-soft" style="font-size: 13px;">
                                                "Donating to "
                                                <span class="mono">
                                                    {mint_filter.get().map(|m| short_url(&m)).unwrap_or_default()}
                                                </span>
                                            </p>
                                        </Show>
                                        <div class="qr-frame" role="img" aria-label="Donation payment request QR code" inner_html=info.qr_svg.clone()></div>
                                        <div class="pr-string">{info.pr.clone()}</div>
                                        <Button
                                            format=ButtonFormat::Secondary
                                            class="btn-block".to_string()
                                            disabled=copying
                                            on_click=on_copy
                                        >
                                            {move || if copying.get() { "Copying…" } else { "Copy payment request" }}
                                        </Button>
                                        <p class="field-hint" role="status" aria-live="polite">{move || copy_status.get()}</p>
                                        <p class="field-hint">
                                            "Scan with a wallet that supports Cashu payment requests. "
                                            "Check your wallet for confirmation; recent donations appear above."
                                        </p>
                                    </div>
                                }
                                    .into_any()
                            }
                            Err(e) => {
                                view! {
                                    <div class="callout callout-err">
                                        {format!("Could not build payment request: {e}")}
                                    </div>
                                }
                                    .into_any()
                            }
                        })
                }}
            </Suspense>
        </div>
    }
}

#[component]
fn TokenPanel() -> impl IntoView {
    let token = RwSignal::new(String::new());
    let result_msg = RwSignal::new(String::new());
    let result_ok = RwSignal::new(false);

    let receive_action = ServerAction::<ReceiveToken>::new();
    let is_busy = receive_action.pending();

    Effect::new(move |_| {
        if let Some(result) = receive_action.value().get() {
            match result {
                Ok(res) => {
                    result_ok.set(res.success);
                    if res.success {
                        result_msg.set(format!("Donation confirmed: {}. Thank you!", res.message));
                        token.set(String::new());
                    } else {
                        result_msg.set(format!("Donation failed: {}", res.message));
                    }
                }
                Err(e) => {
                    result_ok.set(false);
                    result_msg.set(format!("Donation failed: {e}"));
                }
            }
        }
    });

    let on_submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();

        if is_busy.get_untracked() {
            return;
        }
        let trimmed = token.get_untracked().trim().to_string();
        if trimmed.is_empty() {
            result_ok.set(false);
            result_msg.set("Donation failed: Paste a Cashu token before donating.".to_string());
            return;
        }

        result_msg.set(String::new());
        receive_action.dispatch(ReceiveToken { token: trimmed });
    };

    view! {
        <div class="card card-pad">
            <form action="/receive-token" method="post" on:submit=on_submit>
                <div class="flex flex-col gap-4 w-full">
                    <Textarea
                        disabled=is_busy
                        label=Signal::stored("Cashu token".to_string())
                        name=Signal::stored("token".to_string())
                        placeholder=Signal::stored("cashuB...".to_string())
                        value=token
                        description=Signal::stored(
                            "Paste a Cashu token (any audited or new mint).".to_string(),
                        )
                    />

                    <Button button_type="submit" disabled=is_busy class="btn-block".to_string()>
                        {move || if is_busy.get() { "Confirming..." } else { "Confirm donation" }}
                    </Button>
                </div>

                <Show when=move || !result_msg.get().is_empty()>
                    <div role="status" aria-live="polite" class=move || {
                        format!(
                            "callout mt-4 {}",
                            if result_ok.get() { "callout-ok" } else { "callout-err" },
                        )
                    }>
                        {move || result_msg.get()}
                    </div>
                </Show>
            </form>
        </div>
    }
}
