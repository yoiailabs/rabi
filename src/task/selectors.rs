//! Task selectors extend workspace selectors without changing their shared rules.
use std::path::Path;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::worktree::WorktreeInfo;

use super::store::{Store, Task};

fn task(context: &Context, selector: &str) -> Result<Option<Task>> {
    if !selector.starts_with("task_") {
        return Ok(None);
    }
    let store = Store::locate(&context.git_root)?;
    match store.resolve(selector) {
        Ok(id) => store.load(&id).map(Some),
        Err(Error::Status {
            code: "not_found", ..
        }) => Ok(None),
        Err(error) => Err(error),
    }
}

fn missing_workspace(task: &Task) -> Error {
    Error::status(
        crate::error::NOT_FOUND,
        format!("Task {} has no live linked workspace", task.id),
    )
}

fn task_workspace(context: &Context, task: &Task) -> Result<WorktreeInfo> {
    let path = task
        .workspace_path
        .as_ref()
        .ok_or_else(|| missing_workspace(task))?;
    let listed = crate::worktree::list_worktrees_without_status(context)?;
    let tree = listed
        .into_iter()
        .find(|tree| {
            !tree.is_default
                && !tree.orphaned
                && crate::paths::same_location(Path::new(&tree.path), path)
        })
        .ok_or_else(|| missing_workspace(task))?;
    if std::fs::symlink_metadata(&tree.registered_path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        return Err(missing_workspace(task));
    }
    crate::worktree::validate_checkout(context, &tree, "task")?;
    if crate::git::current_branch(Path::new(&tree.path))? != task.branch {
        return Err(missing_workspace(task));
    }
    Ok(tree)
}

pub(crate) fn workspace(context: &Context, selector: Option<&str>) -> Result<WorktreeInfo> {
    let Some(selector) = selector else {
        return crate::selectors::resolve_optional_without_status(context, None);
    };
    match task(context, selector)? {
        Some(task) => task_workspace(context, &task),
        None => crate::selectors::resolve_without_status(context, selector),
    }
}

fn task_github_target(
    context: &Context,
    task: &Task,
) -> Result<(Option<u64>, Option<WorktreeInfo>)> {
    match task_workspace(context, task) {
        Ok(tree) => Ok((task.pr_number, Some(tree))),
        Err(Error::Status {
            code: "not_found", ..
        }) if task.pr_number.is_some() => Ok((task.pr_number, None)),
        Err(error) => Err(error),
    }
}

/// Keep a live workspace for local head checks; otherwise a recorded PR has
/// the same behavior as an explicit pr:N selector.
pub(super) fn github_target(
    context: &Context,
    selector: Option<&str>,
) -> Result<(Option<u64>, Option<WorktreeInfo>)> {
    let Some(selector) = selector else {
        let tree = crate::selectors::resolve_optional_without_status(context, None)?;
        crate::worktree::validate_checkout(context, &tree, "task")?;
        return Ok((None, Some(tree)));
    };
    if let Some(task) = task(context, selector)? {
        return task_github_target(context, &task);
    }
    if let Some(number) = selector.strip_prefix("pr:") {
        let number = number
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::validation("Expected pr:N with a positive PR number"))?;
        return Ok((Some(number), None));
    }
    let tree = crate::selectors::resolve_without_status(context, selector)?;
    crate::worktree::validate_checkout(context, &tree, "task")?;
    Ok((None, Some(tree)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{fs, path::PathBuf, process::Command};

    struct Fixture {
        root: PathBuf,
        context: Context,
        task: Task,
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("rabi-task-selectors-{}", uuid::Uuid::new_v4()));
            let repo = root.join("repo");
            let workspace = root.join("workspace");
            fs::create_dir_all(&repo).unwrap();
            for args in [
                vec!["init", "--template=", "-b", "main"],
                vec![
                    "-c",
                    "user.name=fixture",
                    "-c",
                    "user.email=fixture",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "fixture",
                ],
                vec![
                    "worktree",
                    "add",
                    "-b",
                    "topic",
                    workspace.to_str().unwrap(),
                ],
            ] {
                let output = Command::new("git")
                    .args(args)
                    .current_dir(&repo)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            let context = Context {
                retirement_queries: Default::default(),
                git_root: repo.clone(),
                source_checkout: repo,
                selected_checkout: None,
                worktrees_directory: root.clone(),
                project_common_dir: None,
                config: None,
                preferences: Default::default(),
            };
            let task = serde_json::from_value(json!({
                "schema": 1,
                "id": format!("task_{}", uuid::Uuid::new_v4()),
                "createdAt": "2026-01-01T00:00:00Z",
                "updatedAt": "2026-01-01T00:00:00Z",
                "state": {"name": "new"},
                "intent": "Select the linked workspace",
                "branch": "topic",
                "baseRef": "main",
                "pushRemoteUrl": "",
                "workspacePath": workspace,
                "prNumber": 42,
            }))
            .unwrap();
            Self {
                root,
                context,
                task,
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn github_task_target_keeps_live_workspace_and_falls_back_to_recorded_pr() {
        let mut fixture = Fixture::new();
        let (number, tree) = task_github_target(&fixture.context, &fixture.task).unwrap();
        assert_eq!(number, Some(42));
        assert_eq!(
            Path::new(&tree.unwrap().path),
            fixture.task.workspace_path.as_ref().unwrap()
        );
        let workspace = fixture.task.workspace_path.clone().unwrap();
        for unregistered in [false, true] {
            if unregistered {
                let output = Command::new("git")
                    .args(["worktree", "remove", workspace.to_str().unwrap()])
                    .current_dir(&fixture.context.git_root)
                    .output()
                    .unwrap();
                assert!(output.status.success());
                fs::create_dir(&workspace).unwrap();
            } else {
                fixture.task.workspace_path = None;
            }
            if unregistered {
                fixture.task.workspace_path = Some(workspace.clone());
            }
            let (number, tree) = task_github_target(&fixture.context, &fixture.task).unwrap();
            assert_eq!(number, Some(42));
            assert!(tree.is_none());
        }
        fs::remove_dir(&workspace).unwrap();
        let (number, tree) = task_github_target(&fixture.context, &fixture.task).unwrap();
        assert_eq!(number, Some(42));
        assert!(tree.is_none());
        fixture.task.pr_number = None;
        let error = task_github_target(&fixture.context, &fixture.task).unwrap_err();
        assert_eq!(error.exit_code(), crate::error::NOT_FOUND);
        assert!(error.to_string().contains("no live linked workspace"));
    }

    #[test]
    fn github_task_target_uses_recorded_pr_after_branch_switch_or_detach() {
        for args in [vec!["switch", "-c", "other"], vec!["switch", "--detach"]] {
            let mut fixture = Fixture::new();
            let output = Command::new("git")
                .args(args)
                .current_dir(fixture.task.workspace_path.as_ref().unwrap())
                .output()
                .unwrap();
            assert!(output.status.success());
            let (number, tree) = task_github_target(&fixture.context, &fixture.task).unwrap();
            assert_eq!(number, Some(42));
            assert!(tree.is_none());
            fixture.task.pr_number = None;
            let error = task_github_target(&fixture.context, &fixture.task).unwrap_err();
            assert_eq!(error.exit_code(), crate::error::NOT_FOUND);
            assert!(error.to_string().contains(&fixture.task.id));
            assert!(error.to_string().contains("no live linked workspace"));
        }
    }

    #[test]
    fn github_task_target_preserves_checkout_validation_errors_with_recorded_pr() {
        let fixture = Fixture::new();
        fs::write(
            fixture.task.workspace_path.as_ref().unwrap().join(".git"),
            "invalid Git entry\n",
        )
        .unwrap();
        let error = task_github_target(&fixture.context, &fixture.task).unwrap_err();
        assert_eq!(error.exit_code(), crate::error::GENERAL);
        assert!(error.to_string().contains("failed to inspect checkout"));
        assert!(!error.to_string().contains("no live linked workspace"));
    }
}
