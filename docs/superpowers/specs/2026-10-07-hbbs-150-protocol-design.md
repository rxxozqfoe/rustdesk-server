# hbbs: RustDesk 1.5.0 protocol support

Status: design approved in conversation 2026-10-07, pending review of this document.
Scope: `hbbs` only (`src/rendezvous_server.rs`, `libs/hbb_common`). `hbbr`, rustdesk-api and the
Helm chart need no code changes beyond image bumps.

## Background

RustDesk client 1.5.0 (2026-09-30) added two features that need the rendezvous server's help:

- **Kx v1** (rustdesk/rustdesk#16326, rustdesk/hbb_common#614). The client and server negotiate
  a key exchange version. Version 1 splits the exchanged key into one per direction and binds
  the handshake transcript into both. A server marks itself as v1 by setting bit 255 of the
  ephemeral X25519 key it signs and by sending signed `KxParams`.
- **WebRTC** (rustdesk/rustdesk#15684). WebRTC is an extra direct transport, raced against
  UDP/TCP hole punching and the relay. The SDP offer travels in the punch-hole request, the answer
  in the punch reply, and ICE candidates trickle through the rendezvous server. A client only
  offers WebRTC when its local option `enable-webrtc` is on. Against a self-hosted server it is
  off unless the user turns it on.

Upstream OSS rustdesk-server (last commit 2026-08-07) implements neither, so this fork does.
Clients keep working against a server that ignores the new fields: they fall back to Kx v0 and
to hole punching or the relay. The work is therefore additive. Its risk lies in breaking what
works today.

Current state of this fork (`forapi` at 62fe5d5):

- `libs/hbb_common` is pinned at 7e1c392. The 1.5.0 client pins 229b904, 120 commits later. That
  commit carries the Kx v1 helpers and the new proto fields: `KeyExchange.version`/`signed_params`,
  `KxParams`, `webrtc_sdp_offer`/`webrtc_sdp_answer`, `IceCandidate`, and `switch_code`.
- The key exchange runs on TCP connections only. Phase 1 is `key_exchange_phase1`, called from
  `handle_listener_inner`; phase 2 is the `KeyExchange` arm of `handle_tcp`. The ephemeral X25519
  pair (`Inner.secure_tcp_pk_b/sk_b`) is created once per process and shared by every connection.
- A connection's cipher lives in its `Sink` (`SafeTcpStreamSink.encrypt`), and that one `Encrypt`
  is used both to decrypt in the read loop and to encrypt on send.
- A punch-hole or relay request moves the connection's sink into `tcp_punch`, and a WebSocket
  `RegisterPk` moves it into `ws_map`. `send_to_tcp`, `send_to_tcp_sync` and
  `handle_tcp_punch_hole_request` remove the entry when they send, so each map delivers exactly one
  message per entry. Once the sink is taken, the read loop can no longer decrypt, so a second
  encrypted message on that connection closes it.

## Goals

- G1: Track hbb_common 229b904 with no behaviour change.
- G2: Negotiate Kx v1 with 1.5.0 clients, keep v0 for older clients, and allow switching back to
  v0-only without a new image.
- G3: Relay WebRTC signalling (offer, answer, ICE candidates) between two 1.5.0 clients, so
  that peers which turn on WebRTC can connect over it, without letting a client push messages to
  peers it is not connecting to.

## Non-goals

- TURN or STUN hosting. Clients pick their ICE servers (`ice-servers`, public STUN when unset),
  and the hbbr relay already covers peers that cannot connect directly.
- `switch_code` / `/api/switch-grant` verification and the missing `MUST_LOGIN` check on the
  RequestRelay path. These are tracked as a separate sub-project.
- Key exchange over WebSocket (clients do not run one there), and moving hbbs's own
  tokio-tungstenite 0.21 to 0.29.

## Delivery

Three PRs, each merged, released and tested on real clients before the next one starts:

| Step | Release | Rollback |
|---|---|---|
| A1 hbb_common bump | `v1.1.15-mycustom.4` + chart patch | previous image tag |
| A2 Kx v1 | `v1.1.15-mycustom.5` + chart patch | `KX_MAX_VERSION=0`, or the previous image |
| A3 WebRTC signalling | `v1.1.15-mycustom.6` + chart patch | previous image tag |

Every step must pass `cargo fmt -p hbbs -- --check`, `cargo clippy -p hbbs --no-deps -- -D warnings`
and `cargo check -p hbbs`, and must keep the malformed-KeyExchange checks from #29 passing: a
release build closes the connection and keeps running. Per the repository rules, `cargo test` is
not run.

## A1: bump hbb_common to 229b904

- Move the `libs/hbb_common` submodule to 229b904 (rustdesk/hbb_common, "Kx v2" merge).
- Make it compile, and nothing else. hbb_common dropped `message.proto`, so `IdPk` (used in
  `get_pk`) moves from `hbb_common::message_proto` to `hbb_common::rendezvous_proto`. The wire
  format is unchanged. If any other compile error can only be fixed by changing behaviour, stop and
  raise it instead of deciding.
- `Cargo.lock` picks up hbb_common's new dependencies, including tokio-tungstenite 0.29 from
  `rustdesk-org`, which `deny.toml` already allows. New cargo-deny advisories are reported, not
  ignored.
- Local check: a release build completes a v0 handshake with a client-side harness (see A2
  Verification) and survives the three malformed-KeyExchange cases.

Test plan (user, after deploying mycustom.4):

| # | Test | Pass | Fail |
|---|---|---|---|
| 1 | Restart a 1.4.9 client | "Ready" within 30 s; its last-online time updates in the console | stuck connecting / not ready |
| 2 | Restart a 1.5.0 client | same | same |
| 3 | Remote control 1.4.9 ↔ 1.5.0, both directions | session opens, direct or relayed | stuck connecting, key/encryption errors |
| 4 | Log in on a 1.5.0 client without an API server set (HTTP-over-rendezvous) | login succeeds; client log has no `TCP proxy fallback also failed` | login fails or that line appears |
| 5 | `kubectl logs` hbbs the next day | no `panicked`; `Handshake failed` count not rising | either |

## A2: Kx v1

### Protocol (hbbs is the responder)

Phase 1, sent when a TCP connection opens (unchanged trigger):

1. Generate a fresh X25519 pair for this connection.
2. If `KX_MAX_VERSION` ≥ 1, set bit 255 of the public key (`pk[31] |= 0x80`), giving `pk_sent`.
   X25519 ignores that bit, so a v0 client derives the same shared key from it. If
   `KX_MAX_VERSION` is 0, then `pk_sent` is the key as generated.
3. Send `KeyExchange { keys: [sign(pk_sent)], version: V, signed_params }` signed with the
   server's long-term key, where `V = KX_MAX_VERSION`. `signed_params` is the attached signature of
   `KX_PARAMS_DOMAIN ("rdkx-params") ‖ KxParams { pk: pk_sent, version: V }`. When V is 0, both
   `version` and `signed_params` are left unset: the exact bytes hbbs sends today.
4. Keep `{ secret key, pk_sent, advertised: V }` as the connection's pending exchange.

Phase 2, the client's `KeyExchange { keys: [their_pk, sealed], version: picked }`:

1. Take the pending exchange. If there is none (no phase 1 on this connection, or a second
   `KeyExchange`), log and close.
2. `keys.len() != 2` or `picked > advertised`: log and close.
3. `Encrypt::decode(sealed, their_pk, sk)`, which checks lengths and never panics. On error, log
   and close.
4. If `picked == 0`, use `Encrypt::new(key)`. Otherwise use `Encrypt::new_split(key, false,
   &KxTranscript { initiator_pk: their_pk, responder_pk: pk_sent, advertised, picked })`.

### Connection state

The read loop in `handle_listener_inner` gets a per-connection state passed to `handle_tcp` in
place of the bare `Option<Sink>`. A3 needs the same state:

- `sink`: the send half, which keeps the sending `Encrypt`.
- `rx`: the receiving `Encrypt`, owned by the read loop. Both are clones of the negotiated
  `Encrypt`. `Encrypt` is `Clone`, and `enc` and `dec` use separate counters and, under v1,
  separate keys.
- `pending_kx`: the phase-1 secret, consumed once.

The process-wide `Inner.secure_tcp_pk_b/sk_b` is removed.

### Configuration

`KX_MAX_VERSION` is an environment variable read once at startup, like `ALWAYS_USE_RELAY` and
`MUST_LOGIN`. Its value is `0` or `1`, default `1`; any other value logs a warning and uses `1`.
The Helm chart needs no change: set it through `rustdesk-server.hbbs.extraEnv`.

### Observability

Counters for v0 handshakes, v1 handshakes and failed handshakes. Every 10 minutes, when any
counter moved, hbbs logs one info line and resets them:
`key exchange (10 min): v1=12 v0=3 failed=0`. A3 appends its WebRTC counters to this line.

### Verification

A small client-side harness, kept out of the repository, built against hbb_common 229b904. It
follows the 1.5.0 client's `key_exchange` (src/common.rs at tag 1.5.0):

- It verifies the phase-1 signature and the signed params against the server key.
- It runs v1 when advertised, and also v0 by ignoring the advertisement.
- After the handshake it sends an encrypted `OnlineRequest` and expects a decryptable
  `OnlineResponse`, which proves both directions' keys agree.

The harness runs against a release build twice:

- with `KX_MAX_VERSION` unset, where both v1 and v0 succeed;
- with `KX_MAX_VERSION=0`, where v1 is never advertised.

Two kinds of bad phase 2 must close only that connection:

- the three malformed messages from #29, now sent after a v1 advertisement;
- a client that picks version 2.

The summary line must count every one of these runs correctly.

### Test plan (user, after deploying mycustom.5)

| # | Test | Pass | Fail |
|---|---|---|---|
| 1 | 1.5.0 client: restart, remote control once, log in without an API server | ready, connects, logs in | not ready, connect fails, or client log shows `Key exchange version` / `does not match its signature` |
| 2 | 1.4.9 client: same | same | same |
| 3 | hbbs summary line 10 min after 1 and 2 | `v1` > 0, `v0` > 0, `failed` 0 or a few (scanners) | `v1=0`, or `failed` keeps rising |
| 4 | Set `KX_MAX_VERSION=0` via `hbbs.extraEnv`, redeploy, repeat 1 | 1.5.0 client works; summary shows `v1=0` | 1.5.0 client cannot connect |

## A3: WebRTC signalling

### Routing

| Message | From → to | hbbs |
|---|---|---|
| `PunchHoleRequest.webrtc_sdp_offer` | controller → controlled | Copy into `PunchHole.webrtc_sdp_offer`, delivered the way that `PunchHole` already is (UDP, or the controlled side's WebSocket). Do not copy it when hbbs itself forces the relay (`ALWAYS_USE_RELAY`, or exactly one side on the LAN). A relay the client asked for (`force_relay` in the request) keeps the offer: its envelope carries the client's ICE policy. |
| `PunchHoleSent.webrtc_sdp_answer` | controlled → controller | Copy into `PunchHoleResponse.webrtc_sdp_answer` in `handle_hole_sent`, both the UDP and the TCP path. |
| `RelayResponse.webrtc_sdp_answer` | controlled → controller | Forward unchanged, as the field is part of the message. |
| `IceCandidate` with `id` | controller → controlled | Only on an encrypted connection that sent a `PunchHoleRequest` for that `id`. Delivered to the peer like `PunchHole`. |
| `IceCandidate` with `socket_addr` | controlled → controller | Only on an encrypted connection. Delivered to the controller's pending connection for that address; dropped if there is none. |

`session_key` and `candidate` are opaque to hbbs and forwarded unchanged.

### Pending connections

Two maps hold connections hbbs sends to later:

- `tcp_punch`: a connection that sent a punch-hole or relay request and waits for the reply. In
  practice this is the controller.
- `ws_map`: a peer that registered over WebSocket (`RegisterPk`). In practice this is the
  controlled side of a WebSocket client.

Today both move the sink into the map, and the first send removes the entry. So a controller gets
one reply, and a WebSocket peer gets one `PunchHole` until it registers again.

They change to holding a shared handle to the connection's send half (for example
`Arc<Mutex<Sink>>`):

- The connection's own read loop keeps the handle too, and keeps decrypting with `rx`.
- Sending no longer removes the entry.
- The read loop removes its own entry when its connection closes. Re-registering replaces a
  `ws_map` entry, as now.

Every entry thus belongs to a live connection, and no periodic sweep is needed. The controller's
connection can receive the punch reply followed by any number of ICE candidates, and a WebSocket
peer can receive candidates and further punch requests. Messages hbbs already sends through these
maps keep their content and order.

### Abuse limits

- `IceCandidate` on an unencrypted connection: dropped and counted.
- `candidate` longer than 1 KiB, or more than 64 candidates on one connection: dropped and
  counted. Neither closes the connection, so a buggy client degrades to the other transports.
- An `id` this connection did not punch for, or a `socket_addr` with no pending controller:
  dropped and counted.

### Observability

The summary line adds `webrtc offer=N answer=N ice=N dropped=N`.

### Verification

A harness acting as both peers. The controlled peer registers over UDP. The controller runs the
A2 handshake and sends a `PunchHoleRequest` with an offer. The harness checks that:

- the offer arrives in `PunchHole`;
- the answer arrives in `PunchHoleResponse`;
- candidates are delivered in both directions;
- an injected candidate for another `id` and an oversized or excess candidate are dropped;
- a punch-hole request without an offer behaves exactly as today.

### Test plan (user, after deploying mycustom.6)

Two 1.5.0 clients, both with "Enable WebRTC P2P connection" on, ideally on different networks:

| # | Test | Pass | Fail |
|---|---|---|---|
| 1 | Turn off TCP and UDP hole punching on the controller, connect | controller log: `used to establish WebRTC connection`; direct-connection indicator | log shows `Relay`, `WebRTC failed` or `cannot be encrypted` |
| 2 | Defaults restored, connect | connects (WebRTC, UDP or TCP may win the race) | does not connect |
| 3 | 1.4.9 ↔ 1.5.0, both directions | connects as before | does not connect |
| 4 | WebRTC off on one side, connect | connects via punching or relay | does not connect |
| 5 | hbbs summary line after testing | `offer`, `answer`, `ice` > 0; `dropped` 0 | `offer` > 0 with `answer` or `ice` at 0, or `dropped` rising |

If both peers sit behind symmetric NATs, test 1 cannot pass without TURN. In that case read the
client log before treating it as an hbbs failure.

## Risks

- A wrong Kx v1 implementation breaks every 1.5.0 client's encrypted connection to hbbs,
  including HTTP-over-rendezvous. Mitigations: the harness, test plan A2, and `KX_MAX_VERSION=0`.
- The pending-connection refactor touches every punch-hole reply. It also changes one behaviour on
  purpose: a WebSocket peer keeps receiving punch requests after the first one. Mitigations: an
  unchanged message order, the harness case for offer-less punching, and tests A3-3 and A3-4.
- The 120-commit hbb_common bump can change behaviour indirectly, for example in config or
  socket helpers. A1 ships it alone so that any regression is attributable.
- ICE candidates reveal the peers' local addresses to each other. That is inherent to WebRTC
  and only happens between peers that are connecting.

## References

- rustdesk/rustdesk#16326 (Kx v1), #15684 (WebRTC), tag 1.5.0 `src/common.rs` `key_exchange`,
  `src/client.rs`, `src/rendezvous_mediator.rs`
- rustdesk/hbb_common 229b904 `src/tcp.rs` (`KX_VERSION_LATEST`, `KX_PARAMS_DOMAIN`,
  `kx_version_for`, `KxTranscript`, `Encrypt::new_split`, `Encrypt::decode`),
  `protos/rendezvous.proto`
- rxxozqfoe/rustdesk-server#29 (malformed KeyExchange fix)
