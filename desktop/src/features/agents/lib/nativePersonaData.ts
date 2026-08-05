/**
 * Native x0xd mapping for agent personas (former kind 30175).
 *
 * A persona is an owner-authored DEFINITION (identity + harness config). It
 * maps to two native primitives:
 *
 *  1. An **AgentCard** — the deployed-agent identity. A definition has no
 *     `agent_id` until a managed agent is instantiated from it; the card's
 *     `agentId` (SHA-256 of the agent's ML-DSA-65 key) is the authenticated
 *     identity sourced from x0xd, NEVER a reinterpreted 64-hex relay pubkey.
 *  2. A **KV store** — one owner-anchored signed store holds every definition
 *     payload, one LWW entry per persona keyed by its slug.
 *
 * The relay kind 30175 coordinate `(pubkey, "d"=slug)` is replaced by the KV
 * key `slug`; cross-device authority is the store owner anchor, not a relay
 * signature. These projections are pure and transport-agnostic — they emit no
 * relay events and carry no relay URL or pubkey-as-identity.
 */

import type {
  X0xAgentCard,
  X0xAgentId,
  X0xStorePolicy,
  X0xStoreSummary,
} from "@/shared/api/tauriNativeX0x";
import type { AgentPersona, RespondToMode } from "@/shared/api/types";

/** Schema tag carried on every persona definition payload. */
export const PERSONA_DEFINITION_SCHEMA = "buzz.persona.v1";

/**
 * Topic of the single owner-anchored store holding all persona definitions.
 * Stable across reboots; one LWW entry per persona keyed by slug.
 */
export const PERSONA_DEFINITIONS_STORE_TOPIC = "x0x.workspace.personas";

/** Store write policy for persona definitions (owner-authored, mutable). */
export const PERSONA_DEFINITIONS_STORE_POLICY: X0xStorePolicy = "signed";

// The KV entry key for a persona definition IS its slug (= persona.id) — no
// prefix, no transform. Read/write callers pass `persona.id` directly as the
// store key; the inverse (key → slug) is the identity, so no helper is needed.

/**
 * The typed payload persisted as a KV entry value (JSON). snake_case mirrors
 * the backend wire convention so the same struct round-trips through Rust
 * without a second rename layer.
 *
 * `agent_id` links the definition to its most-recently-deployed managed-agent
 * identity. It is `null` until instantiation and is ALWAYS an x0x AgentId —
 * never a relay pubkey. This is the field ManagedAgentNativeIdentity sources.
 */
export type PersonaDefinitionPayload = {
  schema: typeof PERSONA_DEFINITION_SCHEMA;
  slug: string;
  display_name: string;
  avatar_url: string | null;
  system_prompt: string;
  runtime: string | null;
  model: string | null;
  provider: string | null;
  name_pool: string[];
  env_vars: Record<string, string>;
  respond_to: RespondToMode | null;
  respond_to_allowlist: string[];
  parallelism: number | null;
  is_active: boolean;
  /** Named-group id of the originating team, or null for a standalone persona. */
  source_team: string | null;
  created_at_ms: number;
  updated_at_ms: number;
  /** x0x AgentId of the deployed instance, or null until instantiated. */
  agent_id: X0xAgentId | null;
};

/** Project an {@link AgentPersona} (relay-era type) into the native payload. */
export function personaToDefinitionPayload(
  persona: AgentPersona,
  nowMs: number,
): PersonaDefinitionPayload {
  return {
    schema: PERSONA_DEFINITION_SCHEMA,
    slug: persona.id,
    display_name: persona.displayName,
    avatar_url: persona.avatarUrl,
    system_prompt: persona.systemPrompt,
    runtime: persona.runtime,
    model: persona.model,
    provider: persona.provider,
    name_pool: persona.namePool,
    env_vars: persona.envVars,
    respond_to: persona.respondTo,
    respond_to_allowlist: persona.respondToAllowlist,
    parallelism: persona.parallelism,
    is_active: persona.isActive,
    source_team: persona.sourceTeam ?? null,
    created_at_ms: parseMs(persona.createdAt, nowMs),
    updated_at_ms: parseMs(persona.updatedAt, nowMs),
    // Definitions carry no agent identity until a managed agent is spawned.
    agent_id: null,
  };
}

/** Fold a native payload back into the editor/renderer {@link AgentPersona}. */
export function definitionPayloadToPersona(
  payload: PersonaDefinitionPayload,
): AgentPersona {
  return {
    id: payload.slug,
    displayName: payload.display_name,
    avatarUrl: payload.avatar_url,
    systemPrompt: payload.system_prompt,
    runtime: payload.runtime,
    model: payload.model,
    provider: payload.provider,
    namePool: payload.name_pool,
    isBuiltIn: false,
    isActive: payload.is_active,
    sourceTeam: payload.source_team,
    envVars: payload.env_vars,
    respondTo: payload.respond_to,
    respondToAllowlist: payload.respond_to_allowlist,
    parallelism: payload.parallelism,
    createdAt: new Date(payload.created_at_ms).toISOString(),
    updatedAt: new Date(payload.updated_at_ms).toISOString(),
  };
}

/**
 * The AgentCard display projection for a persona. Only the fields a definition
 * can supply before instantiation: `displayName`. `agentId` is resolved by x0xd
 * at spawn (see {@link linkPersonaAgent}), so callers must NOT fabricate one.
 */
export function personaAgentCardDisplay(
  persona: AgentPersona,
): Pick<X0xAgentCard, "displayName"> {
  return { displayName: persona.displayName };
}

/**
 * Return a payload copy linked to a deployed agent identity. Used when a
 * managed agent is spawned: the definition's `agent_id` is set to the x0xd
 * AgentId returned by the daemon (sourced from an authenticated AgentCard),
 * never derived from a relay pubkey.
 */
export function linkPersonaAgent(
  payload: PersonaDefinitionPayload,
  agentId: X0xAgentId,
  nowMs: number,
): PersonaDefinitionPayload {
  return { ...payload, agent_id: agentId, updated_at_ms: nowMs };
}

/** True iff the payload carries a native agent identity link. */
export function personaHasAgentLink(
  payload: PersonaDefinitionPayload,
): payload is PersonaDefinitionPayload & { agent_id: X0xAgentId } {
  return payload.agent_id !== null;
}

/** Creation input for the persona-definitions store (called once per install). */
export function personaDefinitionsStoreCreateInput(): {
  name: string;
  topic: string;
  policy: X0xStorePolicy;
} {
  return {
    name: "Persona definitions",
    topic: PERSONA_DEFINITIONS_STORE_TOPIC,
    policy: PERSONA_DEFINITIONS_STORE_POLICY,
  };
}

/**
 * Predicate for {@link X0xStoreSummary}: is this the persona-definitions store?
 * Used to locate the owner-anchored store among `x0xListStores()` results.
 */
export function isPersonaDefinitionsStore(s: X0xStoreSummary): boolean {
  return s.topic === PERSONA_DEFINITIONS_STORE_TOPIC;
}

/** Encode a payload to the store value (stable JSON, sorted keys). */
export function encodePersonaDefinition(
  payload: PersonaDefinitionPayload,
): string {
  return JSON.stringify(payload);
}

/** Decode a store value back into a payload, validating the schema tag. */
export function decodePersonaDefinition(
  value: string,
): PersonaDefinitionPayload | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return null;
  }
  if (
    parsed !== null &&
    typeof parsed === "object" &&
    (parsed as PersonaDefinitionPayload).schema === PERSONA_DEFINITION_SCHEMA
  ) {
    return parsed as PersonaDefinitionPayload;
  }
  return null;
}

function parseMs(iso: string, fallbackMs: number): number {
  const t = Date.parse(iso);
  return Number.isNaN(t) ? fallbackMs : t;
}
