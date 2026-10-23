# Internet Mint Finder

A Cashu mint directory with real-money reliability audits, run by independent
operators who mirror each other over Nostr. It merges the continuous
[Cashu](https://cashu.space) mint auditor (built on the
[Cashu Development Kit (CDK)](https://github.com/cashubtc/cdk)) with the
NIP-87 mint directory and review aggregator that bitcoinmints.com used to be.

Every instance holds its own ecash and signs its own observations with its own
Nostr key. Instances ingest each other's events plus
NIP-87 announcements (kind 38172) and reviews (kind 38000) from relays, so any
instance shows the union: a directory of every announced or audited mint, each
auditor's own numbers side by side, and the reviews. Reviews are signed in the
browser (NIP-07) and sent to relays; this server never receives them directly.
Each operator runs one box behind a shared, health-checked domain; the stack is
in `deploy/`.

The auditor holds ecash on every tracked mint and constantly swaps between them:
it requests a BOLT11 mint quote on one mint and pays it by melting ecash on
another. Every attempt — latency, fees, and failures — is recorded and published
on a live dashboard, so wallet users can see which mints actually work.

## How it works

```
┌─────────────┐   melt (pay invoice)   ┌─────────────┐
│   Mint A    │ ─────────────────────▶ │   Mint B    │
│ (ecash out) │      via Lightning     │ (ecash in)  │
└─────────────┘                        └─────────────┘
       ▲                                      │
       └────────── next round, reversed ──────┘
```

- **Swap engine** picks a random eligible receiving mint and compatible sending
  mint, respecting donation targets and a liquidity buffer, then
  executes a mint-quote → melt → mint round trip.
- **Health states**: mints that fail are marked `ERROR` and benched for a
  cooldown (default 30 min), after which they are eligible for another swap.
  Successful info probes measure availability; they do not clear an unresolved
  payment failure. A settled swap restores payment health.
- **Funding**: donations are working capital. Donating ecash from a new mint
  automatically puts that mint under audit. Donations are accepted as pasted
  tokens or via NUT-18 payment requests.
- **Analytics**: every swap lands in SQLite; the `/dashboard` page renders
  KPIs, latency/error scatter, time series, histograms, and error breakdowns.

## Stack

- **Backend**: Rust, Axum, upstream CDK 0.18 (`WalletRepository` + SQLite, published on crates.io), rusqlite
  for the auditor's own relational store.
- **Frontend**: Leptos 0.8 (SSR + WASM hydration), hand-rolled design system in
  `style/main.css`, Plotly analytics page.

## Running locally

Requires nightly Rust (Leptos `nightly` feature) and
[`cargo-leptos`](https://github.com/leptos-rs/cargo-leptos):

```sh
rustup toolchain install nightly
rustup target add wasm32-unknown-unknown --toolchain nightly
cargo install --locked cargo-leptos

# Set CDK_MNEMONIC in your environment or .env first.
cargo leptos watch     # dev loop on http://127.0.0.1:3000
```

Direct server build without the WASM bundle:

```sh
cargo run --no-default-features --features=ssr
```

### Configuration

All configuration is via environment variables (a `.env` file is loaded):

| Variable | Default | Purpose |
| --- | --- | --- |
| `CDK_MNEMONIC` | required | Private BIP-39 seed for the CDK wallet. Startup fails if it is missing or invalid. |
| `APP_NAME` | `Cashu Mint Auditor` | Display name; also derives the data directory `~/.<name>`. |
| `SITE_ADDR` | `127.0.0.1:3000` | Listen address. |
| `BASE_URL` | unset | Public base URL; enables the NUT-18 HTTP-POST transport (`/api/donate`). |
| `AUDITOR_MIN_DELAY_SECS` / `AUDITOR_MAX_DELAY_SECS` | 300 / 900 | Delay range between swap rounds. |
| `AUDITOR_MIN_AMOUNT` / `AUDITOR_MAX_AMOUNT` | 5 / 100 | Swap amount range (sats). |
| `AUDITOR_MAX_FEE_PERCENT` | 2.0 | Reject melt quotes above this fee reserve. |
| `AUDITOR_FEE_TOLERANCE_SATS` | 10 | Absolute fee tolerance before the percent check applies. |
| `AUDITOR_ERROR_COOLDOWN_SECS` | 1800 | How long failing mints are benched before retry. |
| `AUDITOR_HEALTH_CHECK_INTERVAL_SECS` | 3600 | No-fee NUT-06 availability cadence; values below one hour are clamped to one hour. |
| `AUDITOR_HEALTH_FAILURE_THRESHOLD` | 2 | Consecutive failed checks before a mint is marked failing. |
| `AUDITOR_SNAPSHOT_INTERVAL_SECS` | 3600 | Balance snapshot interval. |
| `RUST_LOG` | `info` | Tracing filter. |
| `IMF_NOSTR_NSEC` | unset | This auditor's Nostr key (nsec or hex). Unset = nothing is published. `keygen` prints one. |
| `IMF_AUDITORS` | unset | Comma-separated npub/hex keys whose audit events are ingested and shown. |
| `IMF_RELAYS` | damus, nos.lol, primal, cashumints.space | Comma-separated relay URLs; put your own archive relay first. |
| `IMF_PUBLIC_URLS` | `BASE_URL` | URLs announced for this instance (clearnet, onion). |
| `IMF_CONTACT` | unset | Optional, pseudonymous, in the announcement. |

### Controlling audit spend

The auditor attempts at most one paid swap per swap-loop round, then waits a
delay between `AUDITOR_MIN_DELAY_SECS` and `AUDITOR_MAX_DELAY_SECS`. Set both
values to the same number for a fixed timer. Raising those values is the main
way to reduce fee spend. The default 5-15 minute delay permits at most 4-12
attempts per hour. For example, a 15-30 minute delay window permits at most
2-4 attempts per hour:

```sh
AUDITOR_MIN_DELAY_SECS=900
AUDITOR_MAX_DELAY_SECS=1800
```

Each round chooses a random eligible receiver and compatible sender. The amount
respects both mints' advertised sat/BOLT11 limits (NUT-04 and NUT-05), the
configured limits, receiver headroom, and the sender's balance buffer. Pairs
with no valid amount are skipped. For example, a mint with a 100-sat minimum
receives 100-sat audits with the default 5–100-sat configuration, provided the
pair has enough liquidity and headroom. Swap size and fee exposure are controlled with
`AUDITOR_MIN_AMOUNT`, `AUDITOR_MAX_AMOUNT`, `AUDITOR_MAX_FEE_PERCENT`, and
`AUDITOR_FEE_TOLERANCE_SATS`. Failed mints are benched for
`AUDITOR_ERROR_COOLDOWN_SECS`.

Each tracked mint receives one no-fee NUT-06 availability check per hour. The
checks are slightly staggered, use one request with no immediate retry, and
feed 24 hourly uptime buckets independently of payment health. A rate-limited
response is recorded as degraded rather than offline, and two consecutive
failures are required before the mint
is marked failing. Background unissued-quote reclaim sweeps remain disabled.

Amount-limit rejections include the requested amount and the last advertised
limits. CDK 0.18 discards the remote detail for error code 11006; the auditor
labels missing details instead of displaying fabricated zero bounds. Advertised
limits are identified as metadata and can lag a mint's configuration change.

Data lives in `~/.internet-mint-finder/` (`wallet.db` for CDK, `auditor.db` for
audit data). `cargo run --no-default-features --features=ssr -- keygen` prints a
fresh Nostr key pair and wallet mnemonic.

CDK 0.18 applies wallet database migrations automatically when opening
`wallet.db`. Before upgrading an existing installation, stop the auditor and
back up its configured data directory. Keep that backup for any rollback.

### Disabling a mint

A mint is put under audit by donating ecash from it; there is no
network-exposed way to remove one. To disable (soft-delete) a tracked mint, run
the binary's `delete-mint` subcommand on the host:

```sh
cargo run --no-default-features --features=ssr -- delete-mint https://mint.example.com
# non-interactive:
cargo run --no-default-features --features=ssr -- delete-mint --yes https://mint.example.com
```

The mint is hidden from listings and skipped by the swap, health, and snapshot loops;
its historical swaps and checks are kept. Re-donating from the mint
re-enables it. Empty/already-spent donations (0 sat received) are rejected so
they cannot register a junk mint.

## Docker

```sh
docker compose up --build
```

The development compose file runs two instances (`:3000` and `:3001`) sharing
one local strfry relay plus four fake-wallet mints, so the mirror can be
watched locally: donate to one instance, and its swaps appear on the other.
Production deployment (app, archive relay, Caddy with DNS-01, onion) is in
`deploy/`; copy `deploy/.env.example` to `deploy/.env` to start.

## Nostr

- Published: kind `1172` per swap and probe, kind `30172` status per locally
  funded mint, kind `10172` announcement; all from an outbox, so relay outages
  lose nothing. Rows are published in id order with their observation time as
  `created_at`, so a retry yields the same event id.
- Ingested: the same kinds from `IMF_AUDITORS`, and NIP-87 kinds `38172` and
  `38000` from anyone. Announced mints get a directory row and hourly probes;
  reviews attach to known mints by URL or mint pubkey.
- The dashboard filters accept `auditor=local` or `auditor=<hex>`.

## API

REST endpoints (also consumed by the analytics dashboard):

| Endpoint | Description |
| --- | --- |
| `GET /api/health` | Liveness; 503 once the probe loop has stalled, so a DNS health check drops a hung instance. |
| `GET /api/mints/` · `GET /api/mints/{id}` | Tracked mints with live balances. |
| `GET /api/mints/{id}/metrics` | Per-mint reliability metrics (success rate, latency, last error). |
| `GET /api/mints/{id}/uptime` | Per-mint observed uptime timeline from hourly health probes, with swap-history fallback. |
| `GET /api/mints/{id}/donations` · `GET /api/donations?after_id=` | Donation history / polling. |
| `GET /api/mints/{id}/auditors` | Every auditor's own metrics for the mint (this instance first, never merged). |
| `GET /api/mints/{id}/announcement` · `GET /api/mints/{id}/reviews` | NIP-87 announcement and reviews read from relays. |
| `GET /api/auditors` | This instance and every peer auditor announced on Nostr (kind 10172). |
| `POST /api/mints/` | Add a mint by donating a Cashu token (`{"token": "cashuB..."}`). |
| `GET /api/swaps/` · `GET /api/swaps/mint/{id}` | Swap history. |
| `GET /api/stats/` | Aggregate stats. |
| `GET /api/pr?url=` | NUT-18 payment request (optionally mint-locked). |
| `POST /api/donate` | NUT-18 payment payload receiver. |
| `GET /api/kpis`, `/api/scatter`, `/api/top-quality`, `/api/timeseries/*`, `/api/histogram/latency`, `/api/errors`, `/api/graph/`, `/api/filters` | Analytics queries (TTL-cached, filterable). |

## Development

```sh
cargo +nightly test --no-default-features --features=ssr   # backend tests
cargo +nightly fmt
cargo +nightly clippy --no-default-features --features=ssr
```

### Isolated interface preview

```sh
cargo +nightly leptos build
RUSTFLAGS="--cfg erase_components" cargo +nightly run --locked --no-default-features --features=ssr --example preview
```

Open http://127.0.0.1:3100. This preview uses synthetic `.example` mints,
in-memory databases and empty wallets. It never starts the audit engine or
opens your application data directory, and rejects donations. Wallet balances
are intentionally zero. All data disappears when the process stops.

The registry supports name/URL search, health filters, and 20-row pages. REST
lists are capped at 500 rows per request; use `skip` for subsequent pages.
`MintRead.balance_available` distinguishes a wallet read failure from a zero
balance. Aggregate stats fail explicitly if any balance is unavailable.

Polling pauses in hidden tabs and stops when a page is unmounted. Analytics
refreshes on demand and labels stale/unavailable results. NUT-18 QR requests
are offered only when `BASE_URL` is configured; pasted tokens remain available.

Donation targets and their history are committed together. The CDK receive
result supplies the credited amount after fees, independently of concurrent
wallet activity. Settled swap events and both mint counters also commit in one
transaction; write failures are surfaced in the auditor logs.
CDK wallet storage and the auditor database remain separate;
back up both. This does not provide a distributed transaction across the stores.

`.cargo/config.toml` sets `LEPTOS_OUTPUT_NAME` for every build so direct SSR and
Docker builds request the same WASM filename generated by cargo-leptos.

The development preview uses the same `erase_components` flag as cargo-leptos'
development WASM build. Production Docker builds use `cargo leptos build --release`
to produce matching, optimized server and client bundles.
