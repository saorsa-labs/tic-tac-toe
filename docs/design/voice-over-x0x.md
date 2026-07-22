# Voice over x0x — huddle decision and the saorsa-webrtc path

**Status:** Decision record + workstream plan
**Date:** 2026-07-22
**Decision:** Buzz's relay-mixed huddle voice is **cut from tic-tac-toe v1**
(M1b ships media + invites only). Voice returns as its own workstream built
on a revived `saorsa-webrtc`, targeting 1:1 calls first, mesh huddles (≤4)
after.

## Why not bridge Buzz's huddle

Buzz huddles are relay-mixed (`WS /huddle/{channel}/audio` → Opus frames
through the relay). Reproducing that in the bridge means building a voice
mixer into a compatibility facade — serverless is the whole point of
tic-tac-toe, so voice should be peer-to-peer over the mesh, not faked
through a local relay.

## saorsa-webrtc assessment (read-only audit, 2026-07-22)

**Right architecture:** QUIC-native media over ant-quic — no ICE/STUN/TURN,
no SRTP/DTLS second stack; media and signaling can share one PQC QUIC
transport. Signaling is a clean trait (`SignalingTransport`,
`signaling.rs:46`) with QUIC-native message flow
(`CapabilityExchange → ConnectionConfirm → ConnectionReady`, ~1.5 RTT, no
SDP); an `X0xSignaling` adapter over x0x DMs mirrors the shipped gossip
example almost 1:1. `LinkTransport` (`link_transport.rs:102`) is the media
seam an ADR-0022 byte-stream impl plugs into.

**Not shippable today — the honest gaps (its own status docs overstate):**

| Gap | Evidence | Work |
|---|---|---|
| Opus codec is a stub (fake frames; real `opus` crate optional + unwired) | `codecs/src/opus.rs:10,109` | Wire real libopus; interop test |
| QUIC media path only mock-tested (no two-real-nodes test) | `tests/integration_quic_loopback.rs` uses in-process mpsc | Real two-node e2e over ant-quic |
| Reliable-ordered streams only — no QUIC DATAGRAM ⇒ head-of-line blocking under loss | `quic_media_transport.rs` framing | Unreliable/datagram lane in ant-quic 0.27.x for audio frames |
| ant-quic pinned 0.20.x (x0x ships 0.27.34) | `core/Cargo.toml:63`, lockfile | Dep upgrade across 7 minor versions |
| Group calls are types only (`CallArchitecture::{Mesh,SFU}` enum, no impl) | `types.rs:251` | Greenfield mesh (≤4) or elected-mixer (≤8) |
| Mic/speaker I/O absent | — | cpal (or platform) capture/playout |
| ~5 months idle; "Complete 🎉" summary contradicted by TODOs | `FINAL_COMPLETION_SUMMARY.md:3` vs `opus.rs:109` | Treat status docs as aspirational; re-baseline |

## Workstream plan (post-ttt-M1)

1. **V0 — revive:** ant-quic 0.27.x upgrade, wire real Opus, delete/label
   stub codecs, two-real-nodes RTP e2e (the test the repo claims but lacks).
2. **V1 — x0x adapters:** `X0xSignaling` over DMs; `LinkTransport` over
   ADR-0022 byte streams; audio lane on QUIC DATAGRAM (ant-quic work if the
   frame isn't exposed yet — scope with the ant-quic team first).
3. **V2 — 1:1 calls in tic-tac-toe:** call UI reuses Buzz's huddle
   components; acceptance = two-Studio call across the real mesh, packet
   capture shows one QUIC flow.
4. **V3 — mesh huddles ≤4** (client-side mix, jitter buffer); elected-mixer
   for larger rooms is a separate design.

Until V2 lands, tic-tac-toe's huddle UI entry points stay hidden (Stage-0
cut list, alongside pairing and git).
