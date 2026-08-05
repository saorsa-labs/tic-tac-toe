import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";

import { getOsIdleSeconds } from "@/shared/api/osIdle";
import {
  getNativePresence,
  setNativePresence,
} from "@/features/profile/nativeSocialApi";
import {
  mergePresenceUpdate,
  presenceQueryWantsPubkey,
  resolveAutomaticPresenceStatus,
} from "@/features/presence/lib/presence";
import type { PresenceLookup, PresenceStatus } from "@/shared/api/types";

const PRESENCE_STATUS_TICK_INTERVAL_MS = 30_000;
const PRESENCE_ACTIVITY_THROTTLE_MS = 1_000;
const PRESENCE_TTL_SECONDS = 90;
const PRESENCE_PREFERENCE_STORAGE_KEY = "buzz-presence-preference";

type PresencePreference = "auto" | "away" | "offline" | null;

function normalizePubkeys(agentIds: string[]) {
  return [...new Set(agentIds.map((agentId) => agentId.trim().toLowerCase()))]
    .filter((agentId) => agentId.length > 0)
    .sort();
}

function presenceQueryKey(pubkeys: string[]) {
  return ["presence", ...normalizePubkeys(pubkeys)] as const;
}

function presencePreferenceStorageKey(pubkey: string) {
  return `${PRESENCE_PREFERENCE_STORAGE_KEY}:${pubkey}`;
}

function readStoredPresencePreference(pubkey: string): PresencePreference {
  if (typeof window === "undefined" || pubkey.length === 0) {
    return null;
  }

  const value = window.localStorage.getItem(
    presencePreferenceStorageKey(pubkey),
  );
  return value === "auto" || value === "away" || value === "offline"
    ? value
    : null;
}

function writeStoredPresencePreference(
  pubkey: string,
  preference: PresencePreference,
) {
  if (typeof window === "undefined" || pubkey.length === 0) {
    return;
  }

  if (preference === null) {
    window.localStorage.removeItem(presencePreferenceStorageKey(pubkey));
    return;
  }

  window.localStorage.setItem(presencePreferenceStorageKey(pubkey), preference);
}

function resolveAutomaticPresenceStatusSync(
  lastActivityAt: number,
  now: number,
): PresenceStatus {
  return resolveAutomaticPresenceStatus(null, lastActivityAt, now);
}

export function usePresenceQuery(
  pubkeys: string[],
  options?: {
    enabled?: boolean;
  },
) {
  const normalizedPubkeys = normalizePubkeys(pubkeys);
  const enabled = (options?.enabled ?? true) && normalizedPubkeys.length > 0;
  return useQuery<PresenceLookup>({
    enabled,
    queryKey: presenceQueryKey(normalizedPubkeys),
    queryFn: () => getNativePresence(normalizedPubkeys),
    staleTime: 30_000,
    refetchInterval: 60_000,
  });
}

/** Poll the daemon-owned presence frontier; no RelayEvent is emitted or parsed. */
export function usePresenceSubscription() {
  const queryClient = useQueryClient();
  React.useEffect(() => {
    const interval = window.setInterval(() => {
      void queryClient.invalidateQueries({ queryKey: ["presence"] });
    }, 30_000);
    return () => window.clearInterval(interval);
  }, [queryClient]);
}

export function useSetPresenceMutation(pubkey?: string) {
  const queryClient = useQueryClient();
  const normalizedPubkey = pubkey?.trim().toLowerCase() ?? "";

  return useMutation({
    mutationFn: async (status: PresenceStatus) => {
      await setNativePresence(status);
      return { status, ttlSeconds: PRESENCE_TTL_SECONDS };
    },
    onSuccess: ({ status }) => {
      if (normalizedPubkey.length === 0) return;
      queryClient.setQueriesData<PresenceLookup>(
        {
          queryKey: ["presence"],
          predicate: (query) =>
            presenceQueryWantsPubkey(query.queryKey, normalizedPubkey),
        },
        (old) => mergePresenceUpdate(old, normalizedPubkey, status),
      );
    },
  });
}

export function usePresenceSession(pubkey?: string) {
  const normalizedPubkey = pubkey?.trim().toLowerCase() ?? "";
  const presenceQuery = usePresenceQuery(
    normalizedPubkey.length > 0 ? [normalizedPubkey] : [],
    { enabled: normalizedPubkey.length > 0 },
  );
  const setPresenceMutation = useSetPresenceMutation(normalizedPubkey);
  const [presencePreference, setPresencePreference] =
    React.useState<PresencePreference>(() =>
      readStoredPresencePreference(normalizedPubkey),
    );
  // Activity is tracked in a REF, and React state holds only the DERIVED
  // status. The previous shape stored raw `Date.now()` timestamps in state
  // and bumped them from a capture-phase `keydown`/`pointerdown` listener —
  // which re-rendered this hook's host (AppShell, the app root) on every
  // keystroke the user typed anywhere in the app. Presence only needs a
  // render when the automatic status actually flips (online <-> away), so
  // activity updates the ref and re-derives; setState fires on transitions
  // only (see typing-latency.perf.ts / keystroke input-to-paint).
  const lastActivityAtRef = React.useRef(Date.now());
  const [automaticStatus, setAutomaticStatus] = React.useState<PresenceStatus>(
    () =>
      resolveAutomaticPresenceStatusSync(lastActivityAtRef.current, Date.now()),
  );
  const automaticStatusRef = React.useRef(automaticStatus);

  const applyAutomaticStatus = React.useEffectEvent((next: PresenceStatus) => {
    if (next !== automaticStatusRef.current) {
      automaticStatusRef.current = next;
      setAutomaticStatus(next);
    }
  });

  const reevaluateAutomaticStatus = React.useEffectEvent(() => {
    applyAutomaticStatus(
      resolveAutomaticPresenceStatusSync(lastActivityAtRef.current, Date.now()),
    );
  });

  // OS-wide idle is authoritative when available; async, so tick-driven only.
  const reevaluateFromOsIdle = React.useEffectEvent(async () => {
    const osIdleSeconds = await getOsIdleSeconds().catch(() => null);
    if (typeof osIdleSeconds === "number") {
      // Keep the fallback clock consistent so a later null reading can't flap.
      lastActivityAtRef.current = Math.max(
        lastActivityAtRef.current,
        Date.now() - osIdleSeconds * 1000,
      );
    }
    applyAutomaticStatus(
      resolveAutomaticPresenceStatus(
        typeof osIdleSeconds === "number" ? osIdleSeconds : null,
        lastActivityAtRef.current,
        Date.now(),
      ),
    );
  });

  React.useEffect(() => {
    setPresencePreference(readStoredPresencePreference(normalizedPubkey));
    lastActivityAtRef.current = Date.now();
    reevaluateAutomaticStatus();
  }, [normalizedPubkey]);

  React.useEffect(() => {
    writeStoredPresencePreference(normalizedPubkey, presencePreference);
  }, [normalizedPubkey, presencePreference]);

  const recordActivity = React.useEffectEvent(() => {
    lastActivityAtRef.current = Date.now();
    reevaluateAutomaticStatus();
  });

  React.useEffect(() => {
    if (normalizedPubkey.length === 0) {
      return;
    }

    // Fallback activity signal: wheel/pointermove count (passive reading),
    // throttled to 1/s; window visibility never affects presence.
    let lastRecordedAt = 0;

    function handleUserActivity() {
      const now = Date.now();
      if (now - lastRecordedAt < PRESENCE_ACTIVITY_THROTTLE_MS) {
        return;
      }
      lastRecordedAt = now;
      recordActivity();
    }

    window.addEventListener("pointerdown", handleUserActivity, true);
    window.addEventListener("pointermove", handleUserActivity, true);
    window.addEventListener("wheel", handleUserActivity, {
      capture: true,
      passive: true,
    });
    window.addEventListener("keydown", handleUserActivity, true);
    window.addEventListener("focus", handleUserActivity);

    return () => {
      window.removeEventListener("pointerdown", handleUserActivity, true);
      window.removeEventListener("pointermove", handleUserActivity, true);
      window.removeEventListener("wheel", handleUserActivity, {
        capture: true,
      });
      window.removeEventListener("keydown", handleUserActivity, true);
      window.removeEventListener("focus", handleUserActivity);
    };
  }, [normalizedPubkey]);

  React.useEffect(() => {
    if (normalizedPubkey.length === 0) {
      return;
    }

    void reevaluateFromOsIdle();
    const intervalId = window.setInterval(() => {
      void reevaluateFromOsIdle();
    }, PRESENCE_STATUS_TICK_INTERVAL_MS);

    return () => {
      window.clearInterval(intervalId);
    };
  }, [normalizedPubkey]);
  const currentStatus =
    normalizedPubkey.length === 0
      ? "offline"
      : presencePreference === "offline"
        ? "offline"
        : presencePreference === "away"
          ? "away"
          : presencePreference === "auto"
            ? automaticStatus
            : automaticStatus;

  const updatePresence = React.useCallback(
    async (status: PresenceStatus) => {
      const previousPreference = presencePreference;
      const nextPreference: PresencePreference =
        status === "online" ? "auto" : status;

      if (nextPreference === "auto") {
        lastActivityAtRef.current = Date.now();
        reevaluateAutomaticStatus();
      }

      setPresencePreference(nextPreference);
      try {
        await setPresenceMutation.mutateAsync(status);
      } catch (error) {
        setPresencePreference(previousPreference);
        throw error;
      }
    },
    [presencePreference, setPresenceMutation],
  );

  return {
    currentStatus,
    isLoading: presenceQuery.isLoading,
    isPending: setPresenceMutation.isPending,
    error:
      setPresenceMutation.error instanceof Error
        ? setPresenceMutation.error
        : presenceQuery.error instanceof Error
          ? presenceQuery.error
          : null,
    setStatus: updatePresence,
  };
}
