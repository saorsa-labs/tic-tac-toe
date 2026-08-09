// ── Config bridge types ──────────────────────────────────────────────────────

export type ConfigOrigin =
  | "buzzExplicit"
  | "acpNativeRead"
  | "acpConfigOption"
  | "envVar"
  | "configFile"
  | "personaDefault"
  | "globalDefault"
  | "runtimeOverride"
  | "harnessConstraint";

export type ConfigWriteMechanism =
  | { type: "respawnWithEnvVar"; envKey: string }
  | { type: "acpSetConfigOption"; configId: string }
  | { type: "acpSetSessionModel" }
  | { type: "gooseNativeConfigWrite"; configKey: string }
  | { type: "readOnly" };

export type NormalizedField = {
  value: string | null;
  origin: ConfigOrigin;
  writeVia: ConfigWriteMechanism;
  overriddenValue: string | null;
  overriddenOrigin: ConfigOrigin | null;
  /** True if this field must be set for the harness to function. */
  isRequired: boolean;
};

export type ConfigFieldType =
  | { type: "string" }
  | { type: "number" }
  | { type: "boolean" }
  | { type: "enum"; options: string[] };

export type ConfigField = {
  key: string;
  label: string;
  value: string | null;
  origin: ConfigOrigin;
  schemaType: ConfigFieldType;
  writeVia: ConfigWriteMechanism;
};

export type ConfigTierStatus = "available" | "pending" | "notApplicable";

export type ConfigSourceReport = {
  acpNative: ConfigTierStatus;
  acpConfigOptions: ConfigTierStatus;
  envVars: ConfigTierStatus;
  configFile: ConfigTierStatus;
  configFilePath: string | null;
  mcpConfigFilePath: string | null;
};

export type ExtensionEntry = { name: string; kind: string; enabled: boolean };

export type NormalizedConfig = {
  model: NormalizedField | null;
  provider: NormalizedField | null;
  mode: NormalizedField | null;
  thinkingEffort: NormalizedField | null;
  maxOutputTokens: NormalizedField | null;
  contextLimit: NormalizedField | null;
  systemPrompt: NormalizedField | null;
};

export type RuntimeConfigSurface = {
  runtimeId: string | null;
  runtimeLabel: string | null;
  isPreSpawn: boolean;
  normalized: NormalizedConfig;
  advanced: ConfigField[];
  extensions: ExtensionEntry[];
  sources: ConfigSourceReport;
};
