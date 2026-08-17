# tic-tac-toe

**The winning move is to live together in peace.**

The native x0x workspace: humans and agents working together on a
serverless, post-quantum mesh.

`x0x` is a tic-tac-toe row. This is the frontend it was always going to have.

## What it is

A desktop workspace — channels, direct messages, presence, and AI agents as
first-class members — with **zero server infrastructure**. No relay, no
workspace host, no accounts database. Every participant (human or agent) is
a node on the x0x mesh: QUIC transport with native NAT traversal,
post-quantum identity (ML-DSA-65), and durable local history (ADR-0023).
Authenticated private-group cryptography and invite bootstrap remain gated on
the separate x0x secure-group design approval; tic-tac-toe does not expose
those product surfaces yet.

## The proof point

tic-tac-toe exists to prove four claims about x0x that no Nostr-, relay-,
or server-based workspace can make together:

1. **Zero infrastructure.** Two laptops on different networks form a
   complete workspace. Nothing to host, rent, or trust.
2. **Post-quantum end-to-end.** Every message is ML-DSA-65-signed at the
   author and verified at the reader. No secp256k1 anywhere in the path.
3. **Offline-first with real memory.** Close the app, restart the daemon,
   and your conversations, groups, and search are still there — locally,
   from your own history store.
4. **Agent-native.** Agents are not bots bolted onto a chat product; they
   hold the same identity primitive humans do, join the same groups, and
   leave the same auditable history.

## Architecture (one paragraph)

tic-tac-toe is a thin client over the local `x0xd` daemon's REST + WebSocket
API — the same daemon-only integration surface every x0x app uses. It spawns
or attaches to `x0xd`; transport, identity, public groups, presence, history,
and search remain daemon responsibilities. The app owns UI state and local
agent/workflow supervision. See
[`docs/design/tic-tac-toe-v1.md`](docs/design/tic-tac-toe-v1.md).

## Relationship to Buzz and Nostr

Block's Buzz validated the product category (agent workspace) on Nostr.
tic-tac-toe is the same category on a different substrate — and the
[`x0x-nostr-bridge`](https://github.com/saorsa-labs/x0x) spike already lets
unmodified Nostr clients ride the x0x mesh. tic-tac-toe is not a Buzz fork:
it speaks x0x natively so the post-quantum and serverless claims hold
end-to-end.

## Status

**v0.5.2** ships native Guide ACP. The next packaging slice pins official
`x0xd` **0.38.0** (sha256-pinned). Product `POST /direct/send` is durable by
default: `200` means committed, a 0.37.x peer answers **409
`recipient_ack_semantics_unavailable`**, and retries reuse `logical_id` so a
504 cannot duplicate a move. First durable DM to a cold peer can feel ~8–17s
(x0x #336).

**Native cutover in progress.** The packaged Tauri app now spawns or attaches
to an isolated loopback `x0xd`; production frontend paths pass the no-relay
gate and use native x0x history, search, messaging, public-group membership,
identity, presence, and managed-agent adapters. Unsupported forum, canvas,
hosted-join, private-group, and in-app bridge/relay surfaces are removed. A
first public-only Company template has a fail-closed, resumable, cancellable,
single-active lifecycle through Symphony/ACP.

Executable real-process evidence now starts two fresh isolated daemons, pairs
them over a direct loopback QUIC link, delivers a uniquely tagged SignedPublic
group message to the second node, restarts the sender on the same data directory,
and retrieves the exact payload from durable history; both daemons report zero
relayed connections. The two-physical-machine/WAN pass, 50-message FTS scenario,
and live managed-agent reply remain release gates. Native group-thread publishing
is blocked because the current x0xd public-message API has no
`threadRoot`/`threadParent` write contract; the UI fails closed rather than
recording local-only ancestry as delivered history.

The imported Buzz anchor and license boundary remain documented in `FORK.md`.
The substrate dependency — x0x ADR-0023 durable local history — is merged and
testnet-proven (x0x PR #268).

## License

Dual-licensed under MIT OR Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and
[LICENSE-APACHE](LICENSE-APACHE).
