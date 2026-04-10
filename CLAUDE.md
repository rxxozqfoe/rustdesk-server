# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

Fork of RustDesk Server with API integration enhancements: JWT-based authentication, mandatory login support (`MUST_LOGIN`), and WebSocket client support (client >= 1.4.1). Companion API project: https://github.com/lejianwen/rustdesk-api
The main branch is "forapi".

## Build & Development Commands

```bash
# First-time setup (requires submodules + database)
git submodule update --init --recursive
make init-db                             # Create SQLite DB and run migrations

# Build
cargo build
cargo build --release    # LTO enabled, stripped binaries

# Run binaries
cargo run                # hbbs (rendezvous server, default-run)
cargo run --bin hbbr     # relay server
cargo run --bin rustdesk-utils  # key generation & diagnostics

# Test, lint, format (matches CI)
cargo test --all
cargo fmt --all -- --check
cargo clippy --all -- -D warnings
cargo check
```

## Architecture

Three binaries from a single workspace:

- **hbbs** (`src/main.rs`) — Rendezvous/ID server (port 21116 TCP/UDP). Handles peer registration, lookup, relay discovery. Supports TCP and WebSocket(Secure) transports.
- **hbbr** (`src/hbbr.rs`) — Relay server (port 21117). Proxies TCP streams between peers with configurable bandwidth limiting and connection downgrade thresholds.
- **rustdesk-utils** (`src/utils.rs`) — CLI for ed25519 key pair generation, validation, and server connectivity diagnostics.

### Key source files

- `src/rendezvous_server.rs` — Core rendezvous logic (largest file ~62KB): peer registration, WebSocket/TCP handling, encryption, IP blocking
- `src/relay_server.rs` — Relay logic with bandwidth limiting and blacklist management
- `src/common.rs` — Shared utilities, CLI argument parsing, config file loading (INI format via `--config`)
- `src/database.rs` — SQLite peer database with sqlx connection pooling and auto-migration
- `src/peer.rs` — Peer state management and IP-based blocking/rate limiting
- `src/jwt.rs` — JWT token generation/validation for API authentication

### Shared library

`libs/hbb_common` is a **git submodule** containing protocol definitions (protobuf-based `rendezvous_proto`), TCP utilities, and cryptographic helpers using sodiumoxide.

## Key Configuration

Environment variables (also configurable via INI file with `--config`):

| Variable | Purpose |
|----------|---------|
| `DB_URL` | SQLite database path (default: `db_v2.sqlite3`) |
| `RELAY_SERVERS` | Comma-separated relay server addresses |
| `KEY` | Encryption key (`_` = accept any key) |
| `MUST_LOGIN` | Require login to connect (`Y`/`N`, default `N`) |
| `RUSTDESK_API_JWT_KEY` | JWT secret for API token validation |
| `LIMIT_SPEED` / `SINGLE_BANDWIDTH` / `TOTAL_BANDWIDTH` | Rate limiting |
| `DOWNGRADE_START_CHECK` / `DOWNGRADE_THRESHOLD` | Connection downgrade tuning |

## Database (SQLx Online Mode)

Schema is managed via sqlx migrations in `migrations/`. First-time setup:

```bash
make init-db       # sqlx database create + migrate run
```

Other database commands:

```bash
make migrate       # Run pending migrations
make reset-db      # Drop and recreate database
```

- `DATABASE_URL` in `.env` points to the SQLite file (default: `sqlite:./db_v2.sqlite3`)
- `sqlx::query!` macros validate SQL at compile time against the live database (online mode)
- The server also runs `sqlx::migrate!()` at startup, so `cargo run` auto-migrates
- `.sqlx/` offline cache is not used — CI must run `make init-db` before `cargo build`

## Notable Dependencies

- **sqlx 0.8** with compile-time SQL validation (online mode) — requires `make init-db` before first build
- **sodiumoxide** for ed25519 signing and encryption
- **axum 0.5** for HTTP endpoints
- **tokio-tungstenite 0.17** for WebSocket support
- Protocol buffers via `hbb_common::rendezvous_proto`
