/**
 * Native channel directory projection.
 *
 * M3 cutover: replaces the relay kind:39002 `get_channels` query with a
 * projection from x0x named-group data. Each group the local agent belongs to
 * becomes one Buzz `Channel` whose `id` equals the `groupId`. This keeps
 * `nativeScopeForChannel` resolving to `group:<groupId>` for history, publish,
 * and live subscription — the single coherent native data path.
 *
 * Channel creation/admin mutations that x0xd does not expose (rename, archive,
 * topic, purpose, member add/remove beyond the native roster surface) remain
 * registered as Tauri commands but will fail visibly when they reach the
 * (now empty) relay URL. They are not silently faked.
 */

import {
  x0xGetGroup,
  x0xListGroups,
  type X0xNamedGroupSummary,
} from "@/shared/api/tauriNativeX0x";
import type { Channel } from "@/shared/api/types";

/**
 * Project a native named group as a Buzz channel.
 *
 * Containment: only `public_open` groups are creatable through the Tauri
 * boundary (`x0x_create_group` refuses any other preset). `listNativeChannels`
 * resolves each group's `confidentiality` via the detail command and OMITS any
 * group that is not `signed_public`, so a pre-existing non-public daemon group
 * is never laundered as an open channel — it simply does not appear. The send
 * boundary (`x0x_send_group_message`) also fails closed for non-SignedPublic
 * groups, so an omitted group is doubly unreachable.
 */
function projectGroupToChannel(group: X0xNamedGroupSummary): Channel {
  return {
    id: group.groupId,
    name: group.name || "general",
    channelType: "stream",
    visibility: "open",
    description: group.description ?? "",
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
  };
}

/**
 * List all native x0x groups as Buzz channels.
 *
 * Replaces the relay `get_channels` command. Only `signed_public` groups are
 * projectable: each group's `confidentiality` is resolved via the detail
 * command (the list summary carries no policy), and any group that is not
 * `signed_public` — or whose detail cannot be resolved — is OMITTED rather
 * than laundered as an open channel.
 */
export async function listNativeChannels(): Promise<Channel[]> {
  const groups = await x0xListGroups();
  const details = await Promise.all(
    groups.map((group) => x0xGetGroup(group.groupId).catch(() => null)),
  );
  return groups
    .filter((_, index) => details[index]?.confidentiality === "signed_public")
    .map(projectGroupToChannel);
}

export { projectGroupToChannel };

/**
 * Deterministic native DM channel projection from a single peer AgentId.
 *
 * Native one-to-one DMs have no daemon "open-or-get" route — the channel is a
 * pure client projection whose `id` is the peer AgentId (lowercased), so
 * `nativeScopeForChannel` resolves to `dm:<peer>` for history, live, and send.
 * Group-DMs (multiple recipients) have no native contract and are rejected at
 * the open boundary, never projected here.
 */
export function projectDmChannel(peerAgentId: string): Channel {
  const id = peerAgentId.toLowerCase();
  return {
    id,
    name: id,
    channelType: "dm",
    visibility: "private",
    description: "",
    topic: null,
    purpose: null,
    memberCount: 2,
    memberPubkeys: [id],
    lastMessageAt: null,
    archivedAt: null,
    participants: [id],
    participantPubkeys: [id],
    isMember: true,
    ttlSeconds: null,
    ttlDeadline: null,
  };
}
