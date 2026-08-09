//! App-integration lifecycle entrypoints for the local x0xd daemon.

use super::*;

pub(crate) fn fetch_agent() -> Result<Value, String> {
    let data_dir = named_data_dir().ok_or_else(|| "daemon data dir unavailable".to_string())?;
    let port = read_api_port(&data_dir)
        .ok_or_else(|| "daemon api.port missing or non-loopback".to_string())?;
    let token = read_api_token(&data_dir).ok_or_else(|| "daemon api-token missing".to_string())?;
    let url = format!("{}/agent", loopback_api_base(port));
    http_get_json(&url, Some(&token)).map_err(|_| "daemon /agent unreachable".to_string())
}

/// Attach to a healthy `x0xd --name ttt`, or spawn and own that daemon.
///
/// The desktop uses x0xd's authenticated REST/WS API directly. No compatibility
/// process is resolved, spawned, probed, or installed as a runtime default.
pub(crate) fn bring_up_local_stack(app: &tauri::AppHandle) {
    use tauri::Manager;

    let state = app.state::<crate::app_state::AppState>();
    let config = match StackConfig::resolve() {
        Ok(config) => config,
        Err(error) => {
            record_error(&state, error.to_string());
            return;
        }
    };

    let _ = std::fs::create_dir_all(&config.data_dir);
    let probe = LoopbackHttpDaemonProbe;
    let existing = healthy_endpoint(&config.data_dir, &probe);
    let (daemon, api_base) = match existing {
        Some(api_base) => (None, api_base),
        None => {
            let spec = SidecarCommand {
                label: "x0xd",
                binary: config.x0xd_binary.clone(),
                args: vec![
                    "--name".to_string(),
                    INSTANCE_NAME.to_string(),
                    "--skip-update-check".to_string(),
                ],
                env: Vec::new(),
                log_path: Some(config.data_dir.join("x0xd.log")),
            };
            let child = match StdSidecarSpawner.spawn(&spec) {
                Ok(child) => child,
                Err(error) => {
                    record_error(&state, format!("x0xd spawn failed: {error:?}"));
                    return;
                }
            };
            match wait_for_daemon(&config, &probe) {
                Ok(api_base) => (Some(child), api_base),
                Err(message) => {
                    record_error(&state, message);
                    return;
                }
            }
        }
    };

    let handle = LocalStackHandle {
        daemon,
        bridge: None,
        ws_url: String::new(),
    };
    match state.local_stack.lock() {
        Ok(mut guard) => *guard = Some(handle),
        Err(poisoned) => *poisoned.into_inner() = Some(handle),
    }
    eprintln!("local-x0xd: ready at {api_base}");
}

fn healthy_endpoint(data_dir: &Path, probe: &impl DaemonProbe) -> Option<String> {
    let port = read_api_port(data_dir)?;
    let token = read_api_token(data_dir)?;
    let api_base = loopback_api_base(port);
    probe.health(&api_base, &token).ok()?;
    Some(api_base)
}

fn wait_for_daemon(config: &StackConfig, probe: &impl DaemonProbe) -> Result<String, String> {
    let deadline = Instant::now() + config.daemon_timeout;
    loop {
        if let Some(api_base) = healthy_endpoint(&config.data_dir, probe) {
            return Ok(api_base);
        }
        if Instant::now() >= deadline {
            return Err("timed out waiting for x0xd health".to_string());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

pub(crate) fn shutdown_owned(app: &tauri::AppHandle) {
    use tauri::Manager;
    shutdown_state(&app.state::<crate::app_state::AppState>());
}

pub(crate) fn shutdown_state(state: &crate::app_state::AppState) {
    let mut guard = state
        .local_stack
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(mut handle) = guard.take() {
        handle.shutdown();
    }
}

fn record_error(state: &crate::app_state::AppState, message: String) {
    eprintln!("local-x0xd: {message}");
    match state.local_stack_error.lock() {
        Ok(mut guard) => *guard = Some(message),
        Err(poisoned) => *poisoned.into_inner() = Some(message),
    }
}
