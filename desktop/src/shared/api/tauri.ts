import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import type {
  BackendProviderCandidate,
  BackendProviderProbeResult,
  ManagedAgent,
  ManagedAgentBackend,
  RelayEvent,
  CreateManagedAgentInput,
  AgentModelsResponse,
  UpdateManagedAgentInput,
  AcpAvailabilityStatus,
  AcpRuntimeCatalogEntry,
  AuthStatus,
  CommandAvailability,
  InstallRuntimeResult,
  GitBashPrerequisite,
  RuntimeConfigSurface,
} from "@/shared/api/types";
import { x0xHistoryGet } from "@/shared/api/tauriNativeX0x";
import {
  channelIdFromScope,
  historyRowToRelayEvent,
} from "@/shared/api/nativeMessageAdapter";

export * from "@/shared/api/tauriChannels";

export type RawManagedAgent = {
  pubkey: string;
  name: string;
  persona_id: string | null;
  team_id?: string | null;
  acp_command: string;
  agent_command: string;
  agent_command_override?: string | null;
  agent_args: string[];
  mcp_command: string;
  turn_timeout_seconds: number;
  idle_timeout_seconds: number | null;
  max_turn_duration_seconds: number | null;
  parallelism: number;
  system_prompt: string | null;
  avatar_url?: string | null;
  model: string | null;
  provider: string | null;
  persona_out_of_date: boolean;
  persona_orphaned: boolean;
  needs_restart: boolean;
  env_vars?: Record<string, string>;
  status: ManagedAgent["status"];
  pid: number | null;
  created_at: string;
  updated_at: string;
  last_started_at: string | null;
  last_stopped_at: string | null;
  last_exit_code: number | null;
  last_error: string | null;
  last_error_code: number | null;
  log_path: string;
  start_on_app_launch: boolean;
  auto_restart_on_config_change?: boolean;
  backend: ManagedAgentBackend;
  backend_agent_id: string | null;
  // Optional: pre-feature mock fixtures may omit these. Mapped to
  // `"owner-only"` / `[]` in `fromRawManagedAgent`.
  respond_to?: ManagedAgent["respondTo"];
  respond_to_allowlist?: string[];
};

type RawCreateManagedAgentResponse = {
  agent: RawManagedAgent;
  profile_sync_error: string | null;
  spawn_error: string | null;
};

type RawManagedAgentLog = {
  content: string;
  log_path: string;
};

export type RawAcpRuntimeCatalogEntry = {
  id: string;
  label: string;
  avatar_url: string;
  availability: AcpAvailabilityStatus;
  command: string | null;
  binary_path: string | null;
  default_args: string[];
  mcp_command: string | null;
  model_env_var?: string | null;
  provider_env_var?: string | null;
  thinking_env_var?: string | null;
  install_hint: string;
  install_instructions_url: string;
  can_auto_install: boolean;
  underlying_cli_path: string | null;
  node_required: boolean;
  /** Tagged union with snake_case status values — same shape as `AuthStatus`. */
  auth_status: AuthStatus;
  login_hint?: string;
};

export type RawInstallStepResult = {
  step: string;
  command: string;
  success: boolean;
  stdout: string;
  stderr: string;
  exit_code: number | null;
  hint?: string;
};

export type RawInstallRuntimeResult = {
  success: boolean;
  steps: RawInstallStepResult[];
  restarted_count: number;
  failed_restart_count: number;
};

type RawGitBashPrerequisite = {
  available: boolean;
  path: string | null;
  install_instructions_url: string;
  install_hint: string;
};

type RawCommandAvailability = {
  command: string;
  resolved_path: string | null;
  available: boolean;
};

type RawManagedAgentPrereqs = {
  acp: RawCommandAvailability;
  mcp: RawCommandAvailability;
};

/** Error normalized from a rejected Tauri invocation with its wire payload. */
export class TauriInvokeError extends Error {
  readonly payload: unknown;

  constructor(message: string, payload: unknown) {
    super(message);
    this.name = "TauriInvokeError";
    this.payload = payload;
  }
}

function toTauriError(error: unknown): Error {
  if (error instanceof Error) {
    return error;
  }

  if (typeof error === "string") {
    return new TauriInvokeError(error, error);
  }

  if (
    typeof error === "object" &&
    error !== null &&
    "message" in error &&
    typeof error.message === "string"
  ) {
    return new TauriInvokeError(error.message, error);
  }

  try {
    return new TauriInvokeError(JSON.stringify(error), error);
  } catch {
    return new TauriInvokeError("Unknown Tauri error", error);
  }
}

export async function invokeTauri<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await tauriInvoke<T>(command, args);
  } catch (error) {
    const err = toTauriError(error);
    throw err;
  }
}

export function isSharedIdentity(): Promise<boolean> {
  return invokeTauri<boolean>("is_shared_identity");
}

export async function getEventById(eventId: string): Promise<RelayEvent> {
  // Native lookup by canonical msg_id (BLAKE3). The local daemon's history
  // store is indexed on `msg_id` (UNIQUE constraint), so this is a point
  // read — no relay, no network round-trip, no payload/history scan. A
  // canonical id is globally unique within one daemon's store, so no
  // scope/channel hint is needed to disambiguate; the channel id is derived
  // from the row's own scope.
  const row = await x0xHistoryGet(eventId);
  if (!row) {
    throw new Error(`message ${eventId} not found in local history`);
  }
  const event = historyRowToRelayEvent(row, channelIdFromScope(row.scope));
  if (!event) {
    // The id resolves to a stored row that is not a renderable channel
    // message (non-text content type / undecodable envelope). Surface as a
    // miss so callers' catch paths treat it like "not found".
    throw new Error(`message ${eventId} is not a renderable channel message`);
  }
  return event;
}

export type BlobDescriptor = {
  url: string;
  sha256: string;
  size: number;
  type: string;
  uploaded: number;
  dim?: string;
  blurhash?: string;
  thumb?: string;
  duration?: number;
  image?: string;
  /** Original filename captured client-side. */
  filename?: string;
};

export function fromRawManagedAgent(agent: RawManagedAgent): ManagedAgent {
  return {
    pubkey: agent.pubkey,
    name: agent.name,
    personaId: agent.persona_id,
    teamId: agent.team_id ?? null,
    acpCommand: agent.acp_command,
    agentCommand: agent.agent_command,
    agentCommandOverride: agent.agent_command_override ?? null,
    agentArgs: agent.agent_args,
    mcpCommand: agent.mcp_command,
    turnTimeoutSeconds: agent.turn_timeout_seconds,
    idleTimeoutSeconds: agent.idle_timeout_seconds,
    maxTurnDurationSeconds: agent.max_turn_duration_seconds,
    parallelism: agent.parallelism,
    systemPrompt: agent.system_prompt,
    avatarUrl: agent.avatar_url ?? null,
    model: agent.model,
    provider: agent.provider ?? null,
    personaOutOfDate: agent.persona_out_of_date ?? false,
    personaOrphaned: agent.persona_orphaned ?? false,
    needsRestart: agent.needs_restart ?? false,
    envVars: agent.env_vars ?? {},
    status: agent.status,
    pid: agent.pid,
    createdAt: agent.created_at,
    updatedAt: agent.updated_at,
    lastStartedAt: agent.last_started_at,
    lastStoppedAt: agent.last_stopped_at,
    lastExitCode: agent.last_exit_code,
    lastError: agent.last_error,
    lastErrorCode: agent.last_error_code ?? null,
    logPath: agent.log_path,
    startOnAppLaunch: agent.start_on_app_launch,
    autoRestartOnConfigChange: agent.auto_restart_on_config_change ?? true,
    backend: agent.backend,
    backendAgentId: agent.backend_agent_id,
    // Fallbacks for pre-feature mocks/fixtures that don't carry these fields.
    // Real agent records always include them (defaulted server-side).
    respondTo: agent.respond_to ?? "owner-only",
    respondToAllowlist: agent.respond_to_allowlist ?? [],
  };
}

function fromRawAcpRuntimeCatalogEntry(
  entry: RawAcpRuntimeCatalogEntry,
): AcpRuntimeCatalogEntry {
  return {
    id: entry.id,
    label: entry.label,
    avatarUrl: entry.avatar_url,
    availability: entry.availability,
    command: entry.command,
    binaryPath: entry.binary_path,
    defaultArgs: entry.default_args,
    mcpCommand: entry.mcp_command,
    modelEnvVar: entry.model_env_var ?? null,
    providerEnvVar: entry.provider_env_var ?? null,
    thinkingEnvVar: entry.thinking_env_var ?? null,
    installHint: entry.install_hint,
    installInstructionsUrl: entry.install_instructions_url,
    canAutoInstall: entry.can_auto_install,
    underlyingCliPath: entry.underlying_cli_path,
    nodeRequired: entry.node_required,
    authStatus: entry.auth_status,
    loginHint: entry.login_hint ?? null,
  };
}

function fromRawInstallRuntimeResult(
  raw: RawInstallRuntimeResult,
): InstallRuntimeResult {
  return {
    success: raw.success,
    steps: raw.steps.map((step) => ({
      step: step.step,
      command: step.command,
      success: step.success,
      stdout: step.stdout,
      stderr: step.stderr,
      exitCode: step.exit_code,
      hint: step.hint,
    })),
    restartedCount: raw.restarted_count,
    failedRestartCount: raw.failed_restart_count,
  };
}

function fromRawCommandAvailability(
  command: RawCommandAvailability,
): CommandAvailability {
  return {
    command: command.command,
    resolvedPath: command.resolved_path,
    available: command.available,
  };
}

export async function listManagedAgents(): Promise<ManagedAgent[]> {
  return (await invokeTauri<RawManagedAgent[]>("list_managed_agents")).map(
    fromRawManagedAgent,
  );
}
export async function createManagedAgent(input: CreateManagedAgentInput) {
  const response = await invokeTauri<RawCreateManagedAgentResponse>(
    "create_managed_agent",
    {
      input: {
        name: input.name,
        personaId: input.personaId,
        teamId: input.teamId,
        acpCommand: input.acpCommand,
        agentCommand: input.agentCommand,
        harnessOverride: input.harnessOverride ?? false,
        agentArgs: input.agentArgs,
        mcpCommand: input.mcpCommand,
        turnTimeoutSeconds: input.turnTimeoutSeconds,
        idleTimeoutSeconds: input.idleTimeoutSeconds,
        maxTurnDurationSeconds: input.maxTurnDurationSeconds,
        parallelism: input.parallelism,
        systemPrompt: input.systemPrompt,
        avatarUrl: input.avatarUrl,
        model: input.model,
        provider: input.provider,
        envVars: input.envVars ?? {},
        spawnAfterCreate: input.spawnAfterCreate,
        startOnAppLaunch: input.startOnAppLaunch,
        backend: input.backend,
        respondTo: input.respondTo,
        respondToAllowlist: input.respondToAllowlist,
      },
    },
  );
  return {
    agent: fromRawManagedAgent(response.agent),
    profileSyncError: response.profile_sync_error,
    spawnError: response.spawn_error,
  };
}

export async function deleteManagedAgent(
  pubkey: string,
  forceRemoteDelete?: boolean,
): Promise<void> {
  await invokeTauri("delete_managed_agent", {
    pubkey,
    forceRemoteDelete: forceRemoteDelete ?? null,
  });
}

export async function getManagedAgentLog(pubkey: string, lineCount?: number) {
  const response = await invokeTauri<RawManagedAgentLog>(
    "get_managed_agent_log",
    {
      pubkey,
      lineCount,
    },
  );

  return {
    content: response.content,
    logPath: response.log_path,
  };
}

export async function discoverGitBashPrerequisite(): Promise<GitBashPrerequisite | null> {
  const prerequisite = await invokeTauri<RawGitBashPrerequisite | null>(
    "discover_git_bash_prerequisite",
  );
  return (
    prerequisite && {
      available: prerequisite.available,
      path: prerequisite.path,
      installInstructionsUrl: prerequisite.install_instructions_url,
      installHint: prerequisite.install_hint,
    }
  );
}

export async function discoverAcpRuntimes(): Promise<AcpRuntimeCatalogEntry[]> {
  return (
    await invokeTauri<RawAcpRuntimeCatalogEntry[]>("discover_acp_providers")
  ).map(fromRawAcpRuntimeCatalogEntry);
}

export async function installAcpRuntime(
  runtimeId: string,
): Promise<InstallRuntimeResult> {
  const raw = await invokeTauri<RawInstallRuntimeResult>(
    "install_acp_runtime",
    { runtimeId },
  );
  return fromRawInstallRuntimeResult(raw);
}

export async function discoverManagedAgentPrereqs(input: {
  acpCommand?: string;
  mcpCommand?: string;
}) {
  const response = await invokeTauri<RawManagedAgentPrereqs>(
    "discover_managed_agent_prereqs",
    {
      input: {
        acpCommand: input.acpCommand,
        mcpCommand: input.mcpCommand,
      },
    },
  );

  return {
    acp: fromRawCommandAvailability(response.acp),
    mcp: fromRawCommandAvailability(response.mcp),
  };
}

// ── Model discovery ───────────────────────────────────────────────────────────

export async function getAgentModels(pubkey: string) {
  return invokeTauri<AgentModelsResponse>("get_agent_models", { pubkey });
}

export async function getAgentConfigSurface(
  pubkey: string,
): Promise<RuntimeConfigSurface> {
  return invokeTauri<RuntimeConfigSurface>("get_agent_config_surface", {
    pubkey,
  });
}

export async function putAgentSessionConfig(
  pubkey: string,
  payload: unknown,
): Promise<void> {
  return invokeTauri<void>("put_agent_session_config", { pubkey, payload });
}

/** File-layer config for a runtime (e.g. `~/.config/goose/config.yaml`). */
export type RuntimeFileConfigSubset = {
  /** Provider set in the harness config file. */
  provider: string | null;
  /** Model set in the harness config file. */
  model: string | null;
  /** Credential env key names whose values are present in the file config. */
  satisfiedEnvKeys: string[];
};

/**
 * Get the file-layer config for a runtime so dialogs can show
 * "Set in goose config" instead of surfacing a false required-field marker.
 * Returns `null` when the runtime has no config file or it cannot be parsed.
 */
export async function getRuntimeFileConfig(
  runtimeId: string,
): Promise<RuntimeFileConfigSubset | null> {
  return invokeTauri<RuntimeFileConfigSubset | null>(
    "get_runtime_file_config",
    {
      runtimeId,
    },
  );
}

/**
 * Return the key names of all non-empty baked build env vars.
 *
 * Internal (Block) builds bake provider credentials into the binary at compile
 * time. This returns the *key names only* — never the values — so dialogs can
 * treat them as satisfied without exposing secrets to the frontend.
 *
 * OSS builds return an empty array (no baked env).
 */
export async function getBakedBuildEnvKeys(): Promise<string[]> {
  return invokeTauri<string[]>("get_baked_build_env_keys");
}

/**
 * A single baked build env entry.
 *
 * The value is already masked in Rust for secret keys (keys not in the
 * explicit safe-to-reveal allowlist: `BUZZ_AGENT_PROVIDER`, `BUZZ_AGENT_MODEL`,
 * `DATABRICKS_HOST`, `DATABRICKS_MODEL`). Non-allowlisted keys have their
 * values replaced with `••••••`. Non-secret values are shown as-is.
 * Empty-value keys are filtered out.
 */
export type BakedEnvEntry = {
  key: string;
  /** Display value — real value or `••••••` for masked keys. */
  value: string;
  /** `true` when the value was replaced by the mask placeholder in Rust. */
  masked: boolean;
};

/**
 * Return the baked build env entries with values shown (masked where
 * appropriate) for display in the Agent defaults card.
 *
 * Provider and model arrive as `BUZZ_AGENT_PROVIDER` / `BUZZ_AGENT_MODEL`
 * keys and are included in the list alongside other baked vars.
 *
 * OSS builds return an empty array — the baked-env section is hidden.
 */
export async function getBakedBuildEnv(): Promise<BakedEnvEntry[]> {
  return invokeTauri<BakedEnvEntry[]>("get_baked_build_env");
}

type RawUpdateManagedAgentResponse = {
  agent: RawManagedAgent;
  profile_sync_error: string | null;
};

export async function updateManagedAgent(
  input: UpdateManagedAgentInput,
): Promise<{ agent: ManagedAgent; profileSyncError: string | null }> {
  const response = await invokeTauri<RawUpdateManagedAgentResponse>(
    "update_managed_agent",
    { input },
  );
  return {
    agent: fromRawManagedAgent(response.agent),
    profileSyncError: response.profile_sync_error,
  };
}

// ── Backend provider discovery ────────────────────────────────────────────────

export async function discoverBackendProviders(): Promise<
  BackendProviderCandidate[]
> {
  return invokeTauri<BackendProviderCandidate[]>("discover_backend_providers");
}

export async function probeBackendProvider(
  binaryPath: string,
): Promise<BackendProviderProbeResult> {
  return invokeTauri<BackendProviderProbeResult>("probe_backend_provider", {
    binaryPath,
  });
}

// Validate a candidate repos dir without mutating the filesystem. Rejects
// with a human-readable reason; resolves for a valid or empty path.
export async function validateReposDir(dir: string): Promise<void> {
  await invokeTauri("validate_repos_dir", { dir });
}

export const setPreventSleepActive = (active: boolean) =>
  invokeTauri("set_prevent_sleep_active", { active });

export const setAgentManagedProfiles = (enabled: boolean) =>
  invokeTauri("set_agent_managed_profiles", { enabled });

/** Returns true on macOS, Windows, and Linux AppImage installs.
 *  Returns false on Linux non-AppImage packages (e.g. .deb) where
 *  Tauri's updater cannot swap the binary. */
export function isAutoUpdateSupported(): Promise<boolean> {
  return invokeTauri<boolean>("is_auto_update_supported");
}
