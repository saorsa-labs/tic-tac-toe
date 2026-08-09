import assert from "node:assert/strict";
import test from "node:test";

import {
  modelDropdownOptions,
  sharedComputeModelPickerState,
} from "./sharedComputeModelPicker.ts";
import { AUTO_MODEL_DROPDOWN_VALUE } from "./agentConfigOptions.tsx";

const fallback = [{ id: "", label: "Default model" }];
const live = [
  { id: "", label: "Default (auto)" },
  { id: "mesh/model", label: "mesh/model" },
];

test("Buzz shared compute maps persisted auto to Default and hides custom input", () => {
  const state = sharedComputeModelPickerState({
    discoveredOptions: live,
    fallbackOptions: fallback,
    isCustomEditing: false,
    model: "auto",
    provider: "shared-compute",
  });
  assert.equal(state.selectValue, AUTO_MODEL_DROPDOWN_VALUE);
  assert.equal(state.isSharedCompute, true);
  assert.equal(state.isCustom, false);
  assert.equal(state.showCustomInput, false);
});

test("Buzz shared compute fallback is Default auto while normal providers remain unchanged", () => {
  const mesh = sharedComputeModelPickerState({
    discoveredOptions: null,
    fallbackOptions: fallback,
    isCustomEditing: false,
    model: "",
    provider: "shared-compute",
  });
  assert.deepEqual(mesh.options, [{ id: "", label: "Default (auto)" }]);

  const openai = sharedComputeModelPickerState({
    discoveredOptions: null,
    fallbackOptions: fallback,
    isCustomEditing: false,
    model: "auto",
    provider: "openai",
  });
  assert.equal(openai.isCustom, true);
  assert.equal(openai.showCustomInput, true);
});

test("Buzz shared compute keeps Default auto when discovery is empty", () => {
  const state = sharedComputeModelPickerState({
    discoveredOptions: [],
    fallbackOptions: fallback,
    isCustomEditing: false,
    model: "auto",
    provider: "shared-compute",
  });

  assert.deepEqual(state.options, [{ id: "", label: "Default (auto)" }]);
  assert.equal(state.selectValue, AUTO_MODEL_DROPDOWN_VALUE);
  assert.equal(state.showCustomInput, false);
});

test("Buzz shared compute dropdown contains Default plus live models and no custom escape hatch", () => {
  const options = modelDropdownOptions({
    options: live,
    loading: false,
    loadingValue: "loading",
    allowCustom: false,
  });
  assert.deepEqual(
    options.map((option) => option.label),
    ["Default (auto)", "mesh/model"],
  );
});
