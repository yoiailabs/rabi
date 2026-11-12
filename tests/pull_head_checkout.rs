#[path = "common/git.rs"]
mod test_git;
#[path = "common/scripts.rs"]
mod test_support;
use std::fs;
use std::path::{Path, PathBuf};

struct TempTree {
    path: PathBuf,
}

impl TempTree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rabi-pull-head-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp");
        Self { path }
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn git(cwd: &Path, args: &[&str]) -> std::process::Output {
    let output = test_git::command("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Rabi")
        .env("GIT_AUTHOR_EMAIL", "rabi@example.com")
        .env("GIT_COMMITTER_NAME", "Rabi")
        .env("GIT_COMMITTER_EMAIL", "rabi@example.com")
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn git_text(cwd: &Path, args: &[&str]) -> String {
    String::from_utf8(git(cwd, args).stdout)
        .expect("utf8")
        .trim()
        .to_string()
}

fn git_output(cwd: &Path, args: &[&str]) -> std::process::Output {
    test_git::command("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("git")
}

fn branch_config(cwd: &Path, branch: &str, key: &str) -> Option<String> {
    let name = format!("branch.{branch}.{key}");
    let output = git_output(cwd, &["config", "--get", &name]);
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).expect("utf8");
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn upstream(cwd: &Path, branch: &str) -> Option<String> {
    let spec = format!("{branch}@{{upstream}}");
    let output = git_output(cwd, &["rev-parse", "--abbrev-ref", &spec]);
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8(output.stdout)
            .expect("utf8")
            .trim()
            .to_string(),
    )
}

/// Remote has `refs/pull/843/head` and no `deleted-feature` branch.
fn deleted_pr_remote(root: &Path) -> (PathBuf, String) {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    git(
        root,
        &[
            "init",
            "--bare",
            "-b",
            "main",
            origin.to_str().expect("origin"),
        ],
    );
    test_git::configure_repo(&origin);
    git(root, &["init", "-b", "main", seed.to_str().expect("seed")]);
    git(&seed, &["commit", "--allow-empty", "-m", "main"]);
    git(
        &seed,
        &[
            "remote",
            "add",
            "origin",
            origin.to_str().expect("origin url"),
        ],
    );
    git(&seed, &["push", "origin", "main"]);
    git(&seed, &["checkout", "-b", "deleted-feature"]);
    fs::write(seed.join("pr.txt"), "pull 843\n").expect("file");
    git(&seed, &["add", "pr.txt"]);
    git(&seed, &["commit", "-m", "pr head"]);
    let commit = git_text(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["push", "origin", "deleted-feature"]);
    git(&origin, &["update-ref", "refs/pull/843/head", &commit]);
    git(&seed, &["push", "origin", ":deleted-feature"]);
    git(
        root,
        &[
            "clone",
            origin.to_str().expect("origin url"),
            clone.to_str().expect("clone"),
        ],
    );
    let heads = git_text(
        &clone,
        &["ls-remote", "origin", "refs/heads/deleted-feature"],
    );
    assert!(
        heads.is_empty(),
        "the PR branch must stay deleted, got {heads}"
    );
    let pull = git_text(&clone, &["ls-remote", "origin", "refs/pull/843/head"]);
    assert!(
        pull.starts_with(&commit),
        "origin must advertise refs/pull/843/head, got {pull}"
    );
    (clone, commit)
}

fn fake_gh(bin: &Path) {
    fs::create_dir_all(bin).expect("bin");
    let script = bin.join("gh");
    test_support::write_executable(
        &script,
        r#"#!/bin/sh
set -eu
if [ "$1" = "repo" ]; then
  printf '%s\n' '{"url":"https://github.com/acme/tool","owner":{"login":"acme"},"name":"tool"}'
  exit 0
fi
if [ "$1" = "api" ]; then
  case "$2" in
    */issues/843)
      printf '%s\n' '{"number":843,"pull_request":{}}'
      exit 0
      ;;
    */pulls/843)
      printf '%s\n' '{"number":843,"head":{"ref":"deleted-feature","repo":{"full_name":"acme/tool"}},"base":{"repo":{"full_name":"acme/tool"}}}'
      exit 0
      ;;
  esac
fi
echo "unexpected gh invocation: $*" >&2
exit 1
"#,
    )
    .expect("gh script");
}

#[test]
fn gh_checkout_uses_pull_head_when_the_branch_is_deleted() {
    let tree = TempTree::new();
    let (clone, commit) = deleted_pr_remote(&tree.path);
    let bin = tree.path.join("bin");
    fake_gh(&bin);
    let data = tree.path.join("data");
    let config = tree.path.join("config");
    let home = tree.path.join("home");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH"));
    let output = test_git::command(env!("CARGO_BIN_EXE_rabi"))
        .args(["create", "--pr", "843", "--reuse", "--no-setup", "--json"])
        .current_dir(&clone)
        .env("PATH", path)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", &data)
        .env("XDG_CONFIG_HOME", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("rabi");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "rabi gh checkout 843 failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.to_ascii_lowercase().contains("warning"),
        "deleted branch checkout warned\n{stderr}"
    );
    assert!(
        !stderr.contains("upstream"),
        "deleted branch checkout mentioned upstream\n{stderr}"
    );
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["data"]["source"]["kind"], "pr");
    assert_eq!(document["data"]["source"]["number"], 843);
    assert_eq!(document["data"]["source"]["headRef"], "deleted-feature");
    assert_eq!(document["data"]["source"]["checkout"], "pullHead");
    assert_eq!(document["data"]["workspace"]["branch"], "deleted-feature");
    assert_eq!(document["data"]["workspace"]["detached"], false);
    assert_eq!(document["data"]["workspace"]["head"], commit);
    assert!(document["data"]["workspace"]["base"].as_str().is_some());
    let worktree = document["data"]["workspace"]["path"]
        .as_str()
        .expect("path");
    let checked_out = git_text(Path::new(worktree), &["rev-parse", "HEAD"]);
    assert_eq!(checked_out, commit);
    let branch = git_text(Path::new(worktree), &["branch", "--show-current"]);
    assert_eq!(branch, "deleted-feature");
    assert!(branch_config(&clone, "deleted-feature", "merge").is_none());
    assert!(branch_config(&clone, "deleted-feature", "remote").is_none());
}

#[test]
fn gh_checkout_names_a_deleted_head_branch() {
    let tree = TempTree::new();
    let (clone, _) = deleted_pr_remote(&tree.path);
    let bin = tree.path.join("bin");
    fake_gh(&bin);
    let output = rabi(&clone, &bin, &["create", "--pr", "843", "--no-setup"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Created workspace 'deleted-feature'"));
    assert!(!stderr.to_ascii_lowercase().contains("warning"), "{stderr}");
}

#[test]
fn stale_remote_ref_does_not_become_upstream() {
    let tree = TempTree::new();
    let (clone, commit) = cloned_before_branch_delete(&tree.path);
    git(
        &clone,
        &["branch", "deleted-feature", "origin/deleted-feature"],
    );
    git(
        &clone,
        &[
            "branch",
            "--set-upstream-to",
            "origin/deleted-feature",
            "deleted-feature",
        ],
    );
    assert_eq!(
        upstream(&clone, "deleted-feature").as_deref(),
        Some("origin/deleted-feature")
    );
    let bin = tree.path.join("bin");
    fake_gh(&bin);
    let output = rabi(
        &clone,
        &bin,
        &["create", "--pr", "843", "--reuse", "--no-setup", "--json"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(!stderr.to_ascii_lowercase().contains("warning"), "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["data"]["source"]["checkout"], "pullHead");
    assert_eq!(document["data"]["workspace"]["head"], commit);
    assert!(branch_config(&clone, "deleted-feature", "merge").is_none());
    assert!(branch_config(&clone, "deleted-feature", "remote").is_none());
    assert!(upstream(&clone, "deleted-feature").is_none());
}

#[test]
fn unfetched_branch_gets_an_upstream() {
    let tree = TempTree::new();
    let (clone, _) = live_branch(&tree.path, "live-feature");
    git(
        &clone,
        &["update-ref", "-d", "refs/remotes/origin/live-feature"],
    );
    let bin = tree.path.join("bin");
    install_gh(&bin, &same_repo_pull("754", "live-feature"));
    let output = rabi(
        &clone,
        &bin,
        &["create", "--pr", "754", "--no-setup", "--json"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(!stderr.to_ascii_lowercase().contains("warning"), "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["data"]["source"]["checkout"], "branch");
    assert_eq!(document["data"]["source"]["headRef"], "live-feature");
    assert_eq!(
        upstream(&clone, "live-feature").as_deref(),
        Some("origin/live-feature")
    );
    assert!(git_output(
        &clone,
        &["rev-parse", "--verify", "refs/remotes/origin/live-feature"]
    )
    .status
    .success());
}

#[test]
fn narrow_fetch_refspec_checkout_sets_tracking_and_creates_worktree() {
    let tree = TempTree::new();
    let (clone, commit) = live_branch(&tree.path, "live-feature");
    git(
        &clone,
        &[
            "config",
            "remote.origin.fetch",
            "+refs/heads/main:refs/remotes/origin/main",
        ],
    );
    git(
        &clone,
        &["update-ref", "-d", "refs/remotes/origin/live-feature"],
    );
    let bin = tree.path.join("bin");
    install_gh(&bin, &same_repo_pull("754", "live-feature"));
    let output = rabi(
        &clone,
        &bin,
        &["create", "--pr", "754", "--no-setup", "--json"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(!stderr.to_ascii_lowercase().contains("warning"), "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["data"]["source"]["checkout"], "branch");
    let worktree = Path::new(
        document["data"]["workspace"]["path"]
            .as_str()
            .expect("path"),
    );
    assert!(worktree.is_dir());
    assert_eq!(git_text(worktree, &["rev-parse", "HEAD"]), commit);
    assert_eq!(
        git_text(worktree, &["branch", "--show-current"]),
        "live-feature"
    );
    assert_eq!(
        branch_config(&clone, "live-feature", "remote").as_deref(),
        Some("origin")
    );
    assert_eq!(
        branch_config(&clone, "live-feature", "merge").as_deref(),
        Some("refs/heads/live-feature")
    );
}

#[test]
fn same_named_tag_cannot_hide_unique_local_branch_commits() {
    for deleted in [false, true] {
        let tree = TempTree::new();
        let (clone, _) = if deleted {
            deleted_pr_remote(&tree.path)
        } else {
            live_branch(&tree.path, "deleted-feature")
        };
        git(
            &clone,
            &["checkout", "-b", "deleted-feature", "refs/heads/main"],
        );
        git(
            &clone,
            &["commit", "--allow-empty", "-m", "unique local commit"],
        );
        let unique = git_text(&clone, &["rev-parse", "refs/heads/deleted-feature"]);
        git(&clone, &["checkout", "main"]);
        git(&clone, &["tag", "deleted-feature", "refs/heads/main"]);
        let bin = tree.path.join("bin");
        fake_gh(&bin);
        let output = rabi(&clone, &bin, &["create", "--pr", "843", "--no-setup"]);
        assert_eq!(
            git_text(&clone, &["rev-parse", "refs/heads/deleted-feature"]),
            unique,
            "checkout moved the local branch (deleted remote head: {deleted})"
        );
        assert!(
            !output.status.success(),
            "checkout should refuse unique local commits: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn failed_worktree_attachment_only_removes_a_new_branch() {
    for preexisting in [false, true] {
        let tree = TempTree::new();
        let (clone, commit) = live_branch(&tree.path, "live-feature");
        if preexisting {
            git(
                &clone,
                &["branch", "--track", "live-feature", "origin/live-feature"],
            );
        }
        let main = git_text(&clone, &["rev-parse", "refs/heads/main"]);
        let main_config = git_text(&clone, &["config", "--get-regexp", "^branch\\.main\\."]);
        // Require the PR fetch to restore the remote ref before attachment fails.
        git(
            &clone,
            &["update-ref", "-d", "refs/remotes/origin/live-feature"],
        );
        let bin = tree.path.join("bin");
        install_gh(&bin, &same_repo_pull("754", "live-feature"));
        let preference = rabi(
            &clone,
            &bin,
            &["config", "set", "workspace-root", "../spaces"],
        );
        assert!(
            preference.status.success(),
            "{}",
            String::from_utf8_lossy(&preference.stderr)
        );
        let real_git = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
            .map(|directory| directory.join("git"))
            .find(|path| path.is_file())
            .expect("real git");
        let shim = bin.join("git");
        test_support::write_executable(
            &shim,
            r#"#!/bin/sh
set -eu
if [ "$1" = "fetch" ]; then
  "$REAL_GIT" "$@"
  printf '%s\n' 'fetch succeeded' >> "$CHECKOUT_STATE"
  exit 0
fi
if [ "$1" = "worktree" ] && [ "${2-}" = "add" ]; then
  "$REAL_GIT" rev-parse --verify refs/heads/live-feature >> "$CHECKOUT_STATE"
  "$REAL_GIT" config --get branch.live-feature.remote >> "$CHECKOUT_STATE"
  "$REAL_GIT" config --get branch.live-feature.merge >> "$CHECKOUT_STATE"
  echo 'injected worktree add failure after fetch' >&2
  exit 1
fi
exec "$REAL_GIT" "$@"
"#,
        )
        .expect("git shim");
        let state = tree.path.join("checkout-state");
        let path = format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH"));
        let output = test_git::command(env!("CARGO_BIN_EXE_rabi"))
            .args(["create", "--pr", "754", "--no-setup"])
            .current_dir(&clone)
            .env("PATH", path)
            .env("HOME", tree.path.join("home"))
            .env("XDG_DATA_HOME", tree.path.join("data"))
            .env("XDG_CONFIG_HOME", tree.path.join("config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("REAL_GIT", real_git)
            .env("CHECKOUT_STATE", &state)
            .output()
            .expect("rabi");
        assert_eq!(output.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("injected worktree add failure after fetch"),
            "{stderr}"
        );
        assert_eq!(
            fs::read_to_string(&state).expect("state before failure"),
            format!("fetch succeeded\n{commit}\norigin\nrefs/heads/live-feature\n")
        );
        assert_eq!(
            git_output(&clone, &["show-ref", "--verify", "refs/heads/live-feature"])
                .status
                .success(),
            preexisting
        );
        if preexisting {
            assert_eq!(
                git_text(&clone, &["rev-parse", "refs/heads/live-feature"]),
                commit
            );
            assert_eq!(
                branch_config(&clone, "live-feature", "remote").as_deref(),
                Some("origin")
            );
            assert_eq!(
                branch_config(&clone, "live-feature", "merge").as_deref(),
                Some("refs/heads/live-feature")
            );
        } else {
            let config = git_output(
                &clone,
                &["config", "--get-regexp", "^branch\\.live-feature\\."],
            );
            assert_eq!(config.status.code(), Some(1));
            assert!(config.stdout.is_empty());
        }
        assert_eq!(git_text(&clone, &["rev-parse", "refs/heads/main"]), main);
        assert_eq!(
            git_text(&clone, &["config", "--get-regexp", "^branch\\.main\\."]),
            main_config
        );
        assert_eq!(
            git_text(&clone, &["rev-parse", "refs/remotes/origin/live-feature"]),
            commit
        );
        let destination = tree.path.join("spaces/live-feature");
        assert!(!destination.exists());
        fs::remove_file(&shim).expect("remove failing shim");
        let retry = rabi(&clone, &bin, &["create", "--pr", "754", "--no-setup"]);
        assert!(
            retry.status.success(),
            "{}",
            String::from_utf8_lossy(&retry.stderr)
        );
        assert!(destination.is_dir());
    }
}

#[test]
fn already_fetched_branch_keeps_its_remote_tip() {
    let tree = TempTree::new();
    let (clone, _) = live_branch(&tree.path, "live-feature");
    let before = git_text(&clone, &["rev-parse", "refs/remotes/origin/live-feature"]);
    let bin = tree.path.join("bin");
    install_gh(&bin, &same_repo_pull("754", "live-feature"));
    let output = rabi(
        &clone,
        &bin,
        &["create", "--pr", "754", "--no-setup", "--json"],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(!stderr.to_ascii_lowercase().contains("warning"), "{stderr}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    assert_eq!(document["data"]["source"]["checkout"], "branch");
    assert_eq!(
        git_text(&clone, &["rev-parse", "refs/remotes/origin/live-feature"]),
        before
    );
    assert_eq!(
        upstream(&clone, "live-feature").as_deref(),
        Some("origin/live-feature")
    );
}

#[test]
fn fork_checkout_does_not_set_an_upstream() {
    let tree = TempTree::new();
    let (clone, commit) = pull_ref_only(&tree.path, 9);
    let bin = tree.path.join("bin");
    install_gh(
        &bin,
        r#"#!/bin/sh
set -eu
if [ "$1" = "repo" ]; then
  printf '%s\n' '{"url":"https://github.com/acme/tool","owner":{"login":"acme"},"name":"tool"}'
  exit 0
fi
if [ "$1" = "api" ]; then
  case "$2" in
    */issues/9)
      printf '%s\n' '{"number":9,"pull_request":{}}'
      exit 0
      ;;
    */pulls/9)
      printf '%s\n' '{"number":9,"head":{"ref":"feature","repo":{"full_name":"other/tool"}},"base":{"repo":{"full_name":"acme/tool"}}}'
      exit 0
      ;;
  esac
fi
echo "unexpected gh invocation: $*" >&2
exit 1
"#,
    );
    let output = rabi(&clone, &bin, &["create", "--pr", "9", "--no-setup"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Created workspace 'other/feature'"));
    assert!(!stderr.to_ascii_lowercase().contains("warning"), "{stderr}");
    assert!(branch_config(&clone, "other/feature", "merge").is_none());
    assert!(branch_config(&clone, "other/feature", "remote").is_none());
    assert!(upstream(&clone, "other/feature").is_none());
    let head = git_text(&clone, &["rev-parse", "other/feature"]);
    assert_eq!(head, commit);
}

#[test]
fn gc_pr_collects_a_merged_pull_head_and_keeps_local_commits() {
    let tree = TempTree::new();
    let (clone, _) = deleted_pr_remote(&tree.path);
    let bin = tree.path.join("bin");
    install_gh(&bin, &merged_pull_gh());
    let checked_out = rabi(
        &clone,
        &bin,
        &["create", "--pr", "843", "--reuse", "--no-setup", "--json"],
    );
    let stdout = String::from_utf8_lossy(&checked_out.stdout);
    assert!(checked_out.status.success(), "{stdout}");
    let document: serde_json::Value = serde_json::from_str(&stdout).expect("json");
    let worktree = document["data"]["workspace"]["path"]
        .as_str()
        .expect("path")
        .to_string();

    let extra = TempTree::new();
    let (extra_clone, _) = deleted_pr_remote(&extra.path);
    let extra_bin = extra.path.join("bin");
    install_gh(&extra_bin, &merged_pull_gh());
    let extra_out = rabi(
        &extra_clone,
        &extra_bin,
        &["create", "--pr", "843", "--reuse", "--no-setup", "--json"],
    );
    let extra_stdout = String::from_utf8_lossy(&extra_out.stdout);
    assert!(extra_out.status.success(), "{extra_stdout}");
    let extra_doc: serde_json::Value = serde_json::from_str(&extra_stdout).expect("json");
    let extra_worktree = extra_doc["data"]["workspace"]["path"]
        .as_str()
        .expect("path");
    fs::write(Path::new(extra_worktree).join("local.txt"), "local\n").expect("local");
    git(Path::new(extra_worktree), &["add", "local.txt"]);
    git(Path::new(extra_worktree), &["commit", "-m", "local"]);
    let skipped = rabi(&extra_clone, &extra_bin, &["gc", "--closed-pr", "--json"]);
    let skipped_stdout = String::from_utf8_lossy(&skipped.stdout);
    let skipped_stderr = String::from_utf8_lossy(&skipped.stderr);
    assert!(
        skipped.status.success(),
        "{skipped_stdout}\n{skipped_stderr}"
    );
    let skipped_doc: serde_json::Value = serde_json::from_str(&skipped_stdout).expect("json");
    assert_eq!(skipped_doc["data"]["items"][0]["action"], "skipped");
    assert!(skipped_doc["data"]["items"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("--delete-unpreserved-branch"));
    assert!(Path::new(extra_worktree).is_dir());

    // The PR state is evidence; commits must also be preserved in the base.
    git(
        &clone,
        &["merge", "--ff-only", "refs/heads/deleted-feature"],
    );
    git(&clone, &["push", "origin", "main"]);
    let collected = rabi(&clone, &bin, &["gc", "--closed-pr", "--apply", "--json"]);
    let collected_stdout = String::from_utf8_lossy(&collected.stdout);
    let collected_stderr = String::from_utf8_lossy(&collected.stderr);
    assert!(
        collected.status.success(),
        "{collected_stdout}\n{collected_stderr}"
    );
    let collected_doc: serde_json::Value = serde_json::from_str(&collected_stdout).expect("json");
    assert_eq!(
        collected_doc["data"]["items"][0]["reasons"],
        serde_json::json!(["pullRequest"])
    );
    assert_eq!(collected_doc["data"]["items"][0]["action"], "removed");
    assert!(!Path::new(&worktree).exists());
}

fn rabi(clone: &Path, bin: &Path, args: &[&str]) -> std::process::Output {
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH"));
    let tree = clone.parent().expect("parent");
    test_git::command(env!("CARGO_BIN_EXE_rabi"))
        .args(args)
        .current_dir(clone)
        .env("PATH", path)
        .env("HOME", tree.join("home"))
        .env("XDG_DATA_HOME", tree.join("data"))
        .env("XDG_CONFIG_HOME", tree.join("config"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("rabi")
}

fn install_gh(bin: &Path, script: &str) {
    fs::create_dir_all(bin).expect("bin");
    let path = bin.join("gh");
    test_support::write_executable(&path, script).expect("gh script");
}

fn same_repo_pull(number: &str, head_ref: &str) -> String {
    format!(
        r#"#!/bin/sh
set -eu
if [ "$1" = "repo" ]; then
  printf '%s\n' '{{"url":"https://github.com/acme/tool","owner":{{"login":"acme"}},"name":"tool"}}'
  exit 0
fi
if [ "$1" = "api" ]; then
  case "$2" in
    */issues/{number})
      printf '%s\n' '{{"number":{number},"pull_request":{{}}}}'
      exit 0
      ;;
    */pulls/{number})
      printf '%s\n' '{{"number":{number},"head":{{"ref":"{head_ref}","repo":{{"full_name":"acme/tool"}}}},"base":{{"repo":{{"full_name":"acme/tool"}}}}}}'
      exit 0
      ;;
  esac
fi
echo "unexpected gh invocation: $*" >&2
exit 1
"#
    )
}

fn merged_pull_gh() -> String {
    r#"#!/bin/sh
set -eu
if [ "$1" = "pr" ]; then
  printf '%s\n' '{"state":"MERGED"}'
  exit 0
fi
if [ "$1" = "repo" ]; then
  printf '%s\n' '{"url":"https://github.com/acme/tool","owner":{"login":"acme"},"name":"tool"}'
  exit 0
fi
if [ "$1" = "api" ]; then
  case "$2" in
    */issues/843)
      printf '%s\n' '{"number":843,"pull_request":{}}'
      exit 0
      ;;
    */pulls/843)
      printf '%s\n' '{"number":843,"head":{"ref":"deleted-feature","repo":{"full_name":"acme/tool"}},"base":{"repo":{"full_name":"acme/tool"}}}'
      exit 0
      ;;
  esac
fi
echo "unexpected gh invocation: $*" >&2
exit 1
"#
    .to_string()
}

fn live_branch(root: &Path, branch: &str) -> (PathBuf, String) {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    git(
        root,
        &[
            "init",
            "--bare",
            "-b",
            "main",
            origin.to_str().expect("origin"),
        ],
    );
    test_git::configure_repo(&origin);
    git(root, &["init", "-b", "main", seed.to_str().expect("seed")]);
    git(&seed, &["commit", "--allow-empty", "-m", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", origin.to_str().expect("origin")],
    );
    git(&seed, &["checkout", "-b", branch]);
    fs::write(seed.join("live.txt"), "live\n").expect("live");
    git(&seed, &["add", "live.txt"]);
    git(&seed, &["commit", "-m", "live"]);
    let commit = git_text(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["push", "origin", "main", branch]);
    git(
        root,
        &[
            "clone",
            origin.to_str().expect("origin"),
            clone.to_str().expect("clone"),
        ],
    );
    (clone, commit)
}

fn pull_ref_only(root: &Path, number: u64) -> (PathBuf, String) {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    git(
        root,
        &[
            "init",
            "--bare",
            "-b",
            "main",
            origin.to_str().expect("origin"),
        ],
    );
    test_git::configure_repo(&origin);
    git(root, &["init", "-b", "main", seed.to_str().expect("seed")]);
    git(&seed, &["commit", "--allow-empty", "-m", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", origin.to_str().expect("origin")],
    );
    git(&seed, &["push", "origin", "main"]);
    git(&seed, &["checkout", "-b", "feature"]);
    fs::write(seed.join("fork.txt"), "fork\n").expect("fork");
    git(&seed, &["add", "fork.txt"]);
    git(&seed, &["commit", "-m", "fork"]);
    let commit = git_text(&seed, &["rev-parse", "HEAD"]);
    git(
        &seed,
        &["push", "origin", &format!("HEAD:refs/pull/{number}/head")],
    );
    git(
        root,
        &[
            "clone",
            origin.to_str().expect("origin"),
            clone.to_str().expect("clone"),
        ],
    );
    (clone, commit)
}

/// Clone while the branch exists, then delete it on the remote only.
fn cloned_before_branch_delete(root: &Path) -> (PathBuf, String) {
    let origin = root.join("origin.git");
    let seed = root.join("seed");
    let clone = root.join("clone");
    git(
        root,
        &[
            "init",
            "--bare",
            "-b",
            "main",
            origin.to_str().expect("origin"),
        ],
    );
    test_git::configure_repo(&origin);
    git(root, &["init", "-b", "main", seed.to_str().expect("seed")]);
    git(&seed, &["commit", "--allow-empty", "-m", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", origin.to_str().expect("origin")],
    );
    git(&seed, &["push", "origin", "main"]);
    git(&seed, &["checkout", "-b", "deleted-feature"]);
    fs::write(seed.join("pr.txt"), "pull 843\n").expect("file");
    git(&seed, &["add", "pr.txt"]);
    git(&seed, &["commit", "-m", "pr head"]);
    let commit = git_text(&seed, &["rev-parse", "HEAD"]);
    git(&seed, &["push", "origin", "deleted-feature"]);
    git(&origin, &["update-ref", "refs/pull/843/head", &commit]);
    git(
        root,
        &[
            "clone",
            origin.to_str().expect("origin"),
            clone.to_str().expect("clone"),
        ],
    );
    git(&seed, &["push", "origin", ":deleted-feature"]);
    (clone, commit)
}

#[test]
fn checkout_records_issue_pr_sync_bases_but_attach_leaves_base_null() {
    for present in [false, true] {
        let tree = TempTree::new();
        let (clone, _) = deleted_pr_remote(&tree.path);
        let bin = tree.path.join("bin");
        let fallback = "refs/remotes/origin/main".to_string();
        if present {
            git(&clone, &["update-ref", "refs/remotes/origin/next", "HEAD"]);
        }
        let script = same_repo_pull("843", "deleted-feature")
            .replace("\"base\":{", "\"base\":{\"ref\":\"next\",");
        install_gh(&bin, &script);
        let out = rabi(
            &clone,
            &bin,
            &["create", "--pr", "843", "--reuse", "--no-setup", "--json"],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let expected = if present {
            "refs/remotes/origin/next"
        } else {
            &fallback
        };
        let list = rabi(&clone, &bin, &["list", "--json"]);
        let doc: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
        assert!(doc.to_string().contains(expected), "{doc}");

        let out = rabi(&clone, &bin, &["set", "deleted-feature", "--base", "HEAD"]);
        assert!(out.status.success());
        let out = rabi(
            &clone,
            &bin,
            &["create", "--pr", "843", "--reuse", "--no-setup", "--json"],
        );
        assert!(out.status.success());
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["data"]["workspace"]["base"], "refs/heads/main");

        install_gh(
            &bin,
            &script.replace("{\"number\":843,\"pull_request\":{}}", "{\"number\":843}"),
        );
        for (number, explicit) in [("844", false), ("845", true)] {
            install_gh(
                &bin,
                &same_repo_pull(number, "unused").replace(
                    &format!("{{\"number\":{number},\"pull_request\":{{}}}}"),
                    &format!("{{\"number\":{number}}}"),
                ),
            );
            let mut args = vec!["create", "--issue", number, "--no-setup", "--json"];
            if explicit {
                args.extend(["--base", "HEAD"]);
            }
            let out = rabi(&clone, &bin, &args);
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let store: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            let worktree = store["data"]["workspace"]["path"].as_str().unwrap();
            let listed = rabi(
                &clone,
                &bin,
                &["show", &format!("issues/{number}"), "--json"],
            );
            let doc: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
            assert_eq!(
                doc["data"]["workspace"]["base"],
                if explicit {
                    "refs/heads/main"
                } else {
                    &fallback
                },
                "{worktree} {doc}"
            );
        }
        git(&clone, &["branch", "attached", "HEAD"]);
        let out = rabi(
            &clone,
            &bin,
            &["create", "--branch", "attached", "--no-setup", "--json"],
        );
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(doc["data"]["workspace"]["base"].is_null());
    }
}
