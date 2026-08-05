import { invokeTauri } from "@/shared/api/tauri";
import { requireAgentId } from "@/features/communities/nativeCommunityApi";
import { getActiveNativeGroupId } from "@/features/communities/nativeCommunityApi";
import { getIdentity } from "@/shared/api/tauriIdentity";
import { x0xGetGroupMembers } from "@/shared/api/tauriNativeX0x";
import type {
  PresenceLookup,
  PresenceStatus,
  Profile,
  UpdateProfileInput,
  UserSearchResult,
  UsersBatchResponse,
} from "@/shared/api/types";

export type NativeContact = {
  agentId: string;
  trustLevel: "blocked" | "unknown" | "known" | "trusted";
  label: string | null;
  addedAt: number;
  lastSeen: number | null;
};

export async function listNativeContacts(): Promise<NativeContact[]> {
  return invokeTauri<NativeContact[]>("x0x_list_contacts");
}

export async function addNativeContact(
  agentIdInput: string,
  label?: string,
): Promise<void> {
  await invokeTauri("x0x_add_contact", {
    input: {
      agentId: requireAgentId(agentIdInput),
      trustLevel: "known",
      label: label?.trim() || null,
    },
  });
}

export async function removeNativeContact(agentIdInput: string): Promise<void> {
  await invokeTauri("x0x_remove_contact", {
    agentId: requireAgentId(agentIdInput),
  });
}

function nativeProfile(agentId: string, displayName: string | null): Profile {
  return {
    pubkey: agentId,
    displayName,
    avatarUrl: null,
    about: null,
    nip05Handle: null,
    ownerPubkey: null,
    hasProfileEvent: displayName !== null && displayName.trim().length > 0,
  };
}

async function nativeDisplayNames(): Promise<Map<string, string | null>> {
  const [groupId, contacts] = await Promise.all([
    getActiveNativeGroupId(),
    listNativeContacts(),
  ]);
  const members = await x0xGetGroupMembers(groupId);
  const names = new Map(
    contacts.map((contact) => [contact.agentId, contact.label] as const),
  );
  for (const member of members) {
    if (member.displayName?.trim())
      names.set(member.agentId, member.displayName);
  }
  return names;
}

type RawAgentCardEnvelope = {
  card?: {
    agent_id?: string;
    agentId?: string;
    display_name?: string;
    displayName?: string;
  };
};

async function getNativeSelfAgentCard(): Promise<{
  agentId: string;
  displayName: string | null;
}> {
  const raw = await invokeTauri<RawAgentCardEnvelope>("x0x_get_agent_card", {
    displayName: null,
    includeGroups: false,
  });
  const card = raw.card;
  if (!card) throw new Error("x0xd returned no AgentCard.");
  const agentId = requireAgentId(card.agentId ?? card.agent_id ?? "");
  return {
    agentId,
    displayName: card.displayName ?? card.display_name ?? null,
  };
}

export async function getNativeSelfProfile(): Promise<Profile> {
  const [{ identityWords }, card, names] = await Promise.all([
    getIdentity(),
    getNativeSelfAgentCard(),
    nativeDisplayNames(),
  ]);
  return nativeProfile(
    card.agentId,
    names.get(card.agentId) ?? card.displayName ?? identityWords.join(" "),
  );
}

export async function getNativeUserProfile(
  agentIdInput: string,
): Promise<Profile> {
  const agentId = requireAgentId(agentIdInput);
  return nativeProfile(
    agentId,
    (await nativeDisplayNames()).get(agentId) ?? null,
  );
}

export async function getNativeUsersBatch(
  agentIdInputs: string[],
): Promise<UsersBatchResponse> {
  const agentIds = [...new Set(agentIdInputs.map(requireAgentId))];
  const names = await nativeDisplayNames();
  const profiles: UsersBatchResponse["profiles"] = {};
  const missing: string[] = [];
  for (const agentId of agentIds) {
    const displayName = names.get(agentId);
    if (displayName === undefined) {
      missing.push(agentId);
    } else {
      profiles[agentId] = {
        displayName,
        avatarUrl: null,
        nip05Handle: null,
        ownerPubkey: null,
        isAgent: true,
      };
    }
  }
  return { profiles, missing };
}

export async function searchNativeProfiles(
  query: string,
  limit: number,
): Promise<UserSearchResult[]> {
  const needle = query.trim().toLowerCase();
  return [...(await nativeDisplayNames()).entries()]
    .filter(
      ([agentId, label]) =>
        !needle ||
        agentId.includes(needle) ||
        label?.toLowerCase().includes(needle),
    )
    .slice(0, limit)
    .map(([agentId, displayName]) => ({
      pubkey: agentId,
      displayName,
      avatarUrl: null,
      nip05Handle: null,
      ownerPubkey: null,
      isAgent: true,
    }));
}

export async function setNativeGroupDisplayName(
  groupId: string,
  displayName: string,
): Promise<void> {
  const name = displayName.trim();
  if (!groupId.trim() || !name) throw new Error("Display name is required.");
  await invokeTauri("x0x_set_group_display_name", { groupId, name });
}

export async function updateNativeProfile(
  input: UpdateProfileInput,
): Promise<Profile> {
  if (input.avatarUrl || input.about || input.nip05Handle) {
    throw new Error("This x0xd version only exposes native display names.");
  }
  const displayName = input.displayName?.trim();
  if (!displayName) throw new Error("Display name is required.");
  await setNativeGroupDisplayName(await getActiveNativeGroupId(), displayName);
  const { agentId } = await getIdentity();
  return nativeProfile(agentId, displayName);
}

export async function getNativePresence(
  agentIds: string[],
): Promise<PresenceLookup> {
  const normalized = [...new Set(agentIds.map(requireAgentId))].sort();
  if (normalized.length === 0) return {};
  return invokeTauri<Record<string, PresenceStatus>>("x0x_get_presence", {
    agentIds: normalized,
  });
}

/**
 * x0xd derives liveness from the mesh and deliberately has no client-authored
 * presence event endpoint. Explicit away/offline writes therefore fail closed
 * instead of falling back to Nostr kind 20001.
 */
export async function setNativePresence(
  _status: PresenceStatus,
): Promise<never> {
  throw new Error("This x0xd version does not expose manual presence status.");
}
