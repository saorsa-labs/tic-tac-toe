import * as React from "react";

import {
  useOpenDmMutation,
  useUpsertCachedChannel,
} from "@/features/channels/hooks";
import type { Channel } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

export function usePrepareDmSendChannel(
  activeChannel: Channel | null,
  currentAgentId?: string,
) {
  const openDmMutation = useOpenDmMutation();
  const upsertCachedChannel = useUpsertCachedChannel();

  return React.useCallback(
    async (additionalParticipantPubkeys: string[] = []) => {
      if (activeChannel?.channelType !== "dm") {
        return activeChannel?.id ?? null;
      }

      const currentParticipantPubkeys = new Set(
        activeChannel.participantPubkeys.map(normalizePubkey),
      );
      const requiresExpandedDm = additionalParticipantPubkeys.some(
        (pubkey) => !currentParticipantPubkeys.has(normalizePubkey(pubkey)),
      );
      if (!requiresExpandedDm) {
        return activeChannel.id;
      }

      const currentNormalizedPubkey = currentAgentId
        ? normalizePubkey(currentAgentId)
        : null;
      const pubkeys = [
        ...new Set(
          [
            ...activeChannel.participantPubkeys,
            ...additionalParticipantPubkeys,
          ].map(normalizePubkey),
        ),
      ].filter((pubkey) => pubkey && pubkey !== currentNormalizedPubkey);
      const expandedDm = await openDmMutation.mutateAsync({ pubkeys });
      await upsertCachedChannel(expandedDm);
      return expandedDm.id;
    },
    [
      activeChannel,
      currentAgentId,
      openDmMutation.mutateAsync,
      upsertCachedChannel,
    ],
  );
}
