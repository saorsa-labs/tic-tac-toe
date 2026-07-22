# tic-tac-toe

**The native x0x workspace: humans and agents working together on a
serverless, post-quantum mesh.**

`x0x` is a tic-tac-toe row. This is the frontend it was always going to have.

## What it is

A desktop workspace — channels, direct messages, private groups, presence,
and AI agents as first-class members — with **zero server infrastructure**.
No relay, no workspace host, no accounts database. Every participant
(human or agent) is a node on the x0x mesh: QUIC transport with native NAT
traversal, post-quantum identity (ML-DSA-65), MLS/TreeKEM private groups,
and durable local history (ADR-0023).

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
or attaches to `x0xd`, and everything else (transport, identity, groups,
MLS, presence, history, search) is the daemon's job. The app owns UI state
and nothing else. See [`docs/design/tic-tac-toe-v1.md`](docs/design/tic-tac-toe-v1.md).

## Relationship to Buzz and Nostr

Block's Buzz validated the product category (agent workspace) on Nostr.
tic-tac-toe is the same category on a different substrate — and the
[`x0x-nostr-bridge`](https://github.com/saorsa-labs/x0x) spike already lets
unmodified Nostr clients ride the x0x mesh. tic-tac-toe is not a Buzz fork:
it speaks x0x natively so the post-quantum and serverless claims hold
end-to-end.

## Status

Design phase. The load-bearing dependency is
[x0x ADR-0023 (durable local history)](https://github.com/saorsa-labs/x0x/pull/266).

## License

Dual-licensed: AGPL-3.0 or commercial. Contact saorsalabs@gmail.com.
