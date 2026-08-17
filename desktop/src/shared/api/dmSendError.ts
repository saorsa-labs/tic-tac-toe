/**
 * Product copy for ADR 0030 `POST /direct/send` refusals.
 *
 * The two 409s prescribe opposite repairs. A 504 is "maybe arrived" — the
 * send path already reuses `logical_id` (the envelope clientId), so the
 * copy must not tell the user to send a *new* message.
 */
const DM_SEND_ERRORS: Record<string, string> = {
  recipient_ack_semantics_unavailable:
    "Peer needs upgrading — it can't confirm durable delivery yet.",
  idempotency_conflict:
    "That message id was already used for different content. Retrying won't help — send it as a new message.",
  recipient_key_unavailable:
    "Peer not found — no published key or contact card for that agent yet.",
  logical_id_requires_durable_ack:
    "Message id requires durable delivery; drop the id or keep durable send.",
  require_gossip_ack_removed:
    "This client sent a field the daemon removed (require_gossip_ack). Update the app.",
  timeout:
    "Delivery wasn't confirmed. The message may still have arrived — retrying keeps the same id so it will not duplicate.",
};

const CODE_PATTERN = new RegExp(
  Object.keys(DM_SEND_ERRORS)
    .map((code) => code.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"))
    .join("|"),
);

/** Map a daemon/Tauri send error to the sentence the composer should show. */
export function formatDmSendError(
  error: unknown,
  fallback = "Failed to send message.",
): string {
  const raw =
    typeof error === "string"
      ? error
      : error instanceof Error && error.message
        ? error.message
        : "";
  if (!raw) {
    return fallback;
  }
  const match = raw.match(CODE_PATTERN);
  if (match) {
    return DM_SEND_ERRORS[match[0]] ?? raw;
  }
  return raw;
}
