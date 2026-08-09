// Modified from block/buzz @ 710ed9ff — see FORK.md (Stage 1: local x0xd + bridge supervisor state)
use std::{
    collections::HashMap,
    sync::{atomic::AtomicBool, Arc, Mutex},
};

use crate::managed_agents::config_bridge::SessionConfigCache;
use crate::managed_agents::{ManagedAgentPairRuntime, ManagedAgentRuntimeKey};
pub struct AppState {
    pub http_client: reqwest::Client,
    /// The active workspace's native x0x named-group id (opaque stable string
    /// — the daemon's roster id, NOT an MLS hex or pubkey). Bound when a
    /// community is created (`x0x_create_group`)
    /// and applied on workspace activation. Read by `x0x_get_active_group_id`
    /// for runtime keying (ManagedAgentRuntimeKey.group_id) and group-scoped
    /// history/membership resolution. Fail-closed: `None` until a native group
    /// is bound, so callers never synthesize a relay-scope surrogate.
    #[allow(dead_code)]
    // read by x0x_get_active_group_id (native group command, landing alongside)
    pub active_group_id: Mutex<Option<String>>,
    /// Set during backend setup when managed agents are eligible for launch
    /// restore. `apply_workspace` consumes it after installing the workspace
    /// relay and identity, so agents never start against the fallback relay.
    pub managed_agent_restore_pending: AtomicBool,
    /// Whether desktop may repair managed-agent kind:0 profiles from its local
    /// records. Disabled by the agent-managed profiles experiment so an agent's
    /// own profile updates are not overwritten on start or restore.
    pub managed_agent_profile_reconcile_enabled: AtomicBool,
    /// Shared shutdown signal checked by launch-time agent restoration.
    pub shutdown_started: AtomicBool,
    /// Serializes every managed-runtime transition that changes the protected
    /// PID set: spawn/register, adoption, stop, shutdown, and sweep snapshots.
    /// Never perform network I/O while holding this lock.
    pub managed_agent_runtime_transition: Mutex<()>,
    pub managed_agents_store_lock: Mutex<()>,
    pub channel_templates_store_lock: Mutex<()>,
    pub managed_agent_processes: Mutex<HashMap<ManagedAgentRuntimeKey, ManagedAgentPairRuntime>>,
    /// Set when the legacy-identity migration cannot reach the OS keyring this
    /// boot but a migration marker shows a key was archived there. The frontend
    /// routes to a "unlock the keyring and relaunch" recovery screen. Mutually
    /// exclusive with `identity_lost` (guaranteed at the resolve boundary).
    ///
    /// Ordering: writers store with `Ordering::Release` after the migration
    /// runs, so a reader observing `false` with `Ordering::Acquire` sees the
    /// completed state. Writer: `setup()` via `resolve_persisted_identity`.
    pub keyring_locked: AtomicBool,
    /// Set when the legacy-identity migration found a prior migration marker
    /// but the keyring is reachable-and-empty and no plaintext `identity.key`
    /// fallback exists — the archived key is gone. The frontend routes to a
    /// recovery screen. The displayed identity remains the daemon AgentId.
    ///
    /// Ordering: writers store with `Ordering::Release` after the migration
    /// runs. Writer: `setup()` via `resolve_persisted_identity`.
    pub identity_lost: AtomicBool,
    /// Set when the boot-time Phase 2 reset attempted a wipe but verification
    /// failed. The sentinel is preserved so the next relaunch retries;
    /// identity resolution was skipped. The frontend shows a reset-failed
    /// recovery screen via `get_recovery_state`.
    ///
    /// Ordering: written once in `setup()` with `Ordering::Release`; read in
    /// `get_recovery_state` and `signing_keys` with `Ordering::Acquire`.
    pub reset_failed: AtomicBool,
    /// Cached ACP session config from running agents, keyed by canonical
    /// `(agent pubkey, relay URL)` runtime identity.
    /// Populated when the harness emits `session_config_captured` observer events.
    pub session_config_cache: Mutex<HashMap<ManagedAgentRuntimeKey, SessionConfigCache>>,
    /// IOKit power assertion state — prevents idle sleep while agents run.
    pub prevent_sleep: Arc<Mutex<crate::prevent_sleep::PreventSleepState>>,
    /// The local x0xd + x0x-nostr-bridge supervisor handle, set during setup
    /// once bring-up succeeds. `None` until then, or permanently on failure.
    /// Shutdown takes (reaps) only the app-owned children stored here.
    pub local_stack: Mutex<Option<crate::local_stack::LocalStackHandle>>,
    /// Typed error from a failed local-stack bring-up, captured so a recovery
    /// path can surface it. `None` on success or before setup runs.
    pub local_stack_error: Mutex<Option<String>>,
    /// Authenticated loopback `x0xd` REST/WS client (M3 native transport).
    /// Resolves the transient daemon token/port per call from the local-stack
    /// named data dir — the token is never stored here. Cheaply [`Clone`]
    /// (shares the pooled HTTP client). See [`crate::x0x_client`].
    pub x0x_client: crate::x0x_client::X0xClient,
    /// The supervised x0x-symphonyd handle, set when bring-up succeeds.
    /// `None` until started or permanently on failure. Shutdown reaps only the
    /// app-owned child stored here. Symphony is reaped *before* the x0xd
    /// daemon (it depends on x0xd for signing identity). Independent of the
    /// x0xd local stack above.
    pub local_symphony: Mutex<Option<crate::symphony::SymphonyHandle>>,
    /// Typed error from a failed symphony bring-up, captured so a recovery path
    /// can surface it. `None` on success or before start.
    pub symphony_error: Mutex<Option<String>>,
    /// The instance id of the Company currently bound to the supervised
    /// symphony daemon, or `None` when no company is active. The
    /// single-active-company invariant is enforced at instantiation (a second
    /// company is refused while a non-cancelled instance exists on disk); this
    /// field tracks the *bound* active instance for supervision/listing. It is
    /// cleared on cancel and rebound deterministically by boot reconciliation.
    pub active_company_instance: Mutex<Option<String>>,
    /// Serializes the single-active-company reservation: held across the
    /// active-instance scan and the initial reservation-manifest write so two
    /// concurrent `instantiate_company_template` calls cannot both pass the
    /// scan and create a second company (check-then-act race). Held only for a
    /// brief sync section — never across an await.
    pub company_instantiate_lock: Mutex<()>,
    /// Serializes the full Company lifecycle (instantiate / resume / boot
    /// reconcile): held across every await of a run so two lifecycle attempts
    /// on the one permitted non-cancelled instance never overlap. An async
    /// mutex (not `std::Mutex`) precisely because the critical section spans
    /// provisioning, identity, membership, Symphony bind, and run-issue awaits.
    pub company_run_lock: tokio::sync::Mutex<()>,
}

pub fn try_build_app_state() -> reqwest::Result<AppState> {
    // No env-var signer: production identity is the x0x AgentId (native daemon).
    // The legacy Nostr identity.key is migrated opaquely by
    // `resolve_persisted_identity` and never activated as a signer.
    let http_client = reqwest::Client::builder()
        .resolve("localhost", std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .pool_idle_timeout(std::time::Duration::from_secs(10))
        .pool_max_idle_per_host(1)
        .build()?;

    Ok(AppState {
        http_client: http_client.clone(),
        x0x_client: crate::x0x_client::X0xClient::new(http_client),
        active_group_id: Mutex::new(None),
        managed_agent_restore_pending: AtomicBool::new(false),
        managed_agent_profile_reconcile_enabled: AtomicBool::new(true),
        shutdown_started: AtomicBool::new(false),
        managed_agent_runtime_transition: Mutex::new(()),
        managed_agents_store_lock: Mutex::new(()),
        channel_templates_store_lock: Mutex::new(()),
        managed_agent_processes: Mutex::new(HashMap::new()),
        session_config_cache: Mutex::new(HashMap::new()),
        prevent_sleep: Arc::new(Mutex::new(
            crate::prevent_sleep::PreventSleepState::default(),
        )),
        keyring_locked: AtomicBool::new(false),
        identity_lost: AtomicBool::new(false),
        reset_failed: AtomicBool::new(false),
        active_company_instance: Mutex::new(None),
        company_instantiate_lock: Mutex::new(()),
        company_run_lock: tokio::sync::Mutex::new(()),
        local_stack: Mutex::new(None),
        local_stack_error: Mutex::new(None),
        local_symphony: Mutex::new(None),
        symphony_error: Mutex::new(None),
    })
}

#[cfg(test)]
pub fn build_app_state() -> AppState {
    try_build_app_state().expect("test app state clients must build")
}

impl AppState {
    pub fn get_session_cache(&self, key: &ManagedAgentRuntimeKey) -> Option<SessionConfigCache> {
        self.session_config_cache.lock().ok()?.get(key).cloned()
    }

    pub fn put_session_cache(&self, key: ManagedAgentRuntimeKey, cache: SessionConfigCache) {
        if let Ok(mut map) = self.session_config_cache.lock() {
            map.insert(key, cache);
        }
    }

    pub fn clear_agent_session_cache(&self, key: &ManagedAgentRuntimeKey) {
        if let Ok(mut map) = self.session_config_cache.lock() {
            map.remove(key);
        }
    }

    pub fn clear_agent_session_caches(&self, pubkey: &str) {
        if let Ok(mut map) = self.session_config_cache.lock() {
            map.retain(|key, _| key.pubkey != pubkey);
        }
    }
}

#[path = "app_state_keyring.rs"]
mod keyring_config;
pub(crate) use keyring_config::keyring_service;

#[path = "app_state_identity.rs"]
mod identity;

pub use identity::{replace_lost_identity, resolve_persisted_identity};

#[cfg(test)]
#[path = "app_state_tests.rs"]
mod tests;
