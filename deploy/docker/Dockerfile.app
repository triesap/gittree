FROM rust:1.92-bookworm AS builder
WORKDIR /app
RUN apt-get update \
    && apt-get install -y --no-install-recommends binaryen ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
RUN cargo install trunk --locked
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY services ./services
COPY migrations ./migrations
COPY refs/mf2-i18n ./refs/mf2-i18n
RUN cargo build -p gittree-app --release
WORKDIR /app/crates/app-ui
RUN trunk build --release --dist dist

FROM debian:bookworm-slim
RUN useradd -m -u 10001 gittree
WORKDIR /app
COPY --from=builder /app/target/release/gittree-app /usr/local/bin/gittree-app
COPY --from=builder /app/crates/app-ui/dist /app/crates/app-ui/dist
USER gittree
ENTRYPOINT ["/usr/local/bin/gittree-app"]
