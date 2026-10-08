# hbbs 1.5.0 protocol, A3 (WebRTC signalling) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let two RustDesk 1.5.0 peers that turned WebRTC on connect over it through hbbs. hbbs relays the SDP offer and answer and the trickled ICE candidates in both directions, and refuses candidates that do not belong to a session the sending connection is part of.

**Architecture:** This builds on A2's per-connection `Conn` and comes in four parts:

1. Connections hbbs answers on later (`tcp_punch`, `ws_map`) hold a shared handle to the send half (`SharedSink = Arc<Mutex<Sink>>`). Sending no longer removes an entry; the owning read loop removes its own entry on close.
2. The punch-hole path copies the offer into `PunchHole`, except when hbbs forces the relay, and copies the answer into `PunchHoleResponse`.
3. A new `IceCandidate` arm routes candidates. A candidate from the controller (`id`) goes to the peer that connection punched. A candidate from the controlled side (`socket_addr`) goes to the controller's `tcp_punch` connection.
4. The 10-minute summary line gains WebRTC counters.

**Tech Stack:** Rust 1.98.1 (`rust-toolchain.toml`), tokio, hbb_common 229b904 (rust-protobuf 3 with `with-bytes`), and an out-of-repo Rust harness used as the test.

**Spec:** `docs/superpowers/specs/2026-10-07-hbbs-150-protocol-design.md`, section "A3: WebRTC signalling".

## Global Constraints

- Never run `cargo test` (repository rule). The tests are the out-of-repo harnesses, run against release builds.
- Lint and format only the hbbs package: `cargo fmt -p hbbs -- --check`, `cargo clippy -p hbbs --no-deps -- -D warnings`, `cargo check -p hbbs --all-targets`. Never format or lint `libs/hbb_common`.
- Build with `SQLX_OFFLINE=true`.
- Candidate limits, from the spec: `candidate` longer than 1 KiB (1024 bytes), or more than 64 candidates on one connection, is dropped and counted. A dropped candidate never closes the connection.
- `IceCandidate` on an unencrypted connection is dropped and counted.
- `session_key` and `candidate` are opaque to hbbs and forwarded unchanged.
- Summary line, exactly: `key exchange (10 min): v1=<n> v0=<n> failed=<n>; webrtc offer=<n> answer=<n> ice=<n> dropped=<n>`, logged at info level every 600 s when any count is non-zero.
- Non-goals: TURN or STUN hosting, `switch_code`, and the `MUST_LOGIN` check on RequestRelay.
- Branch from `forapi`, and open PRs against `forapi`. Commit messages and PR text are in English with no AI-attribution lines.
- The release `v1.1.15-mycustom.6` and chart `v0.2.8` are tagged only after the user's explicit go-ahead. Each is an annotated tag, message `Release <tag>`, on the merge commit.

## Wire contract (from RustDesk 1.5.0 client code; hbbs rewrites no field)

| Message | Sender → hbbs (channel) | hbbs → receiver |
|---|---|---|
| `PunchHoleRequest.webrtc_sdp_offer` | controller, encrypted TCP | copied into `PunchHole.webrtc_sdp_offer`. Not copied when hbbs forces the relay (`ALWAYS_USE_RELAY`, or exactly one side on the LAN) |
| `PunchHoleSent.webrtc_sdp_answer` | controlled, new encrypted TCP (closed after sending) | copied into `PunchHoleResponse.webrtc_sdp_answer`, sent through `tcp_punch[controller]` |
| `RelayResponse.webrtc_sdp_answer` | controlled, TCP | already forwarded, since hbbs re-sends the received message |
| `IceCandidate{id, session_key, candidate}` | controller, on its punch TCP, after the answer | to the peer `id` like `PunchHole` (UDP, or its WebSocket sink) |
| `IceCandidate{socket_addr, session_key, candidate}` | controlled, on a separate encrypted "trickle" TCP | to `tcp_punch[decode(socket_addr)]` |

The controller keeps its punch TCP open after `PunchHoleResponse` to send and receive candidates. hbbs must therefore keep that `tcp_punch` entry after replying.

## Review Focus

- A WebSocket-registered peer gets punched more than once. Expected: every `PunchHole` reaches it over its WebSocket (today only the first one does). Covered by the Task 2 test `webrtc-harness … ws-peer`.
- A controller retries its punch-hole request on the same connection, as 1.5.0 does up to three times. Expected: each request is forwarded, and the connection still answers later requests. Covered by the Task 2 test `same-conn`.
- A candidate that carries both `id` and `socket_addr`, or neither. Expected: it is dropped and the connection stays open. Covered by `ice-abuse`, step 4, in Task 4.
- Candidates for a controller whose connection has already closed. Expected: they are dropped, and hbbs keeps running. Covered by `ice-abuse`, step 5, in Task 4.
- hbbs forces the relay (`ALWAYS_USE_RELAY=Y`). Expected: the controlled side gets a `PunchHole` without an offer and with `nat_type` SYMMETRIC. Covered by the Task 3 test `forced-relay`.

## Shared setup for every task

```bash
export SCRATCH=/path/to/your/session/scratchpad   # any writable directory
export REPO=/home/user/rustdesk/rustdesk-server
```

Every harness and script lives in `$SCRATCH` and is **never committed**. The A2 regression tools (`kx-harness`, `kx_bad.py`, `kx-matrix.sh`, `run-hbbs.sh`, `venv`) come from Task 1 of the A1+A2 plan. If `$SCRATCH/kx-harness` is missing, recreate them from that plan:

```bash
git -C $REPO show origin/docs/hbbs-150-plan-a1-a2:docs/superpowers/plans/2026-10-07-hbbs-150-protocol-a1-a2.md
```

---

### Task 1: WebRTC signalling harness and baseline

**Files (all outside the repository):**
- Create: `$SCRATCH/webrtc-harness/Cargo.toml`, `$SCRATCH/webrtc-harness/rust-toolchain.toml`, `$SCRATCH/webrtc-harness/src/main.rs`
- Create: `$SCRATCH/webrtc-matrix.sh`
- Reuse: `$SCRATCH/hbb_common-229b904`, `$SCRATCH/run-hbbs.sh` (A1+A2 plan, Task 1)

**Interfaces:**
- Produces: `webrtc-harness PORT PUBKEY_FILE CASE`, where CASE is one of `offer | answer | ice | same-conn | ws-peer | no-offer | forced-relay | ice-abuse`. It prints one line per check and exits 0 when every check passed.
- Produces: `webrtc-matrix.sh HBBS_BINARY [CASE…]`. It runs each case against a fresh hbbs (`forced-relay` with `ALWAYS_USE_RELAY=Y`), prints `PASS <case>` / `FAIL <case>`, and exits 1 if any case failed.

- [ ] **Step 1: Create the crate**

`$SCRATCH/webrtc-harness/Cargo.toml`:

```toml
[package]
name = "webrtc-harness"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
hbb_common = { path = "../hbb_common-229b904" }

# Keep cargo from looking for a parent workspace.
[workspace]
```

```bash
cp $REPO/rust-toolchain.toml $SCRATCH/webrtc-harness/rust-toolchain.toml
```

`$SCRATCH/webrtc-harness/src/main.rs`:

```rust
//! Plays both peers of a RustDesk 1.5.0 WebRTC signalling exchange against hbbs
//! on 127.0.0.1:
//! - the controller talks TCP from 127.0.0.2 after the 1.5.0 key exchange
//!   (src/common.rs `key_exchange` at client tag 1.5.0);
//! - the controlled peer registers over UDP from 127.0.0.3, or over WebSocket.
//!
//! The different loopback addresses keep hbbs from treating the peers as one
//! intranet, in which case it would send FetchLocalAddr instead of PunchHole.
//!
//! usage: webrtc-harness PORT PUBKEY_FILE CASE
//!   CASE: offer | answer | ice | same-conn | ws-peer | no-offer | forced-relay | ice-abuse
//! Prints one line per check; exit status 0 when every check passed.

use hbb_common::{
    anyhow::{anyhow, bail},
    base64::{engine::general_purpose::STANDARD, Engine as _},
    protobuf::Message as _,
    rendezvous_proto::*,
    sodiumoxide::{
        crypto::{box_, secretbox, sign},
        randombytes::randombytes_uniform,
    },
    tcp::{FramedStream, KxTranscript},
    tokio,
    udp::FramedSocket,
    websocket::WsFramedStream,
    AddrMangle, ResultType,
};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

const MS: u64 = 3_000;
const SHORT_MS: u64 = 1_500;
const OFFER: &str = "webrtc://harness-offer";
const ANSWER: &str = "webrtc://harness-answer";

struct Ctx {
    port: u16,
    server_pk: sign::PublicKey,
    licence_key: String,
}

type Union = rendezvous_message::Union;

fn hbbs_addr(ctx: &Ctx) -> String {
    format!("127.0.0.1:{}", ctx.port)
}

fn new_id(tag: &str) -> String {
    format!("wrtc{tag}{:06}", randombytes_uniform(1_000_000))
}

fn ensure(ok: bool, what: String) -> ResultType<()> {
    if ok {
        println!("  ok: {what}");
        Ok(())
    } else {
        bail!("{what}")
    }
}

fn parse(bytes: &[u8]) -> Option<Union> {
    RendezvousMessage::parse_from_bytes(bytes).ok()?.union
}

fn punch_hole(u: Union) -> Option<PunchHole> {
    match u {
        Union::PunchHole(p) => Some(p),
        _ => None,
    }
}

fn punch_hole_response(u: Union) -> Option<PunchHoleResponse> {
    match u {
        Union::PunchHoleResponse(p) => Some(p),
        _ => None,
    }
}

fn ice(u: Union) -> Option<IceCandidate> {
    match u {
        Union::IceCandidate(c) => Some(c),
        _ => None,
    }
}

fn online_response(u: Union) -> Option<OnlineResponse> {
    match u {
        Union::OnlineResponse(r) => Some(r),
        _ => None,
    }
}

fn left(deadline: tokio::time::Instant) -> u64 {
    deadline
        .saturating_duration_since(tokio::time::Instant::now())
        .as_millis() as u64
}

/// The next message on a TCP stream that `want` accepts, skipping others;
/// None once `ms` pass or the connection closes.
async fn recv_tcp<T>(conn: &mut FramedStream, ms: u64, want: fn(Union) -> Option<T>) -> Option<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let wait = left(deadline);
        if wait == 0 {
            return None;
        }
        let bytes = conn.next_timeout(wait).await?.ok()?;
        if let Some(t) = parse(&bytes).and_then(want) {
            return Some(t);
        }
    }
}

async fn recv_ws<T>(ws: &mut WsFramedStream, ms: u64, want: fn(Union) -> Option<T>) -> Option<T> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
    loop {
        let wait = left(deadline);
        if wait == 0 {
            return None;
        }
        let bytes = ws.next_timeout(wait).await?.ok()?;
        if let Some(t) = parse(&bytes).and_then(want) {
            return Some(t);
        }
    }
}

/// Connects from `local_ip` and runs the 1.5.0 client key exchange, v1 when
/// hbbs advertises it. The returned stream encrypts and decrypts.
async fn controller(ctx: &Ctx, local_ip: &str) -> ResultType<FramedStream> {
    let local: SocketAddr = format!("{local_ip}:0").parse()?;
    let mut conn = FramedStream::new(hbbs_addr(ctx), Some(local), MS).await?;
    let first = conn.next_timeout(MS).await.ok_or_else(|| anyhow!("no phase 1"))??;
    let Some(Union::KeyExchange(ex)) = parse(&first) else {
        bail!("phase 1 is not a KeyExchange");
    };
    let signed_pk = sign::verify(&ex.keys[0], &ctx.server_pk)
        .map_err(|_| anyhow!("phase 1 signature does not verify"))?;
    let pk: [u8; 32] = signed_pk
        .as_slice()
        .try_into()
        .map_err(|_| anyhow!("phase 1 key is not 32 bytes"))?;
    let picked = ex.version.min(1);
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
    Ok(conn)
}

/// A TCP connection that ignores phase 1 and stays unencrypted.
async fn plain(ctx: &Ctx, local_ip: &str) -> ResultType<FramedStream> {
    let local: SocketAddr = format!("{local_ip}:0").parse()?;
    let mut conn = FramedStream::new(hbbs_addr(ctx), Some(local), MS).await?;
    let _phase1 = conn.next_timeout(MS).await;
    Ok(conn)
}

fn register_pk(id: &str) -> RendezvousMessage {
    let (pk, _) = box_::gen_keypair();
    let mut msg = RendezvousMessage::new();
    msg.set_register_pk(RegisterPk {
        id: id.into(),
        uuid: format!("harness-{id}").into_bytes().into(),
        pk: pk.0.to_vec().into(),
        ..Default::default()
    });
    msg
}

fn register_ok(u: Union) -> Option<bool> {
    match u {
        Union::RegisterPkResponse(r) => {
            Some(r.result.enum_value() == Ok(register_pk_response::Result::OK))
        }
        _ => None,
    }
}

/// A controlled peer registered over UDP. RegisterPk alone records its UDP
/// address and marks it online for 30 s.
struct UdpPeer {
    sock: FramedSocket,
    id: String,
}

impl UdpPeer {
    async fn register(ctx: &Ctx, local_ip: &str, tag: &str) -> ResultType<Self> {
        let id = new_id(tag);
        let mut peer = Self {
            sock: FramedSocket::new(format!("{local_ip}:0")).await?,
            id,
        };
        let hbbs: SocketAddr = hbbs_addr(ctx).parse()?;
        peer.sock.send(&register_pk(&peer.id), hbbs).await?;
        match peer.recv(MS, register_ok).await {
            Some(true) => Ok(peer),
            other => bail!("RegisterPk {} over UDP: {other:?}", peer.id),
        }
    }

    async fn recv<T>(&mut self, ms: u64, want: fn(Union) -> Option<T>) -> Option<T> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(ms);
        loop {
            let wait = left(deadline);
            if wait == 0 {
                return None;
            }
            let (bytes, _) = self.sock.next_timeout(wait).await?.ok()?;
            if let Some(t) = parse(&bytes).and_then(want) {
                return Some(t);
            }
        }
    }
}

async fn send_punch(ctx: &Ctx, conn: &mut FramedStream, id: &str, offer: &str) -> ResultType<()> {
    let mut msg = RendezvousMessage::new();
    msg.set_punch_hole_request(PunchHoleRequest {
        id: id.into(),
        licence_key: ctx.licence_key.clone(),
        version: "1.5.0".into(),
        webrtc_sdp_offer: offer.into(),
        ..Default::default()
    });
    Ok(conn.send(&msg).await?)
}

async fn send_ice(conn: &mut FramedStream, ic: IceCandidate) -> ResultType<()> {
    let mut msg = RendezvousMessage::new();
    msg.set_ice_candidate(ic);
    Ok(conn.send(&msg).await?)
}

/// Controller punches a UDP peer with an offer; returns both ends and the
/// PunchHole the peer got.
async fn punched_session(
    ctx: &Ctx,
    offer: &str,
) -> ResultType<(FramedStream, UdpPeer, PunchHole)> {
    let mut b = UdpPeer::register(ctx, "127.0.0.3", "b").await?;
    let mut a = controller(ctx, "127.0.0.2").await?;
    send_punch(ctx, &mut a, &b.id, offer).await?;
    let ph = b
        .recv(MS, punch_hole)
        .await
        .ok_or_else(|| anyhow!("the peer got no PunchHole"))?;
    Ok((a, b, ph))
}

/// The controlled side answers on a new encrypted TCP connection, as 1.5.0
/// does when it has an offer.
async fn send_answer(ctx: &Ctx, b: &UdpPeer, ph: &PunchHole) -> ResultType<()> {
    let mut bt = controller(ctx, "127.0.0.3").await?;
    let mut msg = RendezvousMessage::new();
    msg.set_punch_hole_sent(PunchHoleSent {
        socket_addr: ph.socket_addr.clone(),
        id: b.id.clone(),
        relay_server: ph.relay_server.clone(),
        version: "1.5.0".into(),
        webrtc_sdp_answer: ANSWER.into(),
        ..Default::default()
    });
    Ok(bt.send(&msg).await?)
}

async fn case_offer(ctx: &Ctx) -> ResultType<()> {
    let (_a, _b, ph) = punched_session(ctx, OFFER).await?;
    ensure(
        ph.webrtc_sdp_offer == OFFER,
        format!("PunchHole carries the offer (got {:?})", ph.webrtc_sdp_offer),
    )
}

async fn case_answer(ctx: &Ctx) -> ResultType<()> {
    let (mut a, b, ph) = punched_session(ctx, OFFER).await?;
    send_answer(ctx, &b, &ph).await?;
    let resp = recv_tcp(&mut a, MS, punch_hole_response)
        .await
        .ok_or_else(|| anyhow!("the controller got no PunchHoleResponse"))?;
    ensure(!resp.socket_addr.is_empty(), "PunchHoleResponse has socket_addr".into())?;
    ensure(
        resp.webrtc_sdp_answer == ANSWER,
        format!("PunchHoleResponse carries the answer (got {:?})", resp.webrtc_sdp_answer),
    )
}

async fn case_ice(ctx: &Ctx) -> ResultType<()> {
    let (mut a, mut b, ph) = punched_session(ctx, OFFER).await?;
    send_answer(ctx, &b, &ph).await?;
    recv_tcp(&mut a, MS, punch_hole_response)
        .await
        .ok_or_else(|| anyhow!("the controller got no PunchHoleResponse"))?;
    send_ice(
        &mut a,
        IceCandidate {
            id: b.id.clone(),
            session_key: "sk".into(),
            candidate: "cand-a".into(),
            ..Default::default()
        },
    )
    .await?;
    let got = b.recv(MS, ice).await;
    ensure(
        got.as_ref().map(|c| c.candidate.as_str()) == Some("cand-a"),
        format!("controller → peer candidate (got {got:?})"),
    )?;
    let mut trickle = controller(ctx, "127.0.0.3").await?;
    send_ice(
        &mut trickle,
        IceCandidate {
            socket_addr: ph.socket_addr.clone(),
            session_key: "sk".into(),
            candidate: "cand-b".into(),
            ..Default::default()
        },
    )
    .await?;
    let got = recv_tcp(&mut a, MS, ice).await;
    ensure(
        got.as_ref().map(|c| c.candidate.as_str()) == Some("cand-b"),
        format!("peer → controller candidate (got {got:?})"),
    )
}

async fn case_same_conn(ctx: &Ctx) -> ResultType<()> {
    let mut b = UdpPeer::register(ctx, "127.0.0.3", "b").await?;
    let mut a = controller(ctx, "127.0.0.2").await?;
    for n in 1..=2 {
        send_punch(ctx, &mut a, &b.id, "").await?;
        ensure(
            b.recv(MS, punch_hole).await.is_some(),
            format!("punch request {n} on one connection reaches the peer"),
        )?;
    }
    let mut msg = RendezvousMessage::new();
    msg.set_online_request(OnlineRequest {
        id: "wrtcharness".into(),
        peers: vec![b.id.clone()],
        ..Default::default()
    });
    a.send(&msg).await?;
    ensure(
        recv_tcp(&mut a, MS, online_response).await.is_some(),
        "the connection still answers after its sink was handed to tcp_punch".into(),
    )
}

async fn case_ws_peer(ctx: &Ctx) -> ResultType<()> {
    let id = new_id("w");
    let url = format!("ws://127.0.0.1:{}", ctx.port + 2);
    let mut w = WsFramedStream::new(url, None, None, MS).await?;
    w.send(&register_pk(&id)).await?;
    ensure(
        recv_ws(&mut w, MS, register_ok).await == Some(true),
        "RegisterPk over WebSocket".into(),
    )?;
    for n in 1..=2 {
        let mut a = controller(ctx, "127.0.0.2").await?;
        send_punch(ctx, &mut a, &id, "").await?;
        ensure(
            recv_ws(&mut w, MS, punch_hole).await.is_some(),
            format!("punch request {n} reaches the WebSocket peer"),
        )?;
    }
    Ok(())
}

async fn case_no_offer(ctx: &Ctx) -> ResultType<()> {
    let (_a, _b, ph) = punched_session(ctx, "").await?;
    ensure(ph.webrtc_sdp_offer.is_empty(), "no offer, none forwarded".into())?;
    let from = AddrMangle::decode(&ph.socket_addr);
    ensure(
        from.ip() == IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))
            || from.ip().to_string() == "::ffff:127.0.0.2",
        format!("PunchHole names the controller (got {from})"),
    )
}

async fn case_forced_relay(ctx: &Ctx) -> ResultType<()> {
    let (_a, _b, ph) = punched_session(ctx, OFFER).await?;
    ensure(
        ph.webrtc_sdp_offer.is_empty(),
        format!("forced relay: offer not forwarded (got {:?})", ph.webrtc_sdp_offer),
    )?;
    ensure(
        ph.nat_type.enum_value() == Ok(NatType::SYMMETRIC),
        "forced relay: nat_type SYMMETRIC".into(),
    )
}

async fn case_ice_abuse(ctx: &Ctx) -> ResultType<()> {
    let (mut a, mut b, ph) = punched_session(ctx, OFFER).await?;
    let mut c = UdpPeer::register(ctx, "127.0.0.5", "c").await?;
    fn cand(id: &str, socket_addr: &[u8], candidate: &str) -> IceCandidate {
        IceCandidate {
            id: id.into(),
            socket_addr: socket_addr.to_vec().into(),
            session_key: "sk".into(),
            candidate: candidate.into(),
            ..Default::default()
        }
    }
    // 1. a peer this connection did not punch
    send_ice(&mut a, cand(&c.id, &[], "to-c")).await?;
    ensure(c.recv(SHORT_MS, ice).await.is_none(), "1: not relayed to an unpunched peer".into())?;
    // 2. longer than 1 KiB
    send_ice(&mut a, cand(&b.id, &[], &"x".repeat(2000))).await?;
    ensure(b.recv(SHORT_MS, ice).await.is_none(), "2: oversized candidate dropped".into())?;
    // 3. an unencrypted connection
    let mut p = plain(ctx, "127.0.0.4").await?;
    send_ice(&mut p, cand("", &ph.socket_addr, "plain")).await?;
    ensure(
        recv_tcp(&mut a, SHORT_MS, ice).await.is_none(),
        "3: candidate from an unencrypted connection dropped".into(),
    )?;
    // 4. both id and socket_addr, then neither: dropped, connection stays open
    send_ice(&mut a, cand(&b.id, &ph.socket_addr, "both")).await?;
    send_ice(&mut a, cand("", &[], "neither")).await?;
    ensure(b.recv(SHORT_MS, ice).await.is_none(), "4: both/neither not relayed".into())?;
    let mut msg = RendezvousMessage::new();
    msg.set_online_request(OnlineRequest {
        id: "wrtcharness".into(),
        peers: vec![b.id.clone()],
        ..Default::default()
    });
    a.send(&msg).await?;
    ensure(
        recv_tcp(&mut a, MS, online_response).await.is_some(),
        "4: connection still open after dropped candidates".into(),
    )?;
    // 5. the controller's connection is gone
    drop(a);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut trickle = controller(ctx, "127.0.0.3").await?;
    send_ice(&mut trickle, cand("", &ph.socket_addr, "late")).await?;
    println!("  ok: 5: sent a candidate for a closed controller (dropped; hbbs must keep running)");
    // 6. more than 64 on one connection: a fresh session
    let mut a2 = controller(ctx, "127.0.0.2").await?;
    send_punch(ctx, &mut a2, &b.id, OFFER).await?;
    b.recv(MS, punch_hole)
        .await
        .ok_or_else(|| anyhow!("6: the peer got no PunchHole"))?;
    for i in 0..70 {
        send_ice(&mut a2, cand(&b.id, &[], &format!("c{i}"))).await?;
    }
    let mut n = 0;
    while let Some(got) = b.recv(SHORT_MS, ice).await {
        if got.candidate.starts_with('c') {
            n += 1;
        }
    }
    ensure(n == 64, format!("6: exactly 64 of 70 candidates relayed (got {n})"))
}

async fn run(ctx: &Ctx, case: &str) -> ResultType<()> {
    match case {
        "offer" => case_offer(ctx).await,
        "answer" => case_answer(ctx).await,
        "ice" => case_ice(ctx).await,
        "same-conn" => case_same_conn(ctx).await,
        "ws-peer" => case_ws_peer(ctx).await,
        "no-offer" => case_no_offer(ctx).await,
        "forced-relay" => case_forced_relay(ctx).await,
        "ice-abuse" => case_ice_abuse(ctx).await,
        other => bail!("unknown case {other}"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        eprintln!("usage: webrtc-harness PORT PUBKEY_FILE CASE");
        std::process::exit(2);
    }
    hbb_common::sodiumoxide::init().ok();
    let licence_key = std::fs::read_to_string(&args[2]).expect("read the public key file");
    let licence_key = licence_key.trim().to_owned();
    let pk = STANDARD.decode(&licence_key).expect("public key file is base64");
    let ctx = Ctx {
        port: args[1].parse().expect("PORT is a number"),
        server_pk: sign::PublicKey::from_slice(&pk).expect("public key is 32 bytes"),
        licence_key,
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
        .block_on(run(&ctx, &args[3]));
    match result {
        Ok(()) => println!("{}: ok", args[3]),
        Err(err) => {
            println!("{}: FAIL {err}", args[3]);
            std::process::exit(1);
        }
    }
}
```

- [ ] **Step 2: Build it**

Run: `cargo build --release --manifest-path $SCRATCH/webrtc-harness/Cargo.toml 2>&1 | grep -E "^(error|warning)|Finished"`
Expected: `Finished`. A compile error is a harness bug: fix it against hbb_common 229b904 (`src/tcp.rs`, `src/udp.rs`, `src/websocket.rs`, `protos/rendezvous.proto`) before going on.

- [ ] **Step 3: The matrix script**

`$SCRATCH/webrtc-matrix.sh` (then `chmod +x`):

```bash
#!/usr/bin/env bash
# usage: webrtc-matrix.sh HBBS_BINARY [CASE...]   (default: every case)
# Runs each case against a fresh hbbs in its own work dir (forced-relay with
# ALWAYS_USE_RELAY=Y) and prints PASS/FAIL per case. Exit 1 if any failed.
set -u
bin=$1
shift
cases=("$@")
[ ${#cases[@]} -eq 0 ] && cases=(offer answer ice same-conn ws-peer no-offer forced-relay ice-abuse)
h=$SCRATCH/webrtc-harness/target/release/webrtc-harness
fail=0
for c in "${cases[@]}"; do
    env=()
    [ "$c" = forced-relay ] && env=(ALWAYS_USE_RELAY=Y)
    out=$("$SCRATCH/run-hbbs.sh" "$bin" "$SCRATCH/hbbs-run-$c" "${env[@]}" -- "$h" 31116 id_ed25519.pub "$c" 2>&1)
    rc=$?
    if [ $rc -eq 0 ]; then
        echo "PASS $c"
    else
        echo "FAIL $c (exit $rc)"
        echo "$out" | tail -6 | sed 's/^/    /'
        fail=1
    fi
done
exit $fail
```

`run-hbbs.sh` exits 99 when hbbs died. So a crash shows up as `FAIL <case> (exit 99)` even if the harness itself passed.

- [ ] **Step 4: Baseline against current `forapi` (A2, mycustom.5)**

```bash
cd $REPO && git switch forapi && git pull --ff-only && git submodule update
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a3-base
$SCRATCH/webrtc-matrix.sh $SCRATCH/hbbs-a3-base
```

Expected (RED, which A3 must turn green; checked against mycustom.5 when this plan was written):
- `FAIL offer`: the offer is not forwarded.
- `FAIL answer`: the answer is not forwarded.
- `FAIL ice`: hbbs closes the controller's connection on its first candidate.
- `FAIL same-conn`: no `OnlineResponse` after the sink moved.
- `FAIL ws-peer`: the second punch does not reach the WebSocket peer.
- `FAIL ice-abuse`: steps 1–3 pass, then step 4 fails (`Broken pipe`), because hbbs already closed the controller's connection at its first candidate.

`PASS no-offer` and `PASS forced-relay` are expected already: they guard behaviour that must not change. If a FAIL comes from anything other than the reason above (for example `RegisterPk … over UDP`, or a PunchHole that never arrives in `offer`), the harness or the environment is wrong. Fix that first.

Nothing is committed in this task.

---

### Task 2: A3 part 1, shared send halves for `tcp_punch` and `ws_map`

**Files:**
- Modify: `src/rendezvous_server.rs`:
  - a new `SharedSink` type;
  - `Conn.sink`;
  - the `tcp_punch` / `ws_map` field types;
  - in `handle_tcp`: the first line, the PunchHoleRequest, RequestRelay, RegisterPk, KeyExchange and HttpProxyRequest arms;
  - `send_to_tcp`, `send_to_sink`, `send_to_tcp_sync`, `handle_tcp_punch_hole_request`;
  - the `Conn` literals and the end of `handle_listener_inner`;
  - the call in `key_exchange_phase1`.

**Interfaces:**
- Consumes: `Conn { sink, rx, pending_kx }` from A2.
- Produces: `type SharedSink = Arc<Mutex<Sink>>`; `Conn.sink: SharedSink`; `tcp_punch, ws_map: Arc<Mutex<HashMap<SocketAddr, SharedSink>>>`; `async fn send_to_sink(sink: &SharedSink, msg: RendezvousMessage)`. Entries are not removed on send. Each read loop removes its own entries, compared with `Arc::ptr_eq`.

- [ ] **Step 1: Branch**

```bash
cd $REPO && git switch forapi && git pull --ff-only && git switch -c feat/webrtc-signalling
```

- [ ] **Step 2: The type and the fields**

Right before `/// What the read loop of one TCP or WebSocket connection keeps.`, add:

```rust
/// A connection's send half, shared by its read loop and by `tcp_punch` /
/// `ws_map` while hbbs may answer on it later.
type SharedSink = Arc<Mutex<Sink>>;
```

In `struct Conn`, replace

```rust
    /// The send half. It is moved into `tcp_punch` or `ws_map` once the
    /// client asks hbbs to answer on it later.
    sink: Option<Sink>,
```

with

```rust
    /// The send half, also held by `tcp_punch` / `ws_map` once the client
    /// asks hbbs to answer on it later.
    sink: SharedSink,
```

In `pub struct RendezvousServer`, change both `tcp_punch: Arc<Mutex<HashMap<SocketAddr, Sink>>>` and `ws_map: Arc<Mutex<HashMap<SocketAddr, Sink>>>` to `HashMap<SocketAddr, SharedSink>`.

- [ ] **Step 3: `handle_tcp`**

Change its first line `let sink = &mut conn.sink;` to `let sink = &conn.sink;`.

In the `PunchHoleRequest` arm **and** the `RequestRelay` arm, replace

```rust
                    // there maybe several attempt, so sink can be none
                    if let Some(sink) = sink.take() {
                        self.tcp_punch.lock().await.insert(try_into_v4(addr), sink);
                    }
```

with

```rust
                    // Answers, and in 1.5.0 ICE candidates, come back through
                    // tcp_punch; retries just refresh the entry.
                    self.tcp_punch
                        .lock()
                        .await
                        .insert(try_into_v4(addr), sink.clone());
```

In the `RegisterPk` arm, replace

```rust
                                if let Some(sink) = sink.take() {
                                    self.ws_map.lock().await.insert(try_into_v4(addr), sink);
                                }
```

with

```rust
                                self.ws_map
                                    .lock()
                                    .await
                                    .insert(try_into_v4(addr), sink.clone());
```

In the `KeyExchange` arm, replace

```rust
                            if let Some(sink) = sink.as_mut() {
                                match sink {
                                    Sink::Wss(s) => s.encrypt = Some(enc),
                                    Sink::Tss(s) => s.encrypt = Some(enc),
                                }
                            }
```

with

```rust
                            match &mut *sink.lock().await {
                                Sink::Wss(s) => s.encrypt = Some(enc),
                                Sink::Tss(s) => s.encrypt = Some(enc),
                            }
```

In the `HttpProxyRequest` arm, replace

```rust
                    let encrypted = match sink.as_ref() {
                        Some(Sink::Tss(s)) => s.encrypt.is_some(),
                        Some(Sink::Wss(s)) => s.encrypt.is_some(),
                        None => false,
                    };
```

with

```rust
                    let encrypted = match &*sink.lock().await {
                        Sink::Tss(s) => s.encrypt.is_some(),
                        Sink::Wss(s) => s.encrypt.is_some(),
                    };
```

- [ ] **Step 4: Send helpers**

Replace the three functions `send_to_tcp`, `send_to_sink`, `send_to_tcp_sync` with:

```rust
    async fn send_to_tcp(&mut self, msg: RendezvousMessage, addr: SocketAddr) {
        let sink = self.tcp_punch.lock().await.get(&try_into_v4(addr)).cloned();
        if let Some(sink) = sink {
            tokio::spawn(async move {
                Self::send_to_sink(&sink, msg).await;
            });
        }
    }

    #[inline]
    async fn send_to_sink(sink: &SharedSink, msg: RendezvousMessage) {
        sink.lock().await.send(&msg).await;
    }

    #[inline]
    async fn send_to_tcp_sync(
        &mut self,
        msg: RendezvousMessage,
        addr: SocketAddr,
    ) -> ResultType<()> {
        let sink = self.tcp_punch.lock().await.get(&try_into_v4(addr)).cloned();
        if let Some(sink) = sink {
            Self::send_to_sink(&sink, msg).await;
        }
        Ok(())
    }
```

In `handle_tcp_punch_hole_request`, replace

```rust
            let mut sink = self.ws_map.lock().await.remove(&try_into_v4(addr));
            if let Some(s) = sink.as_mut() {
                s.send(&msg).await;
            } else {
```

with

```rust
            let sink = self.ws_map.lock().await.get(&try_into_v4(addr)).cloned();
            if let Some(s) = sink {
                Self::send_to_sink(&s, msg).await;
            } else {
```

In `key_exchange_phase1`, change `Self::send_to_sink(&mut conn.sink, msg_out).await;` to `Self::send_to_sink(&conn.sink, msg_out).await;`.

- [ ] **Step 5: `handle_listener_inner`**

In the WebSocket `Conn { … }` literal, change `sink: Some(Sink::Wss(SafeWsSink { sink: a, encrypt: None })),` to

```rust
                sink: Arc::new(Mutex::new(Sink::Wss(SafeWsSink {
                    sink: a,
                    encrypt: None,
                }))),
```

…and in the TCP literal, change `sink: Some(Sink::Tss(…))` the same way to `sink: Arc::new(Mutex::new(Sink::Tss(SafeTcpStreamSink { sink: a, encrypt: None }))),`.

Replace

```rust
        if conn.sink.is_none() {
            self.tcp_punch.lock().await.remove(&try_into_v4(addr));
        }
```

with

```rust
        // Forget this connection in the maps that may still point at it. A
        // newer connection from the same address keeps its own entry.
        let addr4 = try_into_v4(addr);
        for map in [&self.tcp_punch, &self.ws_map] {
            let mut map = map.lock().await;
            if map.get(&addr4).is_some_and(|s| Arc::ptr_eq(s, &conn.sink)) {
                map.remove(&addr4);
            }
        }
```

- [ ] **Step 6: Check, lint, format**

```bash
SQLX_OFFLINE=true cargo check -p hbbs --all-targets; echo "check=$?"
SQLX_OFFLINE=true cargo clippy -p hbbs --no-deps -- -D warnings; echo "clippy=$?"
cargo fmt -p hbbs -- --check; echo "fmt=$?"
```

Expected: `check=0`, `clippy=0`, `fmt=0`. If fmt fails, run `cargo fmt -p hbbs` and re-check.

- [ ] **Step 7: GREEN for this task, and no regression**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a3-sinks
$SCRATCH/webrtc-matrix.sh $SCRATCH/hbbs-a3-sinks same-conn ws-peer no-offer forced-relay
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a3-sinks $SCRATCH/hbbs-run -- $SCRATCH/kx-matrix.sh 1
```

Expected: `PASS` for all four cases (Review Focus: WebSocket peer punched twice, punch retried on one connection), and the kx matrix still passes with `hbbs: still running`.

- [ ] **Step 8: Commit**

```bash
git add src/rendezvous_server.rs
git commit -m "refactor(rendezvous): share send halves instead of moving them into tcp_punch/ws_map

tcp_punch and ws_map took a connection's sink and the first send
removed it, so a controller got exactly one reply, a WebSocket peer one
PunchHole until it registered again, and the connection could not
answer anything else meanwhile. Keep a shared handle in the maps
instead; sending no longer removes the entry and each read loop
removes its own entries when the connection closes. 1.5.0 needs this
for trickled WebRTC ICE candidates."
```

---

### Task 3: A3 part 2, offer and answer

**Files:**
- Modify: `src/rendezvous_server.rs`: `handle_punch_hole_request` (forced-relay condition and `PunchHole`), `handle_hole_sent` (`PunchHoleResponse`).

**Interfaces:**
- Consumes: Task 2 (`tcp_punch` keeps the controller's sink, so the response with the answer reaches it).
- Produces: in `handle_punch_hole_request`, `let server_forced_relay: bool` and `let webrtc_sdp_offer: String` (the forwarded offer, empty when not forwarded). Task 5 counts on `webrtc_sdp_offer`, and on `phs.webrtc_sdp_answer` in `handle_hole_sent`.

- [ ] **Step 1: Forward the offer unless hbbs forces the relay**

In `handle_punch_hole_request`, replace

```rust
            if ALWAYS_USE_RELAY.load(Ordering::SeqCst) || (peer_is_lan ^ is_lan) {
```

with

```rust
            // When hbbs forces the relay, WebRTC must not race it, so the
            // offer is not forwarded (a relay the client asked for keeps it).
            let server_forced_relay =
                ALWAYS_USE_RELAY.load(Ordering::SeqCst) || (peer_is_lan ^ is_lan);
            if server_forced_relay {
```

…and replace the `PunchHole` branch

```rust
                msg_out.set_punch_hole(PunchHole {
                    socket_addr,
                    nat_type: ph.nat_type,
                    relay_server,
                    controlled_context: MessageField::from_option(controlled_context),
                    ..Default::default()
                });
```

with

```rust
                let webrtc_sdp_offer = if server_forced_relay {
                    String::new()
                } else {
                    std::mem::take(&mut ph.webrtc_sdp_offer)
                };
                msg_out.set_punch_hole(PunchHole {
                    socket_addr,
                    nat_type: ph.nat_type,
                    relay_server,
                    controlled_context: MessageField::from_option(controlled_context),
                    webrtc_sdp_offer,
                    ..Default::default()
                });
```

- [ ] **Step 2: Forward the answer**

In `handle_hole_sent`, add the answer to the `PunchHoleResponse` literal:

```rust
        let mut p = PunchHoleResponse {
            socket_addr: AddrMangle::encode(addr).into(),
            pk: self.get_pk(&phs.version, phs.id).await,
            relay_server: phs.relay_server.clone(),
            webrtc_sdp_answer: phs.webrtc_sdp_answer.clone(),
            ..Default::default()
        };
```

(`RelayResponse` needs nothing: hbbs re-sends the received message, answer included.)

- [ ] **Step 3: Check, lint, format**

Run the three commands from Task 2 Step 6. Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 4: GREEN**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a3-sdp
$SCRATCH/webrtc-matrix.sh $SCRATCH/hbbs-a3-sdp offer answer no-offer forced-relay same-conn ws-peer
```

Expected: `PASS` for all six. (Review Focus: forced relay keeps the offer out.)

- [ ] **Step 5: Commit**

```bash
git add src/rendezvous_server.rs
git commit -m "feat(rendezvous): forward the WebRTC offer and answer

Copy a 1.5.0 controller's SDP offer from PunchHoleRequest into the
PunchHole hbbs sends the controlled peer, unless hbbs itself forces the
relay (ALWAYS_USE_RELAY or exactly one side on the LAN), and copy the
answer from PunchHoleSent into the PunchHoleResponse for the
controller. RelayResponse already carries its answer through."
```

---

### Task 4: A3 part 3, ICE candidate routing and limits

**Files:**
- Modify: `src/rendezvous_server.rs`:
  - two constants;
  - `Conn` (`punched`, `ice_count`) and its two literals;
  - the `PunchHoleRequest` arm;
  - a new `IceCandidate` arm in `handle_tcp`;
  - a new `handle_ice_candidate`.

**Interfaces:**
- Consumes: Task 2 (`SharedSink`, `send_to_sink`, and `tcp_punch` / `ws_map` entries that survive sends).
- Produces: `Conn.punched: Option<String>`, `Conn.ice_count: usize`, `const ICE_MAX_LEN: usize = 1024`, `const ICE_MAX_PER_CONN: usize = 64`, and `async fn handle_ice_candidate(&mut self, ic: IceCandidate, conn: &mut Conn) -> bool`, which returns true when the candidate was relayed. Task 5 counts on that return value.

- [ ] **Step 1: Constants and connection state**

After `static KX_MAX_VERSION: AtomicU32 = AtomicU32::new(1);`, add:

```rust
/// Limits on the WebRTC ICE candidates hbbs relays (1.5.0).
const ICE_MAX_LEN: usize = 1024;
const ICE_MAX_PER_CONN: usize = 64;
```

In `struct Conn`, add:

```rust
    /// The peer this connection's last punch-hole request was for: the only
    /// peer its ICE candidates may go to.
    punched: Option<String>,
    /// ICE candidates received on this connection, relayed or not.
    ice_count: usize,
```

…and `punched: None, ice_count: 0,` to both `Conn { … }` literals in `handle_listener_inner`.

- [ ] **Step 2: Remember the punched peer**

In the `PunchHoleRequest` arm of `handle_tcp`, right after the `tcp_punch` insert from Task 2, add:

```rust
                    conn.punched = Some(ph.id.clone());
```

- [ ] **Step 3: The `IceCandidate` arm**

Add, before the `_ => {}` arm of `handle_tcp`:

```rust
                Some(rendezvous_message::Union::IceCandidate(ic)) => {
                    self.handle_ice_candidate(ic, conn).await;
                    // Never close on a candidate, even a dropped one: the
                    // peers fall back to their other transports.
                    return true;
                }
```

- [ ] **Step 4: `handle_ice_candidate`**

Add after `handle_tcp_punch_hole_request`:

```rust
    /// Relays a trickled WebRTC ICE candidate (1.5.0) between the two peers
    /// of a punch-hole session:
    /// - from the controller (`id` set) to the peer this connection punched;
    /// - from the controlled side (`socket_addr` set) to the controller's
    ///   connection, still in `tcp_punch`.
    ///
    /// Only encrypted connections may send them, within ICE_MAX_LEN and
    /// ICE_MAX_PER_CONN. Returns whether the candidate was relayed.
    async fn handle_ice_candidate(&mut self, ic: IceCandidate, conn: &mut Conn) -> bool {
        conn.ice_count += 1;
        if conn.rx.is_none()
            || conn.ice_count > ICE_MAX_PER_CONN
            || ic.candidate.is_empty()
            || ic.candidate.len() > ICE_MAX_LEN
            || ic.id.is_empty() == ic.socket_addr.is_empty()
        {
            return false;
        }
        if !ic.socket_addr.is_empty() {
            let addr_a = try_into_v4(AddrMangle::decode(&ic.socket_addr));
            let Some(sink) = self.tcp_punch.lock().await.get(&addr_a).cloned() else {
                return false;
            };
            let mut msg_out = RendezvousMessage::new();
            msg_out.set_ice_candidate(ic);
            Self::send_to_sink(&sink, msg_out).await;
            return true;
        }
        if conn.punched.as_deref() != Some(ic.id.as_str()) {
            return false;
        }
        let Some(peer) = self.pm.get_in_memory(&ic.id).await else {
            return false;
        };
        let peer_addr = peer.read().await.socket_addr;
        let mut msg_out = RendezvousMessage::new();
        msg_out.set_ice_candidate(ic);
        let ws_sink = self.ws_map.lock().await.get(&try_into_v4(peer_addr)).cloned();
        match ws_sink {
            Some(sink) => Self::send_to_sink(&sink, msg_out).await,
            None => {
                self.tx.send(Data::Msg(msg_out.into(), peer_addr)).ok();
            }
        }
        true
    }
```

- [ ] **Step 5: Check, lint, format**

Run the three commands from Task 2 Step 6. Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 6: GREEN, full matrix**

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a3
$SCRATCH/webrtc-matrix.sh $SCRATCH/hbbs-a3
$SCRATCH/run-hbbs.sh $SCRATCH/hbbs-a3 $SCRATCH/hbbs-run -- $SCRATCH/kx-matrix.sh 1
```

Expected: `PASS` for all eight cases. That includes `ice-abuse`, which checks:
- steps 1–3: an unpunched peer, an oversized candidate, an unencrypted connection;
- step 4: both or neither of `id` / `socket_addr`, after which the connection still answers;
- step 5: a closed controller, with hbbs still running;
- step 6: 64 of 70 relayed.

(Review Focus: both/neither, a closed controller.) The kx matrix passes with `hbbs: still running`.

- [ ] **Step 7: Commit**

```bash
git add src/rendezvous_server.rs
git commit -m "feat(rendezvous): relay trickled WebRTC ICE candidates

Route 1.5.0 IceCandidate messages between the two peers of a punch-hole
session: from the controller (id set) to the peer that connection
punched, over UDP or its WebSocket, and from the controlled side
(socket_addr set) to the controller's connection in tcp_punch.
Candidates from unencrypted connections, for other peers, with both or
neither address field, longer than 1 KiB or beyond 64 per connection
are dropped without closing the connection."
```

---

### Task 5: A3 part 4, WebRTC counters in the summary line

**Files:**
- Modify: `src/rendezvous_server.rs`:
  - four counters next to `KX_FAILED`;
  - `spawn_kx_summary`;
  - counting in `handle_punch_hole_request`, `handle_hole_sent`, the `RelayResponse` arm and the `IceCandidate` arm.

**Interfaces:**
- Consumes: `webrtc_sdp_offer` from Task 3, `handle_ice_candidate(...) -> bool` from Task 4.
- Produces: the log line `key exchange (10 min): v1=<n> v0=<n> failed=<n>; webrtc offer=<n> answer=<n> ice=<n> dropped=<n>`.

- [ ] **Step 1: RED**

Run: `strings $SCRATCH/hbbs-a3 | grep -c "webrtc offer="`
Expected: `0`. The format string does not exist yet.

- [ ] **Step 2: Counters and the summary**

After `static KX_FAILED: AtomicU64 = AtomicU64::new(0);`, add:

```rust
// WebRTC signalling relayed since the last summary line (1.5.0).
static WEBRTC_OFFER: AtomicU64 = AtomicU64::new(0);
static WEBRTC_ANSWER: AtomicU64 = AtomicU64::new(0);
static WEBRTC_ICE: AtomicU64 = AtomicU64::new(0);
static WEBRTC_DROPPED: AtomicU64 = AtomicU64::new(0);
```

In `spawn_kx_summary`, replace

```rust
            let failed = KX_FAILED.swap(0, Ordering::Relaxed);
            if v1 + v0 + failed > 0 {
                log::info!("key exchange (10 min): v1={v1} v0={v0} failed={failed}");
            }
```

with

```rust
            let failed = KX_FAILED.swap(0, Ordering::Relaxed);
            let offer = WEBRTC_OFFER.swap(0, Ordering::Relaxed);
            let answer = WEBRTC_ANSWER.swap(0, Ordering::Relaxed);
            let ice = WEBRTC_ICE.swap(0, Ordering::Relaxed);
            let dropped = WEBRTC_DROPPED.swap(0, Ordering::Relaxed);
            if v1 + v0 + failed + offer + answer + ice + dropped > 0 {
                log::info!(
                    "key exchange (10 min): v1={v1} v0={v0} failed={failed}; \
                     webrtc offer={offer} answer={answer} ice={ice} dropped={dropped}"
                );
            }
```

…and update its doc comment to say it logs key exchanges and WebRTC signalling.

- [ ] **Step 3: Count**

- In `handle_punch_hole_request`, right after the `let webrtc_sdp_offer = …;` statement from Task 3:

```rust
                if !webrtc_sdp_offer.is_empty() {
                    WEBRTC_OFFER.fetch_add(1, Ordering::Relaxed);
                }
```

- In `handle_hole_sent`, before `let mut p = PunchHoleResponse {`:

```rust
        if !phs.webrtc_sdp_answer.is_empty() {
            WEBRTC_ANSWER.fetch_add(1, Ordering::Relaxed);
        }
```

- In the `RelayResponse` arm of `handle_tcp`, before `msg_out.set_relay_response(rr);`:

```rust
                    if !rr.webrtc_sdp_answer.is_empty() {
                        WEBRTC_ANSWER.fetch_add(1, Ordering::Relaxed);
                    }
```

- Replace the `IceCandidate` arm's `self.handle_ice_candidate(ic, conn).await;` with:

```rust
                    let counter = if self.handle_ice_candidate(ic, conn).await {
                        &WEBRTC_ICE
                    } else {
                        &WEBRTC_DROPPED
                    };
                    counter.fetch_add(1, Ordering::Relaxed);
```

- [ ] **Step 4: Check, lint, format**

Run the three commands from Task 2 Step 6. Expected: `check=0`, `clippy=0`, `fmt=0`.

- [ ] **Step 5: GREEN (about 11 minutes, in two calls)**

The summary needs one hbbs instance running past 600 s, so start it detached:

```bash
SQLX_OFFLINE=true cargo build --release --bin hbbs 2>&1 | tail -1
cp target/release/hbbs $SCRATCH/hbbs-a3
W=$SCRATCH/hbbs-run-summary; rm -rf $W; mkdir -p $W; cd $W
(RUST_LOG=info setsid nohup $SCRATCH/hbbs-a3 -p 31116 > hbbs.log 2>&1 & echo $! > hbbs.pid)
python3 -c "
import socket,time
for _ in range(100):
    try: socket.create_connection(('127.0.0.1',31116),timeout=1).close(); break
    except OSError: time.sleep(0.1)"
H=$SCRATCH/webrtc-harness/target/release/webrtc-harness
for c in offer ice ice-abuse; do $H 31116 id_ed25519.pub $c | tail -1; done
python3 -c 'import time; time.sleep(480)'
```

Then, in a second call:

```bash
W=$SCRATCH/hbbs-run-summary
python3 -c 'import time; time.sleep(140)'
grep "key exchange (10 min)" $W/hbbs.log
kill $(cat $W/hbbs.pid)
```

Expected: exactly one line, `key exchange (10 min): v1=7 v0=0 failed=0; webrtc offer=4 answer=1 ice=66 dropped=12`.

| Counter | Value | Comes from |
|---|---|---|
| v1 | 7 | `offer` 1 + `ice` 3 (controller, answer conn, trickle conn) + `ice-abuse` 3 (controller, trickle conn, second controller) |
| offer | 4 | `offer` 1 + `ice` 1 + `ice-abuse` 2 |
| answer | 1 | `ice` |
| ice | 66 | `ice` 2 + `ice-abuse` step 6: 64 |
| dropped | 12 | `ice-abuse`: steps 1, 2, 3 (one each), step 4 (two), step 5 (one), step 6 (six) |

- [ ] **Step 6: Commit, push, PR**

```bash
cd $REPO
git add src/rendezvous_server.rs
git commit -m "feat(rendezvous): count WebRTC signalling in the summary line

Append offers and answers forwarded and ICE candidates relayed or
dropped to the 10-minute line: key exchange (10 min): v1=.. v0=..
failed=..; webrtc offer=.. answer=.. ice=.. dropped=.."
git push -u origin feat/webrtc-signalling
```

Open the PR against `forapi`. The body must cover:
- the wire contract table above;
- the deliberate behaviour changes: map entries survive sends, a WebSocket peer gets every punch, and a connection keeps answering after a punch-hole request;
- the abuse limits;
- the Task 4 matrix and the Task 5 summary results.

Then run the final whole-branch review before asking the user to merge.

---

### Task 6: A3 release and user test gate

**Files:**
- Modify (k8s-deploy repo): `charts/rustdesk-server/values.yaml` (`image.tag`), `Chart.yaml` (`version` 0.2.7 → 0.2.8), `README.md` (a WebRTC note)

- [ ] **Step 1: CI, merge, tag `v1.1.15-mycustom.6` (after the user's go-ahead)**

```bash
cd $REPO && git switch forapi && git pull --ff-only
git tag -a v1.1.15-mycustom.6 -m "Release v1.1.15-mycustom.6" $(git rev-parse HEAD)
git push origin v1.1.15-mycustom.6
```

Wait for `publish.yml` for this tag to finish `completed success`. The arm64 build takes about 15 minutes, so use `timeout 560 gh run watch <id>` repeatedly. Then confirm the image:

Run: `crane manifest ghcr.io/rxxozqfoe/rustdesk-server:1.1.15-mycustom.6`
Expected: linux/amd64 and linux/arm64 entries.

- [ ] **Step 2: Chart 0.2.8**

In k8s-deploy, branch from `main` and make these changes:
- set `tag: "1.1.15-mycustom.6"`;
- set `version: 0.2.8`;
- after the "Key exchange v1" paragraph of `README.md`, add:

```markdown
**WebRTC (RustDesk 1.5.0)**: from rustdesk-server `1.1.15-mycustom.6`, hbbs
relays the WebRTC offer, answer and ICE candidates between 1.5.0 clients, so
peers that turn on "Enable WebRTC P2P connection" (a client setting, off by
default against a self-hosted server) can connect over WebRTC. ICE servers are
the clients' own `ice-servers` option (public STUN when unset); no TURN server
is provided, and peers that cannot connect directly keep using hbbr. The
`key exchange (10 min)` line counts `webrtc offer/answer/ice/dropped`.
```

Then:
- run `bash ci/validate.sh`, which must pass;
- commit `Use rustdesk-server 1.1.15-mycustom.6; release 0.2.8`, open the PR, and ask the user to merge;
- after the merge and the user's go-ahead, tag `v0.2.8`, wait for `publish.yml`, and confirm with `helm pull oci://ghcr.io/rxxozqfoe/charts/rustdesk-stack --version 0.2.8`.

- [ ] **Step 3: Hand the A3 test plan to the user and wait**

Send the A3 test plan table from the spec (section "A3", five rows). On a failure, collect:
- the controller's client log lines around `used to establish`, `WebRTC failed`, `cannot be encrypted` and `ICE candidate`;
- the hbbs `key exchange (10 min)` line.

Then stop.
