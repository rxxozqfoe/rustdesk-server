# rustdesk-server (rxxozqfoe fork)

[![build](https://github.com/rxxozqfoe/rustdesk-server/actions/workflows/build.yaml/badge.svg)](https://github.com/rxxozqfoe/rustdesk-server/actions/workflows/build.yaml)

A fork of [lejianwen/rustdesk-server](https://github.com/lejianwen/rustdesk-server) (in turn forked from [rustdesk/rustdesk-server](https://github.com/rustdesk/rustdesk-server)). Default integration branch is `forapi`.

## Fork-specific changes

- Fixes the connection-timeout issue when the client is logged into an `API` account.
- Adds `MUST_LOGIN` env var: defaults to `N`; set to `Y` to require the client to be logged in before a connection is accepted.
- Adds `RUSTDESK_API_JWT_KEY`: when set, hbbs validates the JWT issued by rustdesk-api.
- Adds client WebSocket support (client version ≥ 1.4.1).
- Switches sqlx to online mode with bundled SQLite migrations (`migrations/`); `make init-db` is required before the first build.

## Published image

| Item | Value |
|---|---|
| Registry | `ghcr.io/rxxozqfoe/rustdesk-server` |
| Architectures | `linux/amd64`, `linux/arm64` (single multi-arch image) |
| Contents | `hbbs`, `hbbr`, `rustdesk-utils` binaries only (no s6 init, no bundled API) |
| Supply-chain attestations | keyless cosign signature + SBOM + SLSA provenance |
| Triggers | manual `workflow_dispatch`, or push of an `N.N.N-mycustom.N` tag |

> This fork **no longer publishes** the s6 variant, Docker Hub images, Windows binaries, or `.deb` packages.
> If you need an API, deploy [rustdesk-api](https://github.com/lejianwen/rustdesk-api) as a separate service.

## Quick deploy

The simplest path is the two-container `docker-compose.yml` in this repo (`hbbs` + `hbbr`, sharing `./data:/root` for the ed25519 key):

```bash
git clone https://github.com/rxxozqfoe/rustdesk-server.git
cd rustdesk-server
# Edit docker-compose.yml — replace `-r <relay-server-ip[:21117]>` on
# the hbbs service with a relay address your clients can reach, then:
docker compose up -d
```

Or skip compose and run the two containers directly (`--net=host` is required for P2P direct connections):

```bash
docker run -d --name hbbs --net=host \
  -v "$PWD/data:/root" \
  ghcr.io/rxxozqfoe/rustdesk-server:latest \
  hbbs -r <relay-server-ip[:21117]>

docker run -d --name hbbr --net=host \
  -v "$PWD/data:/root" \
  ghcr.io/rxxozqfoe/rustdesk-server:latest \
  hbbr
```

On first start the ed25519 keypair (`id_ed25519` / `id_ed25519.pub`) is generated under `data/`. Both containers **must** share the same volume so they use the same key.

## Ports

| Port | Protocol | Service | Purpose |
|---|---|---|---|
| 21115 | TCP | hbbs | Legacy heartbeat |
| 21116 | TCP + UDP | hbbs | Rendezvous (registration / lookup) |
| 21117 | TCP | hbbr | Relay |
| 21118 | TCP | hbbs | WebSocket |
| 21119 | TCP | hbbr | WebSocket relay |

## Environment variables

| Variable | Default | Purpose |
|---|---|---|
| `KEY` | auto-generated | Encryption key for connections; `_` accepts any key (testing only) |
| `RELAY_SERVERS` | — | Comma-separated relay addresses; equivalent to `hbbs -r` |
| `MUST_LOGIN` | `N` | When `Y`, clients must be logged into the API before connecting |
| `RUSTDESK_API_JWT_KEY` | — | JWT signing key from rustdesk-api; when set, hbbs validates tokens |
| `DB_URL` | `db_v2.sqlite3` | SQLite database path |
| `LIMIT_SPEED` / `SINGLE_BANDWIDTH` / `TOTAL_BANDWIDTH` | — | Bandwidth limiting |
| `DOWNGRADE_START_CHECK` / `DOWNGRADE_THRESHOLD` | — | Connection-downgrade thresholds |

An INI configuration file can also be passed via `--config`; see `src/common.rs`.

## Build from source

```bash
git submodule update --init --recursive
cargo install sqlx-cli --no-default-features --features sqlite    # first time only
make init-db                                                       # create the SQLite DB and run migrations
cargo build --release
```

> **Note:** This project uses sqlx compile-time SQL validation (online mode). You must run `make init-db` before the first build so a local database with the correct schema exists.

Other database commands:

```bash
make migrate     # run pending migrations
make reset-db    # drop and recreate the database
```

Build output lands in `target/release/`:

- `hbbs` — rendezvous / ID server
- `hbbr` — relay server
- `rustdesk-utils` — CLI utilities (key generation, diagnostics)

## Credits

- Upstream RustDesk Server: [rustdesk/rustdesk-server](https://github.com/rustdesk/rustdesk-server)
- Direct fork source: [lejianwen/rustdesk-server](https://github.com/lejianwen/rustdesk-server)

## License

Inherits the AGPL-3.0 license from upstream RustDesk Server. See [`LICENSE`](./LICENSE).
