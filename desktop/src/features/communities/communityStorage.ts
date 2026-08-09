import { homeDir } from "@tauri-apps/api/path";
import { setLocalStorageItemWithRecovery } from "@/shared/lib/localStorageQuota";

const ACTIVE_COMMUNITY_KEY = "buzz-active-community-id";

/**
 * Expand a leading `~` to the user's home directory. The backend rejects
 * `~`-prefixed paths (`std::fs` does not expand the shell tilde), so the UI
 * resolves it before save. Returns non-`~` input unchanged. Empty/whitespace
 * input returns `undefined` so callers can clear the override.
 */
export async function expandTilde(input: string): Promise<string | undefined> {
  const trimmed = input.trim();
  if (!trimmed) return undefined;
  if (trimmed === "~") return homeDir();
  if (trimmed.startsWith("~/")) {
    const home = await homeDir();
    const base = home.endsWith("/") ? home.slice(0, -1) : home;
    return `${base}/${trimmed.slice(2)}`;
  }
  return trimmed;
}

export function clearCommunityStorage(storage: Storage = localStorage): void {
  storage.removeItem(ACTIVE_COMMUNITY_KEY);
}

export function loadActiveCommunityId(
  storage: Storage = localStorage,
): string | null {
  return storage.getItem(ACTIVE_COMMUNITY_KEY);
}

export function saveActiveCommunityId(
  id: string,
  storage: Storage = localStorage,
): boolean {
  if (typeof localStorage !== "undefined" && storage === localStorage) {
    return setLocalStorageItemWithRecovery(ACTIVE_COMMUNITY_KEY, id);
  }
  try {
    storage.setItem(ACTIVE_COMMUNITY_KEY, id);
    return true;
  } catch {
    return false;
  }
}
