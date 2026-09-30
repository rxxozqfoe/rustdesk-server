# syntax=docker/dockerfile:1@sha256:4edf897a3ffa55b89f906fc8cc78afdb3f1834cc9c7083565e611a8a7d5fe99e
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
#
# Build layout (cargo-chef): `chef` holds the toolchain, `planner`
# reduces the workspace to a dependency recipe, and `builder` compiles
# the dependencies from that recipe in their own layer before copying
# the sources. A source-only change reuses the cached dependency layer
# instead of recompiling the whole dependency tree; the layer is rebuilt
# only when Cargo.toml / Cargo.lock change.

# Stage 1: Toolchain — Chainguard rust:latest-dev ships rustup, cargo,
# make, pkgconf and apk. Default user is nonroot, so switch to root for
# the apk installs and the cargo install. The rustc used is the one
# pinned by rust-toolchain.toml (the same one CI lints with), not the
# image's default; the build fails if rustup does not honour the pin.
# The image's cargo/rustc on PATH are apk-packaged binaries, not rustup
# proxies, so they ignore rust-toolchain.toml. The pinned toolchain's
# directory is therefore linked to a fixed path that leads PATH.
# openssl-dev: hbb_common enables tokio-tungstenite's native-tls on every
# target (since the 1.4.9 protocol bump), so openssl-sys links OpenSSL.
FROM cgr.dev/chainguard/rust:latest-dev@sha256:80beb4904bcba0b5f138140ab816148961859573e2ccbb5e9d28ea9d279bf54d AS chef
USER root
WORKDIR /work
ENV PATH="/opt/rust-toolchain/bin:${PATH}"
COPY rust-toolchain.toml ./
RUN apk add --no-cache openssl-dev protobuf-dev sqlite-dev && \
    rustup toolchain install && \
    ln -s "$(dirname "$(dirname "$(rustup which rustc)")")" /opt/rust-toolchain && \
    pinned="$(sed -n 's/^channel = "\(.*\)"$/\1/p' rust-toolchain.toml)" && \
    case "$(rustc --version)" in \
      "rustc ${pinned} "*) ;; \
      *) echo "rustc $(rustc --version) does not match rust-toolchain.toml (${pinned})" >&2; exit 1 ;; \
    esac && \
    cargo install --locked --root /usr/local cargo-chef --version 0.1.78
# sqlx::query! macros compile against the committed .sqlx/ metadata
# instead of a live database, so no sqlx-cli / `make init-db` here.
# Migrations are still embedded by sqlx::migrate!() and run at startup.
ENV SQLX_OFFLINE=true

# Stage 2: Dependency recipe. Only recipe.json leaves this stage, and it
# changes only when the manifests or the lockfile do.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: Build — dependencies first (cached layer), then the sources.
FROM chef AS builder
COPY --from=planner /work/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

# Stage 4: Runtime — Chainguard wolfi-base ships apk and
# ca-certificates-bundle out of the box; sqlite-libs and libssl3 (with
# libcrypto3) need to be pulled in for the hbbs/hbbr/rustdesk-utils
# binaries to load at runtime.
FROM cgr.dev/chainguard/wolfi-base:latest@sha256:bef0f4d47edc72a93d1537eae54eb53db2b2cc352c028128ff0f16c5b5a3c1e4
RUN apk add --no-cache sqlite-libs libssl3
COPY --from=builder /work/target/release/hbbs /usr/bin/hbbs
COPY --from=builder /work/target/release/hbbr /usr/bin/hbbr
COPY --from=builder /work/target/release/rustdesk-utils /usr/bin/rustdesk-utils
WORKDIR /root
ENV HOME=/root
