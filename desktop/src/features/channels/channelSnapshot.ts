/**
 * Per-group cache of the last successfully fetched channel list.
 *
 * Each community mounts a fresh React-Query client, so switching communities
 * (or switching back to one just visited) starts cold and blocks the sidebar
 * on a `x0x_list_groups` round-trip. This module persists the last-known
 * channel list per native group so the sidebar can paint instantly from the
 * snapshot while the live fetch revalidates in the background.
 *
 * Keyed per native `groupId`. A group id is a stable x0x identifier (not a
 * URL), so no normalization is applied — the caller's id is the storage slot.
 */

import type { Channel } from "@/shared/api/types";

const STORAGE_KEY_PREFIX = "buzz-channels.v1";

export function channelSnapshotKey(groupId: string): string {
  return `${STORAGE_KEY_PREFIX}:${groupId}`;
}

function parseChannelSnapshot(json: unknown): Channel[] | null {
  if (typeof json !== "object" || json === null) return null;
  const obj = json as Record<string, unknown>;
  if (obj.version !== 1 || !Array.isArray(obj.channels)) return null;
  return obj.channels as Channel[];
}

/**
 * Reads the cached channel list for a group, or null when absent or malformed.
 */
export function readChannelSnapshot(groupId: string): Channel[] | null {
  try {
    const raw = window.localStorage.getItem(channelSnapshotKey(groupId));
    if (!raw) return null;
    return parseChannelSnapshot(JSON.parse(raw));
  } catch {
    return null;
  }
}

/**
 * Persists the channel list for a group. Skips the write when unchanged so the
 * 60s background refetch does not re-serialize an identical list. Non-fatal on
 * storage failure (e.g. quota exceeded).
 */
export function writeChannelSnapshot(
  groupId: string,
  channels: Channel[],
): void {
  try {
    const key = channelSnapshotKey(groupId);
    const serialized = JSON.stringify({ version: 1, channels });
    if (window.localStorage.getItem(key) === serialized) return;
    window.localStorage.setItem(key, serialized);
  } catch {
    // Storage access failures are non-fatal.
  }
}

/**
 * Removes the channel snapshot for a group. Called when a community is removed.
 */
export function removeChannelSnapshotForGroup(groupId: string): void {
  try {
    window.localStorage.removeItem(channelSnapshotKey(groupId));
  } catch {
    // Storage access failures are non-fatal.
  }
}
