ARG DOCKER_REGISTRY
ARG DOCKER_ALPINE_VERSION=latest

# A static musl binary, the server only: the web client is on hold. The dependencies are built
# first against empty sources, from the manifests alone, so that a change in the sources reuses
# their layer. Cargo reads every workspace member's manifest, so each member gets an empty source
# here. The real sources are touched afterwards: COPY keeps their modification times, which can be
# older than the empty build's, and cargo would then keep the empty binary. moq/, web/wasm/ and
# android/ffi/ keep their empty sources: a release build of the server never compiles them.
FROM ${DOCKER_REGISTRY}rust:alpine${DOCKER_ALPINE_VERSION} AS builder
RUN apk add --no-cache make gcc musl-dev
WORKDIR /keryx
COPY Cargo.toml Cargo.lock ./
COPY server/Makefile server/Cargo.toml server/
COPY core/Cargo.toml core/
COPY moq/Cargo.toml moq/
COPY web/wasm/Cargo.toml web/wasm/
COPY android/ffi/Cargo.toml android/ffi/
RUN for member in server core moq web/wasm android/ffi; do \
        mkdir -p $member/src && touch $member/src/lib.rs; \
    done \
    && echo 'fn main() {}' > server/src/main.rs \
    && make -C server release \
    && rm -r server/src core/src server/build
COPY server/src server/src
COPY core/src core/src
RUN touch server/src/main.rs server/src/lib.rs core/src/lib.rs \
    && make -C server release && mv server/build/keryx-* server/build/keryx


FROM ${DOCKER_REGISTRY}alpine:${DOCKER_ALPINE_VERSION}
ARG KERYX_VERSION
LABEL "org.opencontainers.image.authors"="pouriya.jahanbakhsh@gmail.com"
LABEL "org.opencontainers.image.title"="keryx"
LABEL "org.opencontainers.image.description"="A self-hosted messenger: chat, calls, MCP and end-to-end encryption"
LABEL "org.opencontainers.image.version"="${KERYX_VERSION}"

COPY --from=builder /keryx/server/build/keryx /usr/local/bin/keryx

# Everything lives in one volume, /var/lib/keryx: the configuration at server.toml, which the image
# does not carry, the database, files/ and an empty www/ for [www]. Start from server/server.toml,
# with `listen` on 0.0.0.0 and every path under /var/lib/keryx. Without it the container stops
# with an error naming the missing file.
RUN mkdir -p /var/lib/keryx/files /var/lib/keryx/www
VOLUME /var/lib/keryx
EXPOSE 7777

# No arguments: serve. Arguments: run them as a keryx command with the same configuration, e.g.
# `docker exec <container> keryx --config /var/lib/keryx/server.toml user add <username>`.
ENTRYPOINT ["keryx", "--config", "/var/lib/keryx/server.toml"]
CMD ["serve"]
