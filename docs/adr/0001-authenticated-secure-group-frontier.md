# ADR 0001: Authenticate the secure-group state frontier

- **Status:** Proposed
- **Date:** 2026-07-27
- **Decision owners:** x0x and tic-tac-toe maintainers
- **Scope:** design only; implementation requires David's approval
- **Reviewed x0x state:** `e3013710d7ed69077de9a799dffdbeb5ac80535a`

## Context

tic-tac-toe will eventually retire Buzz's relay-authored membership projection
in favor of x0x authority-signed group state. That hand-off is unsafe until the
roster state, secure-group cryptographic state, and join bootstrap are one
authenticated frontier.

The current x0x frontier has four independent defects.

### 1. Equal-revision siblings can split replicas

`GroupStateCommit` signs a revision, previous-state hash, roster root, and
`security_binding`, but accepted ADR-0016 records that concurrent admins can
author different children of the same parent and leaves deterministic fork
choice unresolved
(`x0x@e301371:docs/adr/0016-role-based-group-authority-flat-admin.md:109-124,210-215`;
`x0x@e301371:src/groups/state_commit.rs:350-451,690-720`).

The daemon-local membership mutex cannot serialize two different admins'
daemons. Both HTTP mutations may return success before either sibling reaches
the other node
(`x0x@e301371:src/server/state.rs:700-719`;
`x0x@e301371:src/server/routes/named_groups.rs:8425-8451,9177-9331,10267-10555`).
Receivers reject whichever equal-revision sibling arrives second, so different
replicas can retain different first arrivals.

### 2. The crypto binding does not identify the exact crypto state

TreeKEM remove currently binds an epoch-only string. Two different sibling
commits can both say `treekem:epoch=N+1` while producing different trees and
update paths. The exact serialized `TreeKemCommit` already travels beside the
state commit and its dependency-level signature covers `tree_hash_after`, but
the x0x state commit does not cross-bind that artifact
(`x0x@e301371:src/mls/treekem.rs:12-15,92-97,365-386`;
`x0x@e301371:src/server/routes/named_groups.rs:9263-9328`;
`saorsa-mls@0.3.8:src/treekem_group.rs:140-161,466-511,605-613,905-935`).

GSS has the corresponding problem: publishing the rotated secret is forbidden,
but an epoch-only binding cannot distinguish two different 32-byte secrets at
the same epoch.

### 3. GSS share and state events are installed independently

Ban persists the new epoch, sends recipient-sealed
`SecureShareDelivered` envelopes, and publishes the signed `MemberBanned`
state commit later
(`x0x@e301371:src/server/routes/named_groups.rs:10305-10368,10397-10408`).
A receiver can install a higher-epoch secret and overwrite
`security_binding` before the signed state commit arrives. The store path
checks withdrawal state but neither verifies a state signature nor recomputes
the state hash
(`x0x@e301371:src/server/routes/named_groups.rs:2040-2060,5803-5890`).

The share is restricted to an active Admin whose actor and transport sender
match, so this is an authorized-malicious or sibling-fork case, not arbitrary
peer injection (`x0x@e301371:src/server/routes/named_groups.rs:5820-5826`).

Accepted ADR-0010 also requires GSS rotation on remove as well as ban. The live
non-TreeKEM remove path does not rotate or reseal the content key
(`x0x@e301371:docs/adr/0010-gss-before-mls-treekem-for-v1-secure-groups.md:35-44,49-68,97-103,140-148`;
`x0x@e301371:src/server/routes/named_groups.rs:8425-8525`).

### 4. Invite bootstrap is unsigned and can reach destructive aliases

`SignedInvite` has a future-facing `signature`, `signable_bytes`, and
`is_signed`, but the join flow does not enforce a signature. `new()` leaves
the signature empty. A structural search over `src/` and `tests/` at the
reviewed SHA found no assignment to a `SignedInvite` signature and no
production invite call to `signable_bytes` or `is_signed`
(`x0x@e301371:src/groups/invite.rs:1-8,32-97,126-220,236-240`).

The unsigned artifact seeds the joiner's stable group ID, crypto plane, secret
epoch, security binding, state revision, roster, state hash, and previous hash
(`x0x@e301371:src/server/routes/named_groups.rs:7641-7677`). The one-time invite
secret authenticates bearer admission to the inviter; it does not authenticate
the inviter's claimed state frontier to the joiner.

The join guard checks both claimed IDs for withdrawn state but checks only
`group_id` for already-joined state
(`x0x@e301371:src/server/routes/named_groups.rs:7728-7770`). A victim who accepts
a crafted link can therefore acquire a stub under `K'` whose claimed stable ID
collides with a real group `K`, then subscribe to the topic derived from `K'`
(`x0x@e301371:src/groups/mod.rs:321-324`;
`x0x@e301371:src/server/routes/named_groups.rs:7852`).

A terminal event on that topic is correctly signature- and role-checked
against the selected stub, but the roster used for that check came from the
unsigned link
(`x0x@e301371:src/server/routes/named_groups.rs:1990-2010,5063-5115`;
`x0x@e301371:src/groups/state_commit.rs:719-742`). On success, the tombstone
writer derives `K` from the stub, clears its key material, and performs an
unguarded insert over every alias. It can replace the real record at `K` with
the withdrawn, keyless stub
(`x0x@e301371:src/server/routes/named_groups.rs:8808-8810,9008-9025`).

This chain requires the victim to accept a crafted invite. It is not claimed
as remote-unauthenticated, and this research did not execute an exploit.

## Decision

### A. Define one authenticated frontier

A membership mutation is complete only when all applicable artifacts agree:

1. authority-signed roster state commit;
2. exact secure-plane artifact or GSS key-confirmation tag;
3. monotonic state and secret epochs;
4. previous-state hash and roster root; and
5. for bootstrap, an authenticated anchor for the complete advertised base
   projection.

No component may be installed as current state before the applicable joins
above verify. Durable writes of the roster and crypto state must be atomic from
the caller's perspective.

Use versioned, discriminated bindings:

```text
treekem:commit-postcard-v1:epoch=<u64>:blake3=<64-lower-hex>
gss:key-confirm-v1:epoch=<u64>:blake3=<64-lower-hex>
```

After the compatibility transition, unknown schemes and legacy epoch-only
schemes fail closed.

### B. Cross-bind the exact TreeKEM commit

For TreeKEM, derive a BLAKE3 digest using a hard-coded context such as
`x0x security binding treekem commit postcard v1` over:

1. a length-prefixed stable group ID; and
2. the exact received postcard bytes of `TreeKemCommit`.

Hash the transmitted bytes directly. Do not decode and re-encode them as a
security precondition.

The sender transaction order is:

1. retain a recoverable parent-epoch checkpoint;
2. generate and locally apply the TreeKEM commit;
3. digest its exact serialized bytes;
4. seal the roster commit over the versioned digest;
5. persist both artifacts; and
6. publish only after persistence succeeds.

On local failure, restore the checkpoint. On receipt, verify the state commit,
the TreeKEM commit, and their cross-binding before installing either.

### C. Use GSS key confirmation and a two-phase join

For each fresh uniformly random 32-byte GSS secret:

1. derive a confirmation key with BLAKE3 derive-key context
   `x0x gss confirmation key v1`;
2. MAC a canonical, length-prefixed context containing stable group ID, new
   secret epoch, new state revision, previous state hash, and new roster root;
3. place the 32-byte tag in the versioned GSS binding; and
4. bind the sealed-share AAD to that tag or to the state-commit hash.

The daemon must keep pending state keyed by
`(stable_group_id, secret_epoch, confirmation_tag)`:

- commit first: retain the authenticated expected tag with no installed new
  secret;
- share first: retain the sealed or decrypted candidate as pending, but do not
  install or use it;
- both present: derive and compare the tag, then atomically install the secret
  and state.

Order must not affect the result. GSS remove and ban use the same rotate,
confirm, reseal-to-survivors, and two-phase receive path. GSS secrets must
remain uniformly random; password-derived or other low-entropy inputs are
forbidden because the public tag is an offline guess verifier.

The existing `old_epoch < secret_epoch` conditional remains until this pending
join is deployed. Removing it earlier loses a valid share under today's
reorderable installation. Sender self-delivery is not a permanent reason to
keep it: the sender seals and stores the bumped revision before publish, so a
looped-back commit fails state validation before the mutation closure can run
(`x0x@e301371:src/groups/mod.rs:524-548`;
`x0x@e301371:src/server/routes/named_groups.rs:1967-1987,10324-10338,10397-10407`;
`x0x@e301371:src/groups/state_commit.rs:690-711`).

### D. Authenticate invite bootstrap

Modern invite bootstrap must authenticate every adopted field. Keep the
existing field coverage, but replace the current ambiguous concatenation with
a versioned, length-prefixed or otherwise canonical signable encoding. The
invite must carry an ML-DSA signature whose signer:

1. is identified by authenticated key material, not unsigned `inviter` text;
2. is active and Admin-or-higher in the advertised base roster; and
3. signs the stable ID, group ID, policy, genesis data, complete base frontier,
   invite secret, and expiry.

The joiner must:

1. validate size, expiry, and one-time admission semantics;
2. verify the invite signature;
3. recompute the advertised state projection and require its hash to equal
   `base_state_hash`;
4. require plane-specific versioned binding syntax; and
5. only then create or persist a local stub.

A self-consistent hash from the unsigned link is not authentication. Legacy
unsigned invites must be reissued after the enforcement cutoff or joined
through a flow in which an authority-signed state anchor establishes the
frontier before any claimed base fields are installed.

The adjacent group-card import is the enforcement model. `GroupCard` has
canonical length-prefixed signable bytes plus ML-DSA sign and verify methods
(`x0x@e301371:src/groups/directory.rs:31-85,88-165,169-195`), and
`import_group_card` rejects a failed signature at the entry point, before it
takes the membership lock or looks up local state
(`x0x@e301371:src/server/routes/named_groups.rs:11560-11572,11580-11593`).
This is specifically the model for unauthenticated ingress. The separate
`GroupCardPublished` metadata arm tolerates an empty card signature at `:5741`,
but only where authorization is instead derived from the transport-provided
sender identity: `sender_hex` must name an active Admin-or-higher member of the
existing record at `:5732-5737`, the card stable ID must match at `:5738-5740`,
and the sink is the group-card cache at `:5744-5749`, not `named_groups`. The
`verified` flag also gates this arm at `:4525-4533`, but the source defines it
as a best-effort identity-discovery-cache annotation at `:4501-4514` and
explicitly says it is not the membership-authorization control. Do not count
that racy annotation as an additional authentication precondition. The
conditional signature pattern must not be copied into invite admission, which
has no equivalent sender authority. Invite join must reject both absent and
invalid authentication unconditionally, before a remote artifact can reach
frontier adoption, locking, or alias fan-out.

### E. Close stable-ID collision and destructive fan-out independently

Invite handling must test both `group_id` and `stable_group_id` for
already-joined state as it already does for withdrawn state. Concurrency
control must serialize on the claimed stable group as well as the MLS group
ID; two links with different `group_id` values but the same stable ID must not
race into two stubs.

Do not repair the collision by pruning
`collect_same_stable_group_aliases`. It has nine production callers and is
correctly answering which keys currently name a stable group. In the tombstone
case, `K` reaches the output alias set at `named_groups.rs:8663-8664`,
`:8666-8671`, and `:9022`; the first and third share one caller-derived source,
while the map scan is the second independent source. Removing one insertion is
not a class-closing repair.

Repair the destructive boundary at `named_groups.rs:9023-9025`: before
overwriting an existing record, require an authority-backed same-group
predicate between that record and the tombstone. Absent keys may still receive
the tombstone.

Source probes now support simple equality of `mls_group_id` as the write-site
discriminator at two of the three callers:

- `group_deleted` clones the resolved record at `:5085` and passes the applied
  result at `:5114`;
- `withdrawn_card_import` clones the keyed local record at `:11589`, and the
  sole intervening mutation at `:268-309` does not write `mls_group_id` before
  the tombstone call at `:11605`.

The local-withdraw caller likewise passes `terminal_info` cloned from the
record addressed by `id` (`:9571-9603,9619`), but the legitimate alias shapes
that its fan-out is expected to replace have not been enumerated. That is the
remaining proof obligation: demonstrate that local withdrawal never
intentionally overwrites an alias record with a different MLS ID. Until that
test exists, `mls_group_id` equality remains a strongly supported candidate
rather than a final wire/storage invariant. If the premise fails, use a
stronger authority-backed identity relation rather than weakening the guard.

The invite signature, symmetric join guard, and destructive write guard are
all required. They sever the demonstrated chain at different boundaries and
defend against different future writers.

### F. Resolve siblings before retiring the Buzz membership projection

Native retirement of Buzz kind `13534` is blocked until x0x defines one of:

1. a deterministic, network-wide sibling fork choice plus explicit
   rebase/retry or rejection of the losing administrative operation; or
2. a strictly enforced single-committer policy.

The implementation must retain a parent-epoch checkpoint until branch choice
is confirmed. Fork detection must happen at or above the roster-apply gate;
the current bare rejection before crypto-artifact processing is insufficient.
HTTP success is not branch confirmation.

## Rollout sequence

1. **Contain the existing invite/tombstone chain.** Add the symmetric
   already-joined/locking rule and the same-group destructive write guard.
   These changes do not require new wire formats.
2. **Deploy fail-closed receivers.** Parse versioned TreeKEM/GSS bindings,
   persist pending GSS joins, expose structured rejection diagnostics, and keep
   legacy receive behavior only behind an explicit transition policy.
3. **Deploy authenticated invite minting and verification.** Reissue legacy
   links; do not silently adopt unsigned base frontiers.
4. **Deploy cross-bound senders.** Generate exact TreeKEM bindings and GSS
   confirmation tags only after compatible receivers exist.
5. **Unify GSS remove and ban.** Rotate and reseal on both paths through the
   two-phase protocol.
6. **Deploy sibling choice and retained checkpoints.** Prove convergence before
   removing the Buzz membership projection or claiming native parity.
7. **Remove compatibility paths.** Reject epoch-only bindings and unsigned
   invites after telemetry and migration evidence show the supported fleet has
   crossed the cutoff.

No tic-tac-toe product code is authorized by this proposed ADR.

## Validation

The change is accepted only when the following tests fail on the reviewed
behavior and pass on the implementation:

1. a forged or unsigned modern invite cannot seed any base frontier field;
2. an invite with a valid signature but altered revision, roster, stable ID,
   secret epoch, binding, or expiry is rejected;
3. a self-consistent but unauthorized base hash is rejected;
4. an invite claiming an existing stable ID cannot create a competing stub,
   including two simultaneous links with different MLS IDs;
5. accepting a crafted invite and then receiving an attacker-roster-authorized
   `GroupDeleted` cannot alter, withdraw, or clear key material from the real
   group;
6. all three legitimate tombstone callers still update every supported current
   and migrated alias, while a colliding different group remains unchanged;
7. GSS share-first and commit-first delivery converge to identical installed
   state, and neither order exposes the candidate secret before confirmation;
8. wrong tag, wrong epoch, wrong group ID, wrong roster root, relabelled AAD,
   replayed share, and stale commit all fail closed with structured reasons;
9. both GSS ban and GSS remove rotate once, deliver to every survivor, and
   exclude the removed member;
10. two concurrent TreeKEM admin removals from one parent converge on the same
    exact accepted commit digest and roster at every replica;
11. all TreeKEM survivors cross-decrypt post-resolution traffic, while removed
    members cannot decrypt it;
12. crash/restart at each sender and receiver transaction boundary restores
    either the complete previous frontier or the complete new frontier, never a
    mixed state; and
13. sender self-delivery cannot reach the mutation closure after the sender
    stored the sealed revision.

Tests must assert exact commit/binding bytes or digests, not merely equal
numeric epochs.

## Consequences

### Positive

- Membership, cryptographic state, and bootstrap become one auditable PQC
  authority frontier.
- Exact TreeKEM and GSS state can no longer be confused by equal epoch numbers.
- Delivery order stops deciding whether a GSS secret is trusted.
- The invite/tombstone chain is closed at source, admission guard, and
  destructive write boundaries.
- tic-tac-toe gains a concrete gate for retiring relay-authored membership.

### Costs

- TreeKEM senders need rollback/checkpoint retention and receivers need atomic
  cross-plane installation.
- GSS receivers need durable pending-share/pending-commit state and garbage
  collection.
- Unsigned legacy invite links must be reissued or follow a deliberately
  restricted bootstrap path.
- Deterministic sibling resolution is additional protocol work, not a custody
  or UI concern.

### Unchanged boundaries

- Invite authentication does not replace the one-time admission handshake or
  the authority-signed `MemberAdded`; it authenticates the state frontier
  adopted locally.
- This ADR does not design cross-node application-message custody or full
  history recovery.
- This ADR does not authorize product implementation.

## Alternatives rejected

1. **Keep epoch-only bindings.** Equal epochs can name different TreeKEM trees
   or GSS secrets.
2. **Hash only `tree_hash_after`.** It selects the resulting public tree but not
   the exact signed commit/update path every survivor must process.
3. **Install GSS shares immediately.** This preserves arrival-order behavior
   but leaves the join unauthenticated.
4. **Strengthen binding syntax without signing invites.** An attacker can
   construct a syntactically valid binding inside the same unsigned artifact.
5. **Trust a recomputed invite state hash.** Every hash input is attacker
   controlled until an authority authenticates the anchor.
6. **Fix only the already-joined guard.** It closes the current invite route but
   leaves the destructive alias writer reusable by future colliding records.
7. **Prune the shared alias collector.** It changes nine consumers and does not
   address the unguarded destructive operation.
8. **Treat per-daemon first arrival as fork choice.** Different replicas can
   accept different siblings.
9. **Let custody choose the winner.** Delivery systems transport authenticated
   artifacts; they do not grant authorization or define group-state consensus.
