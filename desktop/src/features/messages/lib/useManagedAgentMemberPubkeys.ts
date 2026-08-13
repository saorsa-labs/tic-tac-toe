import * as React from "react";
import { useQuery } from "@tanstack/react-query";

import {
  expandManagedAgentMemberPubkeys,
  resolveManagedAgentNativeIdentityMap,
} from "@/shared/api/managedAgentMentionIdentity";
import type { ChannelMember, ManagedAgent } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

/** Project native child roster membership onto managed lifecycle record keys. */
export function useManagedAgentMemberPubkeys(
  agents: readonly ManagedAgent[],
  members: readonly ChannelMember[],
) {
  const nativeIdentitiesQuery = useQuery({
    queryKey: [
      "managed-agent-native-identities",
      ...agents.map((agent) => [
        normalizePubkey(agent.pubkey),
        agent.status,
        agent.lastStartedAt,
      ]),
    ],
    queryFn: () => resolveManagedAgentNativeIdentityMap(agents),
    enabled: agents.length > 0,
    staleTime: 5_000,
    refetchInterval: agents.some(
      (agent) => agent.status === "running" || agent.status === "deployed",
    )
      ? 5_000
      : false,
    refetchIntervalInBackground: false,
  });

  return React.useMemo(
    () =>
      expandManagedAgentMemberPubkeys(
        members.map((member) => member.pubkey),
        agents.map((agent) => agent.pubkey),
        nativeIdentitiesQuery.data ?? {},
      ),
    [agents, members, nativeIdentitiesQuery.data],
  );
}
