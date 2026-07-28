# ADR 0001: Authenticate the secure-group state frontier

- **Status:** Proposed
- **Date:** 2026-07-27
- **Decision owners:** x0x and tic-tac-toe maintainers
- **Scope:** design only; implementation requires David's approval
- **Reviewed x0x state:** `e3013710d7ed69077de9a799dffdbeb5ac80535a`
- **Reference implementation:** [authenticated secure-group frontier](../design/authenticated-secure-group-frontier.md)

## Context

tic-tac-toe may retire Buzz's relay-authored membership projection only after
x0x makes roster state, secure-group cryptographic state, and join bootstrap
one authenticated frontier.

The reviewed x0x state presents five decision-driving conditions:

1. equal-revision children can be accepted in different orders by different
   replicas, while deterministic fork choice remains future work;
2. an epoch-only TreeKEM or GSS binding does not identify the exact
   cryptographic state;
3. GSS shares and signed group-state commits can be installed independently;
4. invite bootstrap adopts an unsigned base projection before authority is
   established; and
5. `GroupCard` ingress applies inconsistent signature policy. In particular,
   the globally subscribed gossip path deliberately accepts unsigned legacy
   cards, then ranks and serves them without later verification, while other
   paths reject unsigned cards.

Condition 5 permits an unsigned, higher-revision card to displace a signed card
in discovery. The applied-metadata path is not the sole defect and cannot be
fixed in isolation: it is behind an Admin check, while the global gossip
listener has no caller identity in scope.

Independent source review confirmed conditions 2, 3, and 4 against the reviewed
x0x state, removing the acceptance hold on those conditions.

The evolving source analysis, mechanisms, rollout, compatibility bounds, and
probe results live in the linked reference implementation. This ADR is
self-contained; that chapter elaborates the decision but does not define it.

## Decision

### A. Define one authenticated frontier

A membership mutation is complete only when the authority-signed roster state,
the exact applicable secure-plane artifact, monotonic state and secret epochs,
the previous-state hash, the roster root, and any bootstrap anchor agree.

No component may become current before every applicable join verifies.
Roster and cryptographic state must be durably installed or rolled back as one
caller-visible transaction. Versioned bindings are mandatory:

```text
treekem:commit-postcard-v1:epoch=<u64>:blake3=<64-lower-hex>
gss:key-confirm-v1:epoch=<u64>:blake3=<64-lower-hex>
```

Receivers must expose structured rejection diagnostics. Cross-bound senders may
emit exact bindings and confirmation tags only after compatible receivers
exist. Once telemetry and migration evidence show the supported fleet has
crossed the explicit transition cutoff, unknown schemes, legacy epoch-only
schemes, and unsigned invites fail closed.

### B. Cross-bind the exact TreeKEM commit

TreeKEM state commits must bind a domain-separated digest of the exact
transmitted `TreeKemCommit` bytes together with the stable group ID. Senders
must retain a recoverable parent checkpoint, generate and locally apply the
TreeKEM commit, digest its exact bytes, seal the roster commit, persist both
artifacts, and publish only after persistence succeeds. Local failure restores
the checkpoint. Receivers must verify the roster commit, the TreeKEM commit,
and their cross-binding before installing either.

### C. Confirm GSS keys and join in two phases

Each GSS rotation must use a fresh uniformly random 32-byte secret and a
domain-separated confirmation tag covering the stable group ID, secret epoch,
state revision, previous-state hash, and roster root. The tag travels in the
versioned GSS binding, and the sealed-share AAD must bind that tag or the
state-commit hash.

Commit-first and share-first delivery both create pending state. Neither the
candidate secret nor the new roster becomes current until both artifacts are
present and the tag verifies; installation is then atomic. Remove and ban use
the same rotate, confirm, reseal-to-survivors, and receive path. Public
confirmation tags must never be derived from low-entropy secrets.

The existing `old_epoch < secret_epoch` conditional remains until the pending
join is deployed; removing it earlier can lose a valid share under reorderable
delivery. Sender self-delivery is not a permanent reason to retain it.

### D. Authenticate invite bootstrap before adoption

Modern invites must use a versioned canonical encoding and an ML-DSA signature
covering every adopted field, including both group identifiers, policy,
genesis, the complete base frontier, invite secret, and expiry. The signer must
be identified by authenticated key material and authorized as active
Admin-or-higher by the advertised roster.

The joiner must verify the signature and recomputed base projection before it
creates or persists any local stub. A self-consistent hash in an unsigned link
is not authority. Legacy unsigned invites must be reissued or use a transition
flow that establishes an authority-signed anchor before adoption.

### E. Close stable-ID, card-authority, and destructive-write boundaries

Invite admission and locking must treat both the direct group ID and claimed
stable group ID symmetrically, including concurrent links that use different
direct IDs for one stable ID.

Every path that imports, applies, caches, ranks, or serves a `GroupCard` must
verify its signature and authorize its signer against an authenticated
same-group frontier. The global gossip listener is included. An explicit
compatibility path for unsigned legacy cards accepted by global gossip may
quarantine them as non-authoritative, but they must not feed current-state
selection, `supersedes`, or discovery responses. That path ends at the
telemetry-and-migration cutoff defined above. An invite-derived provisional
record cannot authorize a card write.

Unknown groups may be retained as quarantined discovery artifacts, but their
self-asserted owners must not become local authority until an authenticated
same-group relation is established.

Destructive alias fan-out must require an authority-backed same-group relation
before overwriting an existing record. Absent aliases may receive a tombstone;
an unrelated colliding record may not be altered or stripped of key material.
`mls_group_id` equality remains only a candidate write-site discriminator until
tests prove all legitimate alias shapes. If that premise fails, use a stronger
authority-backed identity relation rather than weakening the guard.

### F. Resolve siblings before retiring Buzz membership

Native retirement of Buzz membership kind `13534` requires either:

1. deterministic network-wide sibling choice with visible rebase/retry or
   rejection of the losing administrative operation; or
2. a strictly enforced single-committer policy.

Fork detection must occur at or above the roster-apply gate, and the
parent-epoch checkpoint must remain recoverable until branch confirmation.
HTTP success and per-daemon first arrival are not branch confirmation.

## Validation

The final controls must fail on the reviewed x0x behavior or on a mutation and
pass on the implementation. At minimum they must independently exercise:

1. concurrent sibling creation, deterministic convergence, visible loser
   handling, exact TreeKEM commit identity, survivor cross-decryption, and
   removed-member exclusion;
2. GSS commit-first and share-first convergence, rejection of every mismatched
   context input, and proof that neither order exposes the candidate secret
   before confirmation;
3. wrong tag, wrong epoch, wrong group ID, wrong roster root, relabelled AAD,
   replayed share, and stale commit all failing closed with structured reasons;
4. GSS ban and remove each rotating exactly once, delivering to every survivor,
   and excluding the removed member;
5. crash and restart at each sender and receiver transaction boundary restoring
   either the complete previous frontier or the complete new frontier, never a
   mixed state;
6. sender self-delivery failing to reach the mutation closure after the sender
   stores the sealed revision;
7. absent, invalid, altered, self-consistent-but-unauthorized, and replayed
   invite bootstrap artifacts before any stub is persisted;
8. stable-ID collision under sequential and concurrent admission;
9. unsigned, invalidly signed, and unauthorized higher-revision cards at every
   ingress path, including public gossip and its explicit compatibility path
   for accepted unsigned legacy cards; the raw-QUIC control must first publish
   an acceptable machine-signed identity announcement through the production
   listener, then obtain `verified == true` from the production general
   discovery cache rather than by injecting a cache fixture or bypassing
   `verified`;
10. unauthorized direct import, cache replacement or eviction, discovery
   override, asserted-owner promotion, and metadata/frontier rewrite; and
11. all legitimate tombstone callers plus refusal to modify a colliding
   different group.

Controls must assert authenticated bytes, digests, authority relations, and
installed state—not proxy values such as matching numeric epochs. Acceptance
with an explicitly owned follow-up is not full validation discharge.

## Consequences

### Positive

- Membership, cryptographic state, and bootstrap become one auditable
  post-quantum authority frontier.
- Equal epoch numbers can no longer confuse distinct TreeKEM or GSS states.
- Delivery order stops deciding whether a GSS secret is trusted.
- Invite, card, cache, import, and tombstone boundaries fail closed
  independently.
- tic-tac-toe gains a concrete gate for retiring relay-authored membership.

### Costs

- TreeKEM needs checkpoint retention and atomic cross-plane installation.
- GSS needs durable pending joins, resealing to every survivor, and garbage
  collection.
- Unsigned invites and group cards need an explicit migration or quarantine
  policy.
- Sibling resolution is additional protocol work, not a UI or custody concern.

### Unchanged boundaries

- Invite authentication does not replace one-time admission or the
  authority-signed `MemberAdded`; it authenticates the adopted base frontier.
- This ADR does not design message custody or full-history recovery.
- This ADR does not authorize product implementation.

## Alternatives rejected

1. **Keep epoch-only bindings.** Equal epochs can name different cryptographic
   states.
2. **Hash only the resulting TreeKEM tree.** That does not identify the exact
   signed commit and update path survivors must process.
3. **Install GSS shares immediately.** Arrival order would remain an
   authentication decision.
4. **Strengthen syntax without signing invites.** An attacker can construct a
   self-consistent unsigned projection.
5. **Treat signature validity as group authorization.** It proves who signed,
   not authority over an arbitrary claimed stable group.
6. **Fix only one permissive card site.** The global gossip path and other
   inconsistent ingress policies would remain.
7. **Fix only the already-joined guard or prune the alias collector.** Neither
   closes the reusable destructive-write boundary.
8. **Treat per-daemon first arrival or custody as fork choice.** Transport does
   not define group-state consensus.
