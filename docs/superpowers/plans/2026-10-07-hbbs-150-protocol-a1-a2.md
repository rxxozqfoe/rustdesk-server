# hbbs 1.5.0 protocol, A1 + A2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bump hbbs to hbb_common 229b904 with no behaviour change (A1). Then negotiate Kx v1 with RustDesk 1.5.0 clients, keep v0 for older ones, and provide a `KX_MAX_VERSION` switch back to v0 (A2).

**Architecture:** A1 changes the submodule pin and the one import it breaks. A2 first gives every rendezvous connection a small state struct (`Conn`), which holds the send half, a receive cipher of its own, and the pending phase-1 secret. It then rewrites phase 1 and phase 2 of the key exchange on that state, using hbb_common's `Encrypt::decode` / `Encrypt::new_split`. Last, A2 adds per-outcome counters and a 10-minute summary log line. A3 (WebRTC signalling) builds on `Conn` and gets its own plan after A2 has been tested on real clients.

**Tech Stack:** Rust 1.98.1 (`rust-toolchain.toml`), tokio, hbb_common (sodiumoxide, rust-protobuf 3 with `with-bytes`), plus an out-of-repo Rust harness and Python scripts used as tests.

**Spec:** `docs/superpowers/specs/2026-10-07-hbbs-150-protocol-design.md`

## Global Constraints

- Never run `cargo test` (repository rule). The tests are the out-of-repo harness and scripts from Task 1, run against release builds.
- Lint and format only the hbbs package: `cargo fmt -p hbbs -- --check`, `cargo clippy -p hbbs --no-deps -- -D warnings`, `cargo check -p hbbs --all-targets`. Never format or lint `libs/hbb_common`.
- Build with `SQLX_OFFLINE=true` (no database needed).
- hbb_common pin: `229b904508364c8997aad0fb5af57effac859f60`.
- `KX_MAX_VERSION`: environment variable read once at startup; `0` or `1`, default `1`; any other value logs a warning and uses `1`.
- Summary line, exactly: `key exchange (10 min): v1=<n> v0=<n> failed=<n>`, logged at info level every 600 s when any count is non-zero.
- Branch from `forapi`, and open PRs against `forapi`. Commit messages and PR text are in English with no AI-attribution lines.
- Never add cargo-deny ignores. Report new advisories instead.
- Releases (`v1.1.15-mycustom.4` for A1, `v1.1.15-mycustom.5` for A2) and chart tags only after the user's explicit go-ahead. Each tag is an annotated tag, message `Release <tag>`, on the merge commit on `forapi` (or `main` for the chart).
- A2 does not start until the user reports that the A1 test plan passed.

## Review Focus

- A `KeyExchange` that arrives on a WebSocket connection, where hbbs sends no phase 1. Expected: only that connection is closed, the attempt counts as failed, and hbbs keeps running. Covered by the Task 5 test `kx_bad.py ws`.
- A client older than 1.5.0 against a v1-advertising hbbs. It boxes to the key with bit 255 set and picks 0. Expected: the v0 handshake succeeds, and encrypted requests work. Covered by the Task 5 test `kx-harness … v0 1`.
- A second `KeyExchange` on an already-encrypted connection. Expected: the connection is closed. Covered by the Task 5 test `kx-harness … rekey 1`.
- `KX_MAX_VERSION` set to something other than 0 or 1, such as `7`. Expected: a warning, and v1 advertised. Covered by the Task 5 test run with `KX_MAX_VERSION=7`.
- A client that opens TCP and never answers phase 1. Expected: closed by the existing 30 s idle timeout, with no panic and no counter change. Covered by the Task 5 test `kx_bad.py idle`.

## Shared setup for every task

```bash
export SCRATCH=/path/to/your/session/scratchpad   # any empty writable directory
export REPO=/home/user/rustdesk/rustdesk-server
export W=$SCRATCH/hbbs-run                        # hbbs working dir (keys, sqlite db, log)
```

The harness and scripts live in `$SCRATCH` and are **never committed**.

---

### Task 1: Test harness and baseline

**Files (all outside the repository):**
- Create: `$SCRATCH/hbb_common-229b904/` (git checkout)
- Create: `$SCRATCH/kx-harness/Cargo.toml`, `$SCRATCH/kx-harness/rust-toolchain.toml`, `$SCRATCH/kx-harness/src/main.rs`
- Create: `$SCRATCH/kx_bad.py`, `$SCRATCH/run-hbbs.sh`, `$SCRATCH/kx-matrix.sh`
- Create: `$SCRATCH/venv` (Python venv with `websockets`)

**Interfaces:**
- Produces: `kx-harness ADDR PUBKEY_FILE CASE EXPECT_ADVERTISED`, where CASE is `v1|v0|pick2|rekey`. Exit status 0 means the outcome was the expected one.
- Produces: `kx_bad.py MODE`, where MODE is `short|badbox|ws|idle`. Exit status 0 means hbbs closed the connection.
- Produces: `run-hbbs.sh BIN WORKDIR [ENV=VAL…] -- COMMAND…`. It runs COMMAND against a fresh hbbs on 127.0.0.1:31116 (WebSocket on 31118) and exits 99 if hbbs died.
- Produces: `kx-matrix.sh EXPECT_ADVERTISED`. It runs every case for one hbbs instance and exits non-zero on any unexpected result.

- [ ] **Step 1: Check out hbb_common 229b904 for the harness**

```bash
git clone -q https://github.com/rustdesk/hbb_common $SCRATCH/hbb_common-229b904
git -C $SCRATCH/hbb_common-229b904 checkout -q 229b904508364c8997aad0fb5af57effac859f60
git -C $SCRATCH/hbb_common-229b904 log --oneline -1
```

Expected: `229b904 Merge pull request #614 from rustdesk/kx-v2`

- [ ] **Step 2: Create the harness crate**

`$SCRATCH/kx-harness/Cargo.toml`:

```toml
[package]
name = "kx-harness"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
hbb_common = { path = "../hbb_common-229b904" }

# Keep cargo from looking for a parent workspace.
[workspace]
```

`$SCRATCH/kx-harness/rust-toolchain.toml`:

```bash
cp $REPO/rust-toolchain.toml $SCRATCH/kx-harness/rust-toolchain.toml
```

`$SCRATCH/kx-harness/src/main.rs`:

```rust
//! Plays a RustDesk 1.5.0 client against hbbs's TCP rendezvous port. It runs
//! the key exchange the way src/common.rs `key_exchange` does at client tag
//! 1.5.0, then checks that encrypted requests get decryptable answers.
//!
//! usage: kx-harness HOST:PORT PATH/TO/id_ed25519.pub CASE EXPECT_ADVERTISED
//!   CASE  v1     pick the newest version both speak (a 1.5.0 client)
//!         v0     ignore the advertisement (a client older than 1.5.0)
//!         pick2  claim version 2: hbbs must close the connection
//!         rekey  send a second KeyExchange: hbbs must close the connection
//!   EXPECT_ADVERTISED  the version hbbs must advertise in phase 1 (0 or 1)
//! Exit status 0 when everything went as the case expects.

use hbb_common::{
    anyhow::{anyhow, bail},
    base64::{engine::general_purpose::STANDARD, Engine as _},
    protobuf::Message as _,
    rendezvous_proto::*,
    sodiumoxide::crypto::{box_, secretbox, sign},
    tcp::{FramedStream, KxTranscript, KX_PARAMS_DOMAIN},
    tokio, ResultType,
};

const TIMEOUT_MS: u64 = 3_000;

async fn recv(conn: &mut FramedStream) -> ResultType<RendezvousMessage> {
    match conn.next_timeout(TIMEOUT_MS).await {
        Some(Ok(bytes)) => Ok(RendezvousMessage::parse_from_bytes(&bytes)?),
        Some(Err(err)) => bail!("read failed: {err}"),
        None => bail!("connection closed or timed out"),
    }
}

/// Runs the key exchange. Returns the stream with its cipher set, the version
/// hbbs advertised and the version this client picked.
async fn key_exchange(
    addr: &str,
    server_pk: &sign::PublicKey,
    pick: &dyn Fn(u32) -> u32,
) -> ResultType<(FramedStream, u32, u32)> {
    let mut conn = FramedStream::new(addr, None, TIMEOUT_MS).await?;
    let Some(rendezvous_message::Union::KeyExchange(ex)) = recv(&mut conn).await?.union else {
        bail!("phase 1 is not a KeyExchange");
    };
    if ex.keys.len() != 1 {
        bail!("phase 1 carries {} keys", ex.keys.len());
    }
    let signed_pk = sign::verify(&ex.keys[0], server_pk)
        .map_err(|_| anyhow!("phase 1 signature does not verify"))?;
    let pk: [u8; 32] = signed_pk
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("phase 1 key is not 32 bytes"))?;
    if pk[31] & 0x80 != 0 || !ex.signed_params.is_empty() {
        let signed = sign::verify(&ex.signed_params, server_pk)
            .map_err(|_| anyhow!("signed_params signature does not verify"))?;
        let params = signed
            .strip_prefix(KX_PARAMS_DOMAIN)
            .ok_or_else(|| anyhow!("signed_params lack the domain prefix"))?;
        let params = KxParams::parse_from_bytes(params)?;
        if params.pk[..] != pk[..] || params.version != ex.version {
            bail!("signed_params do not match the phase 1 key or version");
        }
    }
    let picked = pick(ex.version);
    let (our_pk, our_sk) = box_::gen_keypair();
    let key = secretbox::gen_key();
    let nonce = box_::Nonce([0u8; box_::NONCEBYTES]);
    let sealed = box_::seal(&key.0, &nonce, &box_::PublicKey(pk), &our_sk);
    let mut msg = RendezvousMessage::new();
    msg.set_key_exchange(KeyExchange {
        keys: vec![our_pk.0.to_vec().into(), sealed.into()],
        version: picked,
        ..Default::default()
    });
    conn.send(&msg).await?;
    if picked == 0 {
        conn.set_key(key);
    } else {
        conn.set_key_split(
            key,
            true,
            &KxTranscript {
                initiator_pk: &our_pk.0,
                responder_pk: &pk,
                advertised: ex.version,
                picked,
            },
        )?;
    }
    Ok((conn, ex.version, picked))
}

async fn check_online(conn: &mut FramedStream) -> ResultType<()> {
    let mut msg = RendezvousMessage::new();
    msg.set_online_request(OnlineRequest {
        id: "kxharness".into(),
        peers: vec!["kxharness-peer".into()],
        ..Default::default()
    });
    conn.send(&msg).await?;
    match recv(conn).await?.union {
        Some(rendezvous_message::Union::OnlineResponse(_)) => Ok(()),
        other => bail!("expected OnlineResponse, got {other:?}"),
    }
}

async fn check_http_proxy(conn: &mut FramedStream) -> ResultType<()> {
    let mut msg = RendezvousMessage::new();
    msg.set_http_proxy_request(HttpProxyRequest {
        method: "GET".into(),
        path: "/api/".into(),
        ..Default::default()
    });
    conn.send(&msg).await?;
    match recv(conn).await?.union {
        // No api is configured locally, so the answer carries an error; what
        // matters is that it arrives encrypted and decrypts.
        Some(rendezvous_message::Union::HttpProxyResponse(_)) => Ok(()),
        other => bail!("expected HttpProxyResponse, got {other:?}"),
    }
}

async fn check_punch(conn: &mut FramedStream, licence_key: &str) -> ResultType<()> {
    let mut msg = RendezvousMessage::new();
    msg.set_punch_hole_request(PunchHoleRequest {
        id: "999999999".into(),
        licence_key: licence_key.into(),
        ..Default::default()
    });
    conn.send(&msg).await?;
    match recv(conn).await?.union {
        // Sent through tcp_punch after hbbs took the sink: proves the send
        // half keeps its cipher.
        Some(rendezvous_message::Union::PunchHoleResponse(_)) => Ok(()),
        other => bail!("expected PunchHoleResponse, got {other:?}"),
    }
}

/// One fresh connection per check: a punch request moves the connection's
/// sink, so it cannot share a connection with the others.
async fn run_checks(
    addr: &str,
    server_pk: &sign::PublicKey,
    licence_key: &str,
    pick: &dyn Fn(u32) -> u32,
    expect_advertised: u32,
) -> bool {
    let mut all_ok = true;
    for name in ["online", "http-proxy", "punch"] {
        let result = async {
            let (mut conn, advertised, picked) = key_exchange(addr, server_pk, pick).await?;
            println!("{name}: advertised={advertised} picked={picked}");
            if advertised != expect_advertised {
                bail!("hbbs advertised {advertised}, expected {expect_advertised}");
            }
            match name {
                "online" => check_online(&mut conn).await,
                "http-proxy" => check_http_proxy(&mut conn).await,
                _ => check_punch(&mut conn, licence_key).await,
            }
        }
        .await;
        match &result {
            Ok(()) => println!("{name}: ok"),
            Err(err) => println!("{name}: FAIL {err}"),
        }
        all_ok &= result.is_ok();
    }
    all_ok
}

/// The key exchange must be refused: after it (and after a second
/// KeyExchange when `rekey`), hbbs must have closed the connection.
async fn expect_closed(
    addr: &str,
    server_pk: &sign::PublicKey,
    pick: &dyn Fn(u32) -> u32,
    rekey: bool,
    expect_advertised: u32,
) -> bool {
    let (mut conn, advertised, picked) = match key_exchange(addr, server_pk, pick).await {
        Ok(x) => x,
        Err(err) => {
            println!("key exchange: FAIL {err}");
            return false;
        }
    };
    println!("advertised={advertised} picked={picked}");
    if advertised != expect_advertised {
        println!("FAIL hbbs advertised {advertised}, expected {expect_advertised}");
        return false;
    }
    if rekey {
        let mut msg = RendezvousMessage::new();
        msg.set_key_exchange(KeyExchange {
            keys: vec![vec![0u8; 32].into(), vec![0u8; 48].into()],
            ..Default::default()
        });
        if let Err(err) = conn.send(&msg).await {
            println!("closed: ok (send failed: {err})");
            return true;
        }
    }
    match check_online(&mut conn).await {
        Ok(()) => {
            println!("closed: FAIL, hbbs still answers");
            false
        }
        Err(err) => {
            println!("closed: ok ({err})");
            true
        }
    }
}

async fn run(args: &[String]) -> bool {
    let licence_key = std::fs::read_to_string(&args[2]).expect("read the public key file");
    let licence_key = licence_key.trim();
    let pk = STANDARD.decode(licence_key).expect("public key file is base64");
    let server_pk = sign::PublicKey::from_slice(&pk).expect("public key is 32 bytes");
    let expect: u32 = args[4].parse().expect("EXPECT_ADVERTISED is a number");
    let addr = args[1].as_str();
    match args[3].as_str() {
        "v1" => run_checks(addr, &server_pk, licence_key, &|adv| adv.min(1), expect).await,
        "v0" => run_checks(addr, &server_pk, licence_key, &|_| 0, expect).await,
        "pick2" => expect_closed(addr, &server_pk, &|_| 2, false, expect).await,
        "rekey" => expect_closed(addr, &server_pk, &|adv| adv.min(1), true, expect).await,
        other => {
            eprintln!("unknown case {other}");
            std::process::exit(2)
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!("usage: kx-harness HOST:PORT PUBKEY_FILE v1|v0|pick2|rekey EXPECT_ADVERTISED");
        std::process::exit(2);
    }
    hbb_common::sodiumoxide::init().ok();
    let ok = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(run(&args));
    std::process::exit(if ok { 0 } else { 1 });
}
```

- [ ] **Step 3: Build the harness**

Run: `cargo build --release --manifest-path $SCRATCH/kx-harness/Cargo.toml 2>&1 | tail -3`
Expected: `Finished` with no errors. A compile error in `main.rs` is a harness bug: fix it against the hbb_common 229b904 API (`src/tcp.rs`, `protos/rendezvous.proto`) before going on.

- [ ] **Step 4: Create the scripts and the Python venv**

```bash
python3 -m venv $SCRATCH/venv && $SCRATCH/venv/bin/pip install -q websockets
```

`$SCRATCH/kx_bad.py`:

```python
#!/usr/bin/env python3
"""Throwaway: phase-2 KeyExchange cases hbbs must answer by closing only that
connection. hbbs listens on 127.0.0.1: TCP 31116, WebSocket 31118.

usage: kx_bad.py MODE
  short   TCP, after phase 1: keys = [5 bytes, 5 bytes]
  badbox  TCP, after phase 1: keys = [32 bytes, 48 random bytes]
  ws      WebSocket (no phase 1 there): keys = [32 bytes, 48 random bytes]
  idle    TCP, read phase 1 and send nothing; hbbs closes after 30 s idle
Exit status 0 when hbbs closed the connection.
"""
import os
import socket
import sys


def varint(n):
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        out.append((b | 0x80) if n else b)
        if not n:
            return bytes(out)


def field(num, data):
    return varint((num << 3) | 2) + varint(len(data)) + data


def key_exchange(keys):
    # RendezvousMessage.key_exchange = 25 { KeyExchange.keys = 1 }
    return field(25, b"".join(field(1, k) for k in keys))


def frame(payload):
    # hbb_common BytesCodec: (len << 2) in 1 byte up to 0x3F, else 2 bytes LE | 1
    n = len(payload)
    if n <= 0x3F:
        return bytes([n << 2]) + payload
    return ((n << 2) | 1).to_bytes(2, "little") + payload


def closed_after(sock, wait):
    sock.settimeout(wait)
    try:
        return sock.recv(4096) == b""
    except ConnectionResetError:
        return True
    except socket.timeout:
        return False


mode = sys.argv[1]
if mode == "ws":
    from websockets.exceptions import ConnectionClosed
    from websockets.sync.client import connect

    with connect("ws://127.0.0.1:31118") as ws:
        ws.send(key_exchange([os.urandom(32), os.urandom(48)]))
        try:
            ws.recv(timeout=3)
            closed = False
        except ConnectionClosed:
            closed = True
        except TimeoutError:
            closed = False
else:
    s = socket.create_connection(("127.0.0.1", 31116), timeout=5)
    s.recv(4096)  # phase 1
    if mode == "idle":
        closed = closed_after(s, 40)
    else:
        keys = {"short": [b"\0" * 5, b"\0" * 5], "badbox": [os.urandom(32), os.urandom(48)]}[mode]
        s.sendall(frame(key_exchange(keys)))
        closed = closed_after(s, 3)
print(f"{mode}: {'closed by hbbs' if closed else 'NOT closed'}")
sys.exit(0 if closed else 1)
```

`$SCRATCH/run-hbbs.sh` (then `chmod +x`):

```bash
#!/usr/bin/env bash
# usage: run-hbbs.sh HBBS_BINARY WORKDIR [ENV=VAL ...] -- COMMAND...
# Starts hbbs on 127.0.0.1:31116 in WORKDIR with the given environment, runs
# COMMAND, then stops hbbs. Exit status: COMMAND's, or 99 if hbbs died.
set -u
bin=$1 dir=$2
shift 2
envs=()
while [ $# -gt 0 ] && [ "$1" != "--" ]; do envs+=("$1"); shift; done
shift
mkdir -p "$dir" && cd "$dir" || exit 98
env RUST_LOG=info "${envs[@]}" "$bin" -p 31116 > hbbs.log 2>&1 &
pid=$!
python3 - <<'EOF'
import socket, time
for _ in range(100):
    try:
        socket.create_connection(("127.0.0.1", 31116), timeout=1).close()
        break
    except OSError:
        time.sleep(0.1)
EOF
"$@"
status=$?
python3 -c 'import time; time.sleep(1)'
if kill -0 "$pid" 2>/dev/null; then
    kill "$pid"; wait "$pid" 2>/dev/null
    echo "hbbs: still running"
else
    echo "hbbs: EXITED"; status=99
fi
exit $status
```

`$SCRATCH/kx-matrix.sh` (then `chmod +x`):

```bash
#!/usr/bin/env bash
# usage: kx-matrix.sh EXPECT_ADVERTISED   (run inside run-hbbs.sh, cwd = WORKDIR)
# EXPECT_ADVERTISED 1 adds the cases only a v1 hbbs refuses (pick2, rekey).
set -u
expect=$1
h=$SCRATCH/kx-harness/target/release/kx-harness
py=$SCRATCH/venv/bin/python
fail=0
run() { echo "== $*"; "$@" || { echo "!! UNEXPECTED: $*"; fail=1; }; }
run $h 127.0.0.1:31116 id_ed25519.pub v1 "$expect"
run $h 127.0.0.1:31116 id_ed25519.pub v0 "$expect"
if [ "$expect" = 1 ]; then
    run $h 127.0.0.1:31116 id_ed25519.pub pick2 1
    run $h 127.0.0.1:31116 id_ed25519.pub rekey 1
fi
for m in short badbox ws idle; do run $py $SCRATCH/kx_bad.py $m; done
exit $fail
```

- [ ] **Step 5: Build the baseline hbbs (current `forapi`)**

```bash
cd $REPO && git switch forapi && git pull --ff-only
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-baseline
```

Expected: `Finished`.

- [ ] **Step 6: Run the baseline matrix and record it**

Run: `$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-baseline $W -- $SCRATCH/kx-matrix.sh 0`
Expected: every case passes, `v1` and `v0` print `advertised=0 picked=0`, all four `kx_bad.py` modes print `closed by hbbs`, and the run ends with `hbbs: still running` and exit status 0.

Run: `$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-baseline $W -- $SCRATCH/kx-harness/target/release/kx-harness 127.0.0.1:31116 id_ed25519.pub v1 1`
Expected: **FAIL**, `hbbs advertised 0, expected 1`, exit status 1. This is the failing test that A2 must turn green.

Nothing is committed in this task.

---

### Task 2: A1, bump hbb_common to 229b904

**Files:**
- Modify: `libs/hbb_common` (submodule pin)
- Modify: `Cargo.lock`
- Modify: `src/rendezvous_server.rs` (one line in `get_pk`: `hbb_common::message_proto::IdPk`)

**Interfaces:**
- Consumes: Task 1 harness and scripts.
- Produces: `forapi` building against hbb_common 229b904, which exports `tcp::{KxTranscript, KX_PARAMS_DOMAIN}`, `Encrypt::{decode, new_split}` and `Encrypt: Clone`, and `rendezvous_proto::{KxParams, IdPk}`. `KeyExchange` gains `version` and `signed_params`.

- [ ] **Step 1: Branch and move the submodule**

```bash
cd $REPO && git switch -c chore/hbb-common-229b904 forapi
git -C libs/hbb_common fetch -q origin
git -C libs/hbb_common checkout -q 229b904508364c8997aad0fb5af57effac859f60
git -C libs/hbb_common log --oneline -1
```

Expected: `229b904 Merge pull request #614 from rustdesk/kx-v2`

- [ ] **Step 2: See the build fail**

Run: `SQLX_OFFLINE=true cargo check -p hbbs --all-targets 2>&1 | grep -E "^error|-->"`
Expected: exactly one error, `error[E0433]: cannot find 'message_proto' in 'hbb_common'` at `src/rendezvous_server.rs` in `get_pk`. If anything else fails and can only be fixed by changing behaviour, stop and report it.

- [ ] **Step 3: Fix the import**

In `get_pk`, replace

```rust
                    &hbb_common::message_proto::IdPk {
```

with

```rust
                    &hbb_common::rendezvous_proto::IdPk {
```

(hbb_common dropped `message.proto`. `IdPk` now lives in `rendezvous.proto` with the same fields and wire format.)

- [ ] **Step 4: Check, lint, format**

```bash
SQLX_OFFLINE=true cargo check -p hbbs --all-targets; echo "check=$?"
SQLX_OFFLINE=true cargo clippy -p hbbs --no-deps -- -D warnings; echo "clippy=$?"
cargo fmt -p hbbs -- --check; echo "fmt=$?"
```

Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 5: Run the matrix against the bumped build**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a1
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a1 $W -- $SCRATCH/kx-matrix.sh 0
```

Expected: exactly the Task 1 baseline result (all pass, `advertised=0 picked=0`, `hbbs: still running`, exit 0).

- [ ] **Step 6: Commit and open the PR**

```bash
git add libs/hbb_common Cargo.lock src/rendezvous_server.rs
git commit -m "chore(deps): bump hbb_common to 229b904

229b904 is what RustDesk 1.5.0 builds against: it adds the Kx v1
helpers and the 1.5.0 rendezvous fields (key exchange version, WebRTC
signalling, switch_code). hbbs only needs IdPk from its new place in
rendezvous_proto, since hbb_common no longer ships message.proto. No
behaviour change."
git push -u origin chore/hbb-common-229b904
```

Open the PR against `forapi`. Its body must say:
- there is no behaviour change;
- which local checks ran (fmt, clippy, check, and the harness matrix: v0 handshake, encrypted online / http-proxy / punch requests, and the malformed and idle cases);
- any new cargo-deny advisories from the lockfile change. Read the `cargo-deny` job, and list them without ignoring any.

---

### Task 3: A1 release and user test gate

**Files:**
- Modify (k8s-deploy repo): `charts/rustdesk-server/values.yaml` (`image.tag`), `Chart.yaml` (`version` 0.2.5 → 0.2.6)

- [ ] **Step 1: Wait for CI and the user's merge**

Run: `gh pr checks <PR> -R rxxozqfoe/rustdesk-server`
Expected: every check passes except the intentionally red `cargo-deny`, if it is red for the advisories it already reports. Ask the user to merge.

- [ ] **Step 2: Tag `v1.1.15-mycustom.4` (after the user's go-ahead)**

```bash
cd $REPO && git fetch -q origin && MERGE=$(git rev-parse origin/forapi)
git tag -a v1.1.15-mycustom.4 -m "Release v1.1.15-mycustom.4" $MERGE
git push origin v1.1.15-mycustom.4
```

Poll `gh run list -w publish.yml -R rxxozqfoe/rustdesk-server -L 3` until the run for this tag shows `completed success`. The arm64 build takes about 15 minutes. Then:

Run: `crane manifest ghcr.io/rxxozqfoe/rustdesk-server:1.1.15-mycustom.4`
Expected: linux/amd64 and linux/arm64 entries.

- [ ] **Step 3: Chart 0.2.6**

In k8s-deploy, branch from `main`:
- set `charts/rustdesk-server/values.yaml` `tag: "1.1.15-mycustom.4"`;
- set `Chart.yaml` `version: 0.2.6`;
- run `bash ci/validate.sh`, which must pass;
- commit `Use rustdesk-server 1.1.15-mycustom.4; release 0.2.6`, open the PR, and ask the user to merge.

After the merge and the user's go-ahead, tag `v0.2.6` on the merge commit, wait for `publish.yml` to succeed, and `helm pull oci://ghcr.io/rxxozqfoe/charts/rustdesk-stack --version 0.2.6` to confirm.

- [ ] **Step 4: Hand the A1 test plan to the user and wait**

Send the A1 test plan table from the spec (section "A1", five rows). Do not start Task 4 until the user reports that all five pass. On a failure, collect the hbbs log and the client log, then stop.

---

### Task 4: A2 part 1, per-connection state (no protocol change)

**Files:**
- Modify: `src/rendezvous_server.rs`. Add `struct Conn` after `impl Sink`. Change the signature and first line of `handle_tcp`, the `KeyExchange` arm of `handle_tcp`, and both branches of `handle_listener_inner`.

**Interfaces:**
- Consumes: hbb_common 229b904 (`Encrypt: Clone`).
- Produces: `struct Conn { sink: Option<Sink>, rx: Option<Encrypt> }`, and `handle_tcp(&mut self, bytes: &[u8], conn: &mut Conn, addr: SocketAddr, key: &str, ws: bool) -> bool`. Task 5 adds a `pending_kx` field and changes `key_exchange_phase1` to take `&mut Conn`.

- [ ] **Step 1: Branch**

```bash
cd $REPO && git switch forapi && git pull --ff-only && git switch -c feat/kx-v1
```

- [ ] **Step 2: Add `Conn`**

After the `impl Sink { … }` block, add:

```rust
/// What the read loop of one TCP or WebSocket connection keeps.
struct Conn {
    /// The send half. It is moved into `tcp_punch` or `ws_map` once the
    /// client asks hbbs to answer on it later.
    sink: Option<Sink>,
    /// Decrypts what the client sends once a key exchange has finished.
    /// Kept apart from the sink, so moving the sink does not stop the read
    /// loop from decrypting.
    rx: Option<Encrypt>,
}
```

- [ ] **Step 3: Pass `Conn` to `handle_tcp`**

Change the signature

```rust
    async fn handle_tcp(
        &mut self,
        bytes: &[u8],
        sink: &mut Option<Sink>,
        addr: SocketAddr,
        key: &str,
        ws: bool,
    ) -> bool {
```

to

```rust
    async fn handle_tcp(
        &mut self,
        bytes: &[u8],
        conn: &mut Conn,
        addr: SocketAddr,
        key: &str,
        ws: bool,
    ) -> bool {
        let sink = &mut conn.sink;
```

The rest of the body keeps using `sink`, which is a `&mut Option<Sink>` as before.

- [ ] **Step 4: Keep a receive cipher**

In the `KeyExchange` arm of `handle_tcp`, replace

```rust
                            if let Some(sink) = sink.as_mut() {
                                match sink {
                                    Sink::Wss(s) => s.encrypt = Some(Encrypt::new(key)),
                                    Sink::Tss(s) => s.encrypt = Some(Encrypt::new(key)),
                                }
                            }
```

with

```rust
                            let enc = Encrypt::new(key);
                            conn.rx = Some(enc.clone());
                            if let Some(sink) = sink.as_mut() {
                                match sink {
                                    Sink::Wss(s) => s.encrypt = Some(enc),
                                    Sink::Tss(s) => s.encrypt = Some(enc),
                                }
                            }
```

(`enc` sends with counter 1 and receives with counter 2. The two clones start at 0/0 and each uses only its own counter, so v0 behaves exactly as before.)

- [ ] **Step 5: Use `Conn` in `handle_listener_inner`**

Replace the body from `let mut sink;` down to (and including) the `if sink.is_none() { … }` block with:

```rust
        let mut conn;
        if ws {
            use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
            let callback = |req: &Request, response: Response| {
                let headers = req.headers();
                let real_ip = headers
                    .get("X-Real-IP")
                    .or_else(|| headers.get("X-Forwarded-For"))
                    .and_then(|header_value| header_value.to_str().ok());
                if let Some(ip) = real_ip {
                    if ip.contains('.') {
                        addr = format!("{ip}:0").parse().unwrap_or(addr);
                    } else {
                        addr = format!("[{ip}]:0").parse().unwrap_or(addr);
                    }
                }
                Ok(response)
            };
            let ws_stream = tokio_tungstenite::accept_hdr_async(stream, callback).await?;
            let (a, mut b) = ws_stream.split();
            conn = Conn {
                sink: Some(Sink::Wss(SafeWsSink {
                    sink: a,
                    encrypt: None,
                })),
                rx: None,
            };
            while let Ok(Some(Ok(msg))) = timeout(30_000, b.next()).await {
                if let tungstenite::Message::Binary(bytes) = msg {
                    if !self.handle_tcp(&bytes, &mut conn, addr, key, ws).await {
                        break;
                    }
                }
            }
        } else {
            let (a, mut b) = Framed::new(stream, BytesCodec::new()).split();
            conn = Conn {
                sink: Some(Sink::Tss(SafeTcpStreamSink {
                    sink: a,
                    encrypt: None,
                })),
                rx: None,
            };
            // Avoid key exchange if answering on nat helper port
            if !key.is_empty() {
                self.key_exchange_phase1(addr, &mut conn.sink).await;
            }
            while let Ok(Some(Ok(mut bytes))) = timeout(30_000, b.next()).await {
                if let Some(rx) = conn.rx.as_mut() {
                    if let Err(err) = rx.dec(&mut bytes) {
                        log::error!("dec tcp data from {:?} err: {:?}", addr, err);
                        break;
                    }
                }
                if !self.handle_tcp(&bytes, &mut conn, addr, key, ws).await {
                    break;
                }
            }
        }
        if conn.sink.is_none() {
            self.tcp_punch.lock().await.remove(&try_into_v4(addr));
        }
```

- [ ] **Step 6: Check, lint, format**

Run the three commands from Task 2 Step 4. Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 7: Matrix, unchanged behaviour**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a2-conn
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a2-conn $W -- $SCRATCH/kx-matrix.sh 0
```

Expected: the Task 1 baseline result.

- [ ] **Step 8: Commit**

```bash
git add src/rendezvous_server.rs
git commit -m "refactor(rendezvous): keep per-connection state with its own receive cipher

The read loop decrypted with the cipher stored in the sink, so once a
punch-hole or relay request moved the sink into tcp_punch, later
encrypted messages on that connection could not be decrypted and the
connection closed. Keep the connection's sink and a receive cipher in a
Conn the read loop owns; the sink keeps the sending one."
```

---

### Task 5: A2 part 2, Kx v1 and `KX_MAX_VERSION`

**Files:**
- Modify: `src/rendezvous_server.rs`:
  - imports;
  - a new `KX_MAX_VERSION` static next to `MUST_LOGIN`;
  - a new `PendingKx` and a `pending_kx` field in `Conn`;
  - `Inner` and its construction in `start` (remove `secure_tcp_pk_b` / `secure_tcp_sk_b`);
  - `start` (read `KX_MAX_VERSION`);
  - the `KeyExchange` arm of `handle_tcp`;
  - `key_exchange_phase1`;
  - the call to it in `handle_listener_inner`;
  - remove `get_symetric_key_from_msg`.

**Interfaces:**
- Consumes: `Conn` and `handle_tcp(… conn: &mut Conn …)` from Task 4. From hbb_common 229b904: `tcp::{KxTranscript, KX_PARAMS_DOMAIN}`, `Encrypt::{decode, new_split}`, `rendezvous_proto::KxParams`, and `KeyExchange.{version, signed_params}`.
- Produces: `static KX_MAX_VERSION: AtomicU32`; `struct PendingKx { sk, pk_sent, advertised }` with `fn finish(self, ex: &KeyExchange) -> ResultType<Encrypt>`; `Conn.pending_kx: Option<PendingKx>`; `async fn key_exchange_phase1(&self, addr: SocketAddr, conn: &mut Conn)`. In the `KeyExchange` arm, Task 6 adds counting to the `Ok(enc)` and `Err(err)` branches.

- [ ] **Step 1: Imports and the version switch**

Add to the `hbb_common::{ … }` import: `anyhow::anyhow`, and in its `tcp` entry `tcp::{listen_any, FramedStream, KxTranscript, KX_PARAMS_DOMAIN}`. Add `AtomicU32` to `sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering}`.

After `static MUST_LOGIN: AtomicBool = AtomicBool::new(false);` add:

```rust
/// The newest key exchange version hbbs advertises (KX_MAX_VERSION). The
/// split-key scheme below is version 1; a newer hbb_common version needs its
/// own review before this may go higher.
static KX_MAX_VERSION: AtomicU32 = AtomicU32::new(1);
```

- [ ] **Step 2: Pending phase-1 state**

Add `pending_kx` to `Conn`:

```rust
    /// The phase-1 secret, until the client's phase 2 consumes it.
    pending_kx: Option<PendingKx>,
```

…and initialise it as `pending_kx: None` in both `Conn { … }` literals in `handle_listener_inner`. After `struct Conn`, add:

```rust
/// The phase-1 half of a key exchange, kept until the client answers.
struct PendingKx {
    sk: SecretKey,
    /// Our ephemeral public key as sent: bit 255 set when v1 was advertised.
    pk_sent: [u8; box_::PUBLICKEYBYTES],
    advertised: u32,
}

impl PendingKx {
    /// Phase 2: open the client's sealed key and build the cipher for the
    /// version the client picked.
    fn finish(self, ex: &KeyExchange) -> ResultType<Encrypt> {
        if ex.keys.len() != 2 {
            bail!("expected 2 keys in phase 2, got {}", ex.keys.len());
        }
        if ex.version > self.advertised {
            bail!(
                "client picked version {} above the advertised {}",
                ex.version,
                self.advertised
            );
        }
        let key = Encrypt::decode(&ex.keys[1], &ex.keys[0], &self.sk)?;
        if ex.version == 0 {
            return Ok(Encrypt::new(key));
        }
        Encrypt::new_split(
            key,
            false,
            &KxTranscript {
                initiator_pk: &ex.keys[0],
                responder_pk: &self.pk_sent,
                advertised: self.advertised,
                picked: ex.version,
            },
        )
    }
}
```

- [ ] **Step 3: Phase 1**

Replace the whole `key_exchange_phase1` function with:

```rust
    async fn key_exchange_phase1(&self, addr: SocketAddr, conn: &mut Conn) {
        let Some(sk) = &self.inner.sk else {
            return;
        };
        let advertised = KX_MAX_VERSION.load(Ordering::SeqCst);
        // A fresh pair per connection: v1 binds it into the session keys.
        let (pk, kx_sk) = box_::gen_keypair();
        let mut pk_sent = pk.0;
        let mut ex = KeyExchange::new();
        if advertised > 0 {
            // Bit 255 tells a 1.5.0+ client to require signed_params. X25519
            // ignores it, so older clients use the key unchanged.
            pk_sent[31] |= 0x80;
            let params = KxParams {
                pk: pk_sent.to_vec().into(),
                version: advertised,
                ..Default::default()
            };
            let mut signed = KX_PARAMS_DOMAIN.to_vec();
            signed.extend(params.write_to_bytes().unwrap_or_default());
            ex.version = advertised;
            ex.signed_params = sign::sign(&signed, sk).into();
        }
        ex.keys = vec![sign::sign(&pk_sent, sk).into()];
        log::trace!("KeyExchange phase 1 to {:?}, version {}", addr, advertised);
        let mut msg_out = RendezvousMessage::new();
        msg_out.set_key_exchange(ex);
        Self::send_to_sink(&mut conn.sink, msg_out).await;
        conn.pending_kx = Some(PendingKx {
            sk: kx_sk,
            pk_sent,
            advertised,
        });
    }
```

In `handle_listener_inner`, change the call to `self.key_exchange_phase1(addr, &mut conn).await;`.

- [ ] **Step 4: Phase 2**

Replace the whole `Some(rendezvous_message::Union::KeyExchange(ex)) => { … }` arm of `handle_tcp` with:

```rust
                Some(rendezvous_message::Union::KeyExchange(ex)) => {
                    log::trace!("KeyExchange {:?} <- bytes: {:?}", addr, hex::encode(bytes));
                    // Unauthenticated input: anything wrong closes only this
                    // connection (panic = "abort" would take hbbs down).
                    let result = match conn.pending_kx.take() {
                        Some(pending) => pending.finish(&ex),
                        None => Err(anyhow!("no phase 1 pending on this connection")),
                    };
                    match result {
                        Ok(enc) => {
                            conn.rx = Some(enc.clone());
                            if let Some(sink) = sink.as_mut() {
                                match sink {
                                    Sink::Wss(s) => s.encrypt = Some(enc),
                                    Sink::Tss(s) => s.encrypt = Some(enc),
                                }
                            }
                            log::debug!("KeyExchange version {} with {}", ex.version, addr);
                            return true;
                        }
                        Err(err) => {
                            log::error!("Handshake failed from {}: {}", addr, err);
                            return false;
                        }
                    }
                }
```

This removes the old arm's debug line that logged the session key.

- [ ] **Step 5: Drop the process-wide key pair and read `KX_MAX_VERSION`**

Make these changes:
- Remove the `secure_tcp_pk_b` and `secure_tcp_sk_b` fields from `struct Inner`.
- In `start`, remove the line `let (secure_tcp_pk_b, secure_tcp_sk_b) = box_::gen_keypair();`, the comment above it, and the two fields in `Inner { … }`.
- Remove the whole `fn get_symetric_key_from_msg`.
- In `start`, right after the `MUST_LOGIN` block's `log::info!`, add:

```rust
        match std::env::var("KX_MAX_VERSION").unwrap_or_default().trim() {
            "" | "1" => {}
            "0" => KX_MAX_VERSION.store(0, Ordering::SeqCst),
            other => log::warn!("KX_MAX_VERSION={other} is not 0 or 1, using 1"),
        }
        log::info!(
            "KX_MAX_VERSION={}",
            KX_MAX_VERSION.load(Ordering::SeqCst)
        );
```

Remove any import that `cargo check` now reports as unused (`box_::PublicKey`, and `secretbox` if nothing else uses it).

- [ ] **Step 6: Check, lint, format**

Run the three commands from Task 2 Step 4. Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 7: Run the A2 matrix**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a2
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a2 $W -- $SCRATCH/kx-matrix.sh 1
```

Expected:
- `v1` prints `advertised=1 picked=1` and passes all three checks. (This is the test that failed in Task 1.)
- `v0` prints `advertised=1 picked=0` and passes all three. (Review Focus: an old client against a v1 hbbs.)
- `pick2` and `rekey` print `closed: ok`. (Review Focus: a second KeyExchange.)
- `short`, `badbox`, `ws` and `idle` print `closed by hbbs`. (Review Focus: WebSocket without phase 1, and idle.)
- The run ends with `hbbs: still running`, exit status 0.

```bash
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a2 $W KX_MAX_VERSION=0 -- $SCRATCH/kx-matrix.sh 0
```

Expected: `advertised=0 picked=0` everywhere, all pass, `hbbs: still running`.

```bash
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a2 $W KX_MAX_VERSION=7 -- $SCRATCH/kx-harness/target/release/kx-harness 127.0.0.1:31116 id_ed25519.pub v1 1
grep -E "KX_MAX_VERSION" $W/hbbs.log
```

Expected: the harness passes, and the log has `KX_MAX_VERSION=7 is not 0 or 1, using 1` and `KX_MAX_VERSION=1`. (Review Focus: an invalid value.)

- [ ] **Step 8: Commit**

```bash
git add src/rendezvous_server.rs
git commit -m "feat(rendezvous): negotiate key exchange v1 with 1.5.0 clients

Advertise version 1 in phase 1: a fresh X25519 pair per connection, bit
255 of the signed key set, and signed KxParams. A 1.5.0 client then
derives one key per direction bound to the handshake transcript; older
clients ignore the new fields and the high bit and keep version 0.
Phase 2 opens the box with Encrypt::decode, refuses a version above
the advertised one, and accepts one exchange per connection.

KX_MAX_VERSION=0 advertises nothing new, as before. The process-wide
ephemeral key pair, and the debug line that logged session keys, go."
```

---

### Task 6: A2 part 3, key exchange summary line

**Files:**
- Modify: `src/rendezvous_server.rs`: three counters and a summary task near `KX_MAX_VERSION`, counting in the `KeyExchange` arm, and starting the task in `start`.

**Interfaces:**
- Consumes: the `Ok(enc)` / `Err(err)` branches from Task 5 Step 4.
- Produces: `fn spawn_kx_summary()`, and the log line `key exchange (10 min): v1=<n> v0=<n> failed=<n>`.

- [ ] **Step 1: Counters and the summary task**

Add `AtomicU64` to the `std::sync::atomic` import. After `KX_MAX_VERSION`, add:

```rust
// Key exchanges since the last summary line, by outcome.
static KX_V0: AtomicU64 = AtomicU64::new(0);
static KX_V1: AtomicU64 = AtomicU64::new(0);
static KX_FAILED: AtomicU64 = AtomicU64::new(0);
const KX_SUMMARY_INTERVAL: Duration = Duration::from_secs(600);

/// Every KX_SUMMARY_INTERVAL, logs how the key exchanges since the last line
/// went, skipping intervals without any.
fn spawn_kx_summary() {
    tokio::spawn(async {
        let mut timer = interval(KX_SUMMARY_INTERVAL);
        timer.tick().await; // the first tick fires at once
        loop {
            timer.tick().await;
            let v1 = KX_V1.swap(0, Ordering::Relaxed);
            let v0 = KX_V0.swap(0, Ordering::Relaxed);
            let failed = KX_FAILED.swap(0, Ordering::Relaxed);
            if v1 + v0 + failed > 0 {
                log::info!("key exchange (10 min): v1={v1} v0={v0} failed={failed}");
            }
        }
    });
}
```

- [ ] **Step 2: Count**

In the `KeyExchange` arm, add as the first line of `Ok(enc) => {`:

```rust
                            let counter = if ex.version == 0 { &KX_V0 } else { &KX_V1 };
                            counter.fetch_add(1, Ordering::Relaxed);
```

…and as the first line of `Err(err) => {`:

```rust
                            KX_FAILED.fetch_add(1, Ordering::Relaxed);
```

In `start`, right after the `KX_MAX_VERSION` `log::info!`, add `spawn_kx_summary();`.

- [ ] **Step 3: Check, lint, format**

Run the three commands from Task 2 Step 4. Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 4: Verify the line (takes about 11 minutes)**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a2
H=$SCRATCH/kx-harness/target/release/kx-harness
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a2 $W -- bash -c "
  $H 127.0.0.1:31116 id_ed25519.pub v1 1; $H 127.0.0.1:31116 id_ed25519.pub v0 1
  $H 127.0.0.1:31116 id_ed25519.pub pick2 1; $SCRATCH/venv/bin/python $SCRATCH/kx_bad.py short
  python3 -c 'import time; time.sleep(610)'"
grep "key exchange (10 min)" $W/hbbs.log
```

Expected: exactly one line, `key exchange (10 min): v1=3 v0=3 failed=2`. `v1` and `v0` each make three connections, and `pick2` and `short` make one failure each.

- [ ] **Step 5: Commit, push, PR**

```bash
git add src/rendezvous_server.rs
git commit -m "feat(rendezvous): log a key exchange summary every 10 minutes

One info line per 10 minutes with handshakes since the last one, by
version and failures, so a rollout can confirm 1.5.0 clients use v1
and nothing breaks: key exchange (10 min): v1=12 v0=3 failed=0."
git push -u origin feat/kx-v1
```

Open the PR against `forapi`. The body must cover:
- the three commits;
- the `KX_MAX_VERSION` switch;
- the deliberate behaviour change from Task 4: a connection keeps decrypting after hbbs moves its sink;
- the local matrix results from Task 5 Step 7 and Task 6 Step 4.

---

### Task 7: A2 release and user test gate

**Files:**
- Modify (k8s-deploy repo): `charts/rustdesk-server/values.yaml` (`image.tag`), `Chart.yaml` (`version` 0.2.6 → 0.2.7)

- [ ] **Step 1: CI, merge, tag**

Do the same as Task 3 Steps 1–2, with tag `v1.1.15-mycustom.5`.

- [ ] **Step 2: Chart 0.2.7**

Do the same as Task 3 Step 3, with `tag: "1.1.15-mycustom.5"`, `version: 0.2.7`, and commit message `Use rustdesk-server 1.1.15-mycustom.5; release 0.2.7`. Add one line to the chart README's configuration notes: `rustdesk-server.hbbs.extraEnv` with `KX_MAX_VERSION=0` turns key exchange v1 off.

- [ ] **Step 3: Hand the A2 test plan to the user and wait**

Send the A2 test plan table from the spec (section "A2", four rows, including the `KX_MAX_VERSION=0` rollback drill through `hbbs.extraEnv`). A3 planning starts only after all four pass. On a failure, ask for:
- the hbbs log lines `Handshake failed` and `key exchange (10 min)`;
- the client log lines around `Key exchange version`.

Then stop.
