// Modified from block/buzz @ 710ed9ff — see FORK.md (native x0xd data layer)
#![recursion_limit = "256"] // Deep Tauri command futures exceed the default layout query depth.
mod app_state;
mod commands;
mod company_template;
mod deep_link;
mod local_stack;
mod managed_agents;
mod migration;
mod models;
mod prevent_sleep;
mod reset;
mod secret_store;
mod shutdown;
mod symphony;
mod symphony_client;
mod templates;
mod util;
mod x0x_client;
use app_state::{resolve_persisted_identity, try_build_app_state, AppState};
use commands::*;
use deep_link::handle_deep_link_url;
use managed_agents::{
    backfill_persona_snapshots, ensure_nest, list_managed_agent_runtimes,
    put_managed_agent_runtime_lifecycle, restart_managed_agent_runtime,
    start_managed_agent_runtime, stop_managed_agent_runtime, try_regenerate_nest,
};
use shutdown::shut_down_app;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
#[cfg(target_os = "macos")]
use tauri::Listener;
use tauri::{Emitter, Manager, RunEvent};
use tauri_plugin_window_state::StateFlags;

#[cfg(target_os = "macos")]
const INITIAL_RENDER_READY_EVENT: &str = "initial-render-ready";

fn reveal_initial_window<R: tauri::Runtime>(window: &tauri::Window<R>) {
    if let Err(error) = window.show() {
        eprintln!("buzz-desktop: failed to reveal main window: {error}");
        return;
    }
    if let Err(error) = window.set_focus() {
        eprintln!("buzz-desktop: failed to focus main window: {error}");
    }
}

#[cfg(target_os = "macos")]
fn set_initial_window_backing<R: tauri::Runtime>(window: &tauri::Window<R>) {
    // The window remains transparent at runtime for vibrancy. Use an opaque
    // native backing only across the first visible frames so the previous app
    // cannot show through before WebKit has submitted its first surface.
    if let Err(error) = window.set_background_color(Some(tauri::window::Color(17, 21, 24, 255))) {
        eprintln!("buzz-desktop: failed to set initial window backing: {error}");
    }
}

#[cfg(target_os = "macos")]
async fn clear_initial_window_backing<R: tauri::Runtime>(window: &tauri::Window<R>) {
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    if let Err(error) = window.set_background_color(None) {
        eprintln!("buzz-desktop: failed to clear initial window backing: {error}");
    }
}

#[cfg(target_os = "macos")]
async fn wait_for_stable_initial_window_geometry<R: tauri::Runtime>(window: &tauri::Window<R>) {
    const MAX_POLLS: usize = 120;
    const REQUIRED_STABLE_POLLS: usize = 4;

    let mut previous_bounds = None;
    let mut stable_polls = 0;

    for _ in 0..MAX_POLLS {
        // Accept whatever geometry the window-state plugin restores — maximized
        // or a normal saved size. macOS applies the restore asynchronously, so
        // we only need consecutive identical outer bounds to know it settled.
        // Gating on `is_maximized()` here would leave `bounds` permanently
        // `None` for restored non-maximized windows and stall the reveal until
        // the poll timeout.
        let bounds = match (window.outer_position(), window.outer_size()) {
            (Ok(position), Ok(size)) => Some((position.x, position.y, size.width, size.height)),
            _ => None,
        };

        if bounds.is_some() && bounds == previous_bounds {
            stable_polls += 1;
            if stable_polls >= REQUIRED_STABLE_POLLS {
                return;
            }
        } else {
            stable_polls = 0;
        }
        previous_bounds = bounds;

        tokio::time::sleep(std::time::Duration::from_millis(16)).await;
    }

    eprintln!("buzz-desktop: initial window geometry did not settle before reveal timeout");
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = match try_build_app_state() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("buzz-desktop: failed to initialize HTTP clients: {error}");
            return;
        }
    };

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // Focus the existing window when a duplicate instance launches.
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
            // Forward application message links from the duplicate launch.
            for arg in &argv {
                if arg.starts_with("buzz://") {
                    handle_deep_link_url(app, arg);
                }
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                // Visibility is excluded: the native reveal plugin below
                // shows the window after saved geometry has been restored.
                .with_state_flags(StateFlags::all() & !StateFlags::VISIBLE)
                .build(),
        )
        .plugin(
            tauri::plugin::Builder::<_, ()>::new("initial-window-reveal")
                .on_webview_ready(|webview| {
                    if webview.label() != "main" {
                        return;
                    }

                    // macOS applies the restored geometry asynchronously. Wait
                    // for several identical outer bounds and for React to
                    // commit the startup surface before revealing it.
                    let window = webview.window();

                    #[cfg(target_os = "macos")]
                    {
                        set_initial_window_backing(&window);

                        let (initial_render_tx, initial_render_rx) = tokio::sync::oneshot::channel();
                        window
                            .app_handle()
                            .once(INITIAL_RENDER_READY_EVENT, move |_| {
                                let _ = initial_render_tx.send(());
                            });

                        tauri::async_runtime::spawn(async move {
                            wait_for_stable_initial_window_geometry(&window).await;

                            if tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                initial_render_rx,
                            )
                            .await
                            .is_err()
                            {
                                eprintln!(
                                    "buzz-desktop: initial render did not commit before reveal timeout"
                                );
                            }

                            reveal_initial_window(&window);
                            clear_initial_window_backing(&window).await;
                        });
                    }

                    #[cfg(not(target_os = "macos"))]
                    {
                        reveal_initial_window(&window);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init());

    // Register the updater only in configured release builds; omit it locally.
    #[cfg(buzz_updater_enabled)]
    let builder = if cfg!(debug_assertions) {
        builder
    } else {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    };

    #[cfg(not(buzz_updater_enabled))]
    let builder = builder;

    let app = builder
        .manage(app_state)
        .manage(ClipboardState::new())
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // ── Phase 2: boot-time sentinel wipe ──────────────────────────────
            // Must run before migrations and identity resolution so the wipe
            // completes atomically on crash recovery.
            //
            // init_nest_dir is called early here (normally it runs inside
            // run_boot_migrations) so reset::run_boot_reset can call nest_dir().
            let reset_outcome = if let Ok(data_dir) = app_handle.path().app_data_dir() {
                let is_dev_for_reset = data_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .map(crate::migration::is_dev_data_dir_name)
                    .unwrap_or(false);
                crate::managed_agents::init_nest_dir(is_dev_for_reset);
                crate::reset::run_boot_reset(&data_dir)
            } else {
                crate::reset::ResetOutcome::default()
            };

            if reset_outcome.failed {
                // Surface reset-failed state — skip identity resolution and
                // all side-effecting setup. The webview still loads so the
                // frontend can show the recovery screen via get_identity.
                let state = app_handle.state::<AppState>();
                state
                    .reset_failed
                    .store(true, std::sync::atomic::Ordering::Release);
                return Ok(());
            }

            // Run all pre-identity data migrations before state loads from disk.
            if reset_outcome.completed {
                migration::run_boot_migrations_after_reset(&app_handle);
            } else {
                migration::run_boot_migrations(&app_handle);
            }

            // Resolve persisted identity key (env var → file → generate+save).
            // This is fatal — the app should not start with an ephemeral identity
            // that will be lost on restart, as that silently breaks channel
            // memberships, DMs, and relay identity.
            let state = app_handle.state::<AppState>();
            if let Err(e) = resolve_persisted_identity(&app_handle, &state) {
                eprintln!("buzz-desktop: fatal: identity resolution failed: {e}");
                std::process::exit(1);
            }

            // When the identity is in recovery mode (lost = keyring empty after
            // migration, or keyring-locked = keyring unreachable but marker
            // present), all owner-keyed side effects (event sync, agent restore,
            // relay publish) are skipped. The frontend shows a recovery screen;
            // the user must relaunch after restoring the identity.
            let identity_lost = state
                .identity_lost
                .load(std::sync::atomic::Ordering::Acquire);
            let keyring_locked = state
                .keyring_locked
                .load(std::sync::atomic::Ordering::Acquire);
            let recovery_mode = identity_lost || keyring_locked;

            // Backfill the pinned persona snapshot for any pre-existing agent
            // that predates the record-authoritative-spawn cutover (persona_id
            // set but no source_version). Must run before
            // restore_managed_agents_on_launch so no agent spawns from an empty
            // snapshot. Synchronous and best-effort — a failure here must not
            // block launch, but a missing persona is logged loudly inside.
            if let Err(e) = backfill_persona_snapshots(&app_handle) {
                eprintln!("buzz-desktop: persona-snapshot backfill failed: {e}");
            }

            // Spawn or attach to the named local x0xd instance. The desktop
            // communicates with its authenticated REST/WS API directly.
            // Skipped in identity-recovery mode to keep recovery boot fast.
            if !recovery_mode {
                crate::local_stack::bring_up_local_stack(&app_handle);

                // Company boot reconciliation: deterministically resume any
                // incomplete provisioning and rebind the single newest
                // non-cancelled Company instance to the supervised symphony
                // daemon. Best-effort + async — never blocks launch; errors are
                // recorded to stderr and surfaced via symphony_supervision_status.
                let reconcile_handle = app_handle.clone();
                tauri::async_runtime::spawn(async move {
                    crate::commands::reconcile_companies(&reconcile_handle).await;
                });
            }

            // Create the Buzz nest (~/.buzz or ~/.buzz-dev for dev builds) before
            // agents are restored, so default_agent_workdir() resolves to the
            // nest directory. Non-fatal: agents fall back to $HOME if nest
            // creation fails.
            if let Err(error) = ensure_nest() {
                eprintln!("buzz-desktop: failed to create nest: {error}");
            }

            // Resolve the REPOS symlink from the persisted repos_dir BEFORE
            // agents are restored below, and decide whether restore is safe.
            // The frontend's apply_workspace runs only after React mounts —
            // later than the async agent restore — so without this an agent
            // could clone into the empty real REPOS dir, and once REPOS is
            // non-empty ensure_repos_symlink refuses forever. resolve_repos_at_boot
            // fails closed: if a repos_dir was configured but its symlink could
            // not be resolved (transiently unavailable external volume), it
            // returns false so we skip restore this launch rather than let an
            // agent clone into the wrong REPOS. See managed_agents::repos.
            let restore_agents = match managed_agents::nest_dir() {
                Some(nest) => managed_agents::resolve_repos_at_boot(&nest),
                None => true,
            };

            // Carry the agent's knowledge from the legacy nest (~/.sprout) into
            // the live nest after it exists. Must run after ensure_nest() so the
            // destination is present. Non-fatal.
            // On a real migration, emit a one-time hint so the user can delete
            // the now-inert ~/.sprout; the frontend dedupes the toast.
            // Suppressed when a reset completed this boot: the nest was wiped and
            // a fresh ~/.sprout-less state is exactly what we want.
            if !reset_outcome.completed && migration::migrate_legacy_nest() {
                let _ = app_handle.emit("legacy-nest-migrated", ());
            }

            // One-time migration for dev builds: copy accumulated knowledge
            // from the shared ~/.buzz nest into the new dedicated ~/.buzz-dev
            // nest so no work is lost when the nest is first namespaced.
            // Runs only when nest_dir() resolved to ~/.buzz-dev (dev instance).
            // Suppressed after a reset so re-importing ~/.buzz into ~/.buzz-dev
            // doesn't re-populate what was just wiped.
            let is_dev_nest = managed_agents::nest_dir()
                .and_then(|p| p.file_name().map(|n| n.to_os_string()))
                .is_some_and(|n| n == ".buzz-dev");
            if !reset_outcome.completed && is_dev_nest {
                migration::migrate_dev_nest();
            }

            // Create/update the local CLI symlink pointing to the
            // bundled CLI binary. Non-fatal: agents find CLI via PATH.
            if let Ok(exe) = std::env::current_exe() {
                if let Some(parent) = exe.parent() {
                    if let Err(error) = managed_agents::ensure_cli_symlink(parent, is_dev_nest) {
                        eprintln!("buzz-desktop: failed to create CLI symlink: {error}");
                    }
                }
            }

            try_regenerate_nest(&app_handle);

            // Handle deep link URLs received while the app is running (macOS)
            // and on cold start. The single-instance plugin handles forwarding
            // from duplicate launches on Windows/Linux.
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let dl_handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        handle_deep_link_url(&dl_handle, url.as_str());
                    }
                });
            }

            // Defer launch-time agent restoration until the frontend binds the
            // daemon's active named group. Starting here would race native
            // identity/group initialization. Preserve boot-time repository and
            // identity recovery gates by marking restoration pending only when
            // both allow it.
            if restore_agents && !recovery_mode {
                state
                    .managed_agent_restore_pending
                    .store(true, Ordering::Release);
            }

            // Periodic sweep: reap orphaned agents from dead instances every 60s.
            // Catches agents that escaped both the Justfile trap and boot-time
            // reaping (e.g. a `just staging` Ctrl+C leak that only gets collected
            // by a different instance's periodic sweep).
            let sweep_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use std::collections::HashSet;
                use std::time::Duration;
                use tauri::Manager;
                let instance_id = managed_agents::current_instance_id(&sweep_handle);
                let state = sweep_handle.state::<AppState>();
                // Two-tick grace: only reap same-instance orphans seen on two
                // consecutive sweeps. Prevents killing a legitimately-starting
                // agent that spawned between the skip-list snapshot and the scan.
                let mut prev_orphans: HashSet<u32> = HashSet::new();
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    // Collect PIDs of our own live agents to avoid killing them.
                    let skip_pids: Vec<u32> = state
                        .managed_agent_processes
                        .lock()
                        .map(|runtimes| runtimes.values().map(|rt| rt.child.id()).collect())
                        .unwrap_or_default();
                    let prev = prev_orphans.clone();
                    let inst = instance_id.clone();
                    // Run the blocking syscall work off the async executor.
                    let new_orphans = tauri::async_runtime::spawn_blocking(move || {
                        let orphans = managed_agents::sweep_system_agent_processes_with_grace(
                            &inst, &skip_pids, &prev,
                        );
                        managed_agents::reap_dead_instance_agents(&inst, &skip_pids);
                        orphans
                    })
                    .await
                    .unwrap_or_default();
                    prev_orphans = new_orphans;
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            title_bar_double_click,
            get_identity,
            get_recovery_state,
            recover_lost_identity,
            get_git_identity,
            get_project_repo_snapshot,
            get_project_repo_diff,
            get_project_local_repo_diff,
            get_project_local_repo_snapshot,
            get_project_repo_sync_status,
            list_project_local_repositories,
            clone_project_repository,
            create_project_remote_branch,
            delete_project_remote_branch,
            push_project_local_repository,
            pull_project_local_repository,
            merge_project_pull_request,
            open_project_terminal,
            open_project_merge_recovery_terminal,
            get_os_idle_seconds,
            is_shared_identity,
            fetch_link_preview_title,
            discover_acp_auth_methods,
            discover_acp_providers,
            discover_git_bash_prerequisite,
            install_acp_runtime,
            connect_acp_runtime,
            discover_managed_agent_prereqs,
            sign_out,
            get_managed_agent_native_identity,
            show_native_notification,
            save_png_data_url,
            copy_text_to_clipboard,
            list_managed_agents,
            list_managed_agent_runtimes,
            start_managed_agent_runtime,
            stop_managed_agent_runtime,
            restart_managed_agent_runtime,
            put_managed_agent_runtime_lifecycle,
            create_managed_agent,
            start_managed_agent,
            stop_managed_agent,
            set_agent_managed_profiles,
            set_managed_agent_start_on_app_launch,
            set_managed_agent_auto_restart,
            delete_managed_agent,
            get_managed_agent_log,
            get_agent_models,
            discover_agent_models,
            get_agent_config_surface,
            get_runtime_file_config,
            get_baked_build_env_keys,
            get_baked_build_env,
            put_agent_session_config,
            get_global_agent_config,
            set_global_agent_config,
            update_managed_agent,
            discover_backend_providers,
            probe_backend_provider,
            list_personas,
            create_persona,
            update_persona,
            delete_persona,
            set_persona_active,
            list_channel_templates,
            create_channel_template,
            update_channel_template,
            delete_channel_template,
            duplicate_channel_template,
            list_teams,
            create_team,
            update_team,
            delete_team,
            export_agent_snapshot,
            preview_agent_snapshot_import,
            confirm_agent_snapshot_import,
            encode_agent_snapshot_for_send,
            export_team_snapshot,
            encode_team_snapshot_for_send,
            preview_team_snapshot_import,
            confirm_team_snapshot_import,
            perform_sidebar_default_haptic,
            validate_repos_dir,
            x0x_history_list,
            x0x_history_search,
            x0x_history_get,
            x0x_publish,
            x0x_send_group_message,
            x0x_send_direct_message,
            x0x_subscribe_live,
            x0x_close_live,
            x0x_close_all_live,
            x0x_get_active_group_id,
            x0x_set_active_group_id,
            x0x_list_groups,
            x0x_get_group,
            x0x_get_group_members,
            x0x_create_group,
            x0x_add_group_member,
            x0x_set_group_member_role,
            x0x_remove_group_member,
            x0x_ban_group_member,
            x0x_unban_group_member,
            x0x_leave_group,
            x0x_update_group,
            x0x_connect_agent,
            x0x_list_contacts,
            x0x_add_contact,
            x0x_remove_contact,
            x0x_set_group_display_name,
            x0x_get_presence,
            x0x_list_task_lists,
            x0x_create_task_list,
            x0x_list_tasks,
            x0x_add_task,
            x0x_update_task,
            x0x_list_stores,
            x0x_create_store,
            x0x_join_store,
            x0x_list_store_keys,
            x0x_get_store_value,
            x0x_put_store_value,
            x0x_delete_store_value,
            x0x_get_agent_card,
            x0x_import_agent_card,
            symphony_supervision_status,
            start_symphony,
            stop_symphony,
            symphony_tasks,
            symphony_task,
            symphony_status,
            symphony_workers,
            symphony_approvals_pending,
            symphony_approve,
            symphony_deny,
            symphony_create_issue,
            symphony_claim,
            symphony_handoff,
            symphony_proofs,
            symphony_proof,
            symphony_routes,
            symphony_subscribe_events,
            list_company_templates,
            instantiate_company_template,
            resume_company_instance,
            cancel_company_run,
            list_company_instances,
            set_prevent_sleep_active,
            is_auto_update_supported,
            set_window_vibrancy,
        ])
        .build(tauri::generate_context!());

    let app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("buzz-desktop: failed to build tauri application: {error}");
            return;
        }
    };

    let shutdown_done = Arc::new(AtomicBool::new(false));

    #[cfg(unix)]
    shutdown::install_signal_handler(app.handle().clone(), Arc::clone(&shutdown_done));

    let run_shutdown_done = Arc::clone(&shutdown_done);
    app.run(move |app_handle, event| match event {
        RunEvent::ExitRequested { .. } => {
            shut_down_app(app_handle, &run_shutdown_done);
        }
        RunEvent::Exit => {
            shut_down_app(app_handle, &run_shutdown_done);
            app_handle.state::<ClipboardState>().release();
        }
        _ => {}
    });
}
