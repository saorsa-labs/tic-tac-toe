/**
 * Resolve a managed agent's avatar URL for persistence.
 *
 * Avatar hosting/upload is no longer available (no relay media transport),
 * so the URL is normalized (trimmed) and returned unchanged — hosted URLs,
 * emoji SVG data URLs, and image data URLs all render directly from their URL.
 */
export async function resolveManagedAgentAvatarUrl(
  avatarUrl: string | null | undefined,
): Promise<string | undefined> {
  return avatarUrl?.trim() || undefined;
}
