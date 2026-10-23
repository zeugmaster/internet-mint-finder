# syntax=docker/dockerfile:1.7

# ---------- Builder ----------
FROM rustlang/rust:nightly-bookworm AS builder

ARG CARGO_LEPTOS_VERSION=0.3.6
ARG CARGO_BUILD_JOBS=1

ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        clang \
        cmake \
        binaryen \
        git \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo install --debug --locked --version ${CARGO_LEPTOS_VERSION} cargo-leptos

WORKDIR /build

COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY src ./src
COPY public ./public
COPY style ./style

RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo leptos build --release \
    && mkdir -p /out \
    && cp target/release/cdk-cashu-auditor /out/cdk-cashu-auditor \
    && cp -r target/site /out/site

# ---------- Runtime ----------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 1000 app \
    && useradd --system --uid 1000 --gid app --home /data --shell /usr/sbin/nologin app \
    && mkdir -p /data /app \
    && chown -R app:app /data /app

WORKDIR /app

COPY --from=builder --chown=app:app /out/cdk-cashu-auditor /app/cdk-cashu-auditor
COPY --from=builder --chown=app:app /out/site /app/site

USER app

ENV HOME=/data \
    LEPTOS_OUTPUT_NAME=cdk-cashu-auditor \
    LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:3000 \
    LEPTOS_RELOAD_PORT=3001 \
    SITE_ADDR=0.0.0.0:3000 \
    RUST_LOG=info

VOLUME ["/data"]
EXPOSE 3000

ENTRYPOINT ["/app/cdk-cashu-auditor"]
