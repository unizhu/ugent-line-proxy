# Build and run ugent-line-proxy.
#
# On an Apple Silicon host, produce a linux/amd64 binary with:
#
#   docker buildx build --platform linux/amd64 \
#     --target export --output type=local,dest=dist .
#
# OrbStack runs the amd64 build under Rosetta. For a runnable image instead of
# a bare binary, drop --target/--output and use --load.

ARG RUST_VERSION=1.96

# ─── Build ──────────────────────────────────────────────────────────

FROM rust:${RUST_VERSION}-slim-bookworm AS builder

WORKDIR /app

# Dependencies first, so edits to src/ do not invalidate the layer.
COPY Cargo.toml Cargo.lock ./
# Stub every target declared in Cargo.toml, or target resolution fails.
RUN mkdir -p src/bin \
    && echo 'fn main() {}' > src/main.rs \
    && echo 'fn main() {}' > src/bin/rms-cli.rs \
    && echo '' > src/lib.rs \
    && cargo build --release --locked \
    && rm -rf src

COPY src ./src
COPY tests ./tests

# Touch the real sources so cargo rebuilds them over the dependency layer.
RUN touch src/main.rs src/lib.rs src/bin/rms-cli.rs \
    && cargo build --release --locked --bin ugent-line-proxy \
    && strip target/release/ugent-line-proxy

# ─── Export: binary only ────────────────────────────────────────────

FROM scratch AS export
COPY --from=builder /app/target/release/ugent-line-proxy /

# ─── Runtime image ──────────────────────────────────────────────────

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --system --create-home --shell /usr/sbin/nologin ugent
USER ugent

COPY --from=builder /app/target/release/ugent-line-proxy /usr/local/bin/

EXPOSE 3000
ENTRYPOINT ["ugent-line-proxy"]
