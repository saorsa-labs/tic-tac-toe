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
  x0xRemoveGroupMember,
  x0xSetGroupMemberRole,
  type X0xGroupMember,
} from "@/shared/api/tauriNativeX0x";
import type { CommunityMember, CommunityMemberRole } from "@/shared/api/types";

export const nativeMembersQueryKey = ["nativeGroupMembers"] as const;
export const myNativeMembershipQueryKey = ["myNativeGroupMembership"] as const;
export const myNativeMembershipLookupQueryKey = [
  "myNativeGroupMembershipLookup",
] as const;

export type NativeGroupMemberView = CommunityMember & {
  /** Daemon-owned roster display name. */
  displayName: string | null;
};

type NativeMembershipLookup = {
  snapshotFound: boolean;
  membershipRequired: boolean;
  membership: CommunityMember | null;
};

function communityRole(role: X0xGroupMember["role"]): CommunityMemberRole {
  if (role === "owner" || role === "admin") return role;
  return "member";
}

/** Convert the daemon roster shape into the existing member-row view model. */
export function nativeMemberToView(
  member: X0xGroupMember,
): NativeGroupMemberView {
  return {
    pubkey: member.agentId,
    role: communityRole(member.role),
    addedBy: member.addedBy,
    createdAt: new Date(member.joinedAtMs).toISOString(),
    displayName: member.displayName,
  };
}

async function listNativeMembers(): Promise<NativeGroupMemberView[]> {
  const groupId = await getActiveNativeGroupId();
  return (await x0xGetGroupMembers(groupId))
    .filter((member) => member.state === "active")
    .map(nativeMemberToView);
}

async function myNativeMembership(): Promise<NativeGroupMemberView | null> {
  const [{ agentId }, members] = await Promise.all([
    getIdentity(),
    listNativeMembers(),
  ]);
  return members.find((member) => member.pubkey === agentId) ?? null;
}

export async function myNativeMembershipLookup(): Promise<NativeMembershipLookup> {
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

async function removeNativeMember(agentIdInput: string): Promise<void> {
  const groupId = await getActiveNativeGroupId();
  await x0xRemoveGroupMember(groupId, requireAgentId(agentIdInput));
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
    queryClient.invalidateQueries({ queryKey: nativeMembersQueryKey }),
    queryClient.invalidateQueries({ queryKey: myNativeMembershipQueryKey }),
    queryClient.invalidateQueries({
      queryKey: myNativeMembershipLookupQueryKey,
    }),
  ]);
}

export function useNativeMembersQuery(enabled = true) {
  return useQuery({
    enabled,
    queryKey: nativeMembersQueryKey,
    queryFn: listNativeMembers,
    staleTime: 30_000,
  });
}

export function useMyNativeMembershipQuery() {
  return useQuery({
    queryKey: myNativeMembershipQueryKey,
    queryFn: myNativeMembership,
    staleTime: 60_000,
  });
}

export function useMyNativeMembershipLookupQuery() {
  return useQuery({
    queryKey: myNativeMembershipLookupQueryKey,
    queryFn: myNativeMembershipLookup,
    staleTime: 60_000,
  });
}

export function useAddNativeMemberMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ pubkey, role }: { pubkey: string; role: string }) =>
      addNativeMember(pubkey, role),
    onSettled: async () => {
      await invalidateMembershipQueries(queryClient);
    },
  });
}

export function useRemoveNativeMemberMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: removeNativeMember,
    onSettled: async () => {
      await invalidateMembershipQueries(queryClient);
    },
  });
}

export function useBanNativeMemberMutation() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: banNativeMember,
    onSettled: async () => {
      await invalidateMembershipQueries(queryClient);
    },
  });
}

export function useChangeNativeMemberRoleMutation() {
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
