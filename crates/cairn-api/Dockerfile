# Built early (M2) on purpose: a deploy that only gets exercised on ship day
# is a deploy that fails on ship day.
FROM rust:1.89-slim-bookworm AS build
WORKDIR /app

RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Manifests first so the dependency layer survives source edits.
COPY Cargo.toml Cargo.lock* ./
COPY crates/cairn-core/Cargo.toml crates/cairn-core/
COPY crates/cairn-api/Cargo.toml  crates/cairn-api/
COPY crates/cairn-cli/Cargo.toml  crates/cairn-cli/
COPY programs/cairn/Cargo.toml    programs/cairn/
RUN mkdir -p crates/cairn-core/src crates/cairn-api/src crates/cairn-cli/src programs/cairn/src \
 && echo 'pub fn stub() {}' > crates/cairn-core/src/lib.rs \
 && echo 'pub fn stub() {}' > crates/cairn-api/src/lib.rs \
 && echo 'fn main() {}'     > crates/cairn-api/src/main.rs \
 && echo 'fn main() {}'     > crates/cairn-cli/src/main.rs \
 && echo 'pub fn stub() {}' > programs/cairn/src/lib.rs \
 && cargo build --release -p cairn-api --bin cairn-api || true

COPY crates ./crates
COPY programs ./programs
# Touch so cargo does not reuse the stub fingerprints.
RUN find crates programs -name '*.rs' -exec touch {} + \
 && cargo build --release -p cairn-api --bin cairn-api

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --system --uid 10001 cairn
WORKDIR /app
COPY --from=build /app/target/release/cairn-api /usr/local/bin/cairn-api
COPY crates/cairn-api/migrations ./migrations

# The volume holds the SQLite read model and, without R2, the audio blobs.
# Both are reconstructible: the escrows come back from a resync, and losing a
# blob downgrades a verification rather than invalidating it.
RUN mkdir -p /data/blobs && chown -R cairn /data /app
USER cairn
ENV DATABASE_URL=sqlite:///data/cairn.db?mode=rwc \
    BLOB_DIR=/data/blobs \
    BIND=0.0.0.0:8080
EXPOSE 8080
CMD ["cairn-api"]
