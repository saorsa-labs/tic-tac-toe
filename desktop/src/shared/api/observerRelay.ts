// Native x0x direct-message transport for managed-agent observer telemetry +
// control. Replaces the kind:24200 Nostr relay subscribe/publish path.
//
// - Telemetry (child→owner) arrives over the owner daemon's `/ws/direct`,
//   filtered to observer envelopes addressed to the owner from known children.
// - Control (owner→child) is sent as an x0x direct message to the child AgentId.
//
// The relay client is never invoked on this path. See `observerNative.ts` for
// the decode/filter/send logic and `observerEnvelope.ts` for the wire schema.

import { getIdentity } from "@/shared/api/tauriIdentity";
import type { X0xLiveFrame } from "./tauriNativeX0x";
import { invokeTauri } from "./tauri";
import {
  sendObserverControl,
  subscribeObserverLive,
  type NativeObserverFrame,
} from "./observerNative";

export type { NativeObserverFrame };

/**
 * Resolve the native x0x AgentId of a managed agent's dedicated child daemon.
 * Returns `null` when no child is provisioned (the agent predates native
 * provisioning or the child failed to come up).
 */
export async function resolveChildAgentId(
  agentPubkey: string,
): Promise<string | null> {
  return invokeTauri<string | null>("get_managed_agent_native_identity", {
    pubkey: agentPubkey,
  });
}

/**
 * Send a control command to a managed agent over x0x direct messaging. The
 * child AgentId is resolved from the managed-agent pubkey; the owner AgentId
 * from the local daemon identity. Fire-and-forget on the send side — the
 * outcome arrives asynchronously as a `control_result` observer frame.
 *
 * Preserves the legacy call signature so `agentControl.ts` is unchanged.
 */
export async function sendAgentObserverControl(
  agentPubkey: string,
  payload: unknown,
): Promise<void> {
  const childAgentId = await resolveChildAgentId(agentPubkey);
  if (!childAgentId) {
    throw new Error(
      "managed agent has no native x0x child identity; cannot send observer control",
    );
  }
  const identity = await getIdentity();
  await sendObserverControl({
    childAgentId,
    ownerAgentId: identity.agentId,
    controlPayload: payload,
  });
}

/**
 * Start the live observer telemetry subscription over the owner daemon's
 * `/ws/direct`. Returns an unsubscribe function.
 *
 * `knownChildAgentIds` is read LIVE by the adapter — pass a mutable `Set` the
 * store updates as agents register/unregister, so dynamically-added children
 * are admitted without reopening the stream. `onFrame` receives the decoded
 * observer frame (or `null` for non-observer/chat/unknown-sender DMs, which the
 * store drops) plus the raw `X0xLiveFrame`.
 */
export async function subscribeToAgentObserverFrames(
  ownerAgentId: string,
  knownChildAgentIds: Set<string>,
  onFrame: (frame: NativeObserverFrame | null, raw: X0xLiveFrame) => void,
): Promise<() => Promise<void>> {
  const sub = await subscribeObserverLive({
    ownerAgentId,
    knownChildAgentIds,
    onFrame,
  });
  return () => sub.close();
}
