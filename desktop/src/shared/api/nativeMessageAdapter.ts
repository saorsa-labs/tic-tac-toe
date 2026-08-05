/**
 * Native x0xd ↔ desktop `RelayEvent` adapter for the M3 messaging cutover.
 *
 * The native transport (`tauriNativeX0x.ts`) carries **generic bytes** — a
 * base64 `payload` plus transport metadata (`origin` agent id, `topic`,
 * server-assigned `threadRoot`/`threadParent`). The desktop rendering layer
 * (channel-window store, timeline virtualizer, reaction/thread overlays)
 * operates on `RelayEvent`. This module is the single boundary that translates
 * between the two shapes so the transport can switch to x0xd without
 * rewriting the rendering pipeline.
 *
 * # Content envelope (ADR-0023 / M3 contract)
 * The x0xd payload is application bytes. For channel messages the desktop
 * serializes a typed {@link ChannelMessageEnvelope} as UTF-8 JSON — **no Nostr
 * kinds or tags on the wire**. The adapter reconstructs the minimal tag set
 * the rendering layer expects (`h` channel, `p` author/mentions, `e` thread
 * refs) purely from typed fields + server ancestry, never from Nostr tags.
 *
 * # Thread ancestry
 * `threadRoot`/`threadParent` are server-assigned BLAKE3 `msgId` hex strings
 * (ADR-0023 §3). In M3 the native send path (`x0xPublish`) does not round-trip
 * thread metadata, so native-published messages arrive flat (`threadRoot` is
 * null). When ancestry IS present (e.g. backfill rows the daemon enriched), it
 * is carried through verbatim as `e` tags — the UI groups by `threadRoot`
 * (`threadRoot === msgId` ⟺ root) with **no ancestry inference**.
 */

import type { RelayEvent } from "@/shared/api/types";
import { KIND_STREAM_MESSAGE } from "@/shared/constants/kinds";
import type {
  X0xHistoryRow,
  X0xLiveMessage,
} from "@/shared/api/tauriNativeX0x";

// ─── Content envelope ───────────────────────────────────────────────────────

/**
 * Typed channel-message payload serialized as x0xd application bytes.
 *
 * This replaces the Nostr event shape on the native wire path. Every field is
 * explicit — there are no Nostr kinds, `h`/`p`/`e` tags, or sig fields.
 */
export type ChannelMessageEnvelope = {
  /** Message body (markdown). */
  text: string;
  /** Sender-claimed timestamp (unix ms). */
  createdAt: number;
  /**
   * Client-generated correlation id. The sender mints a UUID; receivers use it
   * as the `RelayEvent.id` so an optimistic row reconciles with the live frame
   * by identity. This is NOT a Nostr event id — it is a native correlation key.
   */
  clientId: string;
  /** Mentioned x0x AgentIds (64-hex), in send order. */
  mentions?: string[];
};

// ─── Base64 helpers (payload is base64 on the x0xd wire) ─────────────────────

/** Decode a base64 string into a UTF-8 string. */
function decodeBase64Utf8(b64: string): string {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return new TextDecoder().decode(bytes);
}

// ─── Envelope (de)serialization ──────────────────────────────────────────────

/** Serialize a content envelope to x0xd payload bytes (UTF-8 JSON → base64). */
export function encodeChannelMessageEnvelope(
  envelope: ChannelMessageEnvelope,
): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(envelope));
}

/**
 * Decode a base64 payload into a content envelope, or `null` when the bytes
 * are not a valid JSON envelope (non-message content type, malformed payload,
 * legacy rows). Callers treat `null` as "not a renderable channel message."
 */
export function decodeChannelMessageEnvelope(
  payloadB64: string,
): ChannelMessageEnvelope | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(decodeBase64Utf8(payloadB64));
  } catch {
    return null;
  }
  if (
    typeof parsed !== "object" ||
    parsed === null ||
    typeof (parsed as Record<string, unknown>).text !== "string" ||
    typeof (parsed as Record<string, unknown>).clientId !== "string"
  ) {
    return null;
  }
  const obj = parsed as Record<string, unknown>;
  return {
    text: obj.text as string,
    createdAt:
      typeof obj.createdAt === "number"
        ? (obj.createdAt as number)
        : Date.now(),
    clientId: obj.clientId as string,
    mentions: Array.isArray(obj.mentions)
      ? (obj.mentions as string[])
      : undefined,
  };
}

// ─── Tag reconstruction (rendering-layer adapter) ───────────────────────────

/**
 * Reconstruct the minimal Nostr-shaped tag set the rendering layer expects,
 * purely from typed envelope fields + server ancestry. No Nostr tags are
 * carried on the wire — these exist only inside the adapter boundary.
 */
function buildAdapterTags(input: {
  channelId: string;
  authorAgentId: string | null;
  mentions: string[] | undefined;
  threadRoot: string | null;
  threadParent: string | null;
}): string[][] {
  const tags: string[][] = [["h", input.channelId]];

  // Author identity — the x0x AgentId is the sole identity on the native path.
  if (input.authorAgentId) {
    tags.push(["p", input.authorAgentId]);
  }

  // Mentions — explicit agent ids, not relay p-tags reconstructed from kind.
  if (input.mentions) {
    for (const agentId of input.mentions) {
      if (agentId !== input.authorAgentId) {
        tags.push(["p", agentId]);
      }
    }
  }

  // Thread ancestry — verbatim server fields, never inferred. A root row has
  // threadRoot === its own msgId and threadParent === null; the UI groups by
  // threadRoot equality, not by walking e-tags.
  if (input.threadRoot) {
    tags.push(["e", input.threadRoot, "", "root"]);
    if (input.threadParent) {
      tags.push(["e", input.threadParent, "", "reply"]);
    }
  }

  return tags;
}

// ─── Frame → RelayEvent ─────────────────────────────────────────────────────

/**
 * Map a live `message` frame to a `RelayEvent` for the rendering layer.
 *
 * Returns `null` when the payload is not a renderable channel-message envelope
 * (aux content types, malformed JSON, non-message payloads) — callers skip
 * nulls rather than inserting noise into the timeline.
 */
export function liveMessageToRelayEvent(
  msg: X0xLiveMessage,
  channelId: string,
): RelayEvent | null {
  const envelope = decodeChannelMessageEnvelope(msg.payload);
  if (!envelope) {
    return null;
  }

  return {
    id: envelope.clientId,
    pubkey: msg.origin ?? "",
    created_at: Math.floor(envelope.createdAt / 1_000),
    kind: KIND_STREAM_MESSAGE,
    tags: buildAdapterTags({
      channelId,
      authorAgentId: msg.origin,
      mentions: envelope.mentions,
      threadRoot: msg.threadRoot ?? null,
      threadParent: msg.threadParent ?? null,
    }),
    content: envelope.text,
    sig: "",
  };
}

/**
 * Map a durable history row to a `RelayEvent` for the rendering layer.
 *
 * History rows carry the full stored metadata (`seenAtMs`, `contentType`,
 * `threadRoot`/`threadParent`). Non-text content types and undecodable
 * payloads map to `null` (skipped by the pager).
 */
export function historyRowToRelayEvent(
  row: X0xHistoryRow,
  channelId: string,
): RelayEvent | null {
  // Only text payloads are channel messages; other content types (binary,
  // agent-control, etc.) are not timeline rows.
  if (!row.contentType.startsWith("text/")) {
    return null;
  }

  const envelope = decodeChannelMessageEnvelope(row.payload);
  if (!envelope) {
    return null;
  }

  return {
    // Durable ancestry is keyed by x0xd's canonical msg_id, not by the
    // sender's correlation key. Keep clientId as the local render key so a
    // live/optimistic row can still reconcile when history rehydrates.
    id: row.msgId,
    localKey: envelope.clientId,
    pubkey: row.authorAgent ?? "",
    // Local receipt time is authoritative for durable-history ordering.
    created_at: Math.floor(row.seenAtMs / 1_000),
    kind: KIND_STREAM_MESSAGE,
    tags: buildAdapterTags({
      channelId,
      authorAgentId: row.authorAgent,
      mentions: envelope.mentions,
      threadRoot: row.threadRoot,
      threadParent: row.threadParent,
    }),
    content: envelope.text,
    sig: "",
  };
}

// ─── Envelope send helper ───────────────────────────────────────────────────

/**
 * Build the payload bytes for a native channel-message publish.
 *
 * The caller provides the human content; this mints the `clientId` correlation
 * key and serializes the typed envelope. Thread ancestry is NOT set here —
 * `x0xPublish` does not round-trip thread fields in M3; replies remain on the
 * relay dialect.
 */
export function buildChannelMessagePayload(input: {
  text: string;
  mentions?: string[];
}): { payload: Uint8Array; clientId: string; createdAt: number } {
  const clientId = crypto.randomUUID();
  const createdAt = Date.now();
  const envelope: ChannelMessageEnvelope = {
    text: input.text,
    createdAt,
    clientId,
    mentions: input.mentions,
  };
  return {
    payload: encodeChannelMessageEnvelope(envelope),
    clientId,
    createdAt,
  };
}
