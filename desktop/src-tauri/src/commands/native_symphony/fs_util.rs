use std::path::Path;

pub(super) fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Company workflow path has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create Company instance directory: {error}"))?;
    restrict_dir(parent);
    let temporary = path.with_extension("md.tmp");
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("failed to write Company workflow: {error}"))?;
    restrict_file(&temporary);
    std::fs::rename(&temporary, path)
        .map_err(|error| format!("failed to commit Company workflow: {error}"))
}

/// Set directory permissions to 0700 (owner-only) on Unix; no-op elsewhere.
#[cfg(unix)]
fn restrict_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

/// Set file permissions to 0600 (owner-only) on Unix; no-op elsewhere.
#[cfg(unix)]
fn restrict_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_dir(_path: &Path) {}

#[cfg(not(unix))]
fn restrict_file(_path: &Path) {}
