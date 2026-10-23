use crate::components::mint_list::MintList;
use crate::components::stats_card::StatsCard;
use crate::components::swap_list::SwapList;
use leptos::prelude::*;
use leptos_meta::Title;

#[component]
pub fn HomePage() -> impl IntoView {
    view! {
        <Title text="Overview — Internet Mint Finder"/>
        <div class="shell">
            <div class="page-head overview-head rise">
                <div>
                    <div class="eyebrow"><span class="eyebrow-line"></span>"Mint directory"</div>
                    <h1 class="page-title">"Find a Cashu mint."</h1>
                    <p class="page-subtitle">
                        "Every mint announced or reviewed on Nostr, with what independent auditors "
                        "measured by moving real ecash through it. Measurements, reviews, no scores."
                    </p>
                </div>
                <div class="page-actions">
                    <a href="/dashboard" class="btn btn-secondary" rel="external">"Analytics"</a>
                    <a href="/receive" class="btn btn-primary">"Donate ecash"</a>
                </div>
            </div>

            <StatsCard/>

            <div class="grid-main overview-grid">
                <section class="rise rise-2">
                    <div class="section-head"><h2>"Directory"</h2><span>"Refreshes every 15s"</span></div>
                    <MintList/>
                </section>
                <section class="rise rise-3">
                    <div class="section-head"><h2>"Audit activity"</h2><span>"Latest 50 attempts"</span></div>
                    <SwapList/>
                </section>
            </div>

            <section class="rise rise-4 mt-6">
                <h2 class="section-title">"How the audit works"</h2>
                <div class="steps-grid">
                    <div class="step-card">
                        <div class="step-num">"01 · REQUEST"</div>
                        <h3>"A mint quote is created"</h3>
                        <p>
                            "The auditor asks one mint for a Lightning invoice, exactly like "
                            "a wallet topping up its balance would."
                        </p>
                    </div>
                    <div class="step-card">
                        <div class="step-num">"02 · PAY"</div>
                        <h3>"Another mint pays it"</h3>
                        <p>
                            "Real ecash on a second mint is melted to settle that invoice "
                            "over Lightning. Each successful payment moves real sats."
                        </p>
                    </div>
                    <div class="step-card">
                        <div class="step-num">"03 · RECORD"</div>
                        <h3>"Every outcome is published"</h3>
                        <p>
                            "Latency, fees, and failures are recorded verbatim and shown "
                            "in the activity feed and the detailed analytics."
                        </p>
                    </div>
                </div>
            </section>
        </div>
    }
}
