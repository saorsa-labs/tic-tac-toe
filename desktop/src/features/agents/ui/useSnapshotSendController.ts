/**
 * Snapshot send — pure helpers.
 *
 * The production React hook that wired these helpers to `uploadMediaBytes`
 * was removed when native media/blob upload was cut: a snapshot can no longer
 * be hosted, so it cannot be sent as an attachment or shared via a link. What
 * remains are the payload-agnostic, fully-injected helpers that the unit suite
 * exercises directly — destination eligibility, a single-concurrency guard,
 * and the prepare → encode → upload → send pipeline. `uploadFn` is injected,
 * so the pipeline itself has no dependency on any particular transport.
 */

import type { QueryClient } from "@tanstack/react-query";

import type { BlobDescriptor } from "@/shared/api/tauri";
import { channelsQueryKey } from "@/features/channels/hooks";
import { getTimeoutSnapshot } from "@/features/moderation/lib/timeoutStore";
import { isTimeoutActive } from "@/features/moderation/lib/timeout";
import type { Channel } from "@/shared/api/types";

// ── Public types ──────────────────────────────────────────────────────────────

export type SendPhase =
  | "idle"
  | "preparing"
  | "uploading"
  | "sending"
  | "done"
  | "error";

export type SnapshotSendState = {
  phase: SendPhase;
  error: string | null;
};

/** A joined, non-archived, non-forum destination. */
export function isSendableDestination(ch: Channel): boolean {
  return ch.isMember && ch.archivedAt === null && ch.channelType !== "forum";
}

/**
 * Pure factory for a single-concurrency action guard.
 *
 * Returns `{ runGuarded }` where `runGuarded(action)` executes `action()`
 * only when no other call is currently in flight; any concurrent call receives
 * `false` immediately.  Exported so unit tests can exercise the guard logic
 * directly without requiring a React rendering context.
 *
 * @example
 * ```ts
 * const { runGuarded } = createSendGuard();
 * const [r1, r2] = await Promise.all([
 *   runGuarded(async () => { ...encode/upload/send... }),
 *   runGuarded(async () => { ...encode/upload/send... }),
 * ]);
 * // r1 === true (ran), r2 === false (blocked)
 * ```
 */
export function createSendGuard(): {
  runGuarded: (action: () => Promise<boolean>) => Promise<boolean>;
  get inFlight(): boolean;
} {
  let inFlight = false;
  return {
    runGuarded: async (action) => {
      if (inFlight) return false;
      inFlight = true;
      try {
        return await action();
      } finally {
        inFlight = false;
      }
    },
    get inFlight() {
      return inFlight;
    },
  };
}

/**
 * Read current eligibility for `channelId` directly from live query-cache
 * sources and the timeout external store.  Does NOT read rendered React state
 * or component refs; safe to call inside a `runGuarded` action where render
 * state may be stale.
 *
 * Returns `null` when the channel is eligible; returns a human-readable error
 * string when it is not.
 *
 * Native destinations have no server-owned moderation identity to classify.
 */
export function checkSendEligibility(
  queryClient: QueryClient,
  channelId: string,
  nowMs: number = Date.now(),
): string | null {
  // ── Timeout check ─────────────────────────────────────────────────────────
  // Read the module-level snapshot from timeoutStore directly — this is the
  // same value `useTimeoutState` serves but without requiring a render cycle.
  const timeoutState = getTimeoutSnapshot();
  if (timeoutState.active && isTimeoutActive(timeoutState.expiresAtMs, nowMs)) {
    return "You are currently timed out and cannot send messages.";
  }

  // ── Channel-cache check ───────────────────────────────────────────────────
  const channels = queryClient.getQueryData<Channel[]>(channelsQueryKey) ?? [];
  const channel = channels.find((ch) => ch.id === channelId);

  if (!channel) {
    return "The selected destination is no longer available. Please pick another.";
  }
  if (!isSendableDestination(channel)) {
    return "The selected destination is no longer available. Please pick another.";
  }

  return null;
}

/**
 * The core send pipeline: prepare → [eligibility] → encode → [eligibility] →
 * upload → send.  Extracted as a standalone async function so unit tests can
 * import and exercise it directly with injected dependencies — the production
 * hook calls it inside `runGuarded`.
 *
 * Dependencies are injected rather than closed-over from React scope so the
 * function is pure-async and fully testable without a rendering context.
 */
export async function runSendPipeline(deps: {
  encodeFn: () => Promise<{ fileBytes: number[]; fileName: string }>;
  uploadFn: (bytes: number[], filename: string) => Promise<BlobDescriptor>;
  sendFn: (args: {
    channelId: string;
    content: string;
    mediaTags: string[][];
  }) => Promise<unknown>;
  setStateFn: (state: SnapshotSendState) => void;
  buildMessageFn: (descriptor: BlobDescriptor) => {
    content: string;
    mediaTags: string[][] | null | undefined;
  };
  checkEligibilityFn: () => string | null;
  channelId: string;
}): Promise<boolean> {
  const {
    encodeFn,
    uploadFn,
    sendFn,
    setStateFn,
    buildMessageFn,
    checkEligibilityFn,
    channelId,
  } = deps;

  // ── Eligibility checkpoint 1: before encode ───────────────────────────────
  // Reads live sources directly — timeout store, channel cache, identity cache,
  // relay-self cache.  Not a render snapshot.
  const reason1 = checkEligibilityFn();
  if (reason1 !== null) {
    setStateFn({ phase: "error", error: reason1 });
    return false;
  }

  // ── Prepare (encode) ─────────────────────────────────────────────────────
  setStateFn({ phase: "preparing", error: null });

  let fileBytes: number[];
  let fileName: string;
  try {
    const encoded = await encodeFn();
    fileBytes = encoded.fileBytes;
    fileName = encoded.fileName;
  } catch (err) {
    setStateFn({
      phase: "error",
      error:
        err instanceof Error
          ? `Encode failed: ${err.message}`
          : "Encode failed.",
    });
    return false;
  }

  // ── Eligibility checkpoint 2: after encode, before upload ─────────────────
  // State can change while encode is awaited (channel archived, membership
  // lost, timeout received, relay-self resolves to classify DM).
  const reason2 = checkEligibilityFn();
  if (reason2 !== null) {
    setStateFn({ phase: "error", error: reason2 });
    return false;
  }

  // ── Upload ────────────────────────────────────────────────────────────────
  setStateFn({ phase: "uploading", error: null });

  let descriptor: BlobDescriptor;
  try {
    descriptor = await uploadFn(fileBytes, fileName);
  } catch (err) {
    setStateFn({
      phase: "error",
      error:
        err instanceof Error
          ? `Upload failed: ${err.message}`
          : "Upload failed.",
    });
    return false;
  }

  // Preserve the original filename so `buildImetaTags` emits a `filename`
  // field and the recipient's FileCard renders the correct label. Snapshot
  // sends never emit `thumb`: NIP-92 requires it to be this upload's local
  // thumbnail sidecar, which an agent avatar is not.
  const { thumb: _thumb, ...descriptorWithoutThumb } = descriptor;
  const descriptorWithFilename: BlobDescriptor = {
    ...descriptorWithoutThumb,
    filename: fileName,
  };

  // ── Build message content + NIP-92 imeta tags ─────────────────────────────
  const { content, mediaTags } = buildMessageFn(descriptorWithFilename);

  // ── Send ──────────────────────────────────────────────────────────────────
  setStateFn({ phase: "sending", error: null });

  try {
    await sendFn({
      channelId,
      content,
      mediaTags: mediaTags ?? [],
    });
  } catch (err) {
    setStateFn({
      phase: "error",
      error:
        err instanceof Error ? `Send failed: ${err.message}` : "Send failed.",
    });
    return false;
  }

  setStateFn({ phase: "done", error: null });
  return true;
}

/**
 * Compose the single-concurrency guard with destination resolution and the
 * send pipeline.
 *
 * With the production send hook removed, the only consumers are the unit
 * tests, which call this twice concurrently with injected counters to prove
 * one encode/upload/send runs and one call is blocked.
 */
export function runGuardedSend(
  guard: ReturnType<typeof createSendGuard>,
  resolveChannelId: () => Promise<string>,
  buildPipelineDeps: (
    channelId: string,
  ) => Parameters<typeof runSendPipeline>[0],
  setStateFn: (state: SnapshotSendState) => void,
): Promise<boolean> {
  return guard.runGuarded(async () => {
    // Mark the action pending before opening the DM so the active UI disables
    // immediately. The in-memory guard still protects same-tick invocations
    // that arrive before React commits that state.
    setStateFn({ phase: "preparing", error: null });

    let channelId: string;
    try {
      channelId = await resolveChannelId();
    } catch (error) {
      setStateFn({
        phase: "error",
        error:
          error instanceof Error
            ? `Couldn’t open the conversation: ${error.message}`
            : "Couldn’t open the conversation.",
      });
      return false;
    }

    return runSendPipeline(buildPipelineDeps(channelId));
  });
}
