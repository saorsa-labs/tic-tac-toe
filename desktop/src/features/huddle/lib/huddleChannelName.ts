import type { Channel, ChannelMember } from "@/shared/api/types";
import { normalizePubkey, truncatePubkey } from "@/shared/lib/pubkey";

type BuildHuddleChannelNameInput = {
  channel: Channel;
  currentAgentId?: string;
  members?: readonly ChannelMember[];
};

function firstName(label: string): string {
  return label.trim().split(/\s+/)[0] ?? "";
}

function channelParticipantLabel(
  channel: Channel,
  pubkey: string,
  membersByPubkey: Map<string, ChannelMember>,
): string {
  const normalized = normalizePubkey(pubkey);
  const memberName = membersByPubkey.get(normalized)?.displayName?.trim();
  if (memberName) {
    return firstName(memberName);
  }

  const participantIndex = channel.participantPubkeys.findIndex(
    (participantPubkey) => normalizePubkey(participantPubkey) === normalized,
  );
  const fallbackName =
    participantIndex >= 0
      ? channel.participants[participantIndex]?.trim()
      : null;
  if (fallbackName) {
    return firstName(fallbackName);
  }

  return truncatePubkey(pubkey);
}

export function buildHuddleChannelName({
  channel,
  currentAgentId,
  members = [],
}: BuildHuddleChannelNameInput): string {
  if (channel.channelType !== "dm") {
    const channelName = channel.name.trim();
    return channelName ? `${channelName} huddle` : "huddle";
  }

  const membersByPubkey = new Map(
    members.map((member) => [normalizePubkey(member.pubkey), member]),
  );
  const normalizedCurrentAgentId = currentAgentId
    ? normalizePubkey(currentAgentId)
    : null;
  const participantPubkeys = channel.participantPubkeys;
  const orderedPubkeys =
    normalizedCurrentAgentId &&
    participantPubkeys.some(
      (pubkey) => normalizePubkey(pubkey) === normalizedCurrentAgentId,
    )
      ? [
          currentAgentId ?? "",
          ...participantPubkeys.filter(
            (pubkey) => normalizePubkey(pubkey) !== normalizedCurrentAgentId,
          ),
        ]
      : participantPubkeys;
  const names = orderedPubkeys
    .map((pubkey) => channelParticipantLabel(channel, pubkey, membersByPubkey))
    .filter(Boolean);

  if (names.length === 0) {
    return "huddle";
  }

  return `${names.join(" <> ")} huddle`;
}
