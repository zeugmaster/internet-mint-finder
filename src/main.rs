#![recursion_limit = "512"]

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use cdk_cashu_auditor::app::{shell, App};
    use cdk_cashu_auditor::server::auditor::Auditor;
    use cdk_cashu_auditor::server::http::{api_router, https_redirect};
    use cdk_cashu_auditor::server::state::AppState;
    use leptos::prelude::*;
    use leptos_axum::{generate_route_list, LeptosRoutes};
    use std::sync::Arc;
    use tracing::info;
    use tracing_subscriber::{fmt, prelude::*, EnvFilter};

    // CLI subcommands run before the server boots and exit; everything else
    // falls through to the HTTP server. These are host-only maintenance tools
    // (no network-exposed path): `delete-mint` disables a mint; `wallet-doctor`
    // reports per-mint proof health; `cleanup-proofs` wipes and restores a
    // wallet's local proofs.
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("delete-mint") => {
            dotenvy::dotenv().ok();
            if let Err(e) = delete_mint_cli(&args[2..]).await {
                eprintln!("delete-mint: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("wallet-doctor") => {
            dotenvy::dotenv().ok();
            if let Err(e) = wallet_doctor_cli(&args[2..]).await {
                eprintln!("wallet-doctor: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("cleanup-proofs") => {
            dotenvy::dotenv().ok();
            if let Err(e) = cleanup_proofs_cli(&args[2..]).await {
                eprintln!("cleanup-proofs: {e}");
                std::process::exit(1);
            }
            return;
        }
        Some("keygen") => {
            keygen_cli();
            return;
        }
        _ => {}
    }

    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = std::env::var("SITE_ADDR").unwrap_or_else(|_| leptos_options.site_addr.to_string());

    let state = AppState::new(leptos_options)
        .await
        .expect("Failed to initialize AppState");

    let auditor = Arc::new(Auditor::new(
        state.wallet_repository.clone(),
        state.auditor_db.clone(),
        state.auditor_config.clone(),
    ));
    auditor.start().await;
    let nostr = cdk_cashu_auditor::server::nostr::start(&state).await;

    let routes = generate_route_list(App);

    let app = Router::new()
        .merge(api_router())
        .leptos_routes(&state, routes, {
            let state = state.clone();
            move || {
                provide_context(state.clone());
                shell(state.leptos_options.clone())
            }
        })
        .fallback(leptos_axum::file_and_error_handler::<AppState, _>(shell))
        .layer(axum::middleware::from_fn(https_redirect))
        .with_state(state);

    info!("listening on http://{}", &addr);
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();

    auditor.stop().await;
    nostr.stop().await;
    info!("shutdown complete");
}

/// `keygen`: print a fresh Nostr key pair for `IMF_NOSTR_NSEC` / `IMF_AUDITORS`
/// and a fresh BIP-39 mnemonic for `CDK_MNEMONIC`. Nothing is stored.
#[cfg(feature = "ssr")]
fn keygen_cli() {
    use nostr_sdk::prelude::{Keys, ToBech32};
    let keys = Keys::generate();
    let mut entropy = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut entropy);
    let mnemonic = bip39::Mnemonic::from_entropy(&entropy).expect("16 bytes of entropy");
    println!("# Auditor identity (keep the nsec secret; share the npub with peers)");
    println!(
        "IMF_NOSTR_NSEC={}",
        keys.secret_key().to_bech32().unwrap_or_default()
    );
    println!(
        "# npub: {}",
        keys.public_key().to_bech32().unwrap_or_default()
    );
    println!("# hex:  {}", keys.public_key().to_hex());
    println!();
    println!("# Wallet seed (never share, never reuse on a second instance)");
    println!("CDK_MNEMONIC=\"{mnemonic}\"");
}

/// `delete-mint [--yes|-y] <mint_url>`: soft-delete (disable) a tracked mint.
///
/// History (swaps, checks, snapshots) is preserved; the mint is hidden from
/// listings and skipped by the audit, health, and snapshot loops. Re-donating from
/// the mint re-enables it. Reachable only with host access — there is no
/// network-exposed delete path.
#[cfg(feature = "ssr")]
async fn delete_mint_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use cdk_cashu_auditor::server::mint_url::normalize_mint_url;
    use cdk_cashu_auditor::server::state::auditor_db_path;
    use cdk_cashu_auditor::server::storage::AuditorDatabase;
    use std::io::Write;

    let mut assume_yes = false;
    let mut url: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            "--yes" | "-y" => assume_yes = true,
            flag if flag.starts_with('-') => return Err(format!("unknown flag: {flag}").into()),
            value if url.is_some() => {
                return Err(format!("unexpected extra argument: {value}").into())
            }
            value => url = Some(value),
        }
    }
    let Some(url) = url else {
        return Err("usage: cdk-cashu-auditor delete-mint [--yes] <mint_url>".into());
    };
    let url = &normalize_mint_url(url);

    let db_path = auditor_db_path()?;
    if !db_path.exists() {
        return Err(format!("auditor database not found at {}", db_path.display()).into());
    }
    let db = AuditorDatabase::open(&db_path)?;

    let Some(mint) = db.get_mint_by_url(url).await? else {
        return Err(format!("mint not found or already deleted: {url}").into());
    };

    println!("Mint found:");
    println!("  ID:        {}", mint.id);
    println!("  URL:       {}", mint.url);
    println!("  Name:      {}", mint.name);
    println!("  Donations: {} sat", mint.sum_donations);
    println!("  State:     {:?}", mint.state);
    println!("  Errors:    {}", mint.n_errors);
    println!("  Melts:     {}", mint.n_melts);
    println!("  Mints:     {}", mint.n_mints);
    println!();

    if !assume_yes {
        print!("Soft-delete (disable) {url}? [y/N]: ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    if db.soft_delete_mint_by_url(url).await? {
        println!("Mint soft-deleted (disabled): {url}");
        println!("History kept; re-donate from this mint to re-enable it.");
    } else {
        println!("Mint was already deleted: {url}");
    }
    Ok(())
}

/// Parse `(<mint_url> | --all)` shared by the wallet maintenance CLIs into the
/// list of mint URLs to act on. `--all` reads every tracked mint from the
/// auditor DB; a bare URL targets just that mint.
#[cfg(feature = "ssr")]
async fn resolve_target_mints(
    args: &[String],
    extra_flags: &[&str],
    mut flag_seen: impl FnMut(&str),
    usage: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use cdk_cashu_auditor::server::mint_url::normalize_mint_url;
    use cdk_cashu_auditor::server::state::auditor_db_path;
    use cdk_cashu_auditor::server::storage::AuditorDatabase;

    let mut all = false;
    let mut url: Option<&str> = None;
    for arg in args {
        match arg.as_str() {
            "--all" => all = true,
            flag if extra_flags.contains(&flag) => flag_seen(flag),
            flag if flag.starts_with('-') => return Err(format!("unknown flag: {flag}").into()),
            value if url.is_some() => {
                return Err(format!("unexpected extra argument: {value}").into())
            }
            value => url = Some(value),
        }
    }

    match (url, all) {
        (Some(u), false) => Ok(vec![normalize_mint_url(u)]),
        (None, true) => {
            let db = AuditorDatabase::open(&auditor_db_path()?)?;
            Ok(db
                .get_all_mints()
                .await?
                .into_iter()
                .map(|m| m.url)
                .collect())
        }
        (Some(_), true) => {
            Err(format!("pass either a mint URL or --all, not both. {usage}").into())
        }
        (None, false) => Err(usage.into()),
    }
}

/// `wallet-doctor (<mint_url> | --all)`: read-only health report for each
/// mint's wallet. Shows proof counts by state, an offline NUT-12 DLEQ breakdown
/// of unspent proofs (valid / cryptographically-invalid / no-DLEQ), and a
/// NUT-07 check of how many local-unspent proofs the mint already considers
/// spent ("zombies"). Makes network calls (keyset keys + proof-state check).
#[cfg(feature = "ssr")]
async fn wallet_doctor_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use cdk_cashu_auditor::server::state::open_wallet_repository;

    let usage = "usage: cdk-cashu-auditor wallet-doctor (<mint_url> | --all)";
    let urls = resolve_target_mints(args, &[], |_| {}, usage).await?;

    let repo = open_wallet_repository().await?;
    for url in urls {
        if let Err(e) = doctor_one(&repo, &url).await {
            println!("Mint: {url}\n  check failed: {e}");
        }
        println!();
    }
    Ok(())
}

#[cfg(feature = "ssr")]
async fn doctor_one(
    repo: &std::sync::Arc<cdk::wallet::WalletRepository>,
    url: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use cdk::nuts::{CurrencyUnit, Id, Keys, State};
    use std::collections::BTreeMap;
    use std::str::FromStr;

    let mint_url = cdk::mint_url::MintUrl::from_str(url)?;
    if repo
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .is_err()
    {
        repo.add_wallet(mint_url.clone()).await?;
    }
    let wallet = repo.get_wallet(&mint_url, &CurrencyUnit::Sat).await?;

    println!("Mint: {url}");

    // Proof counts grouped by local state.
    let all = wallet
        .localstore
        .get_proofs(Some(mint_url.clone()), Some(CurrencyUnit::Sat), None, None)
        .await?;
    let mut by_state: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for p in &all {
        let entry = by_state.entry(format!("{:?}", p.state)).or_insert((0, 0));
        entry.0 += 1;
        entry.1 += u64::from(p.proof.amount);
    }
    println!("  proofs: {} total", all.len());
    for (state, (count, amount)) in &by_state {
        println!("    {state}: {count} proof(s), {amount} sat");
    }

    // Offline NUT-12 DLEQ verification of unspent proofs.
    let unspent = wallet.get_unspent_proofs().await?;
    let (mut dleq_ok, mut dleq_invalid, mut dleq_missing) = (0u32, 0u32, 0u32);
    let mut keys_cache: BTreeMap<Id, Keys> = BTreeMap::new();
    for p in &unspent {
        if let std::collections::btree_map::Entry::Vacant(entry) = keys_cache.entry(p.keyset_id) {
            match wallet.keyset(p.keyset_id).await {
                Ok(keyset) => {
                    entry.insert(keyset.keys);
                }
                Err(_) => continue,
            }
        }
        let Some(keys) = keys_cache.get(&p.keyset_id) else {
            continue;
        };
        let Some(mint_pubkey) = keys.amount_key(p.amount) else {
            continue;
        };
        match p.verify_dleq(mint_pubkey) {
            Ok(()) => dleq_ok += 1,
            Err(cdk::nuts::nut12::Error::MissingDleqProof)
            | Err(cdk::nuts::nut12::Error::IncompleteDleqProof) => dleq_missing += 1,
            Err(_) => dleq_invalid += 1,
        }
    }
    println!("  unspent DLEQ: {dleq_ok} valid, {dleq_invalid} INVALID, {dleq_missing} no-dleq");

    // NUT-07: how many local-unspent proofs the mint already burned.
    if !unspent.is_empty() {
        match wallet.check_proofs_spent(unspent.clone()).await {
            Ok(states) => {
                let spent = states.iter().filter(|s| s.state == State::Spent).count();
                let pending = states.iter().filter(|s| s.state == State::Pending).count();
                println!(
                    "  mint says: {spent} of {} unspent already SPENT (zombies), {pending} pending",
                    unspent.len()
                );
            }
            Err(e) => println!("  mint spent-check failed: {e}"),
        }
    }

    let balance = wallet.total_balance().await.map(u64::from).unwrap_or(0);
    println!("  spendable balance: {balance} sat");
    Ok(())
}

/// `cleanup-proofs [--yes] [--no-restore] (<mint_url> | --all)`: delete every
/// local proof for the target mint(s), then (unless `--no-restore`) run a
/// deterministic restore to re-derive proofs from the mint. The wallet store is
/// shared with the running auditor, so stop the auditor first to avoid a locked
/// database. This is the hard reset for a wallet whose local proof store is
/// corrupt beyond the automatic per-failure repair.
#[cfg(feature = "ssr")]
async fn cleanup_proofs_cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use cdk_cashu_auditor::server::state::open_wallet_repository;
    use std::io::Write;

    let mut assume_yes = false;
    let mut restore = true;
    let usage =
        "usage: cdk-cashu-auditor cleanup-proofs [--yes] [--no-restore] (<mint_url> | --all)";
    let urls = resolve_target_mints(
        args,
        &["--yes", "-y", "--no-restore"],
        |flag| match flag {
            "--yes" | "-y" => assume_yes = true,
            "--no-restore" => restore = false,
            _ => {}
        },
        usage,
    )
    .await?;

    println!(
        "This DELETES ALL local proofs for {} mint(s){}.",
        urls.len(),
        if restore {
            ", then re-derives them from each mint (restore)"
        } else {
            " WITHOUT restoring — the wallet is left empty"
        }
    );
    println!("The cdk wallet store is shared with the running auditor; stop it first to avoid a locked database.");

    if !assume_yes {
        print!("Proceed? [y/N]: ");
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        if !answer.trim().eq_ignore_ascii_case("y") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let repo = open_wallet_repository().await?;
    for url in urls {
        if let Err(e) = cleanup_one(&repo, &url, restore).await {
            println!("  [{url}] cleanup failed: {e}");
        }
    }
    Ok(())
}

#[cfg(feature = "ssr")]
async fn cleanup_one(
    repo: &std::sync::Arc<cdk::wallet::WalletRepository>,
    url: &str,
    restore: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    use cdk::nuts::CurrencyUnit;
    use std::str::FromStr;

    let mint_url = cdk::mint_url::MintUrl::from_str(url)?;
    if repo
        .get_wallet(&mint_url, &CurrencyUnit::Sat)
        .await
        .is_err()
    {
        repo.add_wallet(mint_url.clone()).await?;
    }
    let wallet = repo.get_wallet(&mint_url, &CurrencyUnit::Sat).await?;

    // Every proof regardless of state (unspent, pending, reserved).
    let all = wallet
        .localstore
        .get_proofs(Some(mint_url.clone()), None, None, None)
        .await?;
    let ys: Vec<_> = all.iter().map(|p| p.y).collect();
    let count = ys.len();
    if count > 0 {
        wallet.localstore.update_proofs(vec![], ys).await?;
    }
    println!("  [{url}] removed {count} local proof(s)");

    if restore {
        let restored = wallet.restore().await?;
        println!(
            "  [{url}] restored: unspent={} sat, pending={} sat, spent={} sat",
            u64::from(restored.unspent),
            u64::from(restored.pending),
            u64::from(restored.spent)
        );
    }
    Ok(())
}

#[cfg(feature = "ssr")]
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("shutdown signal received");
}

#[cfg(not(feature = "ssr"))]
pub fn main() {}
