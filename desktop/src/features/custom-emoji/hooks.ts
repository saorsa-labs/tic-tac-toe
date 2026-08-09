/**
 * React-query cache key for the custom-emoji palette.
 *
 * The relay-backed query hooks that used to live here (community palette fetch,
 * own-set read, add/remove mutations) were removed at the M3 cutover — the
 * packaged app has no relay transport and there is no native custom-emoji-set
 * API yet. This key is retained because `useReactionHandler` reads the
 * react-query cache to resolve a custom-emoji reaction's image URL for its
 * optimistic update; with the query no longer populated the lookup returns
 * undefined and the shortcode renders as text, which is the correct fallback
 * when no custom sets exist.
 */

export const customEmojiQueryKey = ["custom-emoji"] as const;
