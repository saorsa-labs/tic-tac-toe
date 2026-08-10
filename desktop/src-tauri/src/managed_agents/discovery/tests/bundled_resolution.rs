use super::super::{command_search_dirs_for, profile_target_dirs, resolve_command_in_dirs};

#[test]
fn packaged_exe_sidecar_beats_workspace_artifact() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    let workspace_bin = profile_target_dirs(&workspace)[0].join("buzz-agent");
    let bundled_dir = temp.path().join("installed/tic-tac-toe.app/Contents/MacOS");
    let bundled_bin = bundled_dir.join("buzz-agent");
    for binary in [&workspace_bin, &bundled_bin] {
        std::fs::create_dir_all(binary.parent().expect("binary parent")).expect("create bin dir");
        std::fs::write(binary, b"sidecar").expect("write sidecar");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o755))
                .expect("chmod sidecar");
        }
    }
    let dirs = command_search_dirs_for(&workspace, None, Some(bundled_dir));
    assert_eq!(
        resolve_command_in_dirs("buzz-agent", dirs),
        Some(bundled_bin)
    );
}
