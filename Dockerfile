# syntax=docker/dockerfile:1.7

ARG RUST_VERSION=1.97

FROM rust:${RUST_VERSION}-bookworm AS build
WORKDIR /src

RUN rustup target add wasm32-unknown-unknown

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p tinybird-wasm --target wasm32-unknown-unknown && \
    cp target/wasm32-unknown-unknown/release/tinybird_wasm.wasm /tmp/tinybird_wasm.wasm

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --locked --release -p tinybird-web && \
    cp target/release/tinybird-web /tmp/tinybird-web

FROM debian:bookworm-slim AS runtime

RUN apt-get update && \
    apt-get install --yes --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/* && \
    useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin tinybird && \
    install -d -o tinybird -g tinybird /app/data /app/data/sprites

WORKDIR /app

COPY --from=build /tmp/tinybird-web /usr/local/bin/tinybird-web
COPY --from=build /tmp/tinybird_wasm.wasm /app/tinybird_wasm.wasm
COPY --chown=tinybird:tinybird addons /app/addons

ENV TINYBIRD_WEB_HOST=0.0.0.0 \
    TINYBIRD_WEB_PORT=8877 \
    TINYBIRD_WEB_WASM=/app/tinybird_wasm.wasm \
    TINYBIRD_WEB_SNAPSHOT=/app/data/current-game.json \
    TINYBIRD_WEB_SPRITES=/app/data/sprites \
    TINYBIRD_ADDONS=/app/addons \
    TINYBIRD_LOCAL_ROMS=off \
    TINYBIRD_WEB_OVERLAY=off

USER tinybird
EXPOSE 8877

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8877/api/health > /dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/tinybird-web"]
