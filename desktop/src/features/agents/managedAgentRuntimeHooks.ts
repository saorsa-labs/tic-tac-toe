import {
  useMutation,
  useQuery,
  useQueryClient,
  type QueryClient,
} from "@tanstack/react-query";

import {
  listManagedAgentRuntimes,
  restartManagedAgentRuntime,
  startManagedAgentRuntime,
  stopManagedAgentRuntime,
} from "@/shared/api/tauriManagedAgents";
import type { ManagedAgentRuntimeStatus } from "@/shared/api/types";

export const managedAgentRuntimesQueryKey = ["managed-agent-runtimes"] as const;

export function bootstrapManagedAgentRuntimePairs(
  queryClient: QueryClient,
): void {
  void queryClient.invalidateQueries({
    queryKey: managedAgentRuntimesQueryKey,
  });
}

export function useManagedAgentRuntimesQuery(options?: { enabled?: boolean }) {
  return useQuery({
    enabled: options?.enabled ?? true,
    queryKey: managedAgentRuntimesQueryKey,
    queryFn: listManagedAgentRuntimes,
  });
}

export function useManagedAgentRuntimeAction() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      action,
      pubkey,
      groupId,
    }: {
      action: "start" | "stop" | "restart";
      pubkey: string;
      groupId: string;
    }) => {
      if (action === "stop") return stopManagedAgentRuntime(pubkey, groupId);
      if (action === "restart") {
        return restartManagedAgentRuntime(pubkey, groupId);
      }
      return startManagedAgentRuntime(pubkey, groupId);
    },
    onSuccess: (runtime) => {
      queryClient.setQueryData<ManagedAgentRuntimeStatus[]>(
        managedAgentRuntimesQueryKey,
        (current = []) => {
          const index = current.findIndex(
            (candidate) =>
              candidate.pubkey === runtime.pubkey &&
              candidate.groupId === runtime.groupId,
          );
          if (index === -1) return [...current, runtime];
          return current.map((candidate, candidateIndex) =>
            candidateIndex === index ? runtime : candidate,
          );
        },
      );
    },
  });
}
