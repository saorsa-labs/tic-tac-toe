import { normalizePubkey } from "@/shared/lib/pubkey";

const PUBKEY_HEX_RE = /^[0-9a-f]{64}$/i;

function normalizeValidPubkey(pubkey: string | null | undefined) {
  if (!pubkey) {
    return null;
  }

  const normalized = normalizePubkey(pubkey);
  return PUBKEY_HEX_RE.test(normalized) ? normalized : null;
}

function getTaggedPubkey(
  tags: string[][],
  tagName: string,
  options?: {
    firstTagOnly?: boolean;
  },
) {
  const candidates = options?.firstTagOnly ? tags.slice(0, 1) : tags;

  for (const tag of candidates) {
    const taggedPubkey = tag[0] === tagName ? tag[1]?.toLowerCase() : null;
    if (taggedPubkey && PUBKEY_HEX_RE.test(taggedPubkey)) {
      return taggedPubkey;
    }
  }

  return null;
}

type AuthorResolutionEvent = {
  id: string;
  pubkey: string;
  created_at: number;
  kind: number;
  tags: string[][];
  content: string;
  sig: string;
};

/**
 * Signature validity is enforced server-side: the Rust archive pipeline
 * (`desktop/src-tauri/src/archive/pipeline.rs`) calls `event.verify_signature()`
 * on every event at ingestion and drops invalid events before they reach the
 * frontend. Events arriving here are therefore already signature-verified, so
 * the attribution logic below trusts that precondition rather than re-verifying
 * on the client.
 */

export function resolveEventAuthorPubkey(input: {
  event: AuthorResolutionEvent;
  preferActorTag?: boolean;
  relaySelfPubkey?: string | null;
  requireChannelTagForPTags?: boolean;
}) {
  const {
    event,
    preferActorTag = false,
    relaySelfPubkey,
    requireChannelTagForPTags = false,
  } = input;

  const signerPubkey = normalizePubkey(event.pubkey);
  const normalizedRelaySelf = normalizeValidPubkey(relaySelfPubkey);

  // `actor` and author-attributing `p` tags are delegated authorship claims.
  // The relay creates these for workflow-generated and legacy relay-signed
  // attributed events, so they are only authoritative when the event is signed
  // by the active relay advertised in NIP-11. Missing or malformed relay
  // identity data must leave the signer as the visible author.
  if (!normalizedRelaySelf || signerPubkey !== normalizedRelaySelf) {
    return signerPubkey;
  }

  let attributedPubkey: string | null = null;
  if (preferActorTag) {
    attributedPubkey = getTaggedPubkey(event.tags, "actor");
  }

  if (!attributedPubkey) {
    const canUseAttributedPTag =
      !requireChannelTagForPTags || event.tags.some((tag) => tag[0] === "h");
    if (canUseAttributedPTag) {
      attributedPubkey = getTaggedPubkey(event.tags, "p", {
        firstTagOnly: true,
      });
    }
  }

  if (!attributedPubkey) {
    return signerPubkey;
  }

  return attributedPubkey;
}
