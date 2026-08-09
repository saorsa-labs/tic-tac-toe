import * as React from "react";

import {
  DEFAULT_STORE,
  mutedChannelIdsFromStore,
  readChannelMutesStore,
  storageKey,
  writeChannelMutesStore,
  type ChannelMuteEntry,
  type ChannelMuteStore,
} from "./channelMutesStorage";

/**
 * Per-identity channel mute state, persisted to localStorage and mirrored
 * across tabs/windows via the storage event. The packaged app has no relay
 * transport, so mute state is local-only — there is no cross-device sync.
 */
export function useChannelMutes(pubkey: string | undefined): {
  mutedChannelIds: Set<string>;
  muteChannel: (channelId: string) => void;
  unmuteChannel: (channelId: string) => void;
} {
  const [store, setStore] = React.useState<ChannelMuteStore>(() => {
    if (!pubkey) {
      return DEFAULT_STORE;
    }
    return readChannelMutesStore(pubkey);
  });

  // Reload from storage when the active identity changes.
  React.useEffect(() => {
    if (!pubkey) {
      setStore(DEFAULT_STORE);
      return;
    }
    setStore(readChannelMutesStore(pubkey));
  }, [pubkey]);

  // Mirror changes made in other tabs/windows via the storage event.
  React.useEffect(() => {
    if (!pubkey) {
      return;
    }
    const key = storageKey(pubkey);
    const handler = (e: StorageEvent) => {
      if (e.key !== key) {
        return;
      }
      setStore(readChannelMutesStore(pubkey));
    };
    window.addEventListener("storage", handler);
    return () => {
      window.removeEventListener("storage", handler);
    };
  }, [pubkey]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: store.channels is the relevant dep
  const mutedChannelIds = React.useMemo(
    () => mutedChannelIdsFromStore(store),
    [store.channels],
  );

  const setMuteState = React.useCallback(
    (channelId: string, muted: boolean) => {
      if (!pubkey) return;
      const entry: ChannelMuteEntry = {
        muted,
        updatedAt: Math.floor(Date.now() / 1000),
      };
      setStore((prev) => {
        const next: ChannelMuteStore = {
          version: 1,
          channels: { ...prev.channels, [channelId]: entry },
        };
        if (!writeChannelMutesStore(pubkey, next)) return prev;
        return next;
      });
    },
    [pubkey],
  );

  const muteChannel = React.useCallback(
    (channelId: string) => setMuteState(channelId, true),
    [setMuteState],
  );
  const unmuteChannel = React.useCallback(
    (channelId: string) => setMuteState(channelId, false),
    [setMuteState],
  );

  return {
    mutedChannelIds,
    muteChannel,
    unmuteChannel,
  };
}
