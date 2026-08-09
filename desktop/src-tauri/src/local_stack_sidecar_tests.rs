use super::*;

// ── #1 Packaged exe-parent sidecar resolution ───────────────────────────────

fn write_exec_file(dir: &std::path::Path, name: &str, exec: bool) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, b"#!/bin/sh\nexit 0\n").expect("write sidecar stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if exec { 0o755 } else { 0o644 };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }
    let _ = exec;
    path
}

#[test]
fn find_sidecar_resolves_stripped_exact_name() {
    let dir = tempfile::TempDir::new().unwrap();
    let bin = write_exec_file(dir.path(), "x0xd", true);
    assert_eq!(
        find_sidecar_in(dir.path(), "x0xd").as_deref(),
        Some(bin.as_path())
    );
}

#[test]
fn find_sidecar_resolves_triple_suffixed_when_no_exact() {
    let dir = tempfile::TempDir::new().unwrap();
    let bin = write_exec_file(dir.path(), "x0xd-aarch64-apple-darwin", true);
    assert_eq!(
        find_sidecar_in(dir.path(), "x0xd").as_deref(),
        Some(bin.as_path()),
        "triple-suffixed sidecar resolves when no exact name"
    );
}

#[test]
fn find_sidecar_prefers_exact_name_over_suffixed() {
    let dir = tempfile::TempDir::new().unwrap();
    let exact = write_exec_file(dir.path(), "x0xd", true);
    let _suffixed = write_exec_file(dir.path(), "x0xd-aarch64-apple-darwin", true);
    assert_eq!(
        find_sidecar_in(dir.path(), "x0xd").as_deref(),
        Some(exact.as_path()),
        "exact (stripped) name wins over a triple-suffixed sibling"
    );
}

#[test]
fn find_sidecar_returns_none_when_absent_or_dir_missing() {
    let dir = tempfile::TempDir::new().unwrap();
    assert_eq!(find_sidecar_in(dir.path(), "x0xd"), None);
    assert_eq!(
        find_sidecar_in(std::path::Path::new("/nonexistent/sidecar/dir"), "x0xd"),
        None
    );
}

#[test]
fn find_sidecar_ignores_unrelated_prefix_only_files() {
    let dir = tempfile::TempDir::new().unwrap();
    let _ = write_exec_file(dir.path(), "x0xdconfig", true);
    assert_eq!(find_sidecar_in(dir.path(), "x0xd"), None);
    let bin = write_exec_file(dir.path(), "x0xd-x86_64-pc-windows-gnu", true);
    assert_eq!(
        find_sidecar_in(dir.path(), "x0xd").as_deref(),
        Some(bin.as_path())
    );
}

#[cfg(unix)]
#[test]
fn validate_executable_accepts_executable_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let bin = write_exec_file(dir.path(), "x0xd", true);
    assert!(validate_executable(&bin, "x0xd").is_ok());
}

#[cfg(unix)]
#[test]
fn validate_executable_rejects_non_executable_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let bin = write_exec_file(dir.path(), "x0xd", false);
    assert!(
        validate_executable(&bin, "x0xd").is_err(),
        "no exec bit ⇒ reject"
    );
}

#[test]
fn validate_executable_rejects_directory_and_missing() {
    let dir = tempfile::TempDir::new().unwrap();
    assert!(
        validate_executable(dir.path(), "x0xd").is_err(),
        "directory is not an executable file"
    );
    assert!(
        validate_executable(std::path::Path::new("/nonexistent/x0xd-bin"), "x0xd").is_err(),
        "missing path rejected, not panicked"
    );
}
