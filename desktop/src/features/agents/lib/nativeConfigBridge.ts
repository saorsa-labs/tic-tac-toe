/**
 * Native x0xd sourcing for the agent config-bridge surface.
 *
 * The config bridge (`get_agent_config_surface`) surfaces model / provider /
 * system prompt for a managed agent. Relay era: those resolved from the
 * kind 30175 persona record. Native era: they resolve from the persona
 * definition KV payload (`x0x.workspace.personas`), keyed by the agent's
 * linked persona slug, and config edits write back to that store.
 *
 * Per the managed-agent identity contract, each interactive agent owns a
 * dedicated loopback x0xd child; its `AgentCard.agent_id` is the identity.
 * The bridge keys config by that agent id + its linked persona slug — never
 * by a relay pubkey and never via BUZZ_RELAY_URL.
 */

import type {
  ConfigOrigin,
  ConfigWriteMechanism,
  NormalizedConfig,
  NormalizedField,
} from "@/shared/api/configBridgeTypes";
import type { X0xAgentId } from "@/shared/api/tauriNativeX0x";
import {
  PERSONA_DEFINITIONS_STORE_TOPIC,
  encodePersonaDefinition,
  type PersonaDefinitionPayload,
} from "@/features/agents/lib/nativePersonaData";

/** Env var the desktop sets from the persona model at spawn time. */
const PERSONA_MODEL_ENV_KEY = "BUZZ_AGENT_MODEL";
/** Env var the desktop sets from the persona provider at spawn time. */
const PERSONA_PROVIDER_ENV_KEY = "BUZZ_AGENT_PROVIDER";

/**
 * Project a persona definition payload into the bridge's normalized config.
 * Persona-authored values carry origin `personaDefault`; spawn-time knobs
 * (model/provider) write via a respawn with the harness env var, while the
 * system prompt is definition-bound (edited through the persona store).
 */
export function personaToNormalizedConfig(
  payload: PersonaDefinitionPayload,
): NormalizedConfig {
  return {
    model: personaField(
      payload.model,
      "respawnWithEnvVar",
      PERSONA_MODEL_ENV_KEY,
    ),
    provider: personaField(
      payload.provider,
      "respawnWithEnvVar",
      PERSONA_PROVIDER_ENV_KEY,
    ),
    mode: null,
    thinkingEffort: null,
    maxOutputTokens: null,
    contextLimit: null,
    systemPrompt: personaField(payload.system_prompt, "readOnly"),
  };
}

/** The store write-back for a persona config edit (no relay publication). */
export function personaConfigWrite(
  slug: string,
  next: PersonaDefinitionPayload,
): {
  storeId: string;
  key: string;
  value: string;
  contentType: string;
} {
  return {
    storeId: PERSONA_DEFINITIONS_STORE_TOPIC,
    key: slug,
    value: encodePersonaDefinition(next),
    contentType: "application/json",
  };
}

/**
 * The agent identity the bridge keys config by. The dedicated child daemon's
 * signed `AgentCard.agent_id` — never the relay pubkey. Returns null when the
 * native link is absent; the bridge must not fall back to a relay identity.
 */
export function bridgeAgentIdentity(agent: {
  agentId?: X0xAgentId | null;
}): X0xAgentId | null {
  const id = agent.agentId;
  if (typeof id === "string" && /^[0-9a-f]{64}$/.test(id)) return id;
  return null;
}

// ── Helpers ─────────────────────────────────────────────────────────────────

const PERSONA_ORIGIN: ConfigOrigin = "personaDefault";

function personaField(
  value: string | null,
  writeType: Extract<ConfigWriteMechanism, { type: "readOnly" }>["type"],
): NormalizedField;
function personaField(
  value: string | null,
  writeType: "respawnWithEnvVar",
  envKey: string,
): NormalizedField;
function personaField(
  value: string | null,
  writeType: "readOnly" | "respawnWithEnvVar",
  envKey?: string,
): NormalizedField {
  const writeVia: ConfigWriteMechanism =
    writeType === "respawnWithEnvVar" && envKey !== undefined
      ? { type: "respawnWithEnvVar", envKey }
      : { type: "readOnly" };
  return {
    value,
    origin: PERSONA_ORIGIN,
    writeVia,
    overriddenValue: null,
    overriddenOrigin: null,
    // Model/provider are required for the harness to function; the prompt is
    // not (a definition may intentionally ship an empty body).
    isRequired: writeType === "respawnWithEnvVar",
  };
}
