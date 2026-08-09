import type { Channel, ChannelMember } from "@/shared/api/types";
import { normalizePubkey } from "@/shared/lib/pubkey";

type CanStartHuddleInput = {
  channel: Channel;
  currentAgentId?: string;
  selfMember: ChannelMember | null;
};

export function canStartHuddleInChannel({
  channel,
  currentAgentId,
  selfMember,
}: CanStartHuddleInput): boolean {
  if (channel.archivedAt !== null) {
    return false;
  }

  if (channel.channelType === "dm") {
    if (channel.isMember) {
      return true;
    }

    if (!currentAgentId) {
      return false;
    }

    const normalizedCurrentAgentId = normalizePubkey(currentAgentId);
    return channel.participantPubkeys.some(
      (pubkey) => normalizePubkey(pubkey) === normalizedCurrentAgentId,
    );
  }

  return channel.visibility === "open" || selfMember !== null;
}
