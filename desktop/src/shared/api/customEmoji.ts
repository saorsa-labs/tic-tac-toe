/**
 * Pure parsing/render helpers for NIP-30 custom emoji (kind:30030 sets).
 *
 * The packaged app has no relay transport, so there is no community-palette
 * fetch or own-set publish here — this module only parses `["emoji", shortcode,
 * url]` tags and resolves reaction shortcodes to image URLs. Message bodies
 * carry their emoji tags inline, so rendering still works; the only thing gone
 * is the live community palette (the union of every member's kind:30030), which
 * had no native equivalent at cutover time.
 */

import type { RelayEvent } from "@/shared/api/types";
import type { CustomEmoji } from "@/shared/lib/remarkCustomEmoji";

/** NIP-30 emoji set (parameterized-replaceable). */
export const KIND_EMOJI_SET = 30030;

/** d-tag for a member's own custom emoji set. */
export const CUSTOM_EMOJI_SET_D_TAG = "buzz:custom-emoji";

/**
 * Resolve the image URL for a reaction whose content is a custom-emoji
 * `:shortcode:`, from a known set. Returns undefined for unicode reactions or
 * unknown shortcodes (the kind:7 then carries no emoji tag). With no community
 * palette available, `set` is typically the message's own emoji tags or
 * undefined — either way unknown shortcodes safely render as text.
 */
export function reactionEmojiUrl(
  emoji: string,
  set: ReadonlyArray<CustomEmoji> | undefined,
): string | undefined {
  if (!set || !emoji.startsWith(":") || !emoji.endsWith(":")) return undefined;
  const shortcode = emoji.slice(1, -1).toLowerCase();
  return set.find((e) => e.shortcode === shortcode)?.url;
}

/** NIP-30 shortcode chars. Matches the relay's `[A-Za-z0-9_-]` validation. */
const SHORTCODE_RE = /^[a-z0-9_-]+$/;

/**
 * Normalize a shortcode the same way the relay does: strip surrounding colons
 * and lowercase. Returns null if the result is empty or has invalid chars.
 */
export function normalizeShortcode(raw: string): string | null {
  const stripped = raw.trim().replace(/^:+/, "").replace(/:+$/, "");
  const lower = stripped.toLowerCase();
  return SHORTCODE_RE.test(lower) ? lower : null;
}

/**
 * Suggest a valid custom-emoji shortcode from an uploaded filename.
 * Mirrors Slack's file-first flow: strip the extension, lowercase, and collapse
 * runs of invalid characters into a single underscore.
 */
export function suggestShortcodeFromFilename(filename: string): string | null {
  const basename = filename
    .trim()
    .replace(/^.*[/\\]/, "")
    .replace(/\.[^.]*$/, "");
  const suggested = basename
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "_")
    .replace(/_+/g, "_")
    .replace(/^[_-]+|[_-]+$/g, "");
  return normalizeShortcode(suggested);
}

/**
 * Parse NIP-30 `["emoji", shortcode, url]` tags from a single event into a
 * custom-emoji list. Shortcodes are normalized; malformed/duplicate entries
 * within the one event are skipped (first wins).
 */
export function customEmojiFromTags(
  tags: ReadonlyArray<ReadonlyArray<string>>,
): CustomEmoji[] {
  const seen = new Set<string>();
  const emoji: CustomEmoji[] = [];

  for (const tag of tags) {
    const [name, rawShortcode, url] = tag;
    if (name !== "emoji") continue;
    if (!rawShortcode || !url) continue;
    const shortcode = normalizeShortcode(rawShortcode);
    if (!shortcode) continue;
    if (seen.has(shortcode)) continue;
    seen.add(shortcode);
    emoji.push({ shortcode, url });
  }

  return emoji;
}

export function customEmojiFromEvent(event: RelayEvent | null): CustomEmoji[] {
  if (!event) return [];
  return customEmojiFromTags(event.tags);
}

/**
 * Union several kind:30030 sets into one palette, collapsed to a single entry
 * per shortcode. When sets disagree on a shortcode's URL, the most recently
 * published set wins (`created_at` is signed event data, so this is as
 * deterministic and fetch-order-independent as any pure function of the
 * events); equal timestamps tie-break to the lexicographically-smallest URL so
 * the same inputs always yield the same palette. Output is sorted by shortcode.
 */
export function unionCustomEmoji(
  events: ReadonlyArray<RelayEvent>,
): CustomEmoji[] {
  const byShortcode = new Map<string, { url: string; createdAt: number }>();
  for (const event of events) {
    for (const { shortcode, url } of customEmojiFromTags(event.tags)) {
      const winner = byShortcode.get(shortcode);
      if (
        winner === undefined ||
        event.created_at > winner.createdAt ||
        (event.created_at === winner.createdAt && url < winner.url)
      ) {
        byShortcode.set(shortcode, { url, createdAt: event.created_at });
      }
    }
  }
  return [...byShortcode]
    .map(([shortcode, { url }]) => ({ shortcode, url }))
    .sort((a, b) => a.shortcode.localeCompare(b.shortcode));
}
