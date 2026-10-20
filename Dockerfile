# Kessel container image, also used by the Home Assistant add-on.
#
#     docker build -t kessel .
#     docker run --rm -p 8099:8099 -v kessel-data:/data kessel
#
# Every stage copies an explicit list of paths, so local secrets/, target/ and .git/ never reach a layer.

ARG RUNTIME_BASE=docker.io/library/debian:bookworm-slim
ARG NODE_VERSION=22
ARG BUN_VERSION=1.3.5
ARG RUST_VERSION=1.97.1

# The web UI is built first: the server embeds apps/web/build into its binary.
FROM docker.io/library/node:${NODE_VERSION}-bookworm-slim AS web
ARG BUN_VERSION
RUN npm install --global "bun@${BUN_VERSION}"
WORKDIR /src
COPY package.json bun.lock bunfig.toml ./
COPY apps/web apps/web
COPY packages/api-types packages/api-types
COPY e2e/package.json e2e/package.json
RUN bun install --frozen-lockfile \
    && bun run --filter web build \
    && test -s apps/web/build/index.html

FROM docker.io/library/rust:${RUST_VERSION}-bookworm AS build
ARG KESSEL_VERSION=""
# perl, make and gcc build the vendored OpenSSL that the TLS-PSK transport needs.
RUN apt-get update \
    && apt-get install -y --no-install-recommends perl make gcc \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY rust-toolchain.toml Cargo.toml Cargo.lock clippy.toml rustfmt.toml README.md ./
COPY crates crates
COPY apps/server apps/server
COPY apps/sim apps/sim
COPY --from=web /src/apps/web/build apps/web/build
RUN KESSEL_VERSION="${KESSEL_VERSION}" cargo build -p server --release --locked --features tls-psk \
    && test -x target/release/kessel

FROM ${RUNTIME_BASE} AS runtime
LABEL org.opencontainers.image.title="Kessel" \
      org.opencontainers.image.description="Local Home Connect bridge for Home Assistant" \
      org.opencontainers.image.source="https://github.com/ediiiz/Kessel" \
      org.opencontainers.image.licenses="MIT"
RUN apt-get update \
    && apt-get install -y --no-install-recommends jq ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/kessel /usr/local/bin/kessel
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod 0755 /entrypoint.sh /usr/local/bin/kessel && mkdir -p /data
ENV KESSEL_TOKEN_FILE=/data/kessel.token \
    KESSEL_CONFIG_FILE=/data/kessel-config.json \
    KESSEL_PROFILE_DIR=/data/profiles
WORKDIR /data
VOLUME ["/data"]
EXPOSE 8099
CMD ["/entrypoint.sh"]
