# hbbs/hbbr built from the pinned upstream rustdesk-server with hjviewer's patches.
# Build from the repository root: docker build -f deploy/Dockerfile -t hjviewer-server .
# REGISTRY can point at a mirror of the official images, e.g. public.ecr.aws/docker/library.
ARG REGISTRY=docker.io/library
FROM ${REGISTRY}/rust:1-bookworm AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends git pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY scripts/ scripts/
COPY server/patches/ server/patches/
RUN UPSTREAM_DIR=/src/upstream scripts/fetch-upstream.sh server
RUN cd upstream/rustdesk-server \
    && cargo build --release --locked --bin hbbs --bin hbbr \
    && cp target/release/hbbs target/release/hbbr /usr/local/bin/

FROM ${REGISTRY}/debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/hbbs /usr/local/bin/hbbr /usr/local/bin/
# id_ed25519(.pub) and db_v2.sqlite3 live here
WORKDIR /data
VOLUME /data
