# Authenticated secure-group frontier — reference implementation

Implements: [ADR 0001](../adr/0001-authenticated-secure-group-frontier.md)
Resolves at: `x0x@e3013710d7ed69077de9a799dffdbeb5ac80535a`
Source: the `x0x` repository; every source and x0x ADR citation resolves there unless another repository or version is named explicitly.

This chapter contains the evolving evidence, mechanism, rollout, and validation
detail for ADR 0001. The ADR is authoritative for the frozen decision. Removing
this chapter must not change what the ADR decides.

## Citation contract and audit

Every repository path in this chapter is relative to the x0x repository named
by `Resolves at:`. Throughout this chapter, unless a citation or its governing
block declaration names another file, every bare `:N` or `:N-N` citation
resolves to `src/server/routes/named_groups.rs`. A citation never inherits a
different repository, revision, or file merely from neighbouring prose.

The source document contained 55 citation tokens: 34 explicitly stamped x0x
citations, 20 unstamped but x0x-directed citations, and one version-pinned
`saorsa-mls` citation. Of the 20 unstamped citations, only nine used full
repository-relative paths; ten used bare basenames and one used the partial
path `groups/directory.rs`. None resolved in tic-tac-toe. This chapter binds
the repository and revision in its header and normalizes those eleven paths;
the version-pinned dependency citation retains its own source declaration.

## Grounding

tic-tac-toe will eventually retire Buzz's relay-authored membership projection
in favor of x0x authority-signed group state. That hand-off is unsafe until the
roster state, secure-group cryptographic state, and join bootstrap are one
authenticated frontier.

The source analysis identifies five independent defects. Defects 2, 3, and 4
remain provisional until the assigned independent source review completes;
the mechanism and validation matrix must change if any of them does not hold.

### 1. Equal-revision siblings can split replicas

`GroupStateCommit` signs a revision, previous-state hash, roster root, and
`security_binding`, but accepted ADR-0016 records that concurrent admins can
author different children of the same parent and leaves deterministic fork
choice unresolved
(`x0x@e301371:docs/adr/0016-role-based-group-authority-flat-admin.md:109-124,210-215`;
`x0x@e301371:src/groups/state_commit.rs:350-451,690-720`).

ADR-0016 calls deterministic fork choice “future work” at `:119-124` and
“Future (recorded, not planned)” at `:213-215`. Its rebase-and-retry mechanism
is scoped to “on stale rejection” at `:212`. Two admins operating on different
daemons can each see the same parent as current, so neither local mutation is
stale, neither is rejected, and that retry path does not serialize them.

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

### 5. Group-card signature policy is inconsistent across ingress paths

`GroupCard::verify_signature` proves that `authority_agent_id` derives from
the supplied public key and signed the card bytes, but explicitly leaves group
authorization to apply time
(`x0x@e301371:src/groups/directory.rs:143-195`).

The reviewed server applies four policies at five sites:

1. global `GroupCardPublished` gossip accepts unsigned cards deliberately for
   pre-D.3 compatibility and rejects only invalid non-empty signatures
   (`x0x@e301371:src/server/routes/named_groups.rs:1134-1167`);
2. the directory-shard path rejects an empty or invalid signature
   (`x0x@e301371:src/server/routes/named_groups.rs:1452-1459`);
3. the LTC delivery path rejects an empty or invalid signature
   (`x0x@e301371:src/server/routes/named_groups.rs:1676-1679`);
4. applied metadata accepts an empty signature with no compatibility comment,
   although this path first requires `sender_is_admin` and matching group ID
   (`x0x@e301371:src/server/routes/named_groups.rs:5732-5741`); and
5. direct `import_group_card` verifies unconditionally
   (`x0x@e301371:src/server/routes/named_groups.rs:11560-11567`).

The applied-metadata condition is therefore an undocumented instance of a
documented carve-out, not an isolated check that can simply be tightened.
The deliberate global-gossip instance is the reachable one: its subscription
handler has no caller identity in scope before cache admission. The
applied-metadata instance is behind an existing Admin gate.

The cache consequence is authoritative, not cosmetic. `discover_groups`
merges cached cards into `GET /groups/discover` without re-verifying them
(`x0x@e301371:src/server/routes/named_groups.rs:11320-11347`).
`GroupCard::supersedes` ranks by revision and issue time while documenting that
the caller must already have verified both signatures
(`x0x@e301371:src/groups/directory.rs:203-213`). An unsigned card naming an
existing group with a higher revision can therefore displace the signed card.
The card's own discoverability field controls the public-shard decision, while
the local-withdrawn check only helps when this node already knows the group is
withdrawn
(`x0x@e301371:src/server/routes/named_groups.rs:1195-1210`).

Direct import has a separate authority-binding defect. It verifies the
standalone signature, then refreshes an existing direct-keyed `GroupInfo`
without checking whether `authority_agent_id` is authorized by its current
roster. The card can replace policy, metadata topic, revision/hash frontier
and withdrawal state, conditionally rewrite genesis, and insert its separately
asserted `owner_agent_id` as an Admin
(`x0x@e301371:src/server/routes/named_groups.rs:11560-11572,11630-11654,11680-11763`).
This local-ingress case requires a user or bearer-authenticated local caller;
no crafted import was executed.

Two impact bounds remain open and must travel with this evidence. The review
did not establish whether x0x's pubsub transport restricts publishers on the
global topic, so “any peer on the network” is not widened to “anyone.” It also
did not trace how a tic-tac-toe or x0x client acts on the discovery response.

## Reference implementation

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

`GroupCard` supplies the canonical-signature mechanism, not a complete
authority model. It has length-prefixed signable bytes plus ML-DSA sign and
verify methods
(`x0x@e301371:src/groups/directory.rs:31-85,88-165,169-195`), and
`import_group_card` rejects a failed signature before its membership lock and
local lookup
(`x0x@e301371:src/server/routes/named_groups.rs:11560-11572,11580-11593`).
But `verify_signature` explicitly does not establish that the signer is
authorized for the claimed group (`src/groups/directory.rs:155-165`).
Signature validity is therefore necessary, not sufficient, for frontier
adoption.

The applied-metadata `GroupCardPublished` arm uses the same permissive
empty-signature shape as the global-gossip compatibility carve-out, but without
documenting that compatibility purpose
(`src/server/routes/named_groups.rs:1162-1167,5732-5741`). It is also the less
exposed path: transport-provided `sender_hex` must be Admin-or-higher in the
selected local record and the card's stable ID must match. Tightening this arm
alone would leave the unauthenticated global-gossip admission in place and
could accidentally remove compatibility from only one of two permissive sites.

The applied-metadata path still participates in the crafted-invite chain. Its
resolver loads a direct-key match at `:4554-4588`, while invite bootstrap copies
`base_members_v2` and the claimed stable ID from the unsigned link at
`:7642-7675` and stores the stub under that direct key at `:7832-7849`. The join
path's creator helper selects a self-consistent seeded entry from that same
unsigned roster and validates only its AgentId shape
(`x0x@e301371:src/groups/invite.rs:257-305`;
`src/server/routes/named_groups.rs:7773-7790`). The attacker-authored stub can
therefore answer both the roster-authority question and the stable-ID
comparison at `:5738-5740`: the event's independent `group_id` field
(`src/server/routes/named_groups.rs:755-758`) can be `K'` for direct-key
resolution while the card's `group_id` is victim stable ID `K`.

The `verified` argument at `:4525-4533` has path-specific meaning and must not
be treated as a transport-wide authority proof. For the remote delivery paths
in this chain it has three transport origins:

- The metadata-topic listener passes `PubSubMessage::verified`
  (`src/server/routes/named_groups.rs:6371-6414`). On that path it proves an
  ML-DSA signature by the publishing AgentId: v2 decode binds the embedded key
  to the AgentId and payload
  (`x0x@e301371:src/gossip/pubsub.rs:1093-1138,1173-1206`), failed signed
  messages are dropped at `:784-791`, and senderless legacy messages are
  skipped by the listener. Every metadata event reaching the apply call at
  `:6413` therefore has `verified == true`, including a previously unknown
  attacker signing with its own key. The x0x wrapper's publish, subscribe, and
  topic-peer initialization paths consult no group roster
  (`:398-439,544-612,687-761`); for a connected gossip-plane peer, the
  invite-derived `K'` topic is not a group-authority boundary.
- The gossip-inbox direct path is also origin-key authentication,
  not an identity-cache lookup. It first requires a verified pubsub origin,
  verifies the inner DM envelope under that same origin key, rejects an inner
  sender that differs from the outer origin, then injects
  `DirectMessage::verified = true`
  (`x0x@e301371:src/dm_inbox.rs:345-350,407-428,689-700`;
  `src/direct.rs:1128-1155`). A normal x0x sender using its own valid identity
  keys can satisfy these origin checks; they do not establish authority for
  stable ID `K`. The stock named-group sender chooses this path when the
  recipient's capability is available because its wrappers leave
  `prefer_raw_quic_if_connected` at the `false` default
  (`x0x@e301371:src/dm.rs:665-681`;
  `src/server/routes/direct.rs:20-37`;
  `src/server/routes/named_groups.rs:315-329`;
  `src/lib.rs:4277-4306,4362-4455`). `require_gossip_ack` controls delivery
  acknowledgement, not path selection. This is outbound ordering, not a
  receiver-side security control: the inbox subscription loop and raw-QUIC
  receive loop operate independently
  (`x0x@e301371:src/dm_inbox.rs:304-316`;
  `src/lib.rs:8468-8634`), while the public send surface accepts a
  caller-provided `DmSendConfig` (`src/lib.rs:4169-4195`). A remote peer
  controls its own sender, so the receiver cannot use the stock ordering to
  narrow the accepted attack surface.
- The raw-QUIC receive loop independently injects frames into the same direct
  subscriber surface consumed by the named-group listener
  (`x0x@e301371:src/lib.rs:8468-8634`;
  `src/server/mod.rs:1056-1083`). Only there is `sender` self-asserted and
  `DirectMessage::verified` the identity-cache cross-binding to the
  transport-authenticated MachineId
  (`x0x@e301371:src/direct.rs:190-222`;
  `src/lib.rs:8489-8528,8624-8633`). It is the sender-authentication hinge on
  that path, not a disposable freshness annotation. But the general discovery
  cache is not a prior-trust or group-roster barrier for a previously unknown
  sender. `IdentityAnnouncement::verify` derives and verifies the MachineId
  under the announced machine key, then accepts the no-user/no-certificate arm
  without binding `agent_id` to an agent key
  (`x0x@e301371:src/lib.rs:936-998`). After timestamp, trust, revocation, and
  optional-certificate gates at `:6458-6542`, the listener discards the result
  of the stronger direct-agent-origin check and unconditionally calls
  `upsert_discovered_agent` at `:6618-6626`; that function inserts an unknown
  `agent_id` outright at `:1641-1692`. The stronger check updates the separate
  `authenticated_machine_bindings` store used by the gossip-inbox bridge
  (`:1009-1097,7333-7347`), whereas the raw-QUIC listener reads
  `identity_discovery_cache` (`:8462,8521-8528`). Therefore an unblocked,
  unrevoked attacker with no conflicting machine pin can satisfy the raw path's
  cache precondition by publishing one fresh, machine-signed announcement for
  its own AgentId and MachineId. The resulting `verified == true` authenticates
  that announced sender; it still grants no authority for stable ID `K`.

Thus all three remote producers can reach the cache consequence from a
previously unknown peer without prior roster or trust admission. The
metadata-topic and gossip-inbox paths use origin-key authentication; raw QUIC
uses the attacker-populable discovery-cache binding. In every case the sender
identity is authentic, but `sender_is_admin` obtains its group authority from
the forged roster. Invite join must reject both absent and invalid
authentication before any remote frontier is adopted, and no invite-derived
record may authorize another artifact until its authority anchor is verified.

### E. Close stable-ID collision, card writes, and destructive fan-out independently

Invite handling must test both `group_id` and `stable_group_id` for
already-joined state as it already does for withdrawn state. Concurrency
control must serialize on the claimed stable group as well as the MLS group
ID; two links with different `group_id` values but the same stable ID must not
race into two stubs.

Group-card mutation requires one authority-backed policy across all five
ingress sites. The global-gossip and applied-metadata paths cannot remain
permissive while the directory-shard, LTC, and direct-import paths are strict.
If pre-D.3 compatibility is retained, an unsigned card must be quarantined as
non-authoritative: it cannot enter current-state selection, be passed to
`supersedes`, or be returned from discovery. Every authoritative card path must
verify the signature and authorize the signer against an authenticated
same-group frontier.

A provisional or unauthenticated invite stub must never authorize a stable-ID
cache write. Today the applied-metadata arm can cache an unsigned card under
the stub's claimed stable ID at `:5744-5749`; revision and issue time select
the winner
(`src/server/routes/named_groups.rs:228-240`;
`src/groups/directory.rs:203-213`), even though `supersedes` documents that the
caller must first verify both signatures at
`src/groups/directory.rs:203-205`. Cached entries win the single-card endpoint
at `src/server/routes/named_groups.rs:11520-11539`, and discovery merges them
against synthesized local cards at `:11319-11363`. A `withdrawn` card can
instead evict the cached card at `:5746-5747`. This is a third consequence of
the forged invite stub, separate from the tombstone overwrite.

Both the metadata-topic path and the gossip-inbox direct path establish
`verified == true` from the sender's own origin signature; the raw-QUIC path
establishes it after the sender's machine-signed announcement populates the
general discovery cache. All three authenticate identity, not authority for
the victim stable ID. Static analysis did not establish a redirect consumer
for the card's `metadata_topic`; do not claim a redirect primitive.

Direct card import needs the same distinction between authentication and
authorization. A valid standalone signature proves the identity in
`authority_agent_id`, but not that identity's right to claim `group_id`. For a
non-withdrawn card whose `group_id` already keys a local record,
`import_group_card` currently updates policy, metadata topic, revisions and
withdrawal state, may replace genesis, and inserts the card's asserted owner
as an Admin at `src/server/routes/named_groups.rs:11727-11763`. The import
boundary must refuse to refresh an existing group unless an authority-backed
same-group relation authorizes the signer and frontier. Unknown groups may be
retained as quarantined discovery artifacts, but their self-asserted owner
must not become local authority until that relation is established. This
direct-import observation is also static; it requires the user or caller to
submit the crafted signed card.

Do not repair the collision by pruning
`collect_same_stable_group_aliases`. It has nine production callers and is
correctly answering which keys currently name a stable group. In the tombstone
case, `K` reaches the output alias set at
`src/server/routes/named_groups.rs:8663-8664`, `:8666-8671`, and `:9022`; the
first and third share one caller-derived source, while the map scan is the
second independent source. Removing one insertion is not a class-closing
repair.

Repair the destructive boundary at
`src/server/routes/named_groups.rs:9023-9025`: before overwriting an existing
record, require an authority-backed same-group predicate between that record
and the tombstone. Absent keys may still receive the tombstone.

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

The invite signature, symmetric join guard, authority-backed card-write guards,
and destructive tombstone guard are all required. They sever the demonstrated
chains at different boundaries and defend against different future writers.

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

1. **Contain the existing invite/card/tombstone chains.** Add the symmetric
   already-joined/locking rule, prevent provisional stubs from authorizing
   group-card writes, bind existing-group card imports to established
   authority, and add the same-group destructive write guard. These changes
   do not require new invite wire formats.
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

## Detailed validation scenarios (provisional)

These scenarios are preserved from the reviewed 551-line ADR for the loss
check. They are implementation detail, not the authoritative gate list. ADR
0001's Validation section remains held until independent review of defects 2,
3, and 4 completes; the final chapter matrix must follow that reviewed list.

Each retained scenario must fail on the reviewed behavior or on a mutation and
pass on the implementation:

1. a forged or unsigned modern invite cannot seed any base frontier field;
2. an invite with a valid signature but altered revision, roster, stable ID,
   secret epoch, binding, or expiry is rejected;
3. a self-consistent but unauthorized base hash is rejected;
4. an invite claiming an existing stable ID cannot create a competing stub,
   including two simultaneous links with different MLS IDs;
5. accepting a crafted invite and then receiving an attacker-roster-authorized
   `GroupCardPublished`, with either an empty signature or a valid signature by
   the attacker, cannot insert, replace, or evict the real stable ID's cached
   card through the metadata-topic, gossip-inbox direct, or raw-QUIC direct
   producer; the test does not treat the stock sender's path preference as a
   receiver restriction, and the raw-QUIC case first publishes an acceptable
   machine-signed identity announcement through the production listener, then
   obtains `verified == true` from the production general discovery cache
   rather than injecting a cache fixture or bypassing `verified`;
6. a self-signed card from an identity not authorized for an existing group
   cannot refresh that record, change its metadata topic or frontier, or add
   its asserted owner as an Admin;
7. accepting a crafted invite and then receiving an attacker-roster-authorized
   `GroupDeleted` cannot alter, withdraw, or clear key material from the real
   group;
8. all three legitimate tombstone callers still update every supported current
   and migrated alias, while a colliding different group remains unchanged;
9. GSS share-first and commit-first delivery converge to identical installed
   state, and neither order exposes the candidate secret before confirmation;
10. wrong tag, wrong epoch, wrong group ID, wrong roster root, relabelled AAD,
   replayed share, and stale commit all fail closed with structured reasons;
11. both GSS ban and GSS remove rotate once, deliver to every survivor, and
   exclude the removed member;
12. two concurrent TreeKEM admin removals from one parent converge on the same
    exact accepted commit digest and roster at every replica;
13. all TreeKEM survivors cross-decrypt post-resolution traffic, while removed
    members cannot decrypt it;
14. crash/restart at each sender and receiver transaction boundary restores
    either the complete previous frontier or the complete new frontier, never a
    mixed state; and
15. sender self-delivery cannot reach the mutation closure after the sender
    stored the sealed revision.

Tests must assert exact commit/binding bytes or digests, not merely equal
numeric epochs.
