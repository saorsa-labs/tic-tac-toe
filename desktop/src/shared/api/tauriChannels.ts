import type {
  AddChannelMembersInput,
  AddChannelMembersResult,
  Channel,
  ChannelDetail,
  ChannelMember,
  ChannelType,
  CreateChannelInput,
  OpenDmInput,
  SetChannelPurposeInput,
  SetChannelTopicInput,
  UpdateChannelInput,
} from "@/shared/api/types";
import {
  x0xAddGroupMember,
  x0xCreateGroup,
  x0xGetGroup,
  x0xGetGroupMembers,
  x0xLeaveGroup,
  x0xRemoveGroupMember,
  x0xSetGroupMemberRole,
} from "@/shared/api/tauriNativeX0x";
import {
  listNativeChannels,
  projectDmChannel,
} from "@/features/channels/nativeChannelProjection";

// M3 cutover: channel admin mutations that x0xd does not expose (rename,
// archive/unarchive, topic/purpose, join/leave, nip29 member add/remove, DM
// open/hide, starter channels) have no native contract. They reject visibly
// at the boundary instead of reaching the relay/Nostr stack, and the UI
// surfaces the unavailability rather than silently succeeding.
const CHANNEL_ADMIN_UNAVAILABLE =
  "This channel admin action is unavailable in the native x0x workspace (no relay/Nostr fallback).";

export type RawChannel = {
  id: string;
  name: string;
  channel_type: ChannelType;
  visibility: "open" | "private";
  description: string;
  topic: string | null;
  purpose: string | null;
  member_count: number;
  member_pubkeys: string[];
  last_message_at: string | null;
  archived_at: string | null;
  participants: string[];
  participant_pubkeys: string[];
  is_member?: boolean;
  ttl_seconds: number | null;
  ttl_deadline: string | null;
};

type RawChannelDetail = RawChannel & {
  created_by: string;
  created_at: string;
  updated_at: string;
  topic_set_by: string | null;
  topic_set_at: string | null;
  purpose_set_by: string | null;
  purpose_set_at: string | null;
  topic_required: boolean;
  max_members: number | null;
  nip29_group_id: string | null;
};

export function fromRawChannel(channel: RawChannel): Channel {
  return {
    id: channel.id,
    name: channel.name,
    channelType: channel.channel_type,
    visibility: channel.visibility,
    description: channel.description,
    topic: channel.topic,
    purpose: channel.purpose,
    memberCount: channel.member_count,
    memberPubkeys: channel.member_pubkeys ?? [],
    lastMessageAt: channel.last_message_at,
    archivedAt: channel.archived_at,
    participants: channel.participants,
    participantPubkeys: channel.participant_pubkeys,
    isMember: channel.is_member ?? true,
    ttlSeconds: channel.ttl_seconds,
    ttlDeadline: channel.ttl_deadline,
  };
}

export function fromRawChannelDetail(channel: RawChannelDetail): ChannelDetail {
  return {
    ...fromRawChannel(channel),
    createdBy: channel.created_by,
    createdAt: channel.created_at,
    updatedAt: channel.updated_at,
    topicSetBy: channel.topic_set_by,
    topicSetAt: channel.topic_set_at,
    purposeSetBy: channel.purpose_set_by,
    purposeSetAt: channel.purpose_set_at,
    topicRequired: channel.topic_required,
    maxMembers: channel.max_members,
    nip29GroupId: channel.nip29_group_id,
  };
}

/**
 * Channel directory. M3 cutover: projected from native x0x named groups
 * (`x0x_list_groups`) — the relay `get_channels` Nostr kind:39002 fan-out is
 * gone. DM peers are not projected here because x0xd does not yet expose a DM
 * peer directory; see `nativeChannelProjection`.
 */
export async function getChannels(): Promise<Channel[]> {
  return listNativeChannels();
}

/**
 * Create a stream channel. Mapped to native `x0x_create_group` (one group per
 * channel; the group id becomes the channel id so history/send/live resolve to
 * `group:<id>`). Forum channels have no native contract and reject.
 */
export async function createChannel(
  input: CreateChannelInput,
): Promise<Channel> {
  if (input.channelType === "forum") {
    throw new Error(
      "Forum channels are unavailable in the native x0x workspace (no relay/Nostr fallback).",
    );
  }
  // Only public_open groups are creatable (secure-group crypto is not
  // approved). The channel's visibility LABEL is preserved on the returned
  // projection so solo/orientation channels (e.g. Welcome) keep their display
  // semantics; the backing native group is always a public signed group.
  const group = await x0xCreateGroup({
    name: input.name,
    description: input.description,
    preset: "public_open",
  });
  return {
    id: group.groupId,
    name: group.name || input.name,
    channelType: "stream",
    visibility: "open",
    description: group.description || input.description || "",
    topic: null,
    purpose: null,
    memberCount: group.memberCount,
    memberPubkeys: [],
    lastMessageAt: null,
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: input.ttlSeconds ?? null,
    ttlDeadline: null,
  };
}

export async function ensureStarterChannels(): Promise<Channel[]> {
  // ensure_starter_channels was a relay-only bootstrap (general/welcome). The
  // native workspace ships its own group roster; starter seeding is gone.
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function openDm(input: OpenDmInput): Promise<Channel> {
  // Native one-to-one DMs are a deterministic client projection: the channel
  // id is the peer AgentId, and history/live/send all resolve to dm:<peer>.
  // There is no daemon open-or-get route. Group-DMs (multiple recipients)
  // have no native contract — reject visibly rather than projecting one.
  if (input.pubkeys.length !== 1) {
    throw new Error(
      "Native direct messaging supports one-to-one conversations only; group DMs are unavailable without a native contract.",
    );
  }
  const peer = input.pubkeys[0].trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(peer)) {
    throw new Error("Native DM recipient must be a 64-hex x0x AgentId.");
  }
  return projectDmChannel(peer);
}

export async function hideDm(_channelId: string): Promise<void> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function getChannelDetails(
  channelId: string,
): Promise<ChannelDetail> {
  const group = await x0xGetGroup(channelId);
  return {
    id: group.groupId,
    name: group.name,
    channelType: "stream",
    visibility: "open",
    description: group.description,
    topic: null,
    purpose: null,
    memberCount: group.memberCount,
    memberPubkeys: [],
    lastMessageAt: null,
    archivedAt: null,
    participants: [],
    participantPubkeys: [],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
    createdBy: group.creator,
    createdAt: new Date(group.createdAtMs).toISOString(),
    updatedAt: new Date(group.updatedAtMs).toISOString(),
    topicSetBy: null,
    topicSetAt: null,
    purposeSetBy: null,
    purposeSetAt: null,
    topicRequired: false,
    maxMembers: null,
    nip29GroupId: null,
  };
}

export async function updateChannel(
  _input: UpdateChannelInput,
): Promise<ChannelDetail> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function setChannelTopic(
  _input: SetChannelTopicInput,
): Promise<void> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function setChannelPurpose(
  _input: SetChannelPurposeInput,
): Promise<void> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function archiveChannel(_channelId: string): Promise<void> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function unarchiveChannel(_channelId: string): Promise<void> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

export async function deleteChannel(_channelId: string): Promise<void> {
  throw new Error(CHANNEL_ADMIN_UNAVAILABLE);
}

/**
 * Channel roster. Mapped to native `x0x_get_group_members`. The agent id is
 * surfaced as the member `pubkey` so the UI's existing pubkey-keyed lookups
 * resolve against the native identity surface.
 */
export async function getChannelMembers(
  channelId: string,
): Promise<ChannelMember[]> {
  const members = await x0xGetGroupMembers(channelId);
  return members.map((member) => ({
    pubkey: member.agentId,
    role:
      member.role === "owner" || member.role === "admin"
        ? member.role
        : "member",
    isAgent: true,
    joinedAt: new Date(member.joinedAtMs).toISOString(),
    displayName: member.displayName,
  }));
}

/**
 * Add members to a channel roster. Mapped to native `x0x_add_group_member`
 * (one agent per call; the channel id is the native group id). Native groups
 * have no role-on-add — new members join as `member`, so the legacy `role`
 * field is intentionally not forwarded.
 */
export async function addChannelMembers(
  input: AddChannelMembersInput,
): Promise<AddChannelMembersResult> {
  const added: string[] = [];
  const errors: AddChannelMembersResult["errors"] = [];
  for (const agentId of input.pubkeys) {
    try {
      await x0xAddGroupMember({ groupId: input.channelId, agentId });
      added.push(agentId);
    } catch (error) {
      errors.push({
        pubkey: agentId,
        error: error instanceof Error ? error.message : "Failed to add member.",
      });
    }
  }
  return { added, errors };
}

/**
 * Remove a member from a channel roster. Mapped to native
 * `x0x_remove_group_member` (admin removal or self-leave).
 */
export async function removeChannelMember(
  channelId: string,
  pubkey: string,
): Promise<void> {
  await x0xRemoveGroupMember(channelId, pubkey);
}

/**
 * Change a member's role. Mapped to native `x0x_set_group_member_role`, whose
 * assignable roles are `admin` | `member` (ADR-0016).
 */
export async function changeChannelMemberRole(
  channelId: string,
  pubkey: string,
  role: string,
): Promise<void> {
  await x0xSetGroupMemberRole({
    groupId: channelId,
    agentId: pubkey,
    role: role === "admin" ? "admin" : "member",
  });
}

/**
 * Leave a channel. Mapped to native `x0x_leave_group`.
 */
export async function leaveChannel(channelId: string): Promise<void> {
  await x0xLeaveGroup(channelId);
}
