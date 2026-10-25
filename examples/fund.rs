//! Dev helper: mint fresh ecash on a mint and print it as a token to paste into
//! `/receive`. Uses a throwaway in-memory wallet, so it only makes sense against
//! the fake-wallet mints of docker-compose.yml, which pay their own invoices.
//! Run: cargo +nightly run --no-default-features --features ssr --example fund -- http://localhost:8085 2000
#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use cdk::nuts::{CurrencyUnit, PaymentMethod, Token};
    use cdk::wallet::WalletRepositoryBuilder;
    use rand::RngCore;
    use std::sync::Arc;

    let mut args = std::env::args().skip(1);
    let (Some(url), Some(amount)) = (args.next(), args.next()) else {
        eprintln!("usage: fund <mint-url> <sats>");
        std::process::exit(2);
    };
    let amount: u64 = amount.parse()?;

    let mut seed = [0u8; 64];
    rand::thread_rng().fill_bytes(&mut seed);
    let repo = WalletRepositoryBuilder::new()
        .localstore(Arc::new(
            cdk_sqlite::WalletSqliteDatabase::new(":memory:").await?,
        ))
        .seed(seed)
        .build()
        .await?;
    let wallet = repo
        .create_wallet(url.parse()?, CurrencyUnit::Sat, None)
        .await?;

    let quote = wallet
        .mint_quote(PaymentMethod::BOLT11, Some(amount.into()), None, None)
        .await?;
    eprintln!(
        "quote {} on {url}: waiting for the mint to see it paid",
        quote.id
    );
    for _ in 0..60 {
        let status = wallet.check_mint_quote_status(&quote.id).await?;
        if status.amount_mintable() > cdk::Amount::ZERO {
            let proofs = wallet
                .mint(&quote.id, cdk::amount::SplitTarget::None, None)
                .await?;
            println!(
                "{}",
                Token::new(url.parse()?, proofs, None, CurrencyUnit::Sat)
            );
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err("invoice was not paid within 60 s".into())
}

#[cfg(not(feature = "ssr"))]
fn main() {}
