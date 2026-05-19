# Stage 1: Build
FROM rust:1.86-bookworm@sha256:300ec56abce8cc9448ddea2172747d048ed902a3090e6b57babb2bf19f754081 AS builder
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler pkg-config sqlite3 && \
    rm -rf /var/lib/apt/lists/*
RUN cargo install sqlx-cli --version '~0.8' --no-default-features --features sqlite,rustls
WORKDIR /build
COPY . .
ENV DATABASE_URL=sqlite:./db_v2.sqlite3
RUN make init-db
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim@sha256:67b30a61dc87758f0caf819646104f29ecbda97d920aaf5edc834128ac8493d3
RUN apt-get update && apt-get install -y --no-install-recommends libsqlite3-0 ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /build/target/release/hbbs /usr/bin/hbbs
COPY --from=builder /build/target/release/hbbr /usr/bin/hbbr
COPY --from=builder /build/target/release/rustdesk-utils /usr/bin/rustdesk-utils
WORKDIR /root
ENV HOME=/root
