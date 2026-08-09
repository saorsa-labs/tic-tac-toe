/**
 * Canonical pubkey normalisation.
 *
 * Hex pubkeys are case-insensitive, but callers compare them with `===`.
 * Trimming guards against stray whitespace from user input or tag parsing.
 */
export function normalizePubkey(pubkey: string): string {
  return pubkey.trim().toLowerCase();
}

/**
 * The ONE canonical compact display form for a relay pubkey: `abcd1234…wxyz`.
 *
 * A truncated relay pubkey is a recognition aid for internal/member fallback
 * naming, never an identity proof. The x0x displayed identity is the AgentId +
 * four speakable words (see `<AgentIdentity>`); this helper is retained for
 * internal relay-pubkey fallbacks and accessibility labels only. Do not
 * hand-roll `pubkey.slice(…)` display forms; `check-pubkey-truncation` fails
 * the build if one sneaks in outside this module.
 */
export function truncatePubkey(pubkey: string): string {
  if (pubkey.length <= 12) {
    return pubkey;
  }
  return `${pubkey.slice(0, 8)}…${pubkey.slice(-4)}`;
}
