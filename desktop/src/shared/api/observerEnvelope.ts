// Versioned observer envelope for the native x0x direct-message transport.
//
// Mirrors the Rust type at src-tauri/src/managed_agents/observer_envelope.rs.
// Observer telemetry (child→owner), control commands (owner→child), and control
// results (child→owner) travel as this envelope serialized to UTF-8 JSON in the
// `payload` of an authenticated PQC x0x direct message (Tauri
// `x0x_send_direct_message` owner-side; child x0xd REST producer-side).
//
// The DM plane is shared with chat. The chat adapter skips frames whose payload
// is an observer envelope (by `kind === "observer"`); the observer store adapter
// ingests only those. Cold-loaded history rows additionally carry `content_type`
// == OBSERVER_CONTENT_TYPE for a cheaper filter.
//
// Boundary parsing uses a Zod schema (this is external, wire-shaped input), not
// an inline cast — see the `ts-no-inline-cast-access` rule.

import { z } from "zod";

/** Envelope schema version. Bumped only on a breaking wire change. */
export const OBSERVER_ENVELOPE_VERSION = 1 as const;

/** `kind` discriminator; the chat DM consumer uses it to skip observer frames. */
export const OBSERVER_KIND = "observer" as const;

/** `content_type` stamped on the DM send (cold-load filter key, free-form). */
export const OBSERVER_CONTENT_TYPE = "application/vnd.buzz.observer.v1+json";

/** Frame direction/purpose. Mirrors the Rust `ObserverFrame` enum. */
export const ObserverFrameSchema = z.enum([
  "telemetry",
  "control",
  "control_result",
]);
export type ObserverFrame = z.infer<typeof ObserverFrameSchema>;

/**
 * The application payload — an ObserverEvent-shaped object. Opaque to the
 * transport; the store adapter narrows it to `ObserverEvent` at its boundary.
 */
const ObserverDataSchema = z.unknown();

/** A versioned observer frame carried as an x0x direct-message payload. */
export const ObserverEnvelopeSchema = z.object({
  v: z.number().int().nonnegative(),
  kind: z.literal(OBSERVER_KIND),
  frame: ObserverFrameSchema,
  /** 64-hex child AgentId (emitter for telemetry/result, addressee for control). */
  agent: z.string(),
  /** 64-hex owner AgentId (auth target / sender). */
  owner: z.string(),
  session: z.string().nullable().optional(),
  /** Monotonic per-(agent,session) sequence; secondary transport dedupe guard. */
  seq: z.number().int().default(0),
  ts: z.string().nullable().optional(),
  channel: z.string().nullable().optional(),
  /** Application payload (ObserverEvent-shaped). */
  data: ObserverDataSchema,
});
export type ObserverEnvelope = z.infer<typeof ObserverEnvelopeSchema>;

/**
 * Parse a DM payload string as an observer envelope.
 *
 * Returns `null` for any payload that is not a well-formed observer frame
 * (non-JSON, missing/wrong `kind`, or schema mismatch) — never throws. Used to
 * filter live `/ws/direct` frames by payload shape alone (those carry no
 * `content_type`), and to filter cold-loaded history rows when desired.
 */
export function parseObserverEnvelope(
  payload: string,
): ObserverEnvelope | null {
  let json: unknown;
  try {
    json = JSON.parse(payload);
  } catch {
    return null;
  }
  // Cheap probe before the full schema: require `kind === "observer"`. Narrow
  // with `in` so the property access is type-checked, not an unchecked cast.
  if (
    typeof json !== "object" ||
    json === null ||
    !("kind" in json) ||
    json.kind !== OBSERVER_KIND
  ) {
    return null;
  }
  const parsed = ObserverEnvelopeSchema.safeParse(json);
  return parsed.success ? parsed.data : null;
}

/** Serialize an envelope to UTF-8 JSON for use as a DM payload. */
export function encodeObserverEnvelope(envelope: ObserverEnvelope): string {
  return JSON.stringify(envelope);
}

/** Build an envelope with transport defaults filled. */
export function buildObserverEnvelope(
  frame: ObserverFrame,
  agent: string,
  owner: string,
  data: unknown,
): ObserverEnvelope {
  return {
    v: OBSERVER_ENVELOPE_VERSION,
    kind: OBSERVER_KIND,
    frame,
    agent,
    owner,
    session: null,
    seq: 0,
    ts: null,
    channel: null,
    data,
  };
}
