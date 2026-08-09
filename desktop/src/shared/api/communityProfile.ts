/**
 * Community icon.
 *
 * The icon is read from the workspace via the native Tauri backend
 * (`fetch_workspace_icon`), which serves native x0x communities. The packaged
 * app has no relay transport, so there is no relay publish path to set the
 * icon — icon management is native-only.
 */

import { invokeTauri } from "@/shared/api/tauri";

/**
 * Fetch a community's icon via the Tauri backend (unauthenticated native read;
 * works for inactive communities too). Unreachable backend or no icon → null.
 */
export async function fetchCommunityIcon(
  relayUrl: string,
): Promise<string | null> {
  const icon = await invokeTauri<string | null>("fetch_workspace_icon", {
    relayUrl,
  });
  return icon || null;
}
