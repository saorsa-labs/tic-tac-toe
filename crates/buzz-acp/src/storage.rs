use std::io::Write as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

pub(crate) fn group_scope_hash(group_id: &str) -> String {
    hex::encode(Sha256::digest(group_id.as_bytes()))
}

pub(crate) fn scoped_path(data_dir: &Path, group_id: &str, suffix: &str) -> PathBuf {
    data_dir.join(format!("buzz-acp-{}.{suffix}", group_scope_hash(group_id)))
}

pub(crate) fn ensure_dir(path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        std::fs::File::open(path)?.sync_all()?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
    }
    Ok(())
}

/// Replace one state artifact atomically and durably. The unique temporary
/// file is synced before rename, and the containing directory is synced after
/// rename so a reported transition survives a power loss.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| std::io::Error::other("state path has no UTF-8 filename"))?;
    let temporary = path.with_file_name(format!("{file_name}.{}.tmp", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn remove_durable(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scoped_paths_hash_opaque_group_ids() {
        let root = Path::new("/tmp/child");
        let first = scoped_path(root, "route/../one", "pending");
        let second = scoped_path(root, "route/../two", "pending");
        assert_ne!(first, second);
        assert_eq!(first.parent(), Some(root));
        assert!(!first.to_string_lossy().contains("route"));
        assert_eq!(group_scope_hash("route/../one").len(), 64);
    }

    #[test]
    fn atomic_write_ignores_crash_leftover_temporary_files() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("state.json");
        std::fs::write(directory.path().join("state.json.crashed.tmp"), b"partial")
            .expect("write crash artifact");
        write_atomic(&path, br#"{"ok":true}"#).expect("write final state");
        assert_eq!(std::fs::read(path).expect("read final"), br#"{"ok":true}"#);
    }
}
