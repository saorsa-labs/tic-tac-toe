//! Local clone and pull-request merge commands for the Projects workflow.

use super::project_git::{first_output_line, normalize_branch_option};
use super::project_git_diff::clean_commit;
use super::project_git_exec::{build_git_auth_config, run_git, validate_clone_url, GitAuthConfig};
use super::project_repo_paths::{
    canonical_repos_roots, canonicalize_repos_root, default_repos_root_candidates,
    find_local_repo_dir, local_repo_candidates,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
pub struct ProjectRepoCloneResult {
    pub path: String,
    pub cloned: bool,
    pub message: String,
}

#[derive(Serialize)]
pub struct ProjectRepoMergeResult {
    pub message: String,
    pub merge_commit: String,
}

/// Machine-readable recovery metadata for a failed pull-request merge.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPullRequestMergeRecovery {
    action: String,
    target_branch: String,
    source_branch: String,
}

/// Structured pull-request merge failure returned across the Tauri boundary.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPullRequestMergeError {
    code: String,
    message: String,
    recovery: Option<ProjectPullRequestMergeRecovery>,
}

impl ProjectPullRequestMergeError {
    fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            recovery: None,
        }
    }

    fn conflict(target_branch: String, source_branch: String) -> Self {
        Self {
            code: "merge_conflict".to_string(),
            message: "Pull request has merge conflicts.".to_string(),
            recovery: Some(ProjectPullRequestMergeRecovery {
                action: "open_terminal".to_string(),
                target_branch,
                source_branch,
            }),
        }
    }
}

impl From<String> for ProjectPullRequestMergeError {
    fn from(message: String) -> Self {
        Self::new("merge_failed", message)
    }
}

fn classify_merge_error(
    message: String,
    has_conflicts: bool,
    target_branch: &str,
    source_branch: &str,
) -> ProjectPullRequestMergeError {
    if has_conflicts {
        ProjectPullRequestMergeError::conflict(target_branch.to_string(), source_branch.to_string())
    } else {
        ProjectPullRequestMergeError::new(
            "merge_failed",
            format!("Pull request merge failed: {message}"),
        )
    }
}

struct ProjectRepoMergeGitResult {
    message: String,
    merge_commit: String,
}

/// Validated repository and pull-request metadata for a native merge.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPullRequestMergeInput {
    target_clone_url: String,
    source_clone_url: String,
    target_branch: String,
    source_branch: String,
    expected_commit: String,
}

fn normalize_commit(value: &str) -> Option<String> {
    clean_commit(Some(value.trim().to_ascii_lowercase()))
}

fn same_repository(left: &str, right: &str) -> bool {
    left.trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .eq_ignore_ascii_case(right.trim().trim_end_matches('/').trim_end_matches(".git"))
}

fn clone_destination_root(repos_dir: Option<&str>) -> Result<std::path::PathBuf, String> {
    match canonical_repos_roots(repos_dir) {
        Ok(roots) => roots
            .into_iter()
            .next()
            .ok_or_else(|| "reposDir is not accessible".to_string()),
        Err(error) => {
            if repos_dir.is_some() {
                return Err(error);
            }
            let root = default_repos_root_candidates()
                .into_iter()
                .next()
                .ok_or(error)?;
            std::fs::create_dir_all(&root).map_err(|error| format!("create repos dir: {error}"))?;
            canonicalize_repos_root(root)
        }
    }
}

fn align_unborn_head_branch(
    repo_dir: &std::path::Path,
    branch: Option<&str>,
    auth: &GitAuthConfig,
) -> Result<(), String> {
    let Some(branch) = branch else {
        return Ok(());
    };
    if run_git(&["rev-parse", "--verify", "HEAD"], Some(repo_dir), auth).is_ok() {
        return Ok(());
    }
    let target = format!("refs/heads/{branch}");
    run_git(
        &["symbolic-ref", "HEAD", target.as_str()],
        Some(repo_dir),
        auth,
    )
    .map(|_| ())
}

pub(crate) fn clone_project_repository_blocking(
    repos_dir: Option<&str>,
    project_dtag: &str,
    clone_url: &str,
    default_branch: Option<&str>,
    auth: &GitAuthConfig,
) -> Result<ProjectRepoCloneResult, String> {
    validate_clone_url(clone_url)?;
    let branch = normalize_branch_option(default_branch);
    if let Some(repo_dir) = find_local_repo_dir(repos_dir, project_dtag, Some(clone_url))? {
        return Ok(ProjectRepoCloneResult {
            path: repo_dir.display().to_string(),
            cloned: false,
            message: "Repository is already cloned.".to_string(),
        });
    }

    let repos_root = clone_destination_root(repos_dir)?;
    let repo_name = local_repo_candidates(project_dtag, Some(clone_url))
        .into_iter()
        .next()
        .ok_or_else(|| "Could not derive a directory name for the repository.".to_string())?;
    let repo_dir = repos_root.join(repo_name);
    if repo_dir.exists() {
        return Err(format!(
            "{} already exists but is not a git checkout.",
            repo_dir.display()
        ));
    }
    let repo_path = repo_dir
        .to_str()
        .ok_or_else(|| "repository path is not UTF-8".to_string())?;

    let mut clone_args = vec!["clone"];
    if let Some(ref branch) = branch {
        clone_args.extend(["--branch", branch.as_str()]);
    }
    clone_args.extend(["--end-of-options", clone_url, repo_path]);
    if let Err(error) = run_git(&clone_args, None, auth) {
        if branch.is_none() {
            return Err(error);
        }
        run_git(
            &["clone", "--end-of-options", clone_url, repo_path],
            None,
            auth,
        )?;
    }
    align_unborn_head_branch(&repo_dir, branch.as_deref(), auth)?;

    Ok(ProjectRepoCloneResult {
        path: repo_dir.display().to_string(),
        cloned: true,
        message: format!("Cloned repository to {}.", repo_dir.display()),
    })
}

#[tauri::command]
pub async fn clone_project_repository(
    repos_dir: Option<String>,
    project_dtag: String,
    clone_url: String,
    default_branch: Option<String>,
) -> Result<ProjectRepoCloneResult, String> {
    validate_clone_url(&clone_url)?;
    let auth = build_git_auth_config()?;
    tauri::async_runtime::spawn_blocking(move || {
        clone_project_repository_blocking(
            repos_dir.as_deref(),
            &project_dtag,
            &clone_url,
            default_branch.as_deref(),
            &auth,
        )
    })
    .await
    .map_err(|error| format!("repo clone task failed: {error}"))?
}

#[tauri::command]
pub async fn merge_project_pull_request(
    input: ProjectPullRequestMergeInput,
) -> Result<ProjectRepoMergeResult, ProjectPullRequestMergeError> {
    let ProjectPullRequestMergeInput {
        target_clone_url,
        source_clone_url,
        target_branch,
        source_branch,
        expected_commit,
    } = input;
    validate_clone_url(&target_clone_url)?;
    validate_clone_url(&source_clone_url)?;
    let merger_agent_id = crate::commands::identity::fetch_agent_id()?;
    let target_branch = normalize_branch_option(Some(&target_branch))
        .ok_or_else(|| "Invalid target branch.".to_string())?;
    let source_branch = normalize_branch_option(Some(&source_branch))
        .ok_or_else(|| "Invalid source branch.".to_string())?;
    if target_branch == source_branch && same_repository(&target_clone_url, &source_clone_url) {
        return Err("Source and target branches must be different."
            .to_string()
            .into());
    }
    let expected_commit = normalize_commit(&expected_commit)
        .ok_or_else(|| "Invalid pull request commit.".to_string())?;
    let auth = build_git_auth_config()?;

    let git_result = tauri::async_runtime::spawn_blocking(
        move || -> Result<ProjectRepoMergeGitResult, ProjectPullRequestMergeError> {
            let temp_dir =
                tempfile::tempdir().map_err(|error| format!("create temp dir: {error}"))?;
            let repo_dir = temp_dir.path().join("repo");
            let repo_path = repo_dir
                .to_str()
                .ok_or_else(|| "temporary repository path is not UTF-8".to_string())?;
            run_git(
                &[
                    "clone",
                    "--filter=blob:none",
                    "--no-tags",
                    "--branch",
                    target_branch.as_str(),
                    "--single-branch",
                    "--end-of-options",
                    target_clone_url.as_str(),
                    repo_path,
                ],
                None,
                &auth,
            )?;
            run_git(
                &[
                    "fetch",
                    "--quiet",
                    "--end-of-options",
                    source_clone_url.as_str(),
                    source_branch.as_str(),
                ],
                Some(&repo_dir),
                &auth,
            )?;
            let source_head = run_git(&["rev-parse", "FETCH_HEAD"], Some(&repo_dir), &auth)
                .ok()
                .and_then(|output| first_output_line(&output))
                .ok_or_else(|| "Could not resolve the pull request branch.".to_string())?;
            if source_head.to_ascii_lowercase() != expected_commit {
                return Err(ProjectPullRequestMergeError::new(
                    "branch_changed",
                    "The pull request branch changed. Refresh the pull request before merging."
                        .to_string(),
                ));
            }

            let merge_email = format!("{merger_agent_id}@users.noreply.x0x");
            let merge_result = run_git(
                &[
                    "-c",
                    "user.name=Buzz User",
                    "-c",
                    format!("user.email={merge_email}").as_str(),
                    "merge",
                    "--no-edit",
                    "--end-of-options",
                    expected_commit.as_str(),
                ],
                Some(&repo_dir),
                &auth,
            );
            if let Err(error) = merge_result {
                let has_conflicts = run_git(
                    &["diff", "--name-only", "--diff-filter=U"],
                    Some(&repo_dir),
                    &auth,
                )
                .is_ok_and(|output| !output.trim().is_empty());
                return Err(classify_merge_error(
                    error,
                    has_conflicts,
                    &target_branch,
                    &source_branch,
                ));
            }
            let merge_commit = run_git(&["rev-parse", "HEAD"], Some(&repo_dir), &auth)
                .ok()
                .and_then(|output| first_output_line(&output))
                .ok_or_else(|| "Could not resolve the merge commit.".to_string())?;
            run_git(
                &[
                    "push",
                    "--end-of-options",
                    "origin",
                    format!("HEAD:{target_branch}").as_str(),
                ],
                Some(&repo_dir),
                &auth,
            )?;

            Ok(ProjectRepoMergeGitResult {
                message: format!("Merged {source_branch} into {target_branch}."),
                merge_commit,
            })
        },
    )
    .await
    .map_err(|error| {
        ProjectPullRequestMergeError::new(
            "merge_task_failed",
            format!("pull request merge task failed: {error}"),
        )
    })??;
    Ok(ProjectRepoMergeResult {
        message: git_result.message,
        merge_commit: git_result.merge_commit,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        align_unborn_head_branch, classify_merge_error, normalize_commit, same_repository,
        ProjectPullRequestMergeError,
    };
    use crate::commands::project_git_exec::{build_test_git_auth_config, run_git};

    #[test]
    fn empty_clone_uses_requested_default_branch() {
        let auth = build_test_git_auth_config().expect("build test git config");
        let repo = tempfile::tempdir().expect("create repository");
        run_git(&["init"], Some(repo.path()), &auth).expect("initialize repository");

        align_unborn_head_branch(repo.path(), Some("main"), &auth).expect("align unborn HEAD");

        assert_eq!(
            std::fs::read_to_string(repo.path().join(".git/HEAD"))
                .expect("read HEAD")
                .trim(),
            "ref: refs/heads/main"
        );
    }

    #[test]
    fn normalize_commit_accepts_sha1_and_sha256_hex() {
        assert_eq!(normalize_commit(&"A".repeat(40)), Some("a".repeat(40)));
        assert_eq!(normalize_commit(&"B".repeat(64)), Some("b".repeat(64)));
    }

    #[test]
    fn normalize_commit_rejects_invalid_values() {
        assert_eq!(normalize_commit("abc"), None);
        assert_eq!(normalize_commit(&"z".repeat(40)), None);
    }

    #[test]
    fn repository_comparison_normalizes_git_suffix_and_trailing_slash() {
        assert!(same_repository(
            "https://git.example/owner/repo.git",
            "https://git.example/owner/repo/"
        ));
        assert!(!same_repository(
            "https://git.example/owner/repo",
            "https://git.example/fork/repo"
        ));
    }

    #[test]
    fn merge_conflict_error_has_stable_recovery_metadata() {
        let error =
            ProjectPullRequestMergeError::conflict("main".to_string(), "feature/demo".to_string());

        assert_eq!(error.code, "merge_conflict");
        assert_eq!(error.message, "Pull request has merge conflicts.");
        let recovery = error.recovery.expect("conflict recovery");
        assert_eq!(recovery.action, "open_terminal");
        assert_eq!(recovery.target_branch, "main");
        assert_eq!(recovery.source_branch, "feature/demo");
    }

    #[test]
    fn merge_conflict_error_serializes_for_tauri_clients() {
        let error =
            ProjectPullRequestMergeError::conflict("main".to_string(), "feature/demo".to_string());
        let value = serde_json::to_value(error).expect("serialize merge conflict");

        assert_eq!(value["code"], "merge_conflict");
        assert_eq!(value["recovery"]["targetBranch"], "main");
        assert_eq!(value["recovery"]["sourceBranch"], "feature/demo");
    }

    #[test]
    fn merge_error_classification_only_recovers_conflicts() {
        let conflict = classify_merge_error(
            "CONFLICT (content): Merge conflict in src/main.rs".to_string(),
            true,
            "main",
            "feature/demo",
        );
        assert_eq!(conflict.code, "merge_conflict");
        assert!(conflict.recovery.is_some());

        let other = classify_merge_error(
            "fatal: refusing to merge unrelated histories".to_string(),
            false,
            "main",
            "feature/demo",
        );
        assert_eq!(other.code, "merge_failed");
        assert!(other.recovery.is_none());
    }
}
