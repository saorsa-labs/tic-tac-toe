import { invokeTauri } from "@/shared/api/tauri";
import {
  x0xCreateGroup,
  x0xJoinGroup,
  x0xLeaveGroup,
  x0xListGroups,
  type X0xNamedGroup,
  type X0xNamedGroupSummary,
  type X0xGroupPolicyPreset,
} from "@/shared/api/tauriNativeX0x";
import type { Community } from "./types";

const X0X_AGENT_ID = /^[0-9a-f]{64}$/;

/** Reject bech32/Nostr identifiers instead of silently treating them as AgentIds. */
export function requireAgentId(value: string): string {
  const normalized = value.trim().toLowerCase();
  if (!X0X_AGENT_ID.test(normalized)) {
    throw new Error("Enter a 64-character x0x Agent ID.");
  }
  return normalized;
}

/**
 * Keep the legacy Community render shape while making the opaque daemon group
 * id authoritative. The synthetic URL is a UI namespace only and is never
 * opened as a relay connection.
 */
export function nativeGroupToCommunity(group: X0xNamedGroupSummary): Community {
  return {
    id: group.groupId,
    groupId: group.groupId,
    name: group.name,
    relayUrl: `x0x://group/${encodeURIComponent(group.groupId)}`,
    addedAt: new Date(0).toISOString(),
  };
}

export async function listNativeCommunities(): Promise<Community[]> {
  return (await x0xListGroups()).map(nativeGroupToCommunity);
}

export async function getActiveNativeGroupId(): Promise<string> {
  const groupId = await invokeTauri<string>("x0x_get_active_group_id");
  if (!groupId.trim()) throw new Error("No native workspace is active.");
  return groupId;
}

export async function bindNativeGroup(groupId: string): Promise<void> {
  if (!groupId.trim()) throw new Error("Cannot activate an empty group id.");
  await invokeTauri("x0x_set_active_group_id", { groupId });
}

export async function createNativeCommunity(input: {
  name: string;
  description?: string;
  displayName?: string;
  preset?: X0xGroupPolicyPreset;
}): Promise<X0xNamedGroup> {
  const name = input.name.trim();
  if (!name) throw new Error("Community name is required.");
  const group = await x0xCreateGroup({ ...input, name });
  await bindNativeGroup(group.groupId);
  return group;
}

export async function joinNativeCommunity(input: {
  invite: string;
  displayName?: string;
}): Promise<X0xNamedGroup> {
  const invite = input.invite.trim();
  if (!invite.startsWith("x0x://invite/") && invite.length === 0) {
    throw new Error("Enter an x0x invite link.");
  }
  const group = await x0xJoinGroup({ ...input, invite });
  await bindNativeGroup(group.groupId);
  return group;
}

export async function leaveNativeCommunity(groupId: string): Promise<void> {
  await x0xLeaveGroup(groupId);
}
