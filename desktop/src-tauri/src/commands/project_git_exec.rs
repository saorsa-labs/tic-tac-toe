//! Shared git subprocess plumbing for the project commands.
//!
//! Runs the system `git` in a hardened, env-scrubbed subprocess: no inherited
//! credential helpers, no repo-local hooks, and no global/system git config,
//! with a wall-clock cap per invocation. Authentication is left to git's
//! native transports (ssh-agent, OS keychain) — the packaged app carries no
//! signing identity to inject.

use crate::managed_agents::resolve_command;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use url::Url;

/// Wall-clock cap for a single git invocation. Remote operations talk to
/// external clone URLs, so a slow or adversarial remote must not pin
/// `spawn_blocking` threads indefinitely.
const LOCAL_GIT_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_GIT_TIMEOUT: Duration = Duration::from_secs(300);

fn git_subcommand<'a>(args: &'a [&str]) -> Option<&'a str> {
    let mut index = 0;
    while let Some(argument) = args.get(index).copied() {
        match argument {
            "-c" | "--config" | "-C" | "--git-dir" | "--work-tree" => index += 2,
            "--no-pager" | "--paginate" | "--end-of-options" => index += 1,
            argument
                if argument.starts_with("--config=")
                    || argument.starts_with("--git-dir=")
                    || argument.starts_with("--work-tree=") =>
            {
                index += 1;
            }
            argument if argument.starts_with('-') => index += 1,
            subcommand => return Some(subcommand),
        }
    }
    None
}

fn git_needs_credentials(args: &[&str]) -> bool {
    matches!(
        git_subcommand(args),
        Some("clone" | "fetch" | "push" | "pull" | "ls-remote" | "merge")
    )
}

pub(crate) struct GitAuthConfig {
    git_path: std::path::PathBuf,
    allow_file_transport: bool,
}

fn read_pipe_lossy(pipe: Option<impl Read>) -> String {
    let Some(mut pipe) = pipe else {
        return String::new();
    };
    let mut bytes = Vec::new();
    let _ = pipe.read_to_end(&mut bytes);
    String::from_utf8_lossy(&bytes).to_string()
}

pub(crate) fn run_git(
    args: &[&str],
    cwd: Option<&std::path::Path>,
    auth: &GitAuthConfig,
) -> Result<String, String> {
    let mut command = Command::new(&auth.git_path);
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let needs_credentials = git_needs_credentials(args);
    let timeout = if needs_credentials {
        REMOTE_GIT_TIMEOUT
    } else {
        LOCAL_GIT_TIMEOUT
    };
    configure_git_auth(&mut command, auth);
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    crate::util::configure_no_window(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| format!("failed to run git: {error}"))?;

    // Drain the pipes on background threads so a chatty git process can't
    // deadlock on a full pipe while we poll for exit below.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();
    let stdout_thread = std::thread::spawn(move || read_pipe_lossy(stdout_pipe));
    let stderr_thread = std::thread::spawn(move || read_pipe_lossy(stderr_pipe));

    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if started.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_thread.join();
                    let _ = stderr_thread.join();
                    return Err(format!("git timed out after {}s", timeout.as_secs()));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("failed to wait for git: {error}"));
            }
        }
    };

    let stdout = stdout_thread.join().unwrap_or_default();
    let stderr = stderr_thread.join().unwrap_or_default();
    if !status.success() {
        let stderr = stderr.trim().to_string();
        return Err(if stderr.is_empty() {
            format!("git exited with status {status}")
        } else {
            stderr
        });
    }
    Ok(stdout)
}

fn configure_git_auth(command: &mut Command, auth: &GitAuthConfig) {
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.env("GIT_CONFIG_NOSYSTEM", "1");
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_SSH_COMMAND",
        "GIT_EXTERNAL_DIFF",
    ] {
        command.env_remove(key);
    }
    // Git for Windows maps `/dev/null` to `NUL` internally, so this value
    // disables the global config file on every platform.
    command.env("GIT_CONFIG_GLOBAL", "/dev/null");

    // Harden the environment: disable any inherited credential helper and
    // neutralize repo-local hooks — every process git spawns inherits this
    // environment, and a cloned repository's hooks must never run here. No
    // identity key is injected: authentication is left to git's native
    // transports (ssh-agent, OS keychain).
    let entries: Vec<(&str, String)> = vec![
        ("credential.helper", String::new()),
        ("core.hooksPath", "/dev/null".to_string()),
        ("core.fsmonitor", "false".to_string()),
        ("protocol.allow", "never".to_string()),
        ("protocol.http.allow", "always".to_string()),
        ("protocol.https.allow", "always".to_string()),
        ("protocol.ext.allow", "never".to_string()),
        (
            "protocol.file.allow",
            if auth.allow_file_transport {
                "always"
            } else {
                "never"
            }
            .to_string(),
        ),
    ];
    apply_git_config(command, &entries);
}

fn apply_git_config(command: &mut Command, entries: &[(&str, String)]) {
    command.env("GIT_CONFIG_COUNT", entries.len().to_string());
    for (index, (key, value)) in entries.iter().enumerate() {
        command.env(format!("GIT_CONFIG_KEY_{index}"), key);
        command.env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
}

pub(crate) fn build_git_auth_config() -> Result<GitAuthConfig, String> {
    let git_path = resolve_command("git").ok_or_else(|| "git was not found on PATH".to_string())?;
    Ok(GitAuthConfig {
        git_path,
        allow_file_transport: false,
    })
}

#[cfg(test)]
pub(crate) fn build_test_git_auth_config() -> Result<GitAuthConfig, String> {
    let mut auth = build_git_auth_config()?;
    auth.allow_file_transport = true;
    Ok(auth)
}

/// Normalizes and validates a remote-supplied branch name. Strips a
/// `refs/heads/` prefix, then rejects anything outside a conservative
/// character allowlist, path traversal (`..`), leading/trailing `/`, and
/// flag-shaped values (leading `-`) so a branch can never reach git as an
/// option instead of a positional argument.
pub(crate) fn clean_branch(value: Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_start_matches("refs/heads/"))
        .filter(|value| {
            !value.is_empty()
                && !value.starts_with('-')
                && !value.contains("..")
                && !value.starts_with('/')
                && !value.ends_with('/')
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))
        })
        .map(ToString::to_string)
}

pub(crate) fn clean_target_ref(value: Option<String>) -> Option<String> {
    let value = value?.trim().to_string();
    let name = value.strip_prefix("refs/tags/")?;
    let clean_name = clean_branch(Some(name.to_string()))?;
    (clean_name == name).then_some(format!("refs/tags/{clean_name}"))
}

pub(crate) fn validate_clone_url(clone_url: &str) -> Result<(), String> {
    let parsed = Url::parse(clone_url).map_err(|error| format!("invalid clone URL: {error}"))?;
    // Scheme allowlist is the security boundary: only http/https remotes are
    // permitted, so `file://`, `ssh://`, and `ext::` transports can never
    // reach git here. Authentication is delegated to git's native transports
    // (ssh-agent, OS keychain) — no identity is injected by the app.
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("clone URL must be http or https".into());
    }
    if parsed.host_str().map(str::is_empty).unwrap_or(true) {
        return Err("clone URL must name a repository host".into());
    }
    // Require at least one non-empty path segment so the URL names a concrete
    // repository rather than a bare host.
    let names_repository = parsed
        .path_segments()
        .map(|mut segments| segments.any(|segment| !segment.is_empty()))
        .unwrap_or(false);
    if !names_repository {
        return Err("clone URL must point at a repository path".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        clean_branch, clean_target_ref, git_needs_credentials, git_subcommand, validate_clone_url,
    };

    #[test]
    fn git_subcommand_skips_global_config_options() {
        assert_eq!(
            git_subcommand(&[
                "-c",
                "user.name=Buzz User",
                "-c",
                "user.email=user@example.com",
                "merge",
                "HEAD",
            ]),
            Some("merge")
        );
        assert_eq!(
            git_subcommand(&["--config=credential.useHttpPath=true", "fetch", "origin"]),
            Some("fetch")
        );
    }

    #[test]
    fn remote_and_promisor_operations_receive_credentials() {
        assert!(git_needs_credentials(&["fetch", "origin"]));
        assert!(git_needs_credentials(&[
            "-c",
            "user.name=Buzz User",
            "merge",
            "HEAD"
        ]));
        assert!(!git_needs_credentials(&["rev-parse", "HEAD"]));
    }

    #[test]
    fn clean_branch_accepts_plain_and_prefixed_names() {
        assert_eq!(
            clean_branch(Some("refs/heads/feature/x-1".into())),
            Some("feature/x-1".to_string())
        );
        assert_eq!(
            clean_branch(Some(" main ".into())),
            Some("main".to_string())
        );
    }

    #[test]
    fn clean_branch_rejects_flag_shaped_and_traversal_values() {
        assert_eq!(clean_branch(Some("--upload-pack=/tmp/evil".into())), None);
        assert_eq!(clean_branch(Some("-x".into())), None);
        assert_eq!(clean_branch(Some("a/../b".into())), None);
        assert_eq!(clean_branch(Some("/leading".into())), None);
        assert_eq!(clean_branch(Some("trailing/".into())), None);
        assert_eq!(clean_branch(Some("bad name".into())), None);
        assert_eq!(clean_branch(None), None);
    }

    #[test]
    fn clean_target_ref_accepts_only_tag_refs() {
        assert_eq!(
            clean_target_ref(Some("refs/tags/v1.0.0".into())),
            Some("refs/tags/v1.0.0".to_string())
        );
        assert_eq!(clean_target_ref(Some("refs/heads/main".into())), None);
        assert_eq!(clean_target_ref(Some("refs/tags/../main".into())), None);
        assert_eq!(clean_target_ref(Some("refs/tags/-flag".into())), None);
    }

    #[test]
    fn validate_clone_url_accepts_http_https_repo_urls() {
        assert!(validate_clone_url("https://example.com/owner/repo").is_ok());
        assert!(validate_clone_url("http://example.com/owner/repo.git").is_ok());
        assert!(validate_clone_url("https://example.com/prefix/owner/repo").is_ok());
        // Bare host with no repository path is rejected.
        assert!(validate_clone_url("https://example.com").is_err());
        assert!(validate_clone_url("https://example.com/").is_err());
        // Non-http(s) schemes never reach git.
        assert!(validate_clone_url("ssh://example.com/owner/repo").is_err());
        assert!(validate_clone_url("file:///srv/owner/repo").is_err());
        assert!(validate_clone_url("not a url").is_err());
    }
}
