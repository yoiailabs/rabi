//! Conservative retirement of a merged task's recorded workspace.
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::store::{Task, TaskState};
use crate::{
    error::{Error, Result},
    worktree::{self, RemoveOptions},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CleanupReport {
    pub outcome: String,
    pub reason: String,
}

fn kept(reason: &str) -> CleanupReport {
    CleanupReport {
        outcome: "kept".into(),
        reason: reason.into(),
    }
}

fn refusal(reason: &'static str) -> Error {
    Error::Status {
        code: reason,
        message: "Task workspace cleanup retained local work".into(),
    }
}

fn safe(workspace: &Path, task: &Task) -> Result<()> {
    if crate::git::status_count(workspace)? > 0 {
        return Err(refusal("dirty"));
    }
    // Stashes belong to the common repository, not one checkout. Without a
    // reliable ownership distinction, any stash conservatively retains it.
    if !super::text(workspace, &["stash", "list", "--format=%H"])?.is_empty() {
        return Err(refusal("stash"));
    }
    let head = super::commit(workspace, "HEAD")?;
    // Publication and the fresh merge receipt independently bind this exact
    // tip. New local commits cannot be hidden by an absent/stale upstream.
    if task.advance.pushed_head.as_deref() != Some(&head)
        || task.advance.merged_head_sha.as_deref() != Some(&head)
    {
        return Err(refusal("unpushed"));
    }
    if crate::git::current_branch(workspace)? != task.branch {
        return Err(refusal("workspace_changed"));
    }
    Ok(())
}

fn safe_missing(root: &Path, path: &Path, task: &Task) -> Result<()> {
    if !std::fs::symlink_metadata(path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Err(refusal("workspace_changed"));
    }
    if !super::text(root, &["stash", "list", "--format=%H"])?.is_empty() {
        return Err(refusal("stash"));
    }
    if let Some(tree) = crate::git::list_worktrees(root)?
        .iter()
        .find(|tree| crate::paths::same_location(Path::new(&tree.path), path))
    {
        if tree.branch.as_deref() != Some(&task.branch)
            || tree.head.is_none()
            || tree.head != task.advance.pushed_head
            || tree.head != task.advance.merged_head_sha
        {
            return Err(refusal("workspace_changed"));
        }
    }
    Ok(())
}

fn failure(error: &Error) -> CleanupReport {
    let reason = match error {
        Error::Uncommitted { .. } => "dirty",
        Error::Untrusted(_) => "untrusted_hooks",
        Error::Status { code, .. }
            if matches!(
                *code,
                "dirty" | "stash" | "unpushed" | "workspace_changed" | "primary"
            ) =>
        {
            code
        }
        _ => "remove_refused",
    };
    kept(reason)
}

pub(crate) fn execute(root: &Path, task: &Task) -> Result<CleanupReport> {
    if task.state != TaskState::Merged {
        return Ok(kept("not_merged"));
    }
    let Some(path) = task.workspace_path.as_deref() else {
        return Ok(kept("workspace_unrecorded"));
    };
    let trees = crate::git::list_worktrees(root)?;
    if trees
        .first()
        .is_some_and(|tree| crate::paths::same_location(Path::new(&tree.path), path))
    {
        return Ok(kept("primary"));
    }
    let registered = trees
        .iter()
        .find(|tree| crate::paths::same_location(Path::new(&tree.path), path));
    let missing = std::fs::symlink_metadata(path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    if registered.is_none() && missing && !crate::metadata::records_path(root, path)? {
        // A previous successful retirement is idempotent. Keep task, review,
        // classification and memory records as the durable audit trail.
        return Ok(CleanupReport {
            outcome: "already_gone".into(),
            reason: "workspace_absent".into(),
        });
    }
    if registered.is_some_and(|tree| tree.branch.as_deref() != Some(&task.branch))
        || (registered.is_none() && !missing)
    {
        return Ok(kept("workspace_changed"));
    }
    // Validate the checkout's independent Git registration before inspecting
    // it, just as provider launches and publication do.
    if !missing && super::implement::workspace(root, task).is_err() {
        return Ok(kept("workspace_changed"));
    }
    let check = || {
        if missing {
            safe_missing(root, path, task)
        } else {
            safe(path, task)
        }
    };
    if let Err(error) = check() {
        return Ok(failure(&error));
    }
    // root may be an invoking checkout subdirectory. Preserve remove's hook
    // source semantics while anchoring mutations to the primary checkout.
    let primary = crate::git::git_root(root)?;
    let source = crate::git::top_level(root)?;
    let context = crate::context::open_for_hooks(Some(&primary), &source)?;
    let options = RemoveOptions {
        discard_changes: false,
        keep_branch: true,
        delete_unpreserved_branch: false,
    };
    let Some(selector) = path.to_str() else {
        return Ok(kept("workspace_path_unavailable"));
    };
    let plan = match worktree::preflight_remove(&context, selector, &options, Some(path)) {
        Ok(plan) => plan,
        Err(error) => return Ok(failure(&error.error)),
    };
    // Remove rechecks dirty files, submodules, HEAD and branch after trusted
    // hooks. This additional callback also rechecks stashes and publication.
    let result = worktree::commit_remove_checked(
        &context,
        plan,
        &options,
        &worktree::Logger {
            enabled: false,
            json: true,
        },
        crate::trust::HookControl {
            no_hooks: false,
            json: true,
            prompt: false,
        },
        check,
    );
    Ok(match result {
        Ok(outcome) => CleanupReport {
            outcome: if missing { "already_gone" } else { "removed" }.into(),
            reason: if outcome.metadata_error.is_some() {
                "metadata_cleanup_failed"
            } else if missing {
                "stale_workspace_record_removed"
            } else {
                "workspace_removed_branch_kept"
            }
            .into(),
        },
        Err(error) => failure(&error.error),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf, process::Command};

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    struct Fixture {
        _temporary: super::super::boundary::Boundary,
        root: PathBuf,
        workspace: PathBuf,
        task: Task,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = super::super::boundary::Boundary::new().unwrap();
            let root = temporary.path().join("repository");
            let workspace = temporary.path().join("workspace");
            fs::create_dir(&root).unwrap();
            git(&root, &["init", "-b", "main"]);
            git(&root, &["config", "user.name", "Test"]);
            git(&root, &["config", "user.email", "test@example.invalid"]);
            git(&root, &["commit", "--allow-empty", "-m", "Initial"]);
            git(
                &root,
                &[
                    "worktree",
                    "add",
                    "-b",
                    "topic",
                    workspace.to_str().unwrap(),
                ],
            );
            let head = git(&workspace, &["rev-parse", "HEAD"]);
            crate::metadata::replace(
                &root,
                &workspace,
                &crate::metadata::MetadataInput::default(),
                crate::metadata::BranchOrigin::New,
            )
            .unwrap();
            let task = serde_json::from_value(serde_json::json!({
                "schema":1,"id":format!("task_{}",uuid::Uuid::new_v4()),
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "state":{"name":"merged"},"intent":"Test task","branch":"topic",
                "baseRef":"main","pushRemoteUrl":"https://github.com/example/project.git",
                "workspacePath":workspace,"prNumber":1,
                "advance":{"pushedHead":head,"mergedHeadSha":head}
            }))
            .unwrap();
            Self {
                _temporary: temporary,
                root,
                workspace,
                task,
            }
        }
    }

    #[test]
    fn cleanup_retains_local_work_table() {
        for case in [
            "dirty",
            "staged",
            "untracked",
            "stash",
            "unpushed",
            "primary",
            "moved_branch",
        ] {
            let mut fixture = Fixture::new();
            let expected = match case {
                "dirty" | "staged" => {
                    fs::write(fixture.workspace.join("file"), "original").unwrap();
                    git(&fixture.workspace, &["add", "file"]);
                    git(&fixture.workspace, &["commit", "-m", "Published"]);
                    let head = git(&fixture.workspace, &["rev-parse", "HEAD"]);
                    fixture.task.advance.pushed_head = Some(head.clone());
                    fixture.task.advance.merged_head_sha = Some(head);
                    fs::write(fixture.workspace.join("file"), "modified").unwrap();
                    if case == "staged" {
                        git(&fixture.workspace, &["add", "file"]);
                    }
                    "dirty"
                }
                "untracked" => {
                    fs::write(fixture.workspace.join("untracked"), "keep").unwrap();
                    "dirty"
                }
                "stash" => {
                    fs::write(fixture.workspace.join("untracked"), "keep").unwrap();
                    git(
                        &fixture.workspace,
                        &["stash", "push", "--include-untracked"],
                    );
                    "stash"
                }
                "unpushed" => {
                    git(
                        &fixture.workspace,
                        &["commit", "--allow-empty", "-m", "Local work"],
                    );
                    "unpushed"
                }
                "primary" => {
                    fixture.task.workspace_path = Some(fixture.root.clone());
                    "primary"
                }
                "moved_branch" => {
                    git(&fixture.workspace, &["checkout", "-b", "other"]);
                    "workspace_changed"
                }
                _ => unreachable!(),
            };
            let report = execute(&fixture.root, &fixture.task).unwrap();
            assert_eq!(report.outcome, "kept", "{case}");
            assert_eq!(report.reason, expected, "{case}");
            assert!(fixture.workspace.is_dir(), "{case}");
            assert!(
                crate::metadata::records_path(&fixture.root, &fixture.workspace).unwrap(),
                "{case}"
            );
        }
    }

    #[test]
    fn cleanup_removes_only_workspace_and_its_metadata_and_is_idempotent() {
        let fixture = Fixture::new();
        let report = execute(&fixture.root, &fixture.task).unwrap();
        assert_eq!(report.outcome, "removed");
        assert!(!fixture.workspace.exists());
        assert!(!crate::metadata::records_path(&fixture.root, &fixture.workspace).unwrap());
        assert!(crate::git::reference_exists(&fixture.root, "refs/heads/topic").unwrap());
        assert_eq!(
            execute(&fixture.root, &fixture.task).unwrap().outcome,
            "already_gone"
        );
    }

    #[test]
    fn cleanup_requires_exact_merge_and_publication_receipts() {
        for receipt in ["merge", "publish"] {
            let mut fixture = Fixture::new();
            if receipt == "merge" {
                fixture.task.advance.merged_head_sha = None;
            } else {
                fixture.task.advance.pushed_head = None;
            }
            assert_eq!(
                execute(&fixture.root, &fixture.task).unwrap().reason,
                "unpushed"
            );
            assert!(fixture.workspace.is_dir());
        }
    }

    #[test]
    fn cleanup_rechecks_hook_changes() {
        const FLAG: &str = "RABI_TEST_CLEANUP_HOOK";
        if let Ok(mode) = std::env::var(FLAG) {
            let fixture = Fixture::new();
            let command = match mode.as_str() {
                "dirty" | "subdirectory" => "printf retained > hook-work",
                "stash" => "printf retained > hook-work; git stash push --include-untracked",
                "unpushed" => "git commit --allow-empty -m 'Local hook work'",
                _ => panic!("Unknown hook test"),
            };
            fs::write(
                fixture.root.join(".rabi.toml"),
                format!(
                    "[hooks]\npre-remove = [{}]\n",
                    serde_json::to_string(command).unwrap()
                ),
            )
            .unwrap();
            let subdirectory = fixture.root.join("nested");
            fs::create_dir(&subdirectory).unwrap();
            let root = if mode == "subdirectory" {
                &subdirectory
            } else {
                &fixture.root
            };
            let report = execute(root, &fixture.task).unwrap();
            assert_eq!(report.outcome, "kept", "{mode}");
            let expected = if mode == "subdirectory" {
                "dirty"
            } else {
                &mode
            };
            assert_eq!(report.reason, expected, "{mode}");
            assert!(fixture.workspace.is_dir());
            assert!(crate::metadata::records_path(&fixture.root, &fixture.workspace).unwrap());
            return;
        }
        // Scope trust and user configuration to child processes so concurrent
        // unit tests cannot observe a transient global environment change.
        let environment = super::super::boundary::Boundary::new().unwrap();
        for mode in ["dirty", "stash", "unpushed", "subdirectory"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "task::cleanup::tests::cleanup_rechecks_hook_changes",
                    "--nocapture",
                ])
                .env(FLAG, mode)
                .env("RABI_TRUST_ALL", "1")
                .env("HOME", environment.path())
                .env("XDG_CONFIG_HOME", environment.path().join("config"))
                .env("XDG_STATE_HOME", environment.path().join("state"))
                .env("XDG_DATA_HOME", environment.path().join("data"))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn cleanup_removes_only_stale_registration_and_record_when_directory_is_missing() {
        let fixture = Fixture::new();
        fs::remove_dir_all(&fixture.workspace).unwrap();
        assert_eq!(
            execute(&fixture.root, &fixture.task).unwrap().reason,
            "stale_workspace_record_removed"
        );
        assert!(!crate::metadata::records_path(&fixture.root, &fixture.workspace).unwrap());
        assert!(crate::git::reference_exists(&fixture.root, "refs/heads/topic").unwrap());
    }
}
