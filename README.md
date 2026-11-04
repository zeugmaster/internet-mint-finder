# Internet Mint Finder

A Cashu mint directory and Nostr review aggregator. One Rust/Axum/Leptos service,
one SQLite volume, one Docker image. Browsing is public. Signing in uses a NIP-07
browser extension; IMF never asks for, stores, or handles a user's private key.

## Scope

**Phase one:** discover mints, search and sort community ratings, read reviews,
and create or update a review. The existing interface, themes and branding remain.

**Phase two:** display results from a separately deployed auditor, potentially
headless. The site will consume audit data, not own a wallet or run swaps.
The completed audit UI source is kept in `archive/auditor-ui/` for that phase;
it is excluded from compilation and the image. No audit numbers are shown in phase one.

The website no longer contains an audit engine, wallet recovery, payment or donation
endpoints, peer-auditor replication, server Nostr keys, mirror discovery, DNS failover,
Tor, a bundled relay, or provider-specific TLS infrastructure.

## Docker

From this repository directory:

```sh
docker build -t imf:local .
docker compose up -d
curl --fail http://localhost:3000/api/health/live
```

Open http://localhost:3000. Initial review backfill runs in the background. No
mnemonic, nsec, funding, or external database is required. To change relays or the
public URL, copy `.env.example` to `.env` and edit it before starting Compose.
The image includes the server, hydrated frontend, CSS and local static assets.
It runs as UID/GID 10001 with its only writable data at `/data`.

If another service already uses port 3000, run `IMF_PORT=3002 docker compose up -d`
and open http://localhost:3002. Set `IMF_PORT` in `.env` to keep that port for later
Compose runs. `BASE_URL`, when set, still takes precedence for public links.

Package downloads use the FAU Debian mirror and Debian's security repository over
HTTPS. To use another regular Debian mirror, pass
`--build-arg DEBIAN_MIRROR=https://your-mirror.example/debian` to the build command.

To run the image directly:

```sh
docker run -d --name imf --restart unless-stopped \
  -p 127.0.0.1:3000:3000 -v imf_data:/data \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  -e BASE_URL=https://mints.example.com imf:local
```

Terminate public HTTPS at your existing ingress or reverse proxy and forward to
port 3000. The application does not redirect HTTP itself. Do not remove the data
volume when updating: it contains cached reviews and signed reviews awaiting delivery.
To update with Compose, rebuild the image and run `docker compose up -d`.

The manual **Build deployable image** GitHub Actions workflow tests the code,
builds and smoke-tests the image, and uploads `imf-image.tar.gz` as a downloadable
Linux amd64 artifact. Load the extracted artifact with:

```sh
gunzip -c imf-image.tar.gz | docker load
```

The workflow does not publish to a registry. To deploy across machines, tag and
push the built image to your registry and use its immutable tag or digest.

## Kubernetes

`deploy/kubernetes.yaml` contains a Deployment, Service and persistent volume claim.
Set the image to your registry image, set `BASE_URL` to your public HTTPS origin,
and use your cluster's ingress/TLS setup for the `imf` service:

```sh
kubectl apply -f deploy/kubernetes.yaml
kubectl port-forward service/imf 3000:80
```

The manifest uses **one replica and Recreate updates** because SQLite lives on a
single persistent volume. This deliberately permits brief downtime on updates.
Keep that deployment model; horizontal scaling requires a different shared storage
design. Kubernetes handles process/node recovery. Outbound DNS and WebSocket access
to the configured relays are required for synchronization and publishing.

## Configuration

| Variable | Default | Purpose |
| --- | --- | --- |
| `IMF_DATA_DIR` | `./data` locally; `/data` in image | Persistent directory containing `directory.sqlite3` |
| `IMF_RELAYS` | damus, nos.lol, primal, cashumints.space, nostr.net | Comma-separated `wss://` or development `ws://` relays; empty lists are rejected |
| `BASE_URL` | unset | Public origin for link-preview image URLs |
| `SITE_ADDR` | `127.0.0.1:3000` locally; `0.0.0.0:3000` in image | HTTP listener |
| `APP_NAME` | `Internet Mint Finder` | Display name |
| `RUST_LOG` | `info` | Logging |

Direct Cargo runs read exported environment variables; they do not load `.env`.
An explicit `IMF_RELAYS` value, including one in an existing `.env`, replaces the
default list. Include `wss://relay.nostr.net` there to retain its historical coverage.
The old `CDK_*`, `AUDITOR_*`, `IMF_NOSTR_NSEC`, `IMF_AUDITORS`, and mirror settings
are unused. Never pass wallet secrets to the new image.

## API

Mint IDs are URL-safe base64 of the normalized mint URL, returned by the directory.
All routes and the frontend read the same stored data; requests do not wait for
relays. Read endpoints allow cross-origin GET requests.

| Method/path | Behavior |
| --- | --- |
| `GET /api/mints?search=&sort=RatingHigh&page=0` | Directory, 20 per page; sorts: `Name`, `RatingHigh`, `RatingLow`, `MostReviews` |
| `GET /api/mints/{id}` | One mint or 404 |
| `GET /api/mints/{id}/reviews?limit=100&offset=0` | Latest review per author, total, rated count, average and signing context; limit 1–500 |
| `POST /api/mints/{id}/reviews` | Signed Nostr event JSON; verifies signature, kind, target, size, timestamp and rating |
| `GET /api/reviews/{event_id}` | Whether an accepted review has reached a relay |
| `GET /api/health/live` | Database access and worker progress; external relay outages alone do not fail liveness |
| `GET /api/health/ready` | Ready after initial synchronization or with cached data; reports synchronization and pending reviews |
| `GET /api/health` | Alias for readiness |

A successful submission is stored with its delivery queue entry in one transaction.
`202 {"saved":true,"published":false,...}` means saved locally and queued; it does
**not** mean a relay accepted it. The UI immediately refreshes reviews and marks
pending delivery. A background worker retries after failure and restart, clearing
the queue only after relay acceptance or observing the event back on a relay.
A repeated identical submission is idempotent. A replacement uses the newest
`created_at`; equal timestamps use the lexically lower event ID, following NIP-01.

Login remembers only the selected public key across pages and tabs. This is a
signer selection, not a trusted server session: every write proves its author
through the event signature. Sign-out cancels pending signing; switching extension
accounts requires signing in again. There are no password or nsec entry forms.

## Review compatibility and reliability

See [the event contract and tested formats](docs/reviews.md). New IMF reviews use
NIP-87 kind **38000**, referring to Cashu announcement kind **38172**, with the
existing `[N/5]` rating convention. Kind 38172 is not itself a review.

Ingestion verifies events and merges duplicates across relays. Replay cursors are
independent per relay and advance only after completed, persisted pages. Disconnects,
timeouts and receiver lag leave the replay boundary intact. Live subscriptions
accept late events regardless of their original timestamp; a full historical scan
every six hours also catches events delivered while IMF was offline.

Cached data remains readable during relay outages. Relay retention and availability
still limit what can be recovered from an empty volume. Reviews are public claims
from Nostr accounts; this phase does not claim Sybil-resistant ratings or moderation.
No HTTP requests are made to mint URLs supplied in events.

## Migrating from the combined auditor

1. Stop the old web/auditor process and retain its data volume and wallet seed.
   Do not run two processes against that wallet or delete its `wallet.db`/`auditor.db`.
2. Start this image with a **new volume** and no wallet/signing secrets. Its distinct
   `directory.sqlite3` is rebuilt from configured relays. Existing old databases are
   neither modified nor automatically imported. Reviews no longer retained on any
   configured relay will require a separate export/import before decommissioning
   their old cache.
3. Keep the auditor separately if you still need it. The sibling
   `auditor/cdk-cashu-auditor` remains outside this website deployment. Phase-two
   audit ingestion and its trust policy are not implemented in this release.

The old numeric mint API, wallet/donation, analytics and auditor endpoints are removed.
Current URL-derived mint links are retained. Phase-one API response shapes are now
review-focused; downstream consumers must use the contract above.

For backup, stop the container and back up the whole volume (including any SQLite
WAL files), or use SQLite's online backup facility. Restore to a volume writable by
UID/GID 10001. Cached history and undelivered signed reviews are persistent data.

## Development and checks

Rust 1.95.0 or newer; nightly is no longer required. For full frontend builds:

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked --version 0.3.6 cargo-leptos
cargo leptos build --release --lib-cargo-args=--locked --bin-cargo-args=--locked
node scripts/check-wasm.mjs
SITE_ADDR=127.0.0.1:3000 cargo run --locked --features ssr
```

Let cargo-leptos download its compatible `wasm-opt`. An older system installation
(including Debian Bookworm's Binaryen) can produce a frontend that fails to initialize.

```sh
cargo test --locked --features ssr
cargo test --locked --features ssr -- --include-ignored  # includes a local WebSocket relay test
node scripts/check-browser.mjs
cargo clippy --locked --features ssr --all-targets -- -D warnings
cargo check --locked --features hydrate --target wasm32-unknown-unknown
cargo fmt --all --check
docker compose config --quiet
```

Set `IMF_TEST_HTML_DIR=/tmp/imf-pages` when running the SSR page test to export the
rendered directory and mint pages for visual inspection. Fixtures are synthetic.
