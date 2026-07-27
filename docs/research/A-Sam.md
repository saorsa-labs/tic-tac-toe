# Lane A — Buzz parity and relay-free offline catch-up

- **Author:** Sam
- **Date:** 2026-07-26
- **Scope:** research and design only; no implementation
- **tic-tac-toe state reviewed:** `517812bcc4c8ac1a40cf28e38f986d19f7935033`
- **x0x state reviewed:** `e3013710d7ed69077de9a799dffdbeb5ac80535a`

## Executive conclusion

tic-tac-toe has the right product base and most of the right transport,
identity, group, and local-durability primitives. It does **not** yet have
Buzz-equivalent availability semantics.

The blocking distinction is:

> **Reconciliation discovers which retained objects differ. Custody determines
> whether any node retained a missed object at all.**

x0x's current `GET /history`, FTS search, and WebSocket `backfill` replay the
local daemon's SQLite rows. They give restart survival to a node that witnessed
the message. They cannot recover a message published while that node was
offline. This is intentional: ADR-0023 says each node records what it witnessed,
explicitly defers serving history/cross-node backfill to a new ADR with its own
trust model, and calls its V1 local-only privacy claim load-bearing
(`x0x@e301371:docs/adr/0023-durable-local-history.md:42-50,91-124,164-177`).
That is a designated review procedure, not a prohibition on the deferred work.
The README's “offline-first” claim is therefore true only in the
close-app/restart-daemon sense it actually states
([`README.md:19-31`](../../README.md)); it is not yet relay-equivalent
missed-while-shut-down delivery.

My recommendation is a new x0x ADR for **opaque asynchronous delivery through
explicit participant-owned custodians**:

- recipient devices first;
- then a small, user-selected set of trusted contact/mailbox nodes;
- capability-addressed queues with no stable Agent ID in the outer envelope;
- end-to-end ciphertext only at custodians;
- bounded TTL and bytes, deposit/fetch capabilities, deduplication, receipts,
  and eventual garbage collection;
- deterministic range reconciliation first, Rateless IBLT only after real
  measurements justify it;
- a separate, opt-in same-user device-sync/backup design for full history
  recovery; a bounded delivery spool must not be marketed as an archive;
- no global DHT, no arbitrary public archive, and no reuse of an ADR-0023 local
  history store as the ordinary delivery service.

This is not “no storage.” Offline delivery is impossible without an
always-reachable holder. It is **no privileged global relay**: custody is
explicit, scoped, replaceable, and owned by participants. The privacy cost is
real and must be shown in product language: custodians still observe timing,
sizes, queue pseudonyms, and usually IP addresses; replication gives better
availability by creating more potential correlators.

## 1. Ground truth and evaluation method

tic-tac-toe is already a deep Buzz fork, not a greenfield frontend decision.
The imported anchor is Buzz Desktop `0.4.24` at `710ed9ff`; the full desktop
tree and five path-dependent crates were copied while relay/server crates were
excluded
([`FORK.md:3-18,28-40`](../../FORK.md)). The staged design has also already
chosen a localhost Nostr facade followed by native x0xd integration
([`docs/design/buzz-fork-plan.md:47-59`](../design/buzz-fork-plan.md)).
This report tests that design against the actual missing semantics; it does not
reopen “fork versus frontend.”

The comparison uses three verdicts:

- **Ready** — x0x exposes a daemon contract close enough to back the Buzz
  behavior without inventing a distributed subsystem.
- **Primitive** — transport/storage/crypto exists, but tic-tac-toe must define
  and test an application contract that Buzz's relay currently supplies.
- **Gap** — the required behavior or availability property is absent from the
  reviewed x0x REST/WS router and documented protocol.

Negative claims below are scoped to x0x's registered daemon router
(`x0x@e301371:src/server/mod.rs:1148-1357`),
its API reference, and the cited source modules at `e301371`. The embedded x0x
GUI is treated as evidence of an app-layer prototype, not as a daemon protocol.
The tic-tac-toe baseline was advanced from the originally assigned `db62c23` to
`origin/main` at `517812b`; the only intervening product-tree delta is the M1a
bridge gate (`justfile`, `scripts/bridge-gate.sh`), and the design sources cited
below are unchanged.

## 2. The 13 relay surfaces: parity matrix

These are the 13 source-enumerated relay dependencies in the fork plan
([`docs/design/buzz-fork-plan.md:72-102`](../design/buzz-fork-plan.md)).

| # | Buzz relay obligation | x0x evidence | Verdict and exact gap |
|---|---|---|---|
| 1 | Nostr WS lifecycle: `REQ`/`EVENT`/`EOSE`/`CLOSE`, subscriptions, and NIP-29 group semantics | x0x has generic `/ws` subscribe/publish/unsubscribe frames and `/ws/direct` (`x0x@e301371:src/server/mod.rs:1338-1341`; `x0x@e301371:docs/api-reference.md:895-927`) | **Primitive.** Bridge translation is straightforward for live traffic. EOSE-equivalent `live` exists in source but is missing from `x0x@e301371:docs/api-reference.md`; stored replay is only local. NIP-29 policy and event semantics remain bridge/application work. |
| 2 | NIP-42 relay authentication | x0xd exposes a loopback bearer/session-token control plane and signed Agent/Machine identity; contacts and machine pinning are registered at `x0x@e301371:src/server/mod.rs:1152-1195,1295-1306` | **Ready for localhost, not protocol-equivalent.** The facade can terminate NIP-42 locally and map it to the daemon identity. It must never imply that secp256k1/Nostr auth survives onto the mesh. |
| 3 | `POST /events`, including snapshot imports | x0x has generic `POST /publish`, group public send, secure group encrypt/decrypt, and direct send (`x0x@e301371:src/server/mod.rs:1164-1167,1252-1263,1280-1294`) | **Primitive.** Ordinary live events can map to typed x0x envelopes. Snapshot import is not equivalent to network history import and must not bypass authorship, scope, retention, or duplicate rules. |
| 4 | `POST /query` with Buzz filters and keyset pagination — main timeline read path | Buzz's desktop explicitly depends on server-side composite-cursor history ([`desktop/src-tauri/src/commands/messages.rs:360-408`](../../desktop/src-tauri/src/commands/messages.rs)). x0x provides scope-local history/query/search only (`x0x@e301371:src/server/routes/history.rs:20-50,75-137`) | **Blocking gap for offline parity.** Locally witnessed history can satisfy pagination after restart. There is no network query or remote catch-up source, by ADR-0023 design. The bridge can emulate filters over local rows only after delivery/custody is solved. |
| 5 | NIP-50 message and profile search | Buzz calls relay-backed message/profile search ([`desktop/src-tauri/src/commands/messages.rs:150-158`](../../desktop/src-tauri/src/commands/messages.rs); `tic-tac-toe@517812b:desktop/src-tauri/src/commands/profile.rs:264-334`) | **Primitive.** `GET /history/search` is good scoped local FTS. There is no exact cross-workspace/global people-search equivalent; discovery is local/gossip-sharded. Search cannot find an event never delivered. |
| 6 | Relay-computed `thread_metadata` and verified ancestry | Buzz has explicit forum roots/replies and channel thread navigation ([`desktop/src-tauri/src/commands/messages.rs:160-225`](../../desktop/src-tauri/src/commands/messages.rs); `tic-tac-toe@517812b:desktop/src/app/navigation/useAppNavigation.ts:148-188,203-227`) | **Primitive.** x0x's embedded GUI implements thread topics and a local cache (`x0x@e301371:src/gui/x0x-gui.html:900-990,2127-2145`), proving generic pub/sub can carry the UI. The daemon has no durable thread projection, ancestry validation, counters, or query contract. |
| 7 | `GET /info` membership gate | x0x named groups expose membership, two roles, policy, invites, joins, bans, and requests (`x0x@e301371:docs/api-reference.md:413-470`) | **Mostly ready.** Bridge membership admission can map to named groups. Buzz has owner/admin/member/guest/bot roles while x0x intentionally accepts only admin/member, so app-level softer roles must not be promoted to x0x Admin. |
| 8 | Live thread-summary fan-out | Buzz's thread badges depend on relay-side summary events ([`docs/design/buzz-fork-plan.md:85-88`](../design/buzz-fork-plan.md)) | **Gap as a daemon contract.** The GUI prototype keeps reply counts and participants in memory (`x0x@e301371:src/gui/x0x-gui.html:2127-2145`). A durable, convergent, bounded summary projection must be specified or computed locally from a complete delivered event set. |
| 9 | Blossom media upload/download and `buzz-media://` | Buzz securely opens, validates/transcodes, uploads, and proxies media ([`desktop/src-tauri/src/commands/media.rs:517-550,692-750`](../../desktop/src-tauri/src/commands/media.rs)). x0x exposes direct file transfer create/list/accept/reject (`x0x@e301371:docs/api-reference.md:830-855`) | **Primitive.** File delivery exists; durable blob availability, content-addressed retrieval, thumbnails/posters, gallery semantics, and offline providers do not. Large content needs explicit providers/pinning; a custody mailbox should carry manifests/small envelopes, not become an unbounded media server. |
| 10 | Invite/join-policy APIs | x0x has group invites, request-to-join, approval/rejection, discovery classes, policy, and bans (`x0x@e301371:docs/api-reference.md:413-453,488-552`) | **Ready with mapping work.** This is one of the strongest parity areas. Test role/policy downgrade explicitly; x0x Admin is more powerful than a Buzz moderator role. |
| 11 | Huddle voice WS | Buzz defines huddle lifecycle and reactions ([`crates/buzz-core/src/kind.rs:325-336,450-462`](../../crates/buzz-core/src/kind.rs)). x0x ships feature-gated saorsa-webrtc signaling over DMs and link transport over ADR-0022 streams (`x0x@e301371:src/voice/mod.rs:1-30`) | **Primitive, correctly cut from v1.** Crypto/transport adapters exist, but no huddle REST/WS product contract is registered in the reviewed router. |
| 12 | NIP-11/pairing discovery | Buzz defines pairing as an ephemeral event (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:325-331`). x0x exposes Agent/Machine discovery, connect, list, and pinning (`x0x@e301371:src/server/mod.rs:1174-1195,1295-1306`) | **Primitive.** Desktop multi-device UX and recovery/pair ceremony are not equivalent, but the identity substrate is stronger than a blank gap. Mobile pairing remains an explicit v1 cut. |
| 13 | Git smart-HTTP and project forge semantics | Buzz registers repository, patch, PR, issue, and status kinds ([`crates/buzz-core/src/kind.rs:468-487`](../../crates/buzz-core/src/kind.rs)) and routes project/PR/issue navigation (`tic-tac-toe@517812b:desktop/src/app/navigation/useAppNavigation.ts:82-121`) | **Gap, explicitly acceptable for v1.** No Git/PR/issue route appears in x0x's registered router at `x0x@e301371:src/server/mod.rs:1148-1357`. Generic stores are not a Git forge. |

### Matrix verdict

The staged facade is a **useful migration scaffold**, not a parity shortcut. It
can translate live messages, membership, presence, and identity quickly. It
cannot manufacture:

1. events missed while every authorized recipient device was offline;
2. relay-computed query/thread projections over events it never received;
3. durable blob availability; or
4. application services such as Git, workflows, and moderation.

Thus Stage 1 can demonstrate “Buzz UX over live x0x peers,” but it must not be
described as Buzz availability parity until the custody ADR lands.

## 3. Product parity beyond the 13 transport surfaces

The transport matrix understates Buzz's product surface. The imported desktop
routes expose Agents, Pulse, Projects, Workflows, Channels, DMs, forum posts,
and settings
([`desktop/src/app/navigation/useAppNavigation.ts:49-239`](../../desktop/src/app/navigation/useAppNavigation.ts)).
Buzz's event vocabulary separately names message edit/pin/bookmark/schedule/
reminder/canvas, group-DM membership, agent jobs, forums, workflows,
approvals, audit, huddles, and Git
([`crates/buzz-core/src/kind.rs:337-487`](../../crates/buzz-core/src/kind.rs)).

The rows below cover every capability family exposed by the imported route tree
(`tic-tac-toe@517812b:desktop/src/app/navigation/useAppNavigation.ts:49-239`)
and by Buzz's registered event vocabulary
(`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:305-487`). This is a
protocol/product matrix, not an inventory of every visual preference or button.

| Product behavior | Buzz implementation evidence | Closest x0x capability | Verdict |
|---|---|---|---|
| Identity, contacts, profile, presence | Profile read/update/search and presence are commands (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/profile.rs:19-98,264-360`) | Agent cards, contacts/trust, machine pinning, presence routes (`x0x@e301371:src/server/mod.rs:1152-1195`) | **Mostly ready.** Rich profile/directory fields and people-search ranking are app contracts. |
| Channels and private groups | Channel create/update/topic/archive/member/role/join/leave (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/channels.rs:547-605,700-772,795-864`) | Named groups, public signed messaging, real TreeKEM private groups (`x0x@e301371:docs/api-reference.md:413-553,619-632`) | **Ready/primitive.** Core membership and crypto are real; channel hierarchy and softer roles stay app-defined. |
| Direct and group DMs | Buzz models open, add-member, hide, created (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:369-377`) | Direct encrypted send plus MLS/named secure groups (`x0x@e301371:src/server/mod.rs:1279-1294`) | **Primitive.** Live crypto works; offline custody, conversation lifecycle, unread/read state, and sidebar visibility are missing contracts. |
| Edit/delete/reactions/pins/typing | Buzz has durable kinds plus commands for reactions/deletes/edits (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/messages.rs:820-920`) and event kinds for typing/pinning (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:330-355`) | Embedded GUI sends app-defined events over generic topics and keeps some state in browser storage (`x0x@e301371:src/gui/x0x-gui.html:993-1020,1236-1270,5199-5251`) | **Prototype only.** No authoritative daemon merge/retention/permission model; browser-local state is not cross-device parity. |
| Read state, unread badges, mutes, stars, sections, sort | Buzz multiplexes five replaceable states on kind `30078` by `d` tag and maintains a deliberately narrow unread-trigger set (`tic-tac-toe@517812b:desktop/src/shared/constants/kinds.ts:42-48,73-163`); sidebar counts compare observed events with per-channel/per-message read markers (`tic-tac-toe@517812b:desktop/src/features/channels/useUnreadChannels.ts:820-884`) | Replicated KV stores and local history (`x0x@e301371:docs/api-reference.md:752-828`; `x0x@e301371:src/server/routes/history.rs:20-137`) | **Primitive, availability-blocked.** KV can hold the replaceable preferences, but unread correctness requires the missed message/reply set to arrive. A device that never receives an event must undercount it. |
| Threads and thread summaries | The desktop asks `/query` for top-level rows plus summaries/auxiliary events (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/channel_window.rs:19-55`); full thread reads walk relay `thread_metadata`, not just the loaded window (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/messages.rs:233-289`) | Generic topics plus local history; embedded GUI prototype | **Blocking primitive.** A local projection is feasible only over a complete delivered set. No current daemon contract validates ancestry, repairs gaps, or guarantees whole-subtree availability. |
| Scheduled messages, reminders, bookmarks, personal organization | Buzz registers pinned/bookmarked/scheduled/reminder kinds (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:348-357`) and has channel section/mute/star/sort replaceable state (`tic-tac-toe@517812b:desktop/src/shared/constants/kinds.ts:42-48`) | Task lists, KV stores, local scheduler/exec building blocks | **Gap at product layer.** Data structures exist; scheduling authority, exactly-once firing, expiry, cross-device merge, and UI contracts do not. |
| Custom emoji and community/profile projections | Buzz's imported client defines emoji and community-profile APIs, with event kinds outside the canonical `kinds.ts` file (`tic-tac-toe@517812b:desktop/src/shared/api/customEmoji.ts:26`; `tic-tac-toe@517812b:desktop/src/shared/api/communityProfile.ts:19`) | Agent cards plus replicated KV | **Primitive.** Straightforward typed projections, but namespacing, authority, replacement, moderation, and media availability must be specified. |
| Canvas | Dedicated get/set channel document (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/canvas.rs:9-60`) | Replicated KV stores (`x0x@e301371:docs/api-reference.md:752-828`) | **Primitive.** Define key, ownership, conflicts, history, size, and authorization; do not call a raw KV value a collaborative document. |
| Moderation and audit | Reports, temporary bans/timeouts, moderator queue, restrictions, and audit (`tic-tac-toe@517812b:desktop/src/shared/api/moderation.ts:102-180,250-374`) | Group bans and admin/member policy (`x0x@e301371:docs/api-reference.md:413-470`) | **Gap beyond basic ban.** Reports, timeout expiry, moderator roles, case resolution, and an audit view are not in the reviewed daemon API. |
| Forum/Pulse/social | Forum post/vote/comment kinds and routed post views (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:411-418`; `tic-tac-toe@517812b:desktop/src/app/navigation/useAppNavigation.ts:203-227`) | Signed public group messages and discovery | **Primitive.** Payloads can be carried, but feed ranking, follows, votes, thread indexing, and moderation remain application protocols. |
| Workflows/approvals | Trigger/lifecycle/approval kinds and Tauri commands (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:420-446`; `tic-tac-toe@517812b:desktop/src-tauri/src/commands/workflows.rs:52-105,186-287`) | CRDT task lists plus disabled-by-default remote exec (`x0x@e301371:docs/api-reference.md:634-750,857-886`) | **Gap at product layer.** These are building blocks, not a workflow engine; exactly-once execution is explicitly not promised by task claims. |
| Kanban/task board (x0x extra) | Adjacent to Buzz Projects/Workflows rather than a required relay-parity feature | Replicated task-list create/add/claim/complete with durable local replicas (`x0x@e301371:docs/api-reference.md:634-750`) | **Ready primitive and genuine x0x advantage.** A board UI can map directly, but must present claims as advisory/eventually convergent, never as exclusive locks or exactly-once execution. |
| Agent jobs/runtime | Buzz defines request/accept/progress/result/cancel/error and turn metrics (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:379-409`) | Agent identity/cards, groups, KV, tasks, exec | **Primitive.** Transport identity is strong; lifecycle/process supervision, ACP permissions, agent memory, and owner UX remain outside x0x. |
| Personas, teams, managed-agent projections | Buzz publishes three replaceable projection kinds (`tic-tac-toe@517812b:desktop/src/shared/constants/kinds.ts:49-57`) and routes the Agents surface (`tic-tac-toe@517812b:desktop/src/app/navigation/useAppNavigation.ts:49-81`) | Agent cards, named groups, KV stores | **Primitive.** The identity/data substrate maps cleanly; secret stripping, owner-only observation/control, lifecycle, and orchestration are Stage 4 work, not transport parity. |
| Notifications/offline unread | Relay-side mentions/needs-action feed (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/messages.rs:44-105`) | Local live WS/SSE and local history | **Blocking partial.** Local notifications work while a daemon receives events; no custodian means no event and therefore no eventual unread/mention. |
| Media/gallery | Blossom-backed upload/proxy and validation (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/media.rs:517-550,692-750`) | Point-to-point file transfer | **Primitive.** No durable provider/custody contract. |
| Local archive/save subscriptions | Buzz persists selected channel/owner/reference scopes and event batches (`tic-tac-toe@517812b:desktop/src/shared/api/tauriArchive.ts:22-36,147-220`) | Default-on scoped local history/query/search/purge (`x0x@e301371:docs/adr/0023-durable-local-history.md:68-117`) | **Mostly ready.** x0x's core history is stronger than an app-only archive for restart survival; user-selectable retention and observer/metric scope policy still need mapping. Neither is a remote backup. |
| Voice/huddles | Huddle lifecycle/reaction kinds (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:325-336,450-462`) | saorsa-webrtc adapters behind `voice` feature (`x0x@e301371:src/voice/mod.rs:1-30`) | **Primitive; v1 cut is honest.** |
| Projects/Git | Repository/PR/issue/status kinds and project routes (`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:468-487`; `tic-tac-toe@517812b:desktop/src/app/navigation/useAppNavigation.ts:82-121`) | None in the reviewed daemon router | **Gap; v1 cut is honest.** |

The v1 design already names cross-node backfill, galleries, orchestration, and
Git as non-goals
([`docs/design/tic-tac-toe-v1.md:21-43`](../design/tic-tac-toe-v1.md)).
That is defensible if the demo/product language is equally scoped. The current
acceptance test “history survives” proves daemon restart, not offline delivery
([`docs/design/tic-tac-toe-v1.md:104-120`](../design/tic-tac-toe-v1.md)).

### UX-floor ruling: Watson/Dario are right on outcome, too narrow on mechanism

The Buzz UI does not require an API literally named “cross-node backfill.” It
requires **eventual delivery of the complete authorized event set needed by each
projection**. Unread counts iterate over observed external trigger events
(`tic-tac-toe@517812b:desktop/src/features/channels/useUnreadChannels.ts:820-884`); thread badges
iterate over the replies grouped under a root
(`tic-tac-toe@517812b:desktop/src/features/channels/lib/threadBadgeCounts.ts:4-55`); full thread
reads explicitly repair replies outside the loaded timeline window using the
relay's `thread_metadata` query
(`tic-tac-toe@517812b:desktop/src-tauri/src/commands/messages.rs:233-289`). An event that never
reaches the daemon is absent from all three computations.

Therefore the substantive challenge is confirmed: **a local-only v1 cannot meet
Buzz's unread/thread/catch-up UX floor**. It will visibly undercount or omit
messages sent while all of that recipient's devices were offline. But querying
another node's private ADR-0023 history record is not the only repair and is the
wrong default privacy boundary. Participant-selected custodians can retain
opaque addressed ciphertext and deliver it later; the recipient then validates,
decrypts, and projects it locally. The required new ADR should consequently be
framed as **asynchronous delivery/custody**, with reconciliation as one step,
not as general peer-to-peer history serving.

### Post-publication crux with Lane B: resolve the overloaded term

After Lane B challenged this conclusion, the disagreement reduces to two
different meanings of “cross-node backfill”:

1. If it means **any later cross-node transfer of an authorized event that the
   returning node missed**, Lane B is right: the UX floor requires it. Custody
   fetch is one form of cross-node catch-up, and a local-only design cannot
   substitute for it.
2. If it means **querying another node's private ADR-0023 local history
   record**, that stronger mechanism is not required for ordinary offline
   delivery and would change the load-bearing local-only privacy claim. The
   recommended custodian transfers only opaque, recipient-addressed drops;
   validation, decryption, and projection happen on the returning recipient
   node. The custodian is not serving its own witnessed/decrypted history.

Ciphertext custody meets Buzz's unread/thread/catch-up floor only for a
precisely bounded case: the user was authorized when the event was sent; every
durable projection input (messages, replies, edits, deletions, reactions,
membership changes, and other non-ephemeral state) was deposited; at least one
selected custodian persisted it; and the recipient returned before expiry.
Under those conditions the local projection can converge without a remote
history-store query API.

The shipped kind registry makes the potential surface concrete
(`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:620-637`):

| Shipped classification | Constants |
|---|---:|
| Ephemeral (`20000..=29999`) | 10 |
| Replaceable (`0`, `3`, `41`, `10000..=19999`) | 11 |
| Parameterized-replaceable (`30000..=39999`) | 22 |
| Regular / other | 87 |
| **Actual kind constants** | **130** |
| **Outside the ephemeral range** | **120** |

This count comes from a structural match over scalar `pub const ...: u32`
definitions, excluding the four range bounds, followed by the shipped
predicates; all 130 values are distinct. The correct vocabulary ladder is
therefore **130 / 59 / 20**, not 134 / 59 / 20. The 120 figure is an upper
bound on non-ephemeral protocol vocabulary, **not yet the exact custody deposit
set**. Concrete counterexamples at the imported Buzz anchor are moderation
commands 9040–9044, which are non-ephemeral but never stored or fanned out as
ordinary events; relay-admin commands 9030–9033, which mutate membership without
being stored; NIP-56 report kind 1984, which persists only in the private
moderation queue; and product-feedback kind 42000, which is sidecarred into a
private deployment table
(`buzz@710ed9ff:crates/buzz-relay/src/handlers/ingest.rs:1538-1587,1807-1816`).
The ADR therefore needs a generated per-kind action table: custody the event,
custody the authorized resulting state/audit artifact, retain it in a private
sidecar only, or exclude it.

Consequently, `120 - 10 = 110` is also **not** an exact deposit count. That
subtraction removes moderation, relay-admin, and report inputs but still
includes product feedback plus query-time/never-emitted compatibility kinds
classified below. A scalar count becomes defensible only after the per-kind
action table is generated from all ingest, producer, and read dispatchers.

Lane B's later `≤103 = 120 - 6 relay-only - 11 never-stored` figure is not a
defensible replacement. It mixes the current 130-kind Nostr vocabulary with a
native end state in which some of those kinds disappear, and it subtracts the
two relay-only values (`13534`, `30622`) that **are** persisted at the Buzz
anchor merely because their native producers will differ. If the calculation
is scoped only to the current anchor and only to the exclusions established so
far, the corresponding coarse stored-event upper bound is `120 - 11 - 4 =
105`: the four are the relay-only values that are query-time, fan-out-only, or
never emitted. Even 105 is not an exact custody set, because custody policy is
not equivalent to relay storage policy. Do not put a scalar deposit count in an
ADR until the generated action table exists.

The 33 replaceable classes add a separate merge invariant. A late custody drop
must not overwrite a newer local value: ordinary replaceable state is keyed by
`(pubkey, kind)` and parameterized state by `(pubkey, kind, d-tag)`, with
`created_at` ordering and a deterministic equal-timestamp event-ID tie-break.
The current bridge already rejects stale replaceable events at merge time
(`x0x-nostr-bridge@19ec83b:src/history/engine.rs:204-276`); the native custody
path must preserve that behavior rather than applying arrival order. Kind
`30078` makes this user-visible because its d-tags multiplex read state,
sections, mutes, stars, and sort
(`tic-tac-toe@517812b:desktop/src/shared/constants/kinds.ts:42-48`).

#### Production is separate from delivery: retire the six relay-only kinds

The six values matched by `is_relay_only_kind` are not six equivalent durable
events and should not be carried wholesale into the native protocol
(`tic-tac-toe@517812b:crates/buzz-core/src/kind.rs:680-691`):

| Buzz kind | Actual Buzz behavior at `710ed9ff` | Required native replacement | Custody consequence |
|---|---|---|---|
| `13534` membership list | Persisted replaceable snapshot signed after the relay serializes the membership read/build/write cycle (`buzz@710ed9ff:crates/buzz-relay/src/handlers/side_effects.rs:2816-2874`) | Authorized group-state actors sign ML-DSA state commits whose roster root and retained roster projection are independently verifiable (`x0x@e301371:src/groups/state_commit.rs:1-35,68-175`); native clients read `/groups/:id/members` and `/groups/:id/state*` (`x0x@e301371:src/server/mod.rs:1223-1269`) | Custody signed state commits needed by the recipient, not a relay-authored Nostr snapshot |
| `40901` channel summary | An anchor-wide search under `crates/buzz-relay/src` found no producer; the desktop independently says the relay does not emit it and derives member count plus last-message time itself (`tic-tac-toe@517812b:desktop/src-tauri/src/commands/channels.rs:204-278`) | Local materialized view over authenticated membership and delivered message history | Custody inputs, never the obsolete summary event |
| `40902` presence snapshot | Query compatibility selector only: the relay reads its presence cache on demand and returns synthetic kind `20001`, not a stored `40902` event (`buzz@710ed9ff:crates/buzz-relay/src/api/bridge.rs:1915-1985`) | x0x peers sign ML-DSA presence beacons; the local wrapper emits online/offline events from its current cache (`x0x@e301371:src/presence.rs:377-399,447-503,528-574`) | Presence remains ephemeral and is never custodied |
| `30622` DM visibility | Persisted, private, per-viewer replaceable snapshot of the relay's hidden-DM table (`buzz@710ed9ff:crates/buzz-relay/src/handlers/side_effects.rs:3060-3138`) | User-authored private preference state, signed/encrypted and replicated only to that user's authorized devices; no shared group authority | Sync/custody the user-owned state under user/device capability, not a relay projection |
| `39005` thread summary | Synthesized from `thread_metadata` in query responses; live form is fan-out-only and never stored (`buzz@710ed9ff:crates/buzz-relay/src/api/bridge.rs:404-557`; `buzz@710ed9ff:crates/buzz-relay/src/handlers/side_effects.rs:710-719`) | Deterministic local materialized view over a complete, validated thread event set, as the fork plan already schedules (`tic-tac-toe@517812b:docs/design/buzz-fork-plan.md:124-135`) | Custody messages/replies/edits/deletions; recompute the summary after merge |
| `39006` window bounds | One query-time overlay carrying `has_more` and the keyset cursor; never an application record (`buzz@710ed9ff:crates/buzz-relay/src/api/bridge.rs:559-580`) | Ordinary local history-query response metadata | No custody and no protocol author |

Stage 2 does not yet delete the bridge signer: it replaces the key source with
a per-install compatibility key whose authority ends at the loopback dialect
while x0xd owns the real ML-DSA identity
(`tic-tac-toe@517812b:docs/design/buzz-fork-plan.md:113-122`). That key may keep
wrapping compatibility projections for the unmodified desktop until each
feature crosses the Stage-3 seam. It must not become an x0x authority.

Therefore the production gap is real but bounded differently: **only `13534`
and `30622` are persisted projections in this six-kind set that need an
explicit replacement authority; the first maps to authorized group-state
commits and the second to user-owned private state.** The other four are
absent, ephemeral/on-demand, or query metadata and should disappear as event
kinds. Custody cannot manufacture any projection, but it also must not preserve
compatibility artifacts whose native replacement is local computation.

Lane B is nevertheless right that a latest-by-`created_at` merge rule does not
solve concurrent membership mutations. x0x has a legitimate native author and
a stronger primitive than that merge rule: ML-DSA authority-signed commits
carry a monotonic revision, previous-state hash, and roster root, and receivers
validate the signature, current authority, revision, and parent hash
(`x0x@e301371:src/groups/state_commit.rs:1-35,350-451,690-720`). But x0x's own
accepted ADR is explicit that this chain serializes **per replica**, not across
concurrent sibling commits: two admins can sign different children of the same
parent and different replicas can accept different winners; deterministic
fork choice remains future work
(`x0x@e301371:docs/adr/0016-role-based-group-authority-flat-admin.md:109-124,210-215`).

The accurate ruling is therefore neither “no principal may author membership
after Stage 2” nor “membership convergence is already solved.” Stage 2 retains
a loopback compatibility signer, and Stage 3 has x0x group-state authorities,
but native retirement of kind `13534` must be gated on sibling-commit
convergence. The authorization/fork-strategy decision must specify deterministic
committer and rebase/retry behavior plus a real fork-resolution rule (or a
strictly enforced single-committer policy), then prove a two-admin,
same-parent mutation race converges on every replica. Custody carries the
winning signed chain; it does not choose the winner.

The concrete removal path confirms that the race is reachable, not merely
theoretical. `DELETE /groups/:id/members/:agent_id` permits any admin on the
local node, takes a per-`AppState` membership mutex, clones that node's current
group head, mutates it, signs the next commit with that admin's key, persists
it, and then publishes it
(`x0x@e301371:src/server/routes/named_groups.rs:8425-8451,9177-9226,9263-9331`;
the daemon-local mutex is defined at
`x0x@e301371:src/server/state.rs:700-719`). A second admin's daemon has a
different `AppState` and mutex, so both can author different removal children
of the same parent and both HTTP calls can return success before either event
reaches the other node. Ban has the same shape
(`x0x@e301371:src/server/routes/named_groups.rs:10267-10293,10416-10555`).

Incoming membership events are serialized only inside each receiving daemon
(`x0x@e301371:src/server/routes/named_groups.rs:4575-4591`), and
state-commit validation then rejects whichever sibling arrives second. For the
equal-revision race, the reached predicate is `StaleRevision`; the later
`prev_state_hash` check is not reached
(`x0x@e301371:src/groups/state_commit.rs:693-712`). That is per-replica
serialization, but it is not yet a network fork choice: different replicas can
accept opposite first arrivals. It also means “the sibling is rejected,
therefore no update is lost” is too strong at the product boundary: both
initiating HTTP requests may already have returned success. Until a loser is
deterministically selected and its operation is rebased/retried or surfaced as
rejected, the user can observe a successful administrative action that does not
survive convergence.

The fork also reaches the TreeKEM state, but equality of the numeric epoch is
not a sufficient acceptance assertion. Each sibling removal starts from epoch
`N`, advertises `N+1` in `security_binding`, then applies and persists a
different TreeKEM removal commit
(`x0x@e301371:src/server/routes/named_groups.rs:9263-9302`). Thus two
cryptographically different trees can both report epoch `N+1`. The signed
`GroupStateCommit` binds the string `treekem:epoch=N+1`, not the TreeKEM commit,
tree hash, or confirmed transcript
(`x0x@e301371:src/groups/state_commit.rs:350-451`;
`x0x@e301371:src/server/routes/named_groups.rs:9264-9270`), and the current
`TreeKemMlsGroup` wrapper exposes only epoch, group ID, and member count—not a
public tree/transcript commitment
(`x0x@e301371:src/mls/treekem.rs:427-443`).

The exact frontier already exists in the pinned `saorsa-mls` dependency:
`TreeKemCommit.tree_hash_after` commits to the resulting public tree including
parent hashes, is covered by the committer's signature, and is recomputed and
rejected on mismatch by receivers
(`saorsa-mls@0.3.8:src/treekem_group.rs:140-161,466-511,605-613,905-935`;
the pin is `x0x@e301371:Cargo.toml:20-24`). But the value to cross-bind should
be a digest of the **exact accepted TreeKEM commit bytes**, not
`tree_hash_after` alone. The complete commit also contains the removed leaves,
committer leaf, full update path, epoch, tree hash, and signature. A tree hash
selects the resulting public tree; a commit-byte digest selects the one exact
signed artifact whose encrypted update path every survivor must process.

No dependency accessor or wire-field change is required:
`remove_member_verified` already returns the postcard-serialized
`TreeKemCommit`, and x0x already carries those exact bytes as
`treekem_commit_b64` beside the signed group-state commit
(`x0x@e301371:src/mls/treekem.rs:12-15,92-97,365-386`;
`x0x@e301371:src/server/routes/named_groups.rs:9279-9289,9321-9328`). Hash the
received bytes directly rather than decode/re-encode them. x0x pins postcard
1.1.3 (`x0x@e301371:Cargo.lock:2867-2870`), and postcard documents a stable
wire format from 1.0
([postcard format stability](https://docs.rs/postcard/latest/postcard/#format-stability)),
but direct-byte hashing also avoids making canonical reserialization a security
precondition.

The existing slot already provides authentication. `security_binding` is
length-prefixed into both the state hash and the ML-DSA signable bytes, and
receivers recompute the projected state hash before retaining a commit
(`x0x@e301371:src/groups/state_commit.rs:219-245,350-451`;
`x0x@e301371:src/groups/mod.rs:640-682`). The defect is therefore weak content,
not a missing signature or field. Use an exact, versioned, discriminated
encoding, for example:

```text
treekem:commit-postcard-v1:epoch=<u64>:blake3=<64-lower-hex>
gss:key-confirm-v1:epoch=<u64>:blake3=<64-lower-hex>
```

The TreeKEM digest should use a hard-coded BLAKE3 derive-key context such as
`x0x security binding treekem commit postcard v1` over a length-prefixed stable
group ID and the exact commit bytes. The plane, artifact encoding, version,
algorithm, epoch, and fixed-width digest are all explicit. Unknown schemes must
fail closed after the compatibility transition; a bare digest would make the
TreeKEM and GSS meanings confusable in the one shared field. BLAKE3's official
API distinguishes keyed hashing and context-separated key derivation and
requires application-specific hard-coded derive-key contexts
([BLAKE3 official implementation](https://github.com/BLAKE3-team/BLAKE3#the-blake3-crate)).

That cross-binding requires an ordering change. The serialized TreeKEM commit
does not exist until `remove_member_verified` has mutated the local TreeKEM
group, but the current path seals `GroupStateCommit` first. The required
transaction shape is: snapshot the old TreeKEM state; generate/apply its commit;
digest the returned bytes; seal the roster commit over epoch plus that binding;
persist both; and restore the snapshot if sealing or persistence fails. The
receiver must verify that the delivered TreeKEM commit matches the group-state
binding before atomically installing either side.

Local failure rollback is only the first half. The successful path writes one
post-commit snapshot to `{group_id}.snap`; its replay journal contains the same
post-commit envelope and is deleted when persistence completes
(`x0x@e301371:src/server/routes/named_groups.rs:12442-12455,12601-12638`).
It retains no parent-epoch pre-image. A node that later loses fork choice has
therefore destroyed the input needed to restore the parent and apply the winning
sibling. The authorization ADR must retain a pre-commit checkpoint until its
fork-resolution policy confirms the selected branch; local HTTP success is not
that confirmation.

The receive path also rejects at the wrong abstraction boundary for resolving
this race. `MemberRemoved` calls `apply_stateful_event_to_group`, which runs
`validate_apply`, and converts any error to a bare `false` before it decodes or
processes `treekem_commit_b64`
(`x0x@e301371:src/server/routes/named_groups.rs:1967-1987,4930-4982,5023-5049`).
Thus the exact sibling reaches neither the TreeKEM epoch check nor a diagnostic,
despite ADR-0016 saying equal-revision siblings are surfaced rather than
silently dropped
(`x0x@e301371:docs/adr/0016-role-based-group-authority-flat-admin.md:109-124`).
Fork detection and choice must intervene at or above this roster-apply gate,
where it can classify the sibling, compare the cross-bound TreeKEM artifact,
select a branch, restore the retained pre-image, and visibly rebase/retry or
reject the losing operation.

The GSS frontier needs a different primitive. Publishing the rotated secret
itself is obviously disallowed, while publishing only `gss:epoch=N` repeats the
TreeKEM defect: two divergent 32-byte secrets can share the same counter. Use a
public **key-confirmation tag** instead:

1. derive a distinct confirmation key from the fresh 32-byte GSS secret with
   BLAKE3 derive-key context `x0x gss confirmation key v1`;
2. MAC a canonical, length-prefixed context containing the stable group ID,
   new secret epoch, new revision, previous state hash, and new roster root
   with BLAKE3 keyed mode; and
3. place the resulting 32-byte tag in the versioned GSS binding above.

This follows the same construction pattern as MLS—not its wire format—where a
confirmation key derived from the new epoch secret MACs the confirmed
transcript and receivers verify that tag before accepting the new group state
([RFC 9420 §8.2 and §12.4.2](https://datatracker.ietf.org/doc/html/rfc9420#section-12.4.2)).
There is no hash circularity: the confirmation MAC covers the **previous**
state hash, while the current state hash then covers the resulting
`security_binding`. All five MAC context values are already shipped state
artifacts; `compute_state_hash` binds revision, previous state hash, roster
root, and the binding in that order
(`x0x@e301371:src/groups/state_commit.rs:219-239`). Revision must be explicit:
without it, two siblings that deliberately share a rotated secret and roster
but differ in revision would derive the same confirmation tag.

The tag does not disclose a uniformly random 256-bit GSS secret under the
existing secret-generation assumption
(`x0x@e301371:src/groups/mod.rs:418-436`), but it is an offline verifier for a
guessed key. The protocol must therefore never admit password-derived or other
low-entropy GSS secrets.

GSS verification must be two-phase because the signed roster commit and
recipient-sealed key are separate, reorderable events. Today ban persists the
new epoch, sends `SecureShareDelivered` envelopes, and only then publishes the
`MemberBanned` commit
(`x0x@e301371:src/server/routes/named_groups.rs:10305-10368,10397-10408`).
A receiver currently accepts a higher-epoch share from an active admin, opens
it, installs the secret, and overwrites `security_binding` with the epoch-only
string even if the state commit has not arrived
(`x0x@e301371:src/server/routes/named_groups.rs:5803-5835,5854-5890`). That
cannot verify the join between planes. The current authorization check narrows
the threat: it requires `actor == sender_hex` and the actor to be an active
Admin, so this is not an arbitrary-peer injection path; it is a malicious or
forking authorized-admin case, exactly the sibling frontier under review
(`x0x@e301371:src/server/routes/named_groups.rs:5820-5826`).

The install is also demonstrably unsigned rather than merely early:
`SecureShareDelivered` writes the new secret, epoch, and epoch-only
`security_binding`, then calls `store_named_group_info`
(`x0x@e301371:src/server/routes/named_groups.rs:5883-5887`).
`store_named_group_info_locked` performs only the withdrawn-record guard and
replaces the map slot; it does not recompute the state hash or verify a
signature
(`x0x@e301371:src/server/routes/named_groups.rs:2040-2060`). After share-first
adoption, the projection's binding says epoch `N+1` while its retained
`state_hash` still covers the previous binding.

Invite bootstrap is a second unsigned writer of the same join field. The
`SignedInvite` link is base64url-encoded JSON; its `signature` field is
explicitly future-facing and unvalidated, even though the prospective
`signable_bytes` include `base_security_binding`
(`x0x@e301371:src/groups/invite.rs:1-8,32-38,79-97,176-220,308-317`).
The mint path itself calls the link unsigned
(`x0x@e301371:src/server/routes/identity.rs:357-390`). On join,
`invite_join_group_info` copies the claimed secret epoch and binding into
`GroupInfo`, then separately copies the claimed revision, roster, state hash,
and previous hash from the same artifact; it neither recomputes that projection
nor verifies a signature
(`x0x@e301371:src/server/routes/named_groups.rs:7653-7677`).
The one-time invite secret authenticates bearer admission **to the inviter**;
it does not authenticate the inviter's base state **to the joiner**.

The fail-closed migration must therefore cover invite bootstrap as well as
`SecureShareDelivered`. Before installing an invite's base binding, recompute
the complete advertised projection and require it to equal
`base_state_hash`, then authenticate that anchor. The clean choices are an
ML-DSA-signed, unambiguously encoded invite whose signer key derives the stated
inviter and whose base roster authorizes that inviter, or an embedded
authority-signed state-commit anchor with the same projection. A self-consistent
hash alone is insufficient because every input arrived in the same unsigned
artifact. Unknown or legacy binding schemes must not seed the strengthened
plane after the compatibility transition.

The unsigned invite has a separate stable-ID/tombstone consequence. The join
guard tests both the invite's MLS ID and claimed stable ID for withdrawn
records, but its already-joined predicate tests only the MLS ID
(`x0x@e301371:src/server/routes/named_groups.rs:7728-7770`). A crafted first-use
link can therefore create a stub under `K'` whose attacker-chosen stable ID is
an existing group's `K`; the link also supplies the stub roster
(`x0x@e301371:src/server/routes/named_groups.rs:7671-7673`). Accepting the link
subscribes the victim to the metadata topic derived from `K'`
(`x0x@e301371:src/groups/mod.rs:321-324`;
`x0x@e301371:src/server/routes/named_groups.rs:7852`).

A signed terminal `GroupDeleted` on that topic is then checked correctly—but
against the stub's attacker-authored roster. The terminal apply context takes
`members_v2` from that selected record and authorizes an Admin-or-higher signer
from it
(`x0x@e301371:src/server/routes/named_groups.rs:1990-2010,5063-5115`;
`x0x@e301371:src/groups/state_commit.rs:719-742`). On acceptance,
`retain_withdrawn_group_tombstone` derives stable ID `K` from the stub, clears
its key, and writes the resulting tombstone over every collected alias with a
bare `HashMap::insert`
(`x0x@e301371:src/server/routes/named_groups.rs:9008-9025`). The real record at
`K` can therefore be replaced by the withdrawn, keyless stub. This is not a
remote-unauthenticated path: the victim must first accept a crafted invite
link. No exploit was executed in this research; the chain was established from
source at `e301371`.

The same forged stub has a group-card-cache consequence. Metadata-event
resolution chooses the direct map key before scanning by stable ID
(`x0x@e301371:src/server/routes/named_groups.rs:4554-4588`), and `caller_role`
reads `members_v2` (`x0x@e301371:src/groups/mod.rs:1039-1046`). The invite
helper copies `base_members_v2` and the claimed stable ID from the unsigned link
and stores the result at the invite's direct group key
(`x0x@e301371:src/server/routes/named_groups.rs:7642-7675,7832-7849`).
`creator_agent_id_from_base_state` selects a self-consistent seeded member from
that same unsigned roster and validates only the resulting AgentId shape
(`x0x@e301371:src/groups/invite.rs:257-305`;
`src/server/routes/named_groups.rs:7773-7790`). Consequently the
attacker-authored record can make its sender Admin+ at `:5732-5737` and make
the card's victim ID equal `info.stable_group_id()` at `:5738-5740`. The event
has independent `group_id` and `card` fields (`named_groups.rs:755-758`), so it
can carry wrapper ID `K'` for direct-key resolution and card stable ID `K` for
the victim-keyed sink. An empty signature then skips verification at `:5741`,
and the card is inserted, replaced, or removed under the victim stable ID at
`:5744-5749`.

Revision and issue time are attacker-controlled ordering inputs
(`x0x@e301371:src/server/routes/named_groups.rs:228-240`;
`x0x@e301371:src/groups/directory.rs:203-213`); the latter helper explicitly
says its caller must have verified both signatures at `:203-205`. The cache
wins over local synthesis in `get_group_card` and competes with the real
synthesized card in discovery
(`x0x@e301371:src/server/routes/named_groups.rs:11319-11363,11520-11539`).

The `verified == true` gate at `named_groups.rs:4525-4533` does not block this
metadata-topic path. Its listener passes `PubSubMessage::verified`
(`:6371-6414`), which means that the publishing AgentId's ML-DSA signature
verified over the topic and exact payload. Decode binds the supplied public key
to that AgentId (`x0x@e301371:src/gossip/pubsub.rs:1093-1138,1173-1206`);
failed signed messages are dropped before delivery at `:784-791`, and
senderless legacy messages are skipped at `named_groups.rs:6411`. Every
metadata event reaching the apply call at `:6413` therefore has
`verified == true`; an attacker signing with its own previously unknown key
reaches the handler with an authenticated identity. The x0x wrapper derives
topic IDs and seeds all gossip-plane peers without consulting a group roster
(`gossip/pubsub.rs:398-439,544-612,687-761`). The crafted invite makes the
victim subscribe to the `K'`-derived topic (`groups/mod.rs:321-324`;
`named_groups.rs:7632-7651,7852`), so the open link is group authority, not
sender authentication.

Two remote producer paths relevant to this chain feed the direct listener with
the same boolean (`x0x@e301371:src/server/mod.rs:1056-1083`). The
gossip-inbox path requires a verified pubsub origin, verifies the inner DM
envelope under that origin's key, rejects an inner sender that differs from the
outer origin, then injects `DirectMessage::verified = true`
(`x0x@e301371:src/dm_inbox.rs:345-350,407-428,689-700`;
`src/direct.rs:1128-1155`). That is origin-key authentication, satisfiable by
a normal x0x sender using its own valid identity keys; it does not require a
prior AgentId→MachineId cache entry. The stock named-group sender chooses
gossip inbox when the recipient capability is available because
`DmSendConfig::default()` sets `prefer_raw_quic_if_connected = false`, and
neither the generic direct-route wrapper nor the named-group wrapper changes
that field (`x0x@e301371:src/dm.rs:665-681`;
`src/server/routes/direct.rs:20-37`;
`src/server/routes/named_groups.rs:315-329`). That leaves the raw-first branch
at `src/lib.rs:4362-4402` inert and selects the gossip arm at `:4403-4437` when
`gossip_ok` is true (`:4277-4306`). `require_gossip_ack` controls delivery
acknowledgement, not path selection. This is outbound ordering, not a
receiver-side security control: the inbox subscription and raw-QUIC receive
loops operate independently (`x0x@e301371:src/dm_inbox.rs:304-316`;
`src/lib.rs:8468-8634`), while the public send surface accepts a caller-provided
`DmSendConfig` (`src/lib.rs:4169-4195`). A remote peer controls its own sender,
so the receiver cannot use the stock ordering to narrow the accepted attack
surface.

Only on the raw-QUIC receive path is `DirectMessage.sender` self-asserted and its
`verified` value the identity-cache binding from that AgentId to the
QUIC-authenticated MachineId
(`x0x@e301371:src/direct.rs:190-222`;
`src/lib.rs:8489-8528,8624-8633`). Dropping that check would enable sender
impersonation on the fallback. Keeping it authenticates the sender but does
not repair the next link: `sender_is_admin` still reads the forged invite
roster. The direct listener is therefore not a safer authority boundary; its
gossip-inbox path reaches the same forged-roster check with origin-key
authentication alone.

The raw-QUIC cache binding is not a prior-trust or roster barrier for an unknown
sender. `IdentityAnnouncement::verify` derives `machine_id` from the announced
machine key and verifies the ML-DSA signature under that key
(`x0x@e301371:src/lib.rs:936-974`), but the no-user/no-certificate arm returns
success without binding `agent_id` to an agent public key (`:976-997`). The
identity-announcement listener then applies the freshness, trust, revocation,
and optional-certificate gates at `:6458-6542`. For a previously unknown,
unblocked and unrevoked sender with no conflicting machine pin, a fresh
certificate-free announcement passes those gates.

The listener builds the general `DiscoveredAgent`, calls
`record_authenticated_machine_binding_from_message`, discards its returned
boolean, and calls `upsert_discovered_agent` unconditionally
(`x0x@e301371:src/lib.rs:6600-6626`). The latter inserts an unknown `agent_id`
outright (`:1641-1692`). The direct-origin check at `:1009-1097` protects only
the separate `authenticated_machine_bindings` store passed to the gossip-inbox
bridge at `:7333-7347`; it does not guard the general
`identity_discovery_cache` cloned by the raw-QUIC listener at `:8462` and read
for `verified` at `:8521-8528`. Therefore the raw path's extra precondition
costs the attacker one fresh, machine-signed identity-announcement publish for
its own AgentId and MachineId. A subsequent raw frame from that MachineId with
that self-asserted AgentId receives `verified == true`. This still authenticates
only the announced sender identity; `sender_is_admin` obtains the victim-group
authority from the forged invite roster.

The demonstrated static consequence is therefore cache metadata spoofing,
replacement, or eviction after the crafted-link social step. It does not
establish a redirect primitive. Although the card carries a signed-domain
`metadata_topic` bootstrap hint
(`x0x@e301371:src/groups/directory.rs:48-52,134-140`), this research did not
establish a consumer that acts on the poisoned cached value. No exploit was
run.

The alias mechanics are over-determined but should not be repaired in the
shared collector. `K` enters the output alias set at
`named_groups.rs:8663-8664`, `:8666-8671`, and `:9022`; the first and third
share the same caller-derived stable-ID source, while the map scan is the
second independent source. The collector has nine production consumers, so
pruning it would change unrelated TreeKEM, journal, cache, and migration
behavior. The class-closing boundary is the destructive write at `:9023-9025`:
it must refuse to overwrite an existing record unless an authority-backed
same-group discriminator proves the tombstone and target are the same group.
Equality of `mls_group_id` is a plausible one-site candidate, but it remains
only partly verified. `group_deleted` clones the resolved record at `:5085`;
`withdrawn_card_import` clones the keyed record at `:11589`, and its sole
pre-tombstone mutation (`:268-309`) never writes `mls_group_id` before
`:11605`. The local-withdraw caller also passes `terminal_info` cloned from the
record addressed by `id` (`:9571-9603,9619`), but its legitimate alias shapes
have not been enumerated. That residual must prove local withdrawal never
intentionally overwrites a different-MLS-ID alias before the predicate becomes
a final invariant. Independently, the join guard must test and serialize both
invite IDs, and the invite frontier must be authenticated; each closes a
different scope.

`GroupCard` supplies a canonical signature mechanism, not a complete
authorization model. Its signable bytes and ML-DSA verification are real
(`x0x@e301371:src/groups/directory.rs:31-195`), and direct import rejects a
failed signature before the local lookup
(`x0x@e301371:src/server/routes/named_groups.rs:11560-11593`). But
`verify_signature` explicitly says authorization against the local roster is
an apply-time responsibility (`directory.rs:155-165`). The
`GroupCardPublished` arm does that apply-time check against the forgeable stub,
so its empty-signature exception is part of the defect rather than a
counter-example.

Direct card import exposes the other half of the distinction. A valid
self-certifying signature proves the identity in `authority_agent_id`, but
does not prove that identity may claim an arbitrary `group_id`. For a
non-withdrawn card whose ID directly keys an existing local record,
`import_group_card` updates policy, metadata topic, revision/hash frontier,
withdrawal state, can replace genesis, then inserts the card's asserted owner
as an Admin (`x0x@e301371:src/server/routes/named_groups.rs:11630-11654,11680-11763`).
No existing-roster authorization check occurs on that branch. This is a
separate, user-or-bearer-authenticated-local-caller-triggered authority-binding
defect found by static read; no crafted-card import was executed. The required
rule on both paths is signature **and** authority-backed same-group
authorization before any cached or local frontier is replaced.

The receiver instead needs a pending-state join keyed by
`(group_id, epoch, key-confirmation-tag)`: if the state commit arrives first,
retain its signed expected binding with `shared_secret=None`; if the sealed
share arrives first, retain it as pending but do not install or use it. Only
after decrypting the share, deriving the tag, and matching the exact signed
binding may the daemon install the key. The share AAD should also bind the
versioned tag (or the state-commit hash) so the envelope cannot be relabelled
between sibling states. Delivery order must not change the result.

GSS remove is not an open policy choice. Accepted ADR-0010 requires secret
rotation on both ban and remove
(`x0x@e301371:docs/adr/0010-gss-before-mls-treekem-for-v1-secure-groups.md:35-44,49-68,97-103,140-148`).
The live non-TreeKEM removal path does not rotate the content key, emits no
secret epoch, commits the roster before a fail-open legacy-MLS mutation, and
publishes anyway
(`x0x@e301371:src/server/routes/named_groups.rs:8425-8454,8478-8525`). This is
a shipped spec violation for GSS groups, not successor behavior to preserve.
It is live because newly created `MlsEncrypted && !Hidden` groups deliberately
remain on GSS
(`x0x@e301371:src/server/routes/named_groups.rs:6516-6533`). Route both ban and
remove through the same rotate, key-confirm, seal-to-remaining-members, and
two-phase receive path before treating GSS as a safe migration source.

The immediate remove-rotation remedy and this authenticated frontier are
coupled, not contradictory. In the shipped epoch-only design,
`SecureShareDelivered` requires no change merely to make commit-first and
share-first **converge**: the current guard accepts a missing equal-epoch key or
a higher-epoch share
(`x0x@e301371:src/server/routes/named_groups.rs:5827-5835`). The defect remedy
still needs rotation plus resealing after `remove_member`, a
backward-compatible `secret_epoch` on `MemberRemoved`, and the existing
`MemberBanned` GSS receive branch, with receivers deployed before senders
rotate. `MemberRemoved` currently lacks that field and receive arm, while
`MemberBanned` carries both explicitly to update the binding before state-hash
finalization
(`x0x@e301371:src/server/routes/named_groups.rs:629-642,667-686,4973-4979,5247-5254`).
Lane A changes the same share-adoption behavior for **authentication** by
holding a share pending instead of installing it unsigned. The two changes
must therefore be designed and tested together rather than treating “no
`SecureShareDelivered` change” as a final protocol conclusion.

There is a second ordering hazard in the existing `MemberBanned` GSS branch.
Today `if old_epoch < secret_epoch { next.shared_secret = None; }` preserves a
share that arrived first while clearing an obsolete key when the signed commit
arrives first
(`x0x@e301371:src/server/routes/named_groups.rs:5247-5253`). Once the two-phase
gate ships, share-first no longer advances installed state, so that arrival
order no longer justifies the conditional. Sender self-delivery does not supply
a replacement justification: `seal_commit` advances the sender's revision and
state hash before the record is stored and the event is published, so a
looped-back event fails the `commit.revision <= current_revision` gate before
the GSS closure can run
(`x0x@e301371:src/groups/mod.rs:524-548`;
`x0x@e301371:src/server/routes/named_groups.rs:10324-10338,10397-10407`;
`x0x@e301371:src/groups/state_commit.rs:690-711`). The transport may or may not
loop back; it cannot make the conditional reachable on the origin. Retain the
conditional until the pending-join gate is live—removing it earlier still
discards a valid share under today's reorderable behavior. Any later cleanup
must be backed by both arrival-order tests rather than inferred from the design
alone.

The two-admin test must assert the final roster **and** TreeKEM interoperability:
all surviving replicas accept the same serialized operation order, can
cross-decrypt post-resolution application messages, and excluded members
cannot decrypt them. An equal epoch alone can pass while the group remains
partitioned. The existing rollback helper snapshots and restores only around a
failed single commit install
(`x0x@e301371:src/server/routes/named_groups.rs:2071-2103,2176-2228`); it is not
a sibling-fork resolution mechanism.

Custody alone does **not** provide history from before a user joined, recovery
after every delivery copy expired, a fresh-device archive after another device
already acknowledged/deleted the drop, or economical complete history for a
large public channel. Those require the separate participant-replicated public
log/archive policy and same-user recovery ADR described below. Therefore the
project must not present the bounded mailbox as unqualified Buzz parity.

The consensus ADR shape should be named broadly enough to avoid a word game:
**cross-node catch-up via participant-selected ciphertext custody**. It must
state explicitly that some cross-node recovery is required, follow ADR-0023's
mandated separate-review procedure, and leave its existing V1 local-history
store and load-bearing local-only privacy claim unchanged.

## 4. Why x0x cannot catch up a shut-down node today

### 4.1 Local durability is implemented and valuable

ADR-0023 makes local SQLite history default-on, classifies durable versus
ephemeral traffic, bounds retention, stores reader-side MLS plaintext, and
adds list/search/backfill/local-purge
(`x0x@e301371:docs/adr/0023-durable-local-history.md:68-117`).
The source confirms that `/history` and `/history/search` read the local store
and that purge is local only
(`x0x@e301371:src/server/routes/history.rs:1-6,75-137,165-193`).

The WebSocket `live` marker also means “the rows already in this daemon's
durable store have been replayed,” not “the network has supplied everything
this user missed” (`x0x@e301371:src/server/ws.rs:75-148`). The same source states that
there is no retaining inbox behind direct-message subscriptions
(`x0x@e301371:src/server/ws.rs:163-169`).

### 4.2 Current DM delivery is time-bounded, not store-and-forward

The gossip DM path constructs an encrypted envelope with a 120-second lifetime,
publishes it to the recipient inbox topic, retries for an ACK, and returns
`DmError::Timeout` after exhausting retries
(`x0x@e301371:src/dm_send.rs:34-35,60-127,285-326`).
The optional peer relay forwards an opaque envelope one hop, is disabled by
default, and rejects stale relay envelopes after a 30-second freshness budget
(`x0x@e301371:src/peer_relay.rs:1-66,79-96`).
It improves NAT reachability; it is not durable custody.

For public named groups, `/groups/:id/messages` returns what the daemon cached
after witnessing public messages. The retained state-commit log likewise
admits that each daemon holds only the suffix it witnessed and defers
cross-peer backfill
(`x0x@e301371:docs/api-reference.md:522-553,583-613`).
Private-group membership has redundant welcome delivery and a catch-up listener
for TreeKEM membership repair, but that is not application-message history
(`x0x@e301371:docs/api-reference.md:619-632`).

### 4.3 This is an explicit architecture boundary

ADR-0006 says user/group data belongs with participants or explicitly chosen
replicas, discovery is not custody, and data is unavailable when all holders
are unreachable
(`x0x@e301371:docs/adr/0006-no-global-dht-for-user-and-group-data.md:36-53,70-105`).
Its required follow-up demands explicit replica semantics
(`x0x@e301371:docs/adr/0006-no-global-dht-for-user-and-group-data.md:114-132`).
The correct fix should implement that decision,
not evade it with an unnamed quasi-relay.

The documentation surface is also incomplete: `x0x@e301371:SKILL.md` has no
`/history`, `/history/search`, or `/history/stats` entry; its only “history”
match is group state commits at `x0x@e301371:SKILL.md:517`. The route exists in
source at `x0x@e301371:src/server/mod.rs:1315-1319`. This is not the
availability defect, but it will cause clients and agents to reason about the
shipped surface incorrectly until the capability doc is repaired.

## 5. Catch-up options and their privacy cost

| Model | What it solves | Availability | Privacy/cost | Verdict |
|---|---|---|---|---|
| Retry until sender and recipient overlap | Current DM design | None if they never overlap inside retention/retry window | Best metadata minimization; poor usability, sender must stay online | Insufficient for Buzz parity |
| Pairwise anti-entropy after reconnect | Missing objects held by one of the two peers | Works only if a peer already holds every missed object | Reveals compared ranges/IDs unless capability-scoped; no third-party custody | Necessary reconciliation tool, not an offline solution |
| Same-user device-to-device history transfer | New/returning linked device while another user device retains history | Strong when an existing authorized device is reachable | Exports long-lived decrypted history over a new surface; device authorization/revocation and MLS epoch state are high-risk | Required for multi-device parity; separate ADR and explicit user action |
| Encrypted user-owned backup replica | Reinstall/recovery with no existing device online | Strong while backup and recovery key survive | Long-lived ciphertext, traffic/size metadata, key-recovery and deletion risk; becomes an archive service | Optional only; never implied by the delivery spool |
| Replicate to every group member | Group events while at least one member is online later | High for popular groups | Leaks membership/volume, amplifies storage and traffic, hard deletion | Reject as default |
| One recipient-owned mailbox device | Briar-style spare always-on device | Strong while that device is reachable | Mailbox observes timing/size and the transport endpoint absent an anonymity path; single operational failure point | Good minimum mode |
| Small explicit custodian set | Recipient devices plus selected trusted contacts/nodes | Tunable quorum; no privileged global service | More replicas improve availability but multiply metadata observers and collusion surface | **Recommended default architecture** |
| Always-online full replica | Earthstar-style participant-run replica server | Strong for every object in the authorized share | Custodian holds the replicated application state; coarse share-level access and larger breach/deletion surface | Good for public/non-secret data; too broad for private chat by default |
| Public anonymous queue relays | SimpleX-style asynchronous delivery | Strong if public relay network is healthy | Pairwise rotating queue identifiers reduce linkage, but this recreates a server/relay class and operational dependency | Study metadata techniques; reject as tic-tac-toe default |
| Global DHT/anonymous arbitrary storage | Offline retrieval without prior relationship custodians | Broad, network-dependent | Discovery becomes custody; spam, persistence, deletion, correlation, and global-routing dependence; conflicts with ADR-0006 | Reject |
| Content-addressed blob providers | Large payload retrieval from known providers | Only while a provider/pinner is reachable | Content hash, provider, timing, and interest leakage; not a message inbox | Use for media after an explicit pinning design, not chat custody |
| Static/manual drop bundle | Willow Drop-style USB/email/other carrier | Delay-tolerant and infrastructure-agnostic, but not automatic | Carrier and recipient see transfer timing/size; stale and duplicate bundles; manual operational burden | Valuable disaster/export format, not normal catch-up |

### Evidence from current systems and protocols

- [Briar Mailbox](https://briarproject.org/download-briar-mailbox/) is the
  clearest participant-owned precedent: a linked, powered spare Android device
  accepts encrypted messages while Briar is offline, which the client fetches
  later.
  Briar's manual otherwise says an offline contact receives the message when
  both peers are next online
  ([manual](https://briarproject.org/manual/)). The mailbox solves custody by
  adding an always-on holder; it does not make custody disappear.
- [MLS RFC 9750](https://datatracker.ietf.org/doc/html/rfc9750) separates
  end-to-end group crypto from the Delivery Service that stores key packages
  and routes messages. It explicitly says a P2P deployment can decentralize
  that service, but clients must then implement its delivery properties.
  [RFC 9420](https://datatracker.ietf.org/doc/html/rfc9420) similarly assumes a
  DS while protecting content even from a compromised one. Thus TreeKEM/MLS
  does not itself provide offline delivery.
- [SimpleX](https://simplex.chat/docs/simplex.html) demonstrates useful
  metadata techniques: no network-wide user identifier, per-contact
  unidirectional queue addresses, queue rotation, Tor support, and temporary
  relay retention. It also describes itself as a client-server network in
  which messages pass through relay nodes. Copy its queue unlinkability ideas,
  not its public-relay dependency.
- Signal's
  [sealed-sender design](https://signal.org/blog/sealed-sender/) hides more of
  the sender envelope from its asynchronous service, but Signal explicitly
  identifies timing and IP correlation as remaining work. Encrypting the
  payload and sender identity does not make traffic metadata invisible.
- [Willow Confidential Sync](https://willowprotocol.org/specs/confidential-sync/index.html)
  scopes synchronization to the intersection of authorized areas and performs
  private interest-overlap detection. Its
  [3D range reconciliation](https://willowprotocol.org/specs/rbsr/index.html)
  recursively compares fingerprints and exchanges only mismatched small ranges.
  This is a strong model for capability-scoped catch-up **between holders**.
  It still begins with “entries the peers have available”; it cannot recreate
  an object no holder retained.
- Willow's [Drop Format](https://willowprotocol.org/specs/drop-format/index.html)
  makes the custody/carrier distinction unusually clear: it packages a static
  set for asynchronous transfer over user-chosen media such as USB, email, or
  torrents. That is an excellent recovery/export escape hatch, but the user
  still supplies a carrier that retains the bytes.
- [Practical Rateless Set Reconciliation](https://arxiv.org/abs/2402.02668)
  reports near-optimal communication over widely varying differences and
  meaningful communication/compute improvements with Rateless IBLTs. It is a
  promising optimization after x0x has a stable custody object model and
  measurements; it should not set the first ADR's complexity floor.
- [Earthstar](https://earthstar-project.org/docs/how-it-works) separates
  authority from uptime by encouraging lightweight, redundant, always-online
  replica peers run by a share's users. It is evidence that participant-run
  replicas are operationally intelligible, not evidence that a full application
  replica is the right confidentiality unit for x0x DMs. The latter should
  expose only opaque delivery drops to custodians.
- [Iroh](https://docs.iroh.computer/what-is-iroh) relays encrypted transport when
  direct NAT traversal fails. That improves connection availability, just as
  x0x peer relays do; it does not imply durable message custody.
  [Iroh Blobs](https://docs.iroh.computer/protocols/blobs) makes the separate
  availability requirement explicit through provider/requester roles and
  tags that control garbage collection: a hash is integrity/addressing, not
  an always-available holder.
- [Veilid](https://gitlab.com/veilid/developer-book/-/tree/main) offers an
  encrypted DHT and identity-protecting routes, but its own developer book says
  the framework is beta-like and not ready for production-grade apps. More
  importantly, arbitrary DHT custody contradicts x0x ADR-0006's chosen failure
  model.

## 6. Recommended architecture: explicit ciphertext custody

### 6.1 Keep three stores and three promises separate

1. **Local history store (existing ADR-0023):** decrypted, searchable,
   user-owned, and local-only in V1. It records what this daemon witnessed and
   successfully verified; network serving is explicitly deferred to a separate
   ADR.
2. **Custody spool (new ADR):** opaque end-to-end ciphertext, capability
   addressed, bounded, not searchable by the custodian, and deleted after
   receipt/expiry. It exists only to bridge non-overlapping online windows.
3. **Optional same-user history replica/backup (later ADR):** long-lived
   history encrypted to specifically authorized user devices or recovery keys.
   It exists for device linking and disaster recovery, not ordinary message
   delivery, and it is off by default until its key lifecycle and metadata
   threat model are approved.

Calling all three “history” would blur the most important privacy boundary. The
new surface should use terms such as `delivery`, `custody`, `mailbox`, or
`drops`, not remote history. Likewise, a delivery spool's expiry window must be
visible: after TTL, it cannot recover a long-offline or reinstalled device.

### 6.2 Actors and availability modes

Each user advertises a signed **custody descriptor** to authorized senders:

- recipient-owned always-on devices;
- optionally, a small set of trusted-contact or self-hosted custodian nodes;
- an opaque queue capability per custodian;
- deposit authorization, fetch/delete capability separation;
- TTL, byte/object quotas, accepted size buckets, protocol version;
- expiry/rotation time and replacement descriptor.

The product should expose honest modes:

1. **Overlap only** — no custodian; current privacy/availability.
2. **My devices** — deposit to one or more recipient-owned nodes.
3. **Trusted custody** — recipient devices plus selected contact/self-hosted
   nodes; recommended for Buzz-like availability.
4. **Public relay** — not a default x0x/tic-tac-toe mode.

A configurable deposit policy such as “ACK from any 2 of 3 custodians” makes
the availability/privacy trade visible rather than accidental.

### 6.3 Outer and inner envelope

The custodian-visible outer record should contain only what storage admission
needs:

```text
CustodyDrop {
    protocol_version,
    queue_id,          // random, pairwise, rotating; not AgentId or group_id
    message_id,        // random/dedup identifier, unlinkable across queues
    size_bucket,
    expires_at,
    deposit_proof,     // capability MAC/signature, rate-limitable
    ciphertext,        // opaque recipient/MLS application envelope
}
```

Sender identity, stable recipient identity, group identity, content type,
ordering data, real size, and application message are sealed inside the
recipient-readable ciphertext. The inner artifact retains x0x authorship and
application authorization so a malicious custodian can delay, drop, replay, or
reorder but cannot create an accepted message.

Use per-queue ciphertext diversification: depositing the same logical group
message to several members/custodians must not emit a byte-identical
correlation beacon. Queue IDs and capabilities should rotate on time, volume,
membership change, custodian replacement, and suspected compromise.

### 6.4 Deposit, fetch, and completion

1. Sender attempts normal direct/gossip delivery.
2. In parallel or after a short direct attempt, sender deposits independently
   wrapped drops to the recipient's advertised custodian set.
3. Custodians authenticate only the deposit capability, enforce object/byte/
   rate/TTL limits, persist before ACK, and return signed/MACed deposit
   receipts.
4. When any recipient device returns, it connects over a direct/private x0x
   path, proves fetch capability, and reconciles the authorized queue window.
5. Recipient fetches missing ciphertext, verifies/decrypts the inner artifact,
   deduplicates by inner message ID, then writes the verified event to its
   existing local history store.
6. Recipient emits a signed/MACed receipt. Custodians garbage-collect after
   receipt quorum or TTL; they never promise cryptographic erasure of copies a
   malicious node made.
7. Senders may stop retrying after the configured custody quorum acknowledges,
   but UI delivery state must distinguish `sent`, `custodied`, `received`, and
   `read`.

This leaves ADR-0023's existing local-history decision unchanged: the custodian
does not serve its own witnessed/decrypted history record, and a reinstalled
device cannot ask random peers for a conversation archive. It receives only
drops addressed under capabilities it already owns. That last sentence is a
deliberate non-parity boundary, not a hidden feature: full-history restoration
requires the separate same-user design below.

### 6.5 Reconciliation choice

Start with a bounded, auditable algorithm:

- queue is ordered by a custodian-local monotonic sequence plus random message
  ID;
- recipient supplies the last contiguous receipt and sparse IDs for a bounded
  time/sequence window;
- peers compare deterministic range fingerprints and split mismatches;
- recipient requests only missing drops;
- every request is capability-scoped and quota-limited.

Willow-style range reconciliation is understandable, streamable, and supports
partial authorized ranges. Salt/fingerprint derivation must be queue-specific
so fingerprints cannot be compared across queues.

Rateless IBLT is an optimization candidate when telemetry shows that large,
sparse divergence dominates. The first ADR should define objects, custody,
authorization, deletion, and threat boundaries independently of the set-diff
codec. A second ADR can select/benchmark reconciliation algorithms.

### 6.6 Different replication policies by message class

- **1:1 DMs:** per-recipient custody queue; normal case for this design.
- **Private MLS groups:** client fan-out one independently wrapped copy per
  member custody set in v1. This costs bandwidth but avoids teaching one
  custodian the full roster. Membership commits and application messages must
  preserve MLS order/epoch rules.
- **Small public channels:** participant replicas can exchange signed public
  events; an optional explicit archive role may improve availability but is
  non-authoritative and must be visible in group policy.
- **Large public channels:** do not fan out to every member mailbox. Define a
  separate participant-replicated event-log/archive policy with bounded
  retention and multiple explicit providers.
- **Files/media:** put a small signed manifest/drop in custody. Actual chunks
  require explicit providers and pinning, with honest “available from N
  providers” UI.
- **Presence, typing, control:** remain ephemeral and are never deposited.

### 6.7 Full history recovery is a separate, harder protocol

Buzz's relay can answer an authenticated query for retained history from a
fresh client.
Opaque delivery drops cannot safely promise the same thing: they are bounded,
recipient-addressed, and deleted after receipt or expiry. A linked device can
instead request an explicit export from another device belonging to the same
user, with the source re-encrypting the selected local history to the new
device. If no source device remains, recovery requires an opt-in encrypted
backup replica and a separately protected recovery key.

This is especially important for private MLS groups. ADR-0023 stores
reader-side plaintext because retained ciphertext can become unusable across
epoch advance (`x0x@e301371:docs/adr/0023-durable-local-history.md:91-95`).
A fresh device cannot recover arbitrary old group history merely by downloading
old wire ciphertext; an authorized existing device must export/re-encrypt the
reader-side record, or the user must have opted into a backup format designed
for recovery. That expands the blast radius beyond temporary ciphertext
custody and therefore needs its own ADR, consent UI, device/recovery-key
revocation, selective scope, audit trail, and deletion language.

For the product claim, choose explicitly:

- ship bounded offline delivery first and state that fresh-device full history
  is not yet Buzz parity; or
- complete both ADRs and their acceptance suites before using the unqualified
  “Buzz successor/parity” claim.

## 7. Privacy cost and mitigations

No design can offer offline delivery with zero observable metadata to the
always-on holder. The ADR should enumerate this, not imply that “opaque
ciphertext” means “no metadata.”

| Custodian can observe or influence | Consequence | Mitigation, not erasure |
|---|---|---|
| Deposit/fetch source IP and transport route | Location/network correlation and sender/recipient linkage | x0x peer relay/private route; separate ingress/egress paths; Tor/MASQUE/VPN for high-risk mode |
| Queue pseudonym and repeated access | Long-lived social-edge correlation | Random pairwise queue IDs; automatic time/volume rotation; no Agent ID/group ID outside |
| Deposit/fetch timestamps | Conversation activity and online-window inference | Batching, bounded random delay, polling windows, cover traffic only as explicit opt-in |
| Ciphertext size and count | Message/media type and activity inference | Coarse size buckets, padding, manifest/chunk separation, uniform quota errors |
| Same ciphertext at several custodians | Cross-custodian correlation | Independently wrap/diversify every queue copy |
| Group fan-out pattern | Membership inference, especially under collusion | Per-member paths and jitter; avoid server-side group fan-out; separate public-group policy |
| Refusal, delay, drop, replay, reorder | Selective DoS and stale state | Multiple custodians, receipts, inner signatures/sequence/epoch checks, reconciliation, visible delivery state |
| Stored bytes after “delete” | Custodian can retain illicit copies | Promise protocol GC, not perfect deletion; TTL, minimal plaintext-free records, custodian replacement/revocation |
| Spam/storage exhaustion | Capability theft or sender abuse | Unforgeable deposit capabilities, per-capability quotas, rate limits, proof-of-contact, expiry/rotation |

Replication increases availability and metadata exposure together. A fixed
network-wide default such as “three custodians” would hide that trade. Let the
recipient choose the set and let the UI show current quorum and degraded
coverage.

## 8. Decisions the new x0x ADR must make

The first ADR should be titled:

> **Cross-node catch-up via participant-selected ciphertext custody**

It should decide, at minimum:

1. **Scope:** DMs and private-group application messages first; exclude
   decrypted history export and arbitrary public archives.
2. **Custodian eligibility:** own devices and explicit trusted replicas only;
   no automatic arbitrary DHT placement.
3. **Capabilities:** separate deposit, enumerate/fetch, acknowledge/delete,
   rotate, and revoke rights.
4. **Addressing/privacy:** opaque pairwise queue identifiers; no stable
   recipient/sender/group ID in the outer record.
5. **Durability:** persist-before-ACK, dedupe rules, TTL/byte/object caps,
   failure semantics, and restart tests.
6. **Quorum:** recipient-selected set and ACK policy; behavior under partial
   reachability.
7. **Inner authenticity:** signed/PQC x0x artifacts remain authoritative;
   custodians cannot originate accepted content.
8. **Ordering and replacement:** DM sequence and MLS epoch/commit handling;
   replay and gap detection; latest-wins merge by the shipped replaceable keys,
   `created_at`, and deterministic equal-timestamp event-ID tie-break rather
   than custody arrival order.
9. **Receipts and deletion:** state machine and honest limits of remote
   deletion.
10. **Threat model:** individual and colluding custodians, traffic observers,
    malicious senders/recipients, compromise, and DoS.
11. **Transport:** direct x0x first, peer relay/private path fallback, optional
    anonymity transport policy.
12. **Observability:** counts/bytes/age/quorum health without logging queue
    capabilities or stable social identifiers.
13. **Compatibility:** local history remains ADR-0023; a generated per-kind
    action table distinguishes custodied events, resulting state/audit
    artifacts, private sidecars, and exclusions; the delivery API,
    `x0x@e301371:SKILL.md`, the API reference, and parity CLI update together.
14. **Validation:** non-overlapping-online-window e2e tests, multi-custodian
    loss, restart, equivocation/replay, quota exhaustion, metadata snapshot,
    no-plaintext-at-custodian assertions, and newer-then-older replaceable
    delivery proving that stale custody arrival cannot regress state.
15. **Production versus projection:** the six-row relay-only retirement table
    above is normative. Custody carries authorized source/state artifacts; it
    never grants a custodian authority to mint membership, preferences,
    summaries, presence, or pagination metadata.
16. **Stage boundary:** the bridge-local key may wrap loopback compatibility
    projections only until the native consumer for that feature lands. Native
    acceptance must prove the same UX from x0x-authorized state or deterministic
    local projection before deleting the corresponding Nostr path.
17. **Membership hand-off:** custody does not resolve competing group-state
    siblings. Retiring kind `13534` requires the fork-strategy/authorization
    decision to close x0x ADR-0016's recorded equal-revision fork question and
    an end-to-end test in which two authorized admins mutate the same parent
    concurrently, every replica converges, and the losing proposal is visibly
    rebased/retried or rejected rather than silently lost. For TreeKEM groups,
    equal epoch numbers do not prove convergence: the test must prove an
    identical versioned digest of the exact accepted TreeKEM commit (which
    transitively binds `tree_hash_after` and the update path),
    post-resolution cross-decryption among all survivors, and exclusion of
    removed members. The implementation order must generate the TreeKEM commit
    before sealing the cross-bound group-state commit. Resolution additionally
    requires a retained parent-epoch checkpoint until branch confirmation and
    sibling interception at the roster-apply gate; the current bare rejection
    before TreeKEM processing cannot satisfy the test. GSS ban/remove tests must
    likewise prove that each surviving member installs a key whose derived
    confirmation tag matches the signed state binding, regardless of share/
    state-event arrival order, while removed members cannot derive the new key.

A follow-up reconciliation ADR should compare:

- bounded linear/keyset diff;
- Merkle/range fingerprints;
- Willow-style range reconciliation;
- Rateless IBLT.

Benchmarks must vary total set size, difference size, adversarial IDs, message
size, disconnect duration, and mobile resource budgets. It should not be
allowed to change the custody/privacy contract.

A separate **same-user history transfer and encrypted recovery** ADR must decide
which user/machine identity can authorize an export, how a source device proves
the new device belongs to the same user, how reader-side plaintext is
re-encrypted, what happens after device/recovery-key revocation, which scopes
may be restored, and whether any unattended backup provider is allowed. This
ADR would explicitly revise the load-bearing local-only privacy claim for a
scoped, owner-authorized export path; it must follow ADR-0023's review trigger
and must not arrive as an incidental mailbox feature.

## 9. Consequences for tic-tac-toe sequencing and claims

1. **Keep the Buzz fork and staged (a)→(b) plan.** It is the fastest way to
   preserve UX while exercising the x0xd public surface. The facade is not the
   end state and cannot close the availability gap.
2. **Stage 1 claim:** “live Buzz UX over x0x, with restart-stable local
   history.” Do not claim relay-equivalent offline delivery.
3. **Add a discriminating acceptance test now:** sender A sends while every
   device for recipient B is shut down; A then shuts down; B starts later and
   receives exactly once. This should fail until the custody ADR is
   implemented. The existing restart test does not exercise it.
4. **Add a separate recovery test before claiming full parity:** B links a
   fresh device after the delivery-spool TTL, with B's old device available;
   the new device receives authorized channel/DM history exactly once and can
   search it. A disaster-recovery variant with no old device must fail unless
   the user explicitly enabled encrypted backup.
5. **Split delivery state in the UI:** `sending` → `custodied` → `received` →
   `read`; do not label custodian ACK as recipient delivery.
6. **Gate native Stage 3 “relay removal” on the offline-delivery test.**
   Removing the Nostr facade before x0x replaces the relay's custody promise
   would convert a known architecture gap into user-visible message loss.
7. **Gate unqualified parity on the recovery test as well.** Shipping bounded
   mailbox delivery without device restore is a useful milestone, but it is not
   every Buzz availability behavior.
8. **Keep explicit v1 cuts:** Git, galleries, full orchestration, and voice are
   separate product gaps. Do not let them obscure the foundational custody
   gap.

## Final recommendation

Proceed with tic-tac-toe as the Buzz fork and x0x-native successor, but amend
the design vocabulary:

- x0x already provides **durable local memory**;
- it does not yet provide **asynchronous delivery across non-overlapping
  online windows**;
- the successor to a global Buzz relay should be a **recipient-selected
  ciphertext custody set**, not universal history sync and not a global DHT;
- reconciliation is the efficient fetch mechanism after custody exists;
- full-history device recovery is a second, explicit same-user
  re-encryption/backup design, not an emergent property of the mailbox;
- every added custodian buys availability with observable metadata, so the
  protocol and UI must expose that trade rather than market it away.

That design fits ADR-0006's participant/explicit-replica model, leaves
ADR-0023's V1 local-history store unchanged for ordinary delivery, follows its
explicit review trigger, and gives tic-tac-toe a credible path through its
foundational relay-free availability gaps.
