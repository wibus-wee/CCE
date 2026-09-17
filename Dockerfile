FROM node:24-bookworm-slim AS web
RUN corepack enable
WORKDIR /src
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY apps/web/package.json apps/web/package.json
RUN pnpm install --frozen-lockfile
COPY apps/web apps/web
RUN pnpm web:build

FROM rust:1.97.1-bookworm AS rust
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
COPY apps/cli apps/cli
COPY apps/daemon apps/daemon
COPY apps/gateway apps/gateway
COPY apps/mcp apps/mcp
COPY apps/push apps/push
RUN cargo build --release --locked --workspace

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home cce \
    && install -d -o cce -g cce /var/lib/cce /var/lib/gateway
COPY --from=rust /src/target/release/cce /usr/local/bin/cce
COPY --from=rust /src/target/release/cce-daemon /usr/local/bin/cce-daemon
COPY --from=rust /src/target/release/cce-gateway /usr/local/bin/cce-gateway
COPY --from=rust /src/target/release/cce-mcp /usr/local/bin/cce-mcp
COPY --from=rust /src/target/release/cce-push /usr/local/bin/cce-push
COPY --from=web /src/apps/web/dist /opt/cce/web
USER cce
WORKDIR /repository
EXPOSE 7734 7735
HEALTHCHECK --interval=30s --timeout=5s --start-period=120s \
    CMD curl -fsS http://127.0.0.1:7734/healthz || exit 1
ENTRYPOINT ["cce-daemon"]
CMD [".", "--bind", "0.0.0.0:7734", "--allow-non-loopback", "--web-root", "/opt/cce/web"]
