# syntax=docker/dockerfile:1
#
# Both stages use Chainguard images instead of the Docker Hub
# rust:bookworm + debian:bookworm-slim pair. Chainguard publishes
# sigstore-keyless signatures and SLSA provenance for every published
# digest, so .github/cosign-image-policy.json can enforce a real signed
# policy on FROM instead of allow-listing the previous Docker Hub
# images as `unsigned`.
#
# The free Chainguard registry only ships floating :latest / :latest-dev
# tags; Renovate's docker:pinDigests preset keeps the @sha256 references
# below current.

# Stage 1: Build — Chainguard rust:latest-dev ships rustup, cargo,
# rustc 1.95.0 (the version pinned by rust-toolchain.toml), make,
# pkgconf and apk. Default user is nonroot, so switch to root for the
# apk installs and the cargo install.
FROM cgr.dev/chainguard/rust:latest-dev@sha256:90c1dcb5dc075764ce9630493eee58a22ca28033152accd0401cdf931924708f AS builder
USER root
WORKDIR /work
# Single RUN: hadolint DL3059 (consecutive RUN instructions). --root
# /usr/local on cargo install so the sqlx binary lands in a PATH that
# `make init-db` can resolve without ENV gymnastics.
RUN apk add --no-cache protobuf-dev sqlite-dev && \
    cargo install --root /usr/local sqlx-cli --version '~0.8' --no-default-features --features sqlite,rustls
COPY . .
ENV DATABASE_URL=sqlite:./db_v2.sqlite3
RUN make init-db
RUN cargo build --release

# Stage 2: Runtime — Chainguard wolfi-base ships apk and
# ca-certificates-bundle out of the box; only sqlite-libs needs to be
# pulled in for the hbbs/hbbr/rustdesk-utils binaries to dlopen at
# runtime.
FROM cgr.dev/chainguard/wolfi-base:latest@sha256:315732e5ca8b9f9285ed36ce9a5bb2a99f700ca8f0570d7061f9a4987fcf6688
RUN apk add --no-cache sqlite-libs
COPY --from=builder /work/target/release/hbbs /usr/bin/hbbs
COPY --from=builder /work/target/release/hbbr /usr/bin/hbbr
COPY --from=builder /work/target/release/rustdesk-utils /usr/bin/rustdesk-utils
WORKDIR /root
ENV HOME=/root
