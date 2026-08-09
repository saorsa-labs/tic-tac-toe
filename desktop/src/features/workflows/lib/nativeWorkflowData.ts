/**
 * Native x0xd mapping for workflow definitions (former kind 30620).
 *
 * A workflow definition maps to a **KV store entry** holding the YAML body,
 * scoped to the channel's native group — NOT a relay kind 30620 event. The
 * relay coordinate `(pubkey, "d"=workflow_id)` is replaced by the store key
 * `workflow_id`; cross-device authority is the store owner anchor.
 *
 * Workflow EXECUTION (runs, approvals, trace) is a Symphony-layer concern: it
 * maps to x0x-symphony typed stores (`runs` / `handoffs` / `proofs`) or to a
 * Symphony-compatible `WORKFLOW.md` reference consumed by the Symphony sidecar.
 * This module emits no relay events and carries no relay URL.
 */

import type {
  X0xStorePolicy,
  X0xStoreSummary,
} from "@/shared/api/tauriNativeAuxiliary";
import type { Workflow } from "@/shared/api/workflowTypes";
import { parse as yamlParse } from "yaml";

export const WORKFLOW_DEFINITION_SCHEMA = "buzz.workflow.v1";

/** Store write policy for workflow definitions (owner-authored, mutable). */
export const WORKFLOW_DEFINITION_STORE_POLICY: X0xStorePolicy = "signed";

/** The store key for a workflow definition IS its id — no prefix. */
// Read/write callers pass `workflow.id` directly as the store key.

/** Prefix/suffix shared only by workflow definition stores. */
export const WORKFLOW_DEFINITION_STORE_PREFIX = "x0x.workflow.";
export const WORKFLOW_DEFINITION_STORE_SUFFIX = ".definitions";

/** Topic for a group's workflow-definitions store. */
export function workflowDefinitionStoreTopic(groupId: string): string {
  return `${WORKFLOW_DEFINITION_STORE_PREFIX}${groupId}${WORKFLOW_DEFINITION_STORE_SUFFIX}`;
}

/** Creation input for one group's mutable workflow-definition store. */
export function workflowDefinitionStoreCreateInput(groupId: string): {
  name: string;
  topic: string;
  policy: X0xStorePolicy;
} {
  return {
    name: `Workflow definitions (${groupId})`,
    topic: workflowDefinitionStoreTopic(groupId),
    policy: WORKFLOW_DEFINITION_STORE_POLICY,
  };
}

/** True when a store is the definition store for `groupId`. */
export function isWorkflowDefinitionStore(
  store: X0xStoreSummary,
  groupId: string,
): boolean {
  return store.topic === workflowDefinitionStoreTopic(groupId);
}

/** Stable topic for a workflow's Symphony run ledger (append-only). */
export function workflowRunStoreTopic(workflowId: string): string {
  return `x0x.workflow.${workflowId}.runs`;
}

/** Stable topic for a workflow's Symphony approval/handoff ledger. */
export function workflowHandoffStoreTopic(workflowId: string): string {
  return `x0x.workflow.${workflowId}.handoffs`;
}

/** Stable topic for a workflow's Symphony proof/output ledger. */
export function workflowProofStoreTopic(workflowId: string): string {
  return `x0x.workflow.${workflowId}.proofs`;
}

/** The native payload persisted as the definition KV entry value (JSON). */
export type WorkflowDefinitionPayload = {
  schema: typeof WORKFLOW_DEFINITION_SCHEMA;
  workflow_id: string;
  name: string;
  /** Raw YAML body — the single source of the parsed `definition`. */
  yaml_definition: string;
  /** Native group the workflow belongs to (former `channel_id`). */
  group_id: string | null;
  created_at_ms: number;
  updated_at_ms: number;
};

/** Project a {@link Workflow} (relay-era type) into the native payload. */
export function workflowToDefinitionPayload(
  workflow: Workflow,
  yamlDefinition: string,
  groupId: string | null,
): WorkflowDefinitionPayload {
  return {
    schema: WORKFLOW_DEFINITION_SCHEMA,
    workflow_id: workflow.id,
    name: workflow.name,
    yaml_definition: yamlDefinition,
    group_id: groupId,
    created_at_ms: workflow.createdAt * 1_000,
    updated_at_ms: workflow.updatedAt * 1_000,
  };
}

/** Fold a native payload back into the editor/renderer {@link Workflow}. */
export function definitionPayloadToWorkflow(
  payload: WorkflowDefinitionPayload,
): Workflow {
  const definition = parseDefinitionObject(payload.yaml_definition);
  const name =
    typeof definition.name === "string" && definition.name.trim() !== ""
      ? definition.name
      : payload.workflow_id;
  return {
    id: payload.workflow_id,
    name,
    // The owner is the store anchor; the relay pubkey no longer applies.
    ownerPubkey: "",
    channelId: payload.group_id,
    definition,
    // "active" mirrors the Rust convention; the UI derives the "disabled"
    // display state from `definition.enabled` (see getWorkflowDisplayStatus).
    status: "active",
    createdAt: Math.floor(payload.created_at_ms / 1_000),
    updatedAt: Math.floor(payload.updated_at_ms / 1_000),
  };
}

/**
 * The Symphony-compatible `WORKFLOW.md` reference body for a workflow. The
 * Symphony sidecar consumes this as an opaque orchestration document; the
 * desktop treats it as a stable, shareable artifact (one per workflow).
 */
export function encodeWorkflowDefinition(
  payload: WorkflowDefinitionPayload,
): string {
  return JSON.stringify(payload);
}

/** Parse a store value and reject unrelated or malformed schema versions. */
export function decodeWorkflowDefinition(
  value: string,
): WorkflowDefinitionPayload | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return null;
  }
  if (
    parsed === null ||
    typeof parsed !== "object" ||
    (parsed as { schema?: unknown }).schema !== WORKFLOW_DEFINITION_SCHEMA
  ) {
    return null;
  }
  const payload = parsed as Partial<WorkflowDefinitionPayload>;
  if (
    typeof payload.workflow_id !== "string" ||
    typeof payload.name !== "string" ||
    typeof payload.yaml_definition !== "string" ||
    (payload.group_id !== null && typeof payload.group_id !== "string") ||
    typeof payload.created_at_ms !== "number" ||
    typeof payload.updated_at_ms !== "number"
  ) {
    return null;
  }
  return payload as WorkflowDefinitionPayload;
}

export function workflowToSymphonyMd(
  payload: WorkflowDefinitionPayload,
): string {
  return [
    `# ${payload.name}`,
    "",
    "<!-- buzz.workflow.v1 — Symphony-compatible reference. ",
    "    Orchestration state lives in the runs/handoffs/proofs stores, not here. -->",
    "",
    "```yaml",
    payload.yaml_definition.trimEnd(),
    "```",
    "",
  ].join("\n");
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/**
 * Parse a YAML definition body into a free-form object, falling back to `{}`.
 * Mirrors the Rust `parse_definition`: a malformed workflow must not break the
 * page, so a non-object document yields an empty object rather than an error.
 */
function parseDefinitionObject(yaml: string): Record<string, unknown> {
  try {
    const parsed = yamlParse(yaml);
    if (
      parsed !== null &&
      typeof parsed === "object" &&
      !Array.isArray(parsed)
    ) {
      return parsed as Record<string, unknown>;
    }
  } catch {
    // A malformed definition remains visible by id and editable; one bad
    // store entry must not crash the workflow list.
  }
  return {};
}
