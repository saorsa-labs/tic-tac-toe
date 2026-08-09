import { useQuery } from "@tanstack/react-query";

import { getIdentity, getRecoveryState } from "@/shared/api/tauriIdentity";

export function useIdentityQuery(enabled = true) {
  return useQuery({
    queryKey: ["identity"],
    queryFn: getIdentity,
    enabled,
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/**
 * Boot-time recovery state — fetched BEFORE identity. When any flag is true,
 * route to the recovery screen and do not call `getIdentity` (it fail-closes
 * without a resolved AgentId).
 */
export function useRecoveryStateQuery() {
  return useQuery({
    queryKey: ["recovery-state"],
    queryFn: getRecoveryState,
    staleTime: Number.POSITIVE_INFINITY,
  });
}
