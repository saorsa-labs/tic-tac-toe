import { useQueryClient } from "@tanstack/react-query";
import * as React from "react";

import {
  cacheReconciledManagedAgentRuntimes,
  managedAgentRuntimesQueryKey,
} from "@/features/agents/managedAgentRuntimeHooks";
import { reconcileManagedAgentRuntimes } from "@/shared/api/tauriManagedAgents";
import type { ManagedAgentRuntimeStatus } from "@/shared/api/types";

/** Capped retry backoff: 5s, then 30s, then 2m, then stop. */
const RETRY_BACKOFF_MS = [5_000, 30_000, 120_000] as const;

export function reconcileRetryDelayMs(failureCount: number): number | null {
  if (failureCount < 1) return null;
  return RETRY_BACKOFF_MS[failureCount - 1] ?? null;
}

/**
 * Native group IDs are opaque and case-sensitive. Trim boundary whitespace,
 * reject empty IDs, and otherwise deduplicate only exact matches.
 */
export function canonicalCommunityGroupIds(
  communities: readonly { groupId: string }[],
): string[] {
  const seen = new Set<string>();
  const groupIds: string[] = [];
  for (const community of communities) {
    const groupId = community.groupId.trim();
    if (!groupId || seen.has(groupId)) continue;
    seen.add(groupId);
    groupIds.push(groupId);
  }
  return groupIds;
}

export function pendingReconcileGroupIds(
  groupIds: readonly string[],
  reconciled: ReadonlySet<string>,
  inFlight: ReadonlySet<string>,
  retryAt: ReadonlyMap<string, number>,
  exhausted: ReadonlySet<string>,
  now: number,
): string[] {
  return groupIds.filter((groupId) => {
    if (
      reconciled.has(groupId) ||
      inFlight.has(groupId) ||
      exhausted.has(groupId)
    ) {
      return false;
    }
    return (retryAt.get(groupId) ?? 0) <= now;
  });
}

/**
 * Split a reconcile batch into successful and retryable group IDs.
 * Reconcile rows can carry a daemon-canonical `groupId`, so failures are
 * attributed through the exact `requestedGroupId` echoed by the command.
 */
export function classifyReconcileResult(
  attempted: readonly string[],
  rows: readonly ManagedAgentRuntimeStatus[] | null,
): { succeeded: string[]; failed: string[] } {
  if (rows === null) return { succeeded: [], failed: [...attempted] };

  const attemptedSet = new Set(attempted);
  const failedGroupIds = new Set<string>();
  for (const row of rows) {
    if (row.lifecycle !== "failed" && row.error === null) continue;
    const requestedGroupId = row.requestedGroupId?.trim();
    if (requestedGroupId && attemptedSet.has(requestedGroupId)) {
      failedGroupIds.add(requestedGroupId);
    }
  }

  const succeeded: string[] = [];
  const failed: string[] = [];
  for (const groupId of attempted) {
    if (failedGroupIds.has(groupId)) failed.push(groupId);
    else succeeded.push(groupId);
  }
  return { succeeded, failed };
}

/**
 * Warm every eligible local managed-agent + community pair at app startup.
 * Successful targets are reconciled once, newly added groups are picked up,
 * and only failed/error targets retry with a bounded 5s / 30s / 2m backoff.
 */
export function useManagedAgentRuntimeReconciliation(
  communities: readonly { groupId: string }[],
): void {
  const queryClient = useQueryClient();
  const groupIds = React.useMemo(
    () => canonicalCommunityGroupIds(communities),
    [communities],
  );
  const groupIdsRef = React.useRef(groupIds);

  const reconciledRef = React.useRef<Set<string>>(new Set());
  const inFlightRef = React.useRef<Set<string>>(new Set());
  const failureCountsRef = React.useRef<Map<string, number>>(new Map());
  const retryAtRef = React.useRef<Map<string, number>>(new Map());
  const exhaustedRef = React.useRef<Set<string>>(new Set());
  const retryTimerRef = React.useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const mountedRef = React.useRef(true);
  const runReconcileRef = React.useRef<() => void>(() => undefined);

  const clearRetryTimer = React.useCallback(() => {
    if (retryTimerRef.current !== null) {
      clearTimeout(retryTimerRef.current);
      retryTimerRef.current = null;
    }
  }, []);

  const scheduleNextRetry = React.useCallback(() => {
    clearRetryTimer();
    if (!mountedRef.current) return;

    const active = new Set(groupIdsRef.current);
    let earliest: number | null = null;
    for (const [groupId, retryAt] of retryAtRef.current) {
      if (
        !active.has(groupId) ||
        reconciledRef.current.has(groupId) ||
        inFlightRef.current.has(groupId) ||
        exhaustedRef.current.has(groupId)
      ) {
        continue;
      }
      if (earliest === null || retryAt < earliest) earliest = retryAt;
    }
    if (earliest === null) return;

    retryTimerRef.current = setTimeout(
      () => {
        retryTimerRef.current = null;
        if (mountedRef.current) runReconcileRef.current();
      },
      Math.max(0, earliest - Date.now()),
    );
  }, [clearRetryTimer]);

  runReconcileRef.current = () => {
    if (!mountedRef.current) return;

    const activeGroupIds = groupIdsRef.current;
    const active = new Set(activeGroupIds);
    for (const groupId of [...reconciledRef.current]) {
      if (!active.has(groupId)) reconciledRef.current.delete(groupId);
    }
    for (const groupId of [...failureCountsRef.current.keys()]) {
      if (active.has(groupId)) continue;
      failureCountsRef.current.delete(groupId);
      retryAtRef.current.delete(groupId);
      exhaustedRef.current.delete(groupId);
    }

    const pending = pendingReconcileGroupIds(
      activeGroupIds,
      reconciledRef.current,
      inFlightRef.current,
      retryAtRef.current,
      exhaustedRef.current,
      Date.now(),
    );
    if (pending.length === 0) {
      scheduleNextRetry();
      return;
    }

    for (const groupId of pending) inFlightRef.current.add(groupId);
    scheduleNextRetry();
    const baseline = queryClient.getQueryData<ManagedAgentRuntimeStatus[]>(
      managedAgentRuntimesQueryKey,
    );

    void reconcileManagedAgentRuntimes(pending.map((groupId) => ({ groupId })))
      .then((runtimes) => {
        if (!mountedRef.current) return null;
        cacheReconciledManagedAgentRuntimes(queryClient, baseline, runtimes);
        return classifyReconcileResult(pending, runtimes);
      })
      .catch((error) => {
        if (!mountedRef.current) return null;
        console.warn("[managed-agent-runtimes] reconcile failed:", error);
        return classifyReconcileResult(pending, null);
      })
      .then((result) => {
        if (!mountedRef.current || result === null) return;
        const { succeeded, failed } = result;
        for (const groupId of pending) inFlightRef.current.delete(groupId);
        for (const groupId of succeeded) {
          reconciledRef.current.add(groupId);
          failureCountsRef.current.delete(groupId);
          retryAtRef.current.delete(groupId);
          exhaustedRef.current.delete(groupId);
        }
        const now = Date.now();
        for (const groupId of failed) {
          const failureCount = (failureCountsRef.current.get(groupId) ?? 0) + 1;
          failureCountsRef.current.set(groupId, failureCount);
          const retryDelay = reconcileRetryDelayMs(failureCount);
          if (retryDelay === null) {
            retryAtRef.current.delete(groupId);
            exhaustedRef.current.add(groupId);
          } else {
            retryAtRef.current.set(groupId, now + retryDelay);
          }
        }
        runReconcileRef.current();
      });
  };

  React.useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      clearRetryTimer();
    };
  }, [clearRetryTimer]);

  React.useEffect(() => {
    groupIdsRef.current = groupIds;
    runReconcileRef.current();
  }, [groupIds]);
}
