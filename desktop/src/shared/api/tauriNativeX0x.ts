/**
 * Native x0xd typed workspace API — the single TS seam feature migration binds
 * against as the desktop moves off the relay/Nostr dialect (buzz-fork-plan
 * Stage 3; ADR-0023 durable history).
 *
 * Design contracts (frozen by x0xd + TauriNativeTransport + ThreadContractImpl):
 *
 * - **One seam.** All native workspace traffic flows through these typed
 *   functions over `invokeTauri`. No feature code calls x0xd REST/WS directly.
 * - **No Nostr kinds/tags.** Surfaces use the native vocabulary only —
 *   `scope`, `contentType`, `provenance`, `direction`, `messageClass`,
 *   `replaceKey`. Do not reintroduce `kind`/`tags`/NIP numbers here.
 * - **Explicit thread metadata, never inferred.** History rows and backfill
 *   frames carry canonical `threadRoot`/`threadParent` (lowercase-hex `msg_id`
 *   or null). Thread grouping is `GROUP BY threadRoot`; a row is a root iff
 *   `threadRoot === msgId`. Ancestry is NEVER reconstructed from payload
 *   content or delivery order.
 * - **Explicit backfill-then-live cursors.** History pages by an opaque keyset
 *   `beforeId` (newest-first). The live stream replays a bounded backfill
 *   window (oldest-first) then crosses a `{ type: "live" }` boundary into live
 *   delivery over ONE WS connection — no polling, no implicit gap-closing.
 *   Older-than-window rows are paged via `x0xHistoryList`.
 *
 * Transport status:
 * - **Frozen commands** (shipping): `x0x_history_list`, `x0x_history_search`,
 *   `x0x_publish`, and the cancellable WS-subscribe streaming surface.
 * - **Native data commands**: named-group membership is registered by
 *   `commands/native_membership.rs`; auxiliary task/store/card commands are
 *   registered by their dedicated native command module.
 *
 * Wire shape is snake_case; exported TS types are camelCase. Raw wire types and
 * `fromRaw*` mappers are kept file-local, mirroring `tauriChannels.ts`.
 */

import { Channel } from "@tauri-apps/api/core";

import { invokeTauri } from "@/shared/api/tauri";

// ─── Identity ────────────────────────────────────────────────────────────────

/** 64-char lowercase hex — an x0x `AgentId` (SHA-256 of the agent's ML-DSA-65 key). */
export type X0xAgentId = string;
/** 64-char lowercase hex — a `MachineId` (ant-quic raw pubkey hash). */
export type X0xMachineId = string;
/** 64-char lowercase hex — a `UserId`, when a human identity is linked. */
export type X0xUserId = string;

// ─── Scope ───────────────────────────────────────────────────────────────────

export type X0xScopeKind = "dm" | "group" | "topic";

/**
 * Canonical scope string used by the history REST surface and the WS backfill
 * path: `dm:<agentHex>` | `group:<stableId>` | `topic:<name>`.
 */
export type X0xScope = `dm:${string}` | `group:${string}` | `topic:${string}`;

/** Build a canonical scope. */
export function x0xScope(kind: X0xScopeKind, id: string): X0xScope {
  return `${kind}:${id}` as X0xScope;
}

/** Parse a canonical scope into its kind and identifier. */
export function parseX0xScope(scope: string): {
  kind: X0xScopeKind;
  id: string;
} {
  const at = scope.indexOf(":");
  if (at <= 0) {
    throw new Error(`invalid x0x scope: ${scope}`);
  }
  const kind = scope.slice(0, at);
  if (kind !== "dm" && kind !== "group" && kind !== "topic") {
    throw new Error(`invalid x0x scope kind: ${kind}`);
  }
  return { kind, id: scope.slice(at + 1) };
}

// ─── History row ─────────────────────────────────────────────────────────────

/**
 * Message direction relative to this node. Mirrors the Rust serde
 * representation of `history::record::Direction` (PascalCase, no rename_all).
 */
export type X0xDirection = "Inbound" | "Outbound";

/**
 * How a row's content reached the store. Mirrors the Rust serde representation
 * of `history::record::Provenance`.
 */
export type X0xProvenance =
  | "VerifiedEnvelope"
  | "LocalAppDecrypt"
  | "LocalSend";

/**
 * ADR-0023 §4 message-class taxonomy. Mirrors the Rust serde representation of
 * `history::record::MessageClass`. Ephemeral traffic never produces a row.
 */
export type X0xMessageClass = "Durable" | "Replaceable" | "Ephemeral";

/**
 * One durable (or replaceable) native history row (ADR-0023 §3).
 *
 * `payload` is base64-encoded application bytes — what a UI renders and search
 * indexes. Decode according to `contentType`; never assume UTF-8.
 *
 * Thread fields are canonical and explicit:
 * - `threadRoot === null` ⟺ legacy / no threading metadata.
 * - `threadRoot === msgId` ⟺ this row is a thread root (`threadParent === null`).
 * - otherwise `threadRoot` is the root's `msgId` and `threadParent` is the
 *   direct parent's `msgId`.
 *
 * Group a thread with `threadRoot`; do NOT reconstruct ancestry from payload.
 */
export type X0xHistoryRow = {
  /** Rowid — the keyset cursor value (`beforeId` for the next older page). */
  id: number;
  /** BLAKE3 of the signed artifact (or payload) as lowercase hex. */
  msgId: string;
  scope: X0xScope;
  authorAgent: X0xAgentId | null;
  authorMachine: X0xMachineId | null;
  /** Sender-claimed timestamp (unix ms). */
  sentAtMs: number;
  /** Local receipt timestamp (unix ms) — authoritative for ordering. */
  seenAtMs: number;
  direction: X0xDirection;
  /** MIME content type of `payload`; only `text/*` rows are FTS-indexed. */
  contentType: string;
  /** Base64-encoded decrypted application payload. */
  payload: string;
  /** True when a verbatim signed artifact exists for offline re-verification. */
  signed: boolean;
  provenance: X0xProvenance;
  /** Non-null marks the row replaceable, keyed by this string. */
  replaceKey: string | null;
  /** Canonical thread root (`msgId` hex), or null for legacy rows. */
  threadRoot: string | null;
  /** Canonical thread parent (`msgId` hex), or null on roots / legacy rows. */
  threadParent: string | null;
};

/** Snake-case wire shape returned by `x0x_history_list` / `x0x_history_search`. */
type RawX0xHistoryRow = {
  id: number;
  msg_id: string;
  scope: string;
  author_agent: string | null;
  author_machine: string | null;
  sent_at_ms: number;
  seen_at_ms: number;
  direction: X0xDirection;
  content_type: string;
  payload: string;
  signed: boolean;
  provenance: X0xProvenance;
  replace_key: string | null;
  thread_root: string | null;
  thread_parent: string | null;
};

function fromRawX0xHistoryRow(row: RawX0xHistoryRow): X0xHistoryRow {
  return {
    id: row.id,
    msgId: row.msg_id,
    scope: row.scope as X0xScope,
    authorAgent: row.author_agent,
    authorMachine: row.author_machine,
    sentAtMs: row.sent_at_ms,
    seenAtMs: row.seen_at_ms,
    direction: row.direction,
    contentType: row.content_type,
    payload: row.payload,
    signed: row.signed,
    provenance: row.provenance,
    replaceKey: row.replace_key,
    threadRoot: row.thread_root,
    threadParent: row.thread_parent,
  };
}

// ─── History list / search (FROZEN) ──────────────────────────────────────────

/** Input shared by history listing and search. */
export type X0xHistoryListInput = {
  scope: X0xScope;
  /** Inclusive lower bound on `seenAtMs`. */
  sinceMs?: number;
  /** Inclusive upper bound on `seenAtMs`. */
  untilMs?: number;
  /** Max rows (server clamps; 0 ⇒ default). */
  limit?: number;
  /**
   * Keyset cursor: return rows strictly older than this rowid. Pass the
   * `nextCursor.beforeId` from the previous page; omit for the first page.
   */
  beforeId?: number;
};

/** One newest-first history page. */
export type X0xHistoryPage = {
  rows: X0xHistoryRow[];
  /** True when more rows exist beyond this page (server signalled `has_more`). */
  hasMore: boolean;
  /**
   * Opaque keyset cursor for the next (older) page. Null when the page is
   * exhausted (`hasMore === false`) or the page was empty.
   */
  nextCursor: { beforeId: number } | null;
};

type RawX0xHistoryPage = { rows: RawX0xHistoryRow[]; has_more: boolean };

function fromRawX0xHistoryPage(page: RawX0xHistoryPage): X0xHistoryPage {
  const rows = page.rows.map(fromRawX0xHistoryRow);
  return {
    rows,
    hasMore: page.has_more,
    nextCursor:
      page.has_more && rows.length > 0
        ? { beforeId: rows[rows.length - 1].id }
        : null,
  };
}

/**
 * `x0x_history_list` — scoped durable-history listing, newest-first, keyset
 * paginated via `beforeId`.
 */
export async function x0xHistoryList(
  input: X0xHistoryListInput,
): Promise<X0xHistoryPage> {
  const raw = await invokeTauri<RawX0xHistoryPage>("x0x_history_list", {
    scope: input.scope,
    sinceMs: input.sinceMs ?? null,
    untilMs: input.untilMs ?? null,
    limit: input.limit ?? null,
    beforeId: input.beforeId ?? null,
  });
  return fromRawX0xHistoryPage(raw);
}

/** Input for FTS search within a scope. */
export type X0xHistorySearchInput = X0xHistoryListInput & {
  /** FTS needle (required). */
  q: string;
};

/**
 * `x0x_history_search` — FTS5 search over text payloads within a scope. Same
 * row/page shape as `x0xHistoryList`.
 */
export async function x0xHistorySearch(
  input: X0xHistorySearchInput,
): Promise<X0xHistoryPage> {
  const raw = await invokeTauri<RawX0xHistoryPage>("x0x_history_search", {
    scope: input.scope,
    q: input.q,
    sinceMs: input.sinceMs ?? null,
    untilMs: input.untilMs ?? null,
    limit: input.limit ?? null,
    beforeId: input.beforeId ?? null,
  });
  return fromRawX0xHistoryPage(raw);
}

// ─── Single-row lookup (canonical msg_id) ───────────────────────────────────

/**
 * `x0x_history_get` — one durable-history row by any id `/history` exposes
 * (lowercase 64-hex). Store-dedupe ids resolve by index. Canonical ADR-0029
 * group ids need `scope` (`group:<stable>`) so x0xd can scan that scope
 * (x0x #322). Sender-side LocalSend rows may still project the dedupe id
 * (x0x #321); this client does not match either id.
 *
 * Returns `null` when the id is well-formed but absent from the local store
 * (HTTP 404) — **distinct** from a transport or decode error, which rejects.
 * A malformed id rejects with the daemon's 400.
 *
 * @param msgId Exposed lowercase 64-hex message id.
 * @param scope Optional daemon scope (`group:<stable>`, `dm:<agent>`, `topic:<name>`).
 */
export async function x0xHistoryGet(
  msgId: string,
  scope?: string,
): Promise<X0xHistoryRow | null> {
  const raw = await invokeTauri<RawX0xHistoryRow | null>("x0x_history_get", {
    msgId,
    ...(scope ? { scope } : {}),
  });
  return raw ? fromRawX0xHistoryRow(raw) : null;
}

// ─── Publish / durable group send ───────────────────────────────────────────

/**
 * `x0x_publish` — publish application bytes to a true gossip **topic** scope.
 *
 * Topic-scope only: it gossips and does NOT record durable history, so it MUST
 * NOT be used for group scopes. Group sends go through `x0xSendGroupMessage`.
 */
export async function x0xPublish(input: {
  topic: string;
  /** Application payload bytes; base64-encoded on the wire. */
  payload: Uint8Array;
}): Promise<void> {
  await invokeTauri("x0x_publish", {
    topic: input.topic,
    payloadB64: bytesToBase64(input.payload),
  });
}

/**
 * `x0x_send_group_message` — durable native group send.
 *
 * The Tauri command resolves the group's confidentiality and selects the
 * daemon's authority-signed or encrypted durable send route. The optional
 * canonical message id is returned by transports that expose it.
 */
export async function x0xSendGroupMessage(input: {
  groupId: string;
  /** UTF-8 application envelope. */
  body: string;
  /** `"chat"` (default) or `"announcement"`. */
  kind?: "chat" | "announcement";
  /** Optional 64-hex canonical msg_id of the thread root. */
  threadRoot?: string | null;
  /** Optional 64-hex canonical msg_id of the direct parent. */
  threadParent?: string | null;
}): Promise<string | null> {
  return invokeTauri<string | null>("x0x_send_group_message", {
    input: {
      groupId: input.groupId,
      body: input.body,
      kind: input.kind ?? "chat",
      threadRoot: input.threadRoot ?? null,
      threadParent: input.threadParent ?? null,
    },
  });
}

/**
 * `x0x_send_direct_message` — native one-to-one direct message send.
 *
 * `POST /direct/send`: delivers base64 application bytes to a connected agent
 * over the daemon's authenticated DM path. Since x0xd 0.38.0 this is durable
 * by default (`200` = committed). A 0.37.x peer answers 409
 * `recipient_ack_semantics_unavailable`. Pass `logicalId` (the envelope
 * `clientId`) so a retry is the same request. The daemon records the
 * outbound row under `dm:<recipient>`; the canonical `msg_id`
 * (`compute_local_send_msg_id(request_id, payload)`) is reconciled with the
 * optimistic (clientId-keyed) row via the shared `localKey` once history/live
 * rehydrates — the receipt carries only `requestId`, never the canonical id.
 *
 * Optional `threadRoot`/`threadParent` are 64-hex canonical msg_ids, validated
 * to 32 bytes daemon-side. Live inbound DMs arrive over `/ws/direct` and are
 * peer-filtered by the consumer.
 */
export async function x0xSendDirectMessage(input: {
  /** Recipient AgentId (64-hex). */
  agentId: X0xAgentId;
  /** Application payload bytes; base64-encoded on the wire. */
  payload: Uint8Array;
  /** Stable sender-local identity for durable retry deduplication. */
  logicalId?: string | null;
  /** Optional 64-hex canonical msg_id of the thread root. */
  threadRoot?: string | null;
  /** Optional 64-hex canonical msg_id of the direct parent. */
  threadParent?: string | null;
}): Promise<X0xDirectSendReceipt> {
  return invokeTauri<X0xDirectSendReceipt>("x0x_send_direct_message", {
    input: {
      agentId: input.agentId,
      payloadB64: bytesToBase64(input.payload),
      logicalId: input.logicalId ?? null,
      threadRoot: input.threadRoot ?? null,
      threadParent: input.threadParent ?? null,
    },
  });
}

/**
 * Receipt returned by `POST /direct/send`. The daemon serializes the response
 * snake_case; these fields are all `#[serde(default)]`-nullable so a minimal
 * `{ ok: true }` body still parses.
 */
export type X0xDirectSendReceipt = {
  /** Whether the daemon accepted the DM into its delivery path. */
  ok: boolean;
  /** Chosen delivery path (`loopback` | `gossip_inbox` | `raw_quic` | `raw_quic_acked` | `relayed`). */
  path?: string;
  /** Retry count the DM path used before accepting. */
  retriesUsed?: number;
  /** Hex `request_id` (correlates with the canonical `msg_id` derivation). */
  requestId?: string;
};

// ─── Live subscription — backfill-then-live over one WS connection (FROZEN) ───

/**
 * Backfill window for a scoped live subscription. Only `limit` is honoured by
 * the daemon's WS `WsBackfill` (it replays stored `topic:` rows). Group/DM
 * durable history is NOT replayable on the live path — cold-load those via
 * `x0xHistoryList` (groups; the correct scope is returned on the subscription)
 * or rely on `/ws/direct` DM backfill. Cursors belong to `x0xHistoryList`.
 */
export type X0xLiveBackfill = {
  limit?: number;
};

/**
 * A live `message` frame's payload. Lighter than a full `X0xHistoryRow`: the
 * WS stream carries the live message envelope, not the stored-row metadata.
 * Thread fields are optional on live frames and may be absent — treat as
 * nullable either way.
 */
export type X0xLiveMessage = {
  topic: string;
  /** Base64-encoded application payload bytes. */
  payload: string;
  /** Sender origin (agent id when attributable), else null. */
  origin: X0xAgentId | null;
  /**
   * Canonical message id (64-hex). Present once the daemon ships the
   * `msg_id` addition on `WsOutbound::Message`; null/absent on older daemons.
   * Use this as the stable identity; fall back to a payload-derived id when
   * absent.
   */
  msgId?: string | null;
  /** Canonical thread root (`msgId` hex) when carried, else null/absent. */
  threadRoot?: string | null;
  /** Canonical thread parent (`msgId` hex) when carried, else null/absent. */
  threadParent?: string | null;
};

/**
 * A `direct_message` frame from `/ws/direct`. Mirrors the daemon's
 * `WsOutbound::DirectMessage`. `payload` is base64 application bytes;
 * `sender`/`machineId` are 64-hex ids; `receivedAt` is unix-ms.
 */
export type X0xLiveDirectMessage = {
  /** Canonical message id (64-hex) when the daemon provides it. */
  msgId?: string | null;
  /** Sender agent id (64-hex). */
  sender: X0xAgentId;
  /** Sender machine id (64-hex). */
  machineId: X0xMachineId;
  /** Base64-encoded application payload bytes. */
  payload: string;
  /** Daemon receive timestamp (unix ms). */
  receivedAt: number;
  /** Whether the daemon verified the sender's signature. */
  verified: boolean;
  /** Trust-decision string when the daemon classified the sender. */
  trustDecision?: string | null;
  /** Canonical thread root when carried, else null/absent. */
  threadRoot?: string | null;
  /** Canonical thread parent when carried, else null/absent. */
  threadParent?: string | null;
};

/**
 * Outbound WS frame union delivered over the Tauri Channel. Mirrors `WsOutbound`.
 *
 * - `connected`     — session established (carries `sessionId`/`agentId`).
 * - `message`       — a backfill-replay or live topic message (see `X0xLiveMessage`).
 * - `directMessage` — a backfill-replay or live DM (see `X0xLiveDirectMessage`).
 * - `subscribed`    — subscription ack for the requested topics.
 * - `unsubscribed`  — ack for a (transport-initiated) topic drop.
 * - `live`          — backfill→live boundary; frames after this are live.
 * - `error`         — transport error.
 */
export type X0xLiveFrame =
  | { type: "connected"; sessionId: string; agentId: X0xAgentId }
  | ({ type: "message" } & X0xLiveMessage)
  | ({ type: "direct_message" } & X0xLiveDirectMessage)
  | { type: "subscribed"; topics: string[] }
  | { type: "unsubscribed"; topics: string[] }
  | { type: "live"; topic: string }
  | { type: "error"; message: string };

/** Handle returned by `subscribeX0xLive`; `close()` tears the stream down. */
export type X0xLiveSubscription = {
  /** Close the underlying WS stream (invokes `x0x_close_live`). */
  close: () => Promise<void>;
  /**
   * The canonical durable-history scope to cold-load via `x0xHistoryList`
   * alongside this stream. Set only for groups (`group:<stableId>`, which can
   * differ from the mls id used for REST routing); absent for topic/dm, whose
   * scope the caller already holds. The WS backfill cannot read group-scoped
   * history, so cold-load it separately.
   */
  historyScope?: string;
};

/**
 * `x0x_subscribe_live` — open ONE backfill-then-live WS stream for a scope.
 *
 * The transport resolves `scope` to its live topic(s), replays the backfill
 * window oldest-first, emits a `{ type: "live" }` boundary, then streams live
 * `message` frames. `onFrame` receives every outbound frame. The returned
 * promise resolves once the subscription is established.
 */
export async function subscribeX0xLive(
  input: {
    scope: X0xScope;
    backfill?: X0xLiveBackfill;
    /** Override the live topic(s) derived from `scope` (rare). */
    topics?: string[];
  },
  onFrame: (frame: X0xLiveFrame) => void,
): Promise<X0xLiveSubscription> {
  const channel = new Channel<X0xLiveFrame>((frame) => onFrame(frame));
  const { streamId, historyScope } = await invokeTauri<{
    streamId: string;
    historyScope?: string;
  }>("x0x_subscribe_live", {
    scope: input.scope,
    topics: input.topics ?? null,
    backfill: input.backfill ? { limit: input.backfill.limit ?? null } : null,
    onFrame: channel,
  });
  return {
    close: () =>
      invokeTauri("x0x_close_live", { streamId }).then(() => undefined),
    historyScope,
  };
}

export function closeAllX0xLiveStreams(): Promise<void> {
  return invokeTauri("x0x_close_all_live").then(() => undefined);
}

// ─── Groups / members (REGISTERED NATIVE TRANSPORT) ─────────────────────────
//
// Typed surface for the named-groups roster. Commands proxy the x0xd REST
// surface (`/groups`, `/groups/:id`, `/groups/:id/members`) through
// `commands/native_membership.rs`. Per ADR-0001, the UI performs NO authority
// reconstruction — roster/crypto state is accepted only as token-authenticated
// loopback delegation (the daemon's transient bearer token); these types carry
// display data, not trust decisions. Invite mint/join is gated pending x0x
// frontier review, so no join/mint wrapper is exposed here.

/** Group role (ADR-0016). `owner`/`moderator`/`guest` parse for legacy rosters. */
export type X0xGroupRole = "owner" | "admin" | "moderator" | "member" | "guest";

/** Membership state for a roster entry. */
export type X0xGroupMemberState = "active" | "pending" | "removed" | "banned";

/**
 * Policy preset selected at group creation. Only `public_open` is creatable:
 * secure-group (MLS/GSS/TreeKEM) crypto is not approved, so the Tauri
 * `x0x_create_group` boundary refuses every other preset.
 */
export type X0xGroupPolicyPreset = "public_open";

/** One roster entry. */
export type X0xGroupMember = {
  agentId: X0xAgentId;
  userId?: X0xUserId | null;
  role: X0xGroupRole;
  state: X0xGroupMemberState;
  displayName: string | null;
  joinedAtMs: number;
  updatedAtMs: number;
  addedBy: X0xAgentId | null;
  removedBy: X0xAgentId | null;
};

/** Full named-group detail (`GET /groups/:id`). */
export type X0xNamedGroup = {
  groupId: string;
  name: string;
  description: string;
  creator: X0xAgentId;
  createdAtMs: number;
  updatedAtMs: number;
  /** Pub/sub topic a group send publishes to (`x0xPublish({ topic })`). */
  chatTopic: string;
  metadataTopic: string;
  policyRevision: number;
  rosterRevision: number;
  memberCount: number;
  members: X0xGroupMember[];
  /**
   * `policy.confidentiality` from the daemon (`"signed_public"` for creatable
   * groups). The channel projection omits any group that is not
   * `"signed_public"` so a non-public group is never laundered as an open
   * channel.
   */
  confidentiality?: string | null;
};

/** Lightweight list entry (`GET /groups`). */
export type X0xNamedGroupSummary = {
  groupId: string;
  name: string;
  description: string;
  memberCount: number;
};

export async function x0xListGroups(): Promise<X0xNamedGroupSummary[]> {
  const raw = await invokeTauri<{ groups: X0xNamedGroupSummary[] }>(
    "x0x_list_groups",
  );
  return raw.groups;
}

export async function x0xGetGroup(groupId: string): Promise<X0xNamedGroup> {
  return invokeTauri<X0xNamedGroup>("x0x_get_group", { groupId });
}

export async function x0xGetGroupMembers(
  groupId: string,
): Promise<X0xGroupMember[]> {
  const raw = await invokeTauri<{ members: X0xGroupMember[] }>(
    "x0x_get_group_members",
    { groupId },
  );
  return raw.members;
}

export async function x0xCreateGroup(input: {
  name: string;
  description?: string;
  displayName?: string;
  preset?: X0xGroupPolicyPreset;
}): Promise<X0xNamedGroup> {
  return invokeTauri<X0xNamedGroup>("x0x_create_group", {
    input: {
      name: input.name,
      description: input.description ?? "",
      displayName: input.displayName ?? null,
      preset: input.preset ?? null,
    },
  });
}

// ── Membership mutations ─────────────────────────────────────────────────────
/** `x0x_add_group_member` — add an agent to a named-group roster (public). */
export async function x0xAddGroupMember(input: {
  groupId: string;
  agentId: X0xAgentId;
  displayName?: string;
}): Promise<X0xGroupMember> {
  return invokeTauri<X0xGroupMember>("x0x_add_group_member", {
    input: {
      groupId: input.groupId,
      agentId: input.agentId,
      displayName: input.displayName ?? null,
    },
  });
}

/** `x0x_set_group_member_role` — change a member's role (Admin-or-higher only). */
export async function x0xSetGroupMemberRole(input: {
  groupId: string;
  agentId: X0xAgentId;
  /** Assignable roles: `admin` | `member` (legacy roles parse but aren't assignable). */
  role: "admin" | "member";
}): Promise<X0xGroupMember> {
  return invokeTauri<X0xGroupMember>("x0x_set_group_member_role", {
    input: {
      groupId: input.groupId,
      agentId: input.agentId,
      role: input.role,
    },
  });
}

/** `x0x_remove_group_member` — remove a member (admin) or self-leave (DELETE /groups/:id/members/:agent_id). */
export async function x0xRemoveGroupMember(
  groupId: string,
  agentId: X0xAgentId,
): Promise<void> {
  await invokeTauri("x0x_remove_group_member", {
    input: { groupId, agentId },
  });
}

/** `x0x_ban_group_member` — ban an agent from a group (rekeys survivors). */
export async function x0xBanGroupMember(
  groupId: string,
  agentId: X0xAgentId,
): Promise<void> {
  await invokeTauri("x0x_ban_group_member", {
    input: { groupId, agentId },
  });
}

/** `x0x_unban_group_member` — lift a ban. */
export async function x0xUnbanGroupMember(
  groupId: string,
  agentId: X0xAgentId,
): Promise<void> {
  await invokeTauri("x0x_unban_group_member", {
    input: { groupId, agentId },
  });
}

/** `x0x_leave_group` — leave a named group (DELETE /groups/:id). */
export async function x0xLeaveGroup(groupId: string): Promise<void> {
  await invokeTauri("x0x_leave_group", { groupId });
}

/** `x0x_update_group` — rename / redescribe a named group. */
export async function x0xUpdateGroup(input: {
  groupId: string;
  name?: string;
  description?: string;
}): Promise<X0xNamedGroup> {
  return invokeTauri<X0xNamedGroup>("x0x_update_group", {
    input: {
      groupId: input.groupId,
      name: input.name ?? null,
      description: input.description ?? null,
    },
  });
}

// ─── Task lists (CONTRACT — transport wiring pending) ────────────────────────
//
// Symphony task-list surface. Group-scoped ids use the
// `x0x.group.<group_id>.symphony.<list_id>` convention; membership is enforced
// daemon-side, not in the UI.

export type X0xTaskList = { id: string; topic: string };

export type X0xTaskAction = "claim" | "complete";

/** A task snapshot. `state` is the legacy display string. */
export type X0xTask = {
  id: string;
  title: string;
  description: string;
  /** Legacy display: "empty" | "claimed:<hex>" | "done:<hex>". */
  state: string;
  assignee: X0xAgentId | null;
  priority: number;
  claimedBy: X0xAgentId | null;
  claimedAtMs: number | null;
  completedBy: X0xAgentId | null;
  completedAtMs: number | null;
};

export type X0xTaskListPage = {
  tasks: X0xTask[];
  /**
   * Local-replica fencing precondition (opaque). Echo verbatim on the next
   * mutation; mismatch ⇒ 409. NOT a distributed CAS — two daemons at the same
   * token both accept.
   */
  fenceToken: string;
};

export async function x0xListTaskLists(): Promise<X0xTaskList[]> {
  const raw = await invokeTauri<{ task_lists: X0xTaskList[] }>(
    "x0x_list_task_lists",
  );
  return raw.task_lists;
}

export async function x0xCreateTaskList(input: {
  name: string;
  topic: string;
}): Promise<{ id: string; fenceToken: string }> {
  const raw = await invokeTauri<{
    id: string;
    fence_token: string;
  }>("x0x_create_task_list", { name: input.name, topic: input.topic });
  return { id: raw.id, fenceToken: raw.fence_token };
}

export async function x0xListTasks(listId: string): Promise<X0xTaskListPage> {
  const raw = await invokeTauri<{ tasks: X0xTask[]; fence_token: string }>(
    "x0x_list_tasks",
    { listId },
  );
  return { tasks: raw.tasks, fenceToken: raw.fence_token };
}

export async function x0xAddTask(input: {
  listId: string;
  title: string;
  description?: string;
}): Promise<X0xTask> {
  return invokeTauri<X0xTask>("x0x_add_task", {
    listId: input.listId,
    title: input.title,
    description: input.description ?? null,
  });
}

export async function x0xUpdateTask(input: {
  listId: string;
  taskId: string;
  action: X0xTaskAction;
  fenceToken?: string;
}): Promise<X0xTask> {
  return invokeTauri<X0xTask>("x0x_update_task", {
    listId: input.listId,
    taskId: input.taskId,
    action: input.action,
    fence_token: input.fenceToken ?? null,
  });
}

// ─── Agent cards (CONTRACT — transport wiring pending) ───────────────────────

/** DM transport capabilities advertised by an agent (x0x ≥ 0.18). */
export type X0xDmCapabilities = {
  maxProtocolVersion: number;
  gossipInbox: boolean;
  kemAlgorithm: string;
  maxEnvelopeBytes: number;
  /** ML-KEM-768 public key bytes (base64); empty ⇒ fall back to raw-QUIC. */
  kemPublicKey: string;
};

/** A group reference embedded in an agent card. */
export type X0xCardGroup = { name: string; inviteLink: string };

/** A store reference embedded in an agent card. */
export type X0xCardStore = { name: string; topic: string };

/**
 * Shareable identity card for an x0x agent. Encodable as
 * `x0x://agent/<base64url>`. Signed cards (x0x ≥ 0.24) carry `agentPublicKey`
 * + `signature`; legacy unsigned cards parse with both null.
 */
export type X0xAgentCard = {
  displayName: string;
  agentId: X0xAgentId;
  machineId: X0xMachineId;
  userId?: X0xUserId;
  addresses: string[];
  groups: X0xCardGroup[];
  stores: X0xCardStore[];
  /** Unix seconds when this card was generated. */
  createdAt: number;
  dmCapabilities?: X0xDmCapabilities | null;
  /** Hex ML-DSA-65 public key of the signer, on signed cards. */
  agentPublicKey?: string;
  /** Hex ML-DSA-65 signature over the card's signable bytes, on signed cards. */
  signature?: string;
};

export async function x0xGetAgentCard(
  agentId?: X0xAgentId,
): Promise<X0xAgentCard> {
  return invokeTauri<X0xAgentCard>("x0x_get_agent_card", {
    agentId: agentId ?? null,
  });
}

export async function x0xImportAgentCard(input: {
  /** `x0x://agent/<base64url>` link or raw base64url card bytes. */
  card: string;
}): Promise<X0xAgentCard> {
  return invokeTauri<X0xAgentCard>("x0x_import_agent_card", {
    card: input.card,
  });
}

// ─── Internal helpers ────────────────────────────────────────────────────────

/**
 * Base64-encode a Uint8Array in chunks (safe for large payloads — the spread
 * form blows the call stack past ~8 KB). Browser webview `btoa` is ASCII-only,
 * so bytes are expanded to a binary string first.
 */
function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 0x8000;
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize));
  }
  return btoa(binary);
}
