# syntax=docker/dockerfile:1.7
ARG DEBIAN_MIRROR=https://ftp.fau.de/debian
FROM rust:1.95.0-bookworm AS toolchain

FROM toolchain AS builder
ARG DEBIAN_MIRROR
ARG CARGO_LEPTOS_VERSION=0.3.6
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
RUN sed -i -e 's|http://deb.debian.org/debian-security|https://security.debian.org/debian-security|g' \
    -e "s|http://deb.debian.org/debian|${DEBIAN_MIRROR}|g" /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=5 update \
    && apt-get -o Acquire::Retries=5 install -y --no-install-recommends \
    pkg-config libssl-dev clang cmake ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo install --locked --version ${CARGO_LEPTOS_VERSION} cargo-leptos
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY .cargo ./.cargo
COPY src ./src
COPY public ./public
COPY style ./style
# Let cargo-leptos fetch its compatible wasm-opt; Bookworm's Binaryen breaks hydration.
RUN --mount=type=cache,target=/build/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    cargo leptos build --release --lib-cargo-args=--locked --bin-cargo-args=--locked \
    && mkdir -p /out \
    && cp target/release/imf /out/imf \
    && cp -r target/site /out/site

FROM debian:bookworm-slim AS runtime
ARG DEBIAN_MIRROR
# Bootstrap verified HTTPS before the slim image can install ca-certificates.
COPY --from=toolchain /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
RUN sed -i -e 's|http://deb.debian.org/debian-security|https://security.debian.org/debian-security|g' \
    -e "s|http://deb.debian.org/debian|${DEBIAN_MIRROR}|g" /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=5 update \
    && apt-get -o Acquire::Retries=5 install -y --no-install-recommends ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 app \
    && useradd --system --uid 10001 --gid app --home /data --shell /usr/sbin/nologin app \
    && mkdir -p /data /app && chown app:app /data /app
WORKDIR /app
COPY --from=builder --chown=app:app /out/imf /app/imf
COPY --from=builder --chown=app:app /out/site /app/site
USER 10001:10001
ENV IMF_DATA_DIR=/data \
    TMPDIR=/data \
    LEPTOS_OUTPUT_NAME=imf \
    LEPTOS_SITE_ROOT=/app/site \
    LEPTOS_SITE_PKG_DIR=pkg \
    LEPTOS_SITE_ADDR=0.0.0.0:3000 \
    SITE_ADDR=0.0.0.0:3000 \
    RUST_LOG=info
VOLUME ["/data"]
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=3 \
    CMD curl --fail --silent http://127.0.0.1:3000/api/health/live || exit 1
ENTRYPOINT ["/app/imf"]
