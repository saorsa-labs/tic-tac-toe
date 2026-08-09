import type { Channel } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

export function getDmHuddleMemberPubkeys(
  channel: Channel | null,
  agentPubkeys: ReadonlySet<string> | undefined,
  currentAgentId: string | undefined,
) {
  if (channel?.channelType !== "dm" || !agentPubkeys) {
    return [];
  }

  const normalizedCurrentAgentId = currentAgentId
    ? normalizePubkey(currentAgentId)
    : null;
  const seen = new Set<string>();

  return channel.participantPubkeys.filter((pubkey) => {
    const normalizedPubkey = normalizePubkey(pubkey);
    if (
      normalizedCurrentAgentId &&
      normalizedPubkey === normalizedCurrentAgentId
    ) {
      return false;
    }

    if (!agentPubkeys.has(normalizedPubkey) || seen.has(normalizedPubkey)) {
      return false;
    }

    seen.add(normalizedPubkey);
    return true;
  });
}

export function hasOtherDmParticipant(
  channel: Channel | null,
  currentAgentId: string | undefined,
) {
  if (channel?.channelType !== "dm") {
    return false;
  }

  const normalizedCurrentAgentId = currentAgentId
    ? normalizePubkey(currentAgentId)
    : null;

  return channel.participantPubkeys.some((pubkey) => {
    const normalizedPubkey = normalizePubkey(pubkey);
    return (
      !normalizedCurrentAgentId || normalizedPubkey !== normalizedCurrentAgentId
    );
  });
}
