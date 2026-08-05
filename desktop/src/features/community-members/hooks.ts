import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import {
  getActiveNativeGroupId,
  requireAgentId,
} from "@/features/communities/nativeCommunityApi";
import { getIdentity } from "@/shared/api/tauriIdentity";
import {
  x0xAddGroupMember,
  x0xBanGroupMember,
  x0xGetGroupMembers,
  x0xSetGroupMemberRole,
  type X0xGroupMember,
} from "@/shared/api/tauriNativeX0x";
import type { RelayMember, RelayMemberRole } from "@/shared/api/types";

export const relayMembersQueryKey = ["nativeGroupMembers"] as const;
export const myRelayMembershipQueryKey = ["myNativeGroupMembership"] as const;
export const myRelayMembershipLookupQueryKey = [
  "myNativeGroupMembershipLookup",
] as const;

export type NativeMemberRenderShape = RelayMember & {
  /** Daemon-owned roster display name (not a relay profile lookup). */
  displayName: string | null;
};

type NativeMembershipLookup = {
  snapshotFound: boolean;
  membershipRequired: boolean;
  membership: RelayMember | null;
};

function legacyRole(role: X0xGroupMember["role"]): RelayMemberRole {
  if (role === "owner" || role === "admin") return role;
  return "member";
}

/**
 * Adapter for existing Buzz member rows. `pubkey` contains an x0x AgentId on
 * this native-only path; inputs are validated before they reach the daemon and
 * legacy encoded identifiers are never decoded or reinterpreted.
 */
export function nativeMemberToRelayShape(
  member: X0xGroupMember,
): NativeMemberRenderShape {
  return {
    pubkey: member.agentId,
    role: legacyRole(member.role),
    addedBy: member.addedBy,
    createdAt: new Date(member.joinedAtMs).toISOString(),
    displayName: member.displayName,
  };
}

async function listNativeMembers(): Promise<NativeMemberRenderShape[]> {
  const groupId = await getActiveNativeGroupId();
  return (await x0xGetGroupMembers(groupId))
    .filter((member) => member.state === "active")
    .map(nativeMemberToRelayShape);
}

async function myNativeMembership(): Promise<NativeMemberRenderShape | null> {
  const [{ agentId }, members] = await Promise.all([
    getIdentity(),
    listNativeMembers(),
  ]);
  return members.find((member) => member.pubkey === agentId) ?? null;
}

async function myNativeMembershipLookup(): Promise<NativeMembershipLookup> {
  return {
    snapshotFound: true,
    membershipRequired: true,
    membership: await myNativeMembership(),
  };
}

async function addNativeMember(agentIdInput: string, role: string) {
  const groupId = await getActiveNativeGroupId();
  const agentId = requireAgentId(agentIdInput);
  const member = await x0xAddGroupMember({ groupId, agentId });
  if (role === "admin") {
    return x0xSetGroupMemberRole({ groupId, agentId, role: "admin" });
  }
  return member;
}

async function banNativeMember(agentIdInput: string): Promise<void> {
  const groupId = await getActiveNativeGroupId();
  await x0xBanGroupMember(groupId, requireAgentId(agentIdInput));
}

async function changeNativeMemberRole(agentIdInput: string, role: string) {
  if (role !== "admin" && role !== "member") {
    throw new Error("Native groups only allow admin or member assignment.");
  }
  const groupId = await getActiveNativeGroupId();
  return x0xSetGroupMemberRole({
    groupId,
    agentId: requireAgentId(agentIdInput),
    role,
  });
}

function invalidateMembershipQueries(
  queryClient: ReturnType<typeof useQueryClient>,
) {
  return Promise.all([
    queryClient.invalidateQueries({ queryKey: relayMembersQueryKey }),
    queryClient.invalidateQueries({ queryKey: myRelayMembershipQueryKey }),
    queryClient.invalidateQueries({
      queryKey: myRelayMembershipLookupQueryKey,
    }),
  ]);
}

export function useRelayMembersQuery(enabled = true) {
  return useQuery({
    enabled,
    queryKey: relayMembersQueryKey,
    queryFn: listNativeMembers,
    staleTime: 30_000,
  });
}

export function useMyRelayMembershipQuery() {
  return useQuery({
    queryKey: myRelayMembershipQueryKey,
    queryFn: myNativeMembership,
    staleTime: 60_000,
  });
}

export function useMyRelayMembershipLookupQuery() {
  return useQuery({
    queryKey: myRelayMembershipLookupQueryKey,
    queryFn: myNativeMembershipLookup,
    staleTime: 60_000,
  });
}

export function useAddRelayMemberMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ pubkey, role }: { pubkey: string; role: string }) =>
      addNativeMember(pubkey, role),
    onSettled: async () => {
      await invalidateMembershipQueries(queryClient);
    },
  });
}

export function useRemoveRelayMemberMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: banNativeMember,
    onSettled: async () => {
      await invalidateMembershipQueries(queryClient);
    },
  });
}

export function useChangeRelayMemberRoleMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({
      pubkey,
      role,
      newRole,
    }: {
      pubkey: string;
      role?: string;
      newRole?: string;
    }) => changeNativeMemberRole(pubkey, role ?? newRole ?? "member"),
    onSettled: async () => {
      await invalidateMembershipQueries(queryClient);
    },
  });
}
