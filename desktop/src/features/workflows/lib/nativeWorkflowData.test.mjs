import assert from "node:assert/strict";
import test from "node:test";

import {
  decodeWorkflowDefinition,
  definitionPayloadToWorkflow,
  encodeWorkflowDefinition,
  isWorkflowDefinitionStore,
  workflowDefinitionStoreCreateInput,
  workflowToDefinitionPayload,
} from "./nativeWorkflowData.ts";

const workflow = {
  id: "wf-1",
  name: "Release",
  ownerPubkey: "",
  channelId: "group-1",
  definition: { name: "Release" },
  status: "active",
  createdAt: 1_700_000_000,
  updatedAt: 1_700_000_123,
};

test("workflow native payload round-trips relay-era seconds without drift", () => {
  const payload = workflowToDefinitionPayload(
    workflow,
    "name: Release\nenabled: true\n",
    "group-1",
  );
  assert.equal(payload.created_at_ms, 1_700_000_000_000);
  assert.equal(payload.updated_at_ms, 1_700_000_123_000);

  const decoded = decodeWorkflowDefinition(encodeWorkflowDefinition(payload));
  assert.ok(decoded);
  assert.deepEqual(definitionPayloadToWorkflow(decoded), {
    ...workflow,
    definition: { name: "Release", enabled: true },
  });
});

test("workflow store mapping is exact and owner-authored", () => {
  const input = workflowDefinitionStoreCreateInput("group-1");
  assert.deepEqual(input, {
    name: "Workflow definitions (group-1)",
    topic: "x0x.workflow.group-1.definitions",
    policy: "signed",
  });
  assert.equal(
    isWorkflowDefinitionStore(
      {
        id: input.topic,
        topic: input.topic,
        owner: null,
        policy: "signed",
        version: 1,
        policyVersion: 1,
        ownershipStatus: "anchored",
        durabilityDegraded: false,
      },
      "group-1",
    ),
    true,
  );
});

test("malformed YAML remains visible by id instead of crashing the list", () => {
  const payload = workflowToDefinitionPayload(workflow, "name: [", "group-1");
  assert.deepEqual(definitionPayloadToWorkflow(payload).definition, {});
  assert.equal(definitionPayloadToWorkflow(payload).name, "wf-1");
});

test("workflow payload decoder fails closed for unsupported data", () => {
  assert.equal(decodeWorkflowDefinition("not json"), null);
  assert.equal(decodeWorkflowDefinition('{"schema":"buzz.workflow.v2"}'), null);
  assert.equal(
    decodeWorkflowDefinition('{"schema":"buzz.workflow.v1","workflow_id":7}'),
    null,
  );
});
