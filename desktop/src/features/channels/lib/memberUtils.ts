import type { ChannelMember } from "@/shared/api/types";
import { truncatePubkey } from "@/shared/lib/pubkey";

export const roleOrder: Record<ChannelMember["role"], number> = {
  owner: 0,
  admin: 1,
  member: 2,
  guest: 3,
  bot: 4,
};

export function formatMemberName(
  member: ChannelMember,
  currentAgentId?: string,
) {
  if (currentAgentId && member.pubkey === currentAgentId) {
    return "You";
  }

  return member.displayName ?? truncatePubkey(member.pubkey);
}

export function compareMembersByRole(
  left: ChannelMember,
  right: ChannelMember,
  currentAgentId?: string,
): number {
  if (currentAgentId && left.pubkey === currentAgentId) {
    return -1;
  }
  if (currentAgentId && right.pubkey === currentAgentId) {
    return 1;
  }
  const roleDelta = roleOrder[left.role] - roleOrder[right.role];
  if (roleDelta !== 0) {
    return roleDelta;
  }
  return formatMemberName(left).localeCompare(formatMemberName(right));
}
