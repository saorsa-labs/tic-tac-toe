import {
  bindNativeGroup,
  getActiveNativeGroupId,
} from "@/features/communities/nativeCommunityApi";

type WelcomeGroupActivationOptions = {
  groupId: string;
  isCancelled: () => boolean;
  activate?: (groupId: string) => Promise<void>;
  readActiveGroupId?: () => Promise<string>;
};

/**
 * Run Welcome setup only after the backend has acknowledged the routed group.
 *
 * Channel navigation updates before an async Tauri command can settle. Without
 * this barrier, provisioning and `start_managed_agent` can observe the group
 * that was active on the previous screen. The read-back is intentional: a
 * resolved setter alone is not sufficient evidence for a group-scoped launch.
 */
export async function withAcknowledgedWelcomeGroup<T>(
  {
    groupId,
    isCancelled,
    activate = bindNativeGroup,
    readActiveGroupId = getActiveNativeGroupId,
  }: WelcomeGroupActivationOptions,
  run: () => Promise<T>,
): Promise<T | undefined> {
  if (isCancelled()) return undefined;

  const activeGroupId = await readActiveGroupId().catch(() => null);
  if (isCancelled()) return undefined;
  if (activeGroupId !== groupId) {
    await activate(groupId);
  }
  if (isCancelled()) return undefined;

  const acknowledgedGroupId = await readActiveGroupId();
  if (acknowledgedGroupId !== groupId) {
    throw new Error(
      `Welcome group activation was not acknowledged (expected ${groupId}, received ${acknowledgedGroupId}).`,
    );
  }
  if (isCancelled()) return undefined;

  return run();
}
