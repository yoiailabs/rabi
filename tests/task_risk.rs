#[path = "common/git.rs"]
mod test_git;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
}

impl Repo {
    fn new(policy: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("rabi-risk-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        test_git::write_config(&root.join("home"));
        let fixture = Self { root, repo };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "Test"]);
        fixture.git(&["config", "user.email", "test@example.invalid"]);
        fixture.write("docs/start.md", "initial\n");
        if let Some(policy) = policy {
            fixture.write(".rabi/policy.toml", policy);
        }
        fixture.commit();
        fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        fixture.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        fixture.git(&["checkout", "-b", "topic"]);
        fixture
    }

    fn command(&self, program: &str, args: &[&str], cwd: &Path) -> Command {
        let mut cmd = test_git::command(program);
        cmd.args(args)
            .current_dir(cwd)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_TRUST_ALL")
            .env_remove("GHQ_ROOT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
        cmd
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.command("git", args, &self.repo).output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn write(&self, path: &str, contents: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn commit(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "-m", "fixture"]);
    }

    fn rabi(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_rabi"), args, &self.repo)
            .output()
            .unwrap()
    }

    fn json(&self, args: &[&str]) -> serde_json::Value {
        let result = self.rabi(args);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["schema"], 2);
        assert_eq!(json["command"], "task risk");
        assert_eq!(json["ok"], true);
        assert!(json["error"].is_null());
        json
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const LOW_DOCS: &str = "schema = 1\n[[rule]]\npaths = [\"docs/**\"]\ntier = \"low\"\n";

#[test]
fn policy_is_read_from_base_tip_never_head_or_worktree() {
    let repo = Repo::new(Some(LOW_DOCS));
    let base = repo.git(&["rev-parse", "origin/main"]);
    repo.write("docs/start.md", "changed\n");
    repo.write(".rabi/policy.toml", "schema = 2\n");
    repo.commit();
    repo.write(".rabi/policy.toml", "not valid toml");
    let result = repo.json(&["task", "risk", "--json"]);
    let data = &result["data"];
    assert_eq!(data["base"]["ref"], "origin/main");
    assert_eq!(data["base"]["sha"], base);
    assert_eq!(data["mergeBase"], base);
    assert_eq!(data["head"], repo.git(&["rev-parse", "HEAD"]));
    assert_eq!(
        data["policy"]["source"],
        format!("{base}:.rabi/policy.toml")
    );
    assert_eq!(data["policy"]["present"], true);
    assert_eq!(data["tier"], "high");
    assert_eq!(data["floor"], true);
    let files = data["files"].as_array().unwrap();
    assert_eq!(
        files.iter().find(|f| f["path"] == "docs/start.md").unwrap()["tier"],
        "low"
    );
    let policy = files
        .iter()
        .find(|f| f["path"] == ".rabi/policy.toml")
        .unwrap();
    assert_eq!(policy["floor"], true);
    assert!(policy["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "floor:builtin:.rabi/policy.toml"));
    assert_eq!(data["totals"]["files"], 2);
    assert!(result["warnings"].as_array().unwrap().is_empty());
    assert_eq!(data.as_object().unwrap().len(), 9);
}

#[test]
fn base_tip_policy_is_used_even_when_merge_base_has_no_policy() {
    let repo = Repo::new(None);
    repo.write("docs/start.md", "topic\n");
    repo.commit();
    repo.git(&["checkout", "main"]);
    repo.write(".rabi/policy.toml", LOW_DOCS);
    repo.commit();
    let base = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["checkout", "topic"]);
    let json = repo.json(&["task", "risk", "--base", "main", "--json"]);
    assert_eq!(json["data"]["tier"], "low");
    assert_eq!(json["data"]["base"]["sha"], base);
    assert_ne!(json["data"]["mergeBase"], base);
}

#[test]
fn missing_policy_uses_defaults_and_explicit_base_needs_no_origin() {
    let repo = Repo::new(None);
    repo.write("docs/new.md", "new\n");
    repo.commit();
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    let json = repo.json(&["task", "risk", "--base", "main", "--output", "json"]);
    assert_eq!(json["data"]["policy"]["present"], false);
    assert_eq!(json["data"]["tier"], "high");
    assert_eq!(json["data"]["files"][0]["reasons"][0], "default-tier");
    let failure = repo.rabi(&["task", "risk", "--json"]);
    assert_eq!(failure.status.code(), Some(1));
    let error: serde_json::Value = serde_json::from_slice(&failure.stdout).unwrap();
    assert_eq!(error["error"]["code"], "git_failed");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("origin/main"));
}

#[test]
fn invalid_base_policy_is_an_error_not_a_fallback() {
    for policy in [
        "schema = 2",
        "schema = 1\n[ci]\nx = 1",
        "schema = 1\ndefault-tier = 'tiny'",
    ] {
        let repo = Repo::new(Some(policy));
        let output = repo.rabi(&["task", "risk", "--json"]);
        assert_eq!(output.status.code(), Some(1));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"]["code"], "failed");
        assert!(json["data"].is_null());
    }
}

#[test]
fn rename_uses_old_sensitive_path_and_preserves_nul_delimited_names() {
    let repo = Repo::new(Some(
        "schema = 1\ndefault-tier = 'low'\n[[rule]]\npaths = ['docs/**']\ntier = 'high'",
    ));
    repo.git(&["mv", "docs/start.md", "moved\tfile\n.md"]);
    repo.commit();
    let json = repo.json(&["task", "risk", "--json"]);
    let files = json["data"]["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["oldPath"], "docs/start.md");
    assert_eq!(files[0]["path"], "moved\tfile\n.md");
    assert_eq!(files[0]["status"], "R100");
    assert_eq!(files[0]["tier"], "high");
    assert_eq!(json["data"]["totals"]["changedLines"], 0);
}

#[test]
fn uncommitted_files_and_index_are_not_inspected() {
    let repo = Repo::new(Some(LOW_DOCS));
    let before = repo.json(&["task", "risk", "--json"]);
    assert_eq!(before["data"]["tier"], "low");
    assert_eq!(before["data"]["reasons"][0], "no changes");
    repo.write("untracked", "not classified\n");
    repo.write("docs/start.md", "uncommitted\n");
    repo.git(&["add", "docs/start.md"]);
    repo.write("docs/start.md", "also unstaged\n");
    repo.write(".rabi.toml", "invalid checkout config");
    // A malformed index makes any accidental Git status/index read fail.
    let index = repo.repo.join(".git/index");
    fs::write(&index, b"not a git index").unwrap();
    let after = repo.json(&["task", "risk", "--json"]);
    assert_eq!(before, after);
    assert_eq!(fs::read(index).unwrap(), b"not a git index");
    let output = repo.rabi(&["task", "risk"]);
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("Tier: low"));
    assert!(output.stderr.is_empty());
}

#[test]
fn task_group_and_argument_validation_use_usage_status() {
    let repo = Repo::new(None);
    for args in [
        vec!["task", "missing"],
        vec!["task", "risk", "a", "b"],
        vec!["task", "risk", "--base"],
    ] {
        assert_eq!(repo.rabi(&args).status.code(), Some(2), "{args:?}");
    }
    assert!(repo.rabi(&["task", "--help"]).status.success());
    // Like other command groups, a bare group prints its help.
    let bare = repo.rabi(&["task"]);
    assert!(bare.status.success());
    assert!(String::from_utf8(bare.stdout)
        .unwrap()
        .contains("rabi task risk"));
}

#[test]
fn selector_and_global_repo_work_for_primary_and_linked_workspaces() {
    let repo = Repo::new(Some(LOW_DOCS));
    repo.git(&["checkout", "main"]);
    let linked = repo.root.join("linked");
    repo.git(&["worktree", "add", linked.to_str().unwrap(), "topic"]);
    let args = [
        "-R",
        repo.repo.to_str().unwrap(),
        "task",
        "risk",
        linked.to_str().unwrap(),
        "--json",
    ];
    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &args, &repo.root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let primary = repo.json(&["task", "risk", ".", "--json"]);
    assert_eq!(primary["data"]["tier"], "low");
}

#[test]
fn risk_does_not_write_index_refs_records_or_run_fsmonitor_or_external_diff() {
    let repo = Repo::new(Some(LOW_DOCS));
    repo.write("docs/start.md", "committed\n");
    repo.commit();
    repo.write("docs/start.md", "uncommitted\n");
    repo.git(&["config", "core.fsmonitor", "touch should-not-exist"]);
    repo.git(&["config", "diff.external", "touch should-not-exist"]);
    let index = fs::read(repo.repo.join(".git/index")).unwrap();
    let refs = repo.git(&["show-ref"]);
    assert!(!repo.repo.join(".git/rabi").exists());
    assert!(!repo.root.join("state").exists());
    repo.json(&["task", "risk", "--json"]);
    assert_eq!(fs::read(repo.repo.join(".git/index")).unwrap(), index);
    assert_eq!(repo.git(&["show-ref"]), refs);
    assert!(!repo.repo.join("should-not-exist").exists());
    assert!(!repo.repo.join(".git/rabi").exists());
    assert!(!repo.root.join("state").exists());
}

#[test]
fn uncommitted_attributes_do_not_change_binary_detection_or_line_totals() {
    let repo = Repo::new(Some("schema = 1\ndefault-tier = 'low'"));
    fs::write(repo.repo.join("image"), b"first\0bytes").unwrap();
    repo.commit();
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    fs::write(repo.repo.join("image"), b"other\0bytes").unwrap();
    repo.commit();
    for staged in [false, true] {
        repo.write(".gitattributes", "image diff\n");
        if staged {
            repo.git(&["add", ".gitattributes"]);
        }
        let json = repo.json(&["task", "risk", "--json"]);
        assert_eq!(json["data"]["tier"], "high");
        assert_eq!(json["data"]["totals"]["changedLines"], 0);
        assert!(json["data"]["files"][0]["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r == "kind:binary"));
    }
    // A head commit that marks the binary as text does not lower it either.
    repo.git(&["add", ".gitattributes"]);
    repo.commit();
    let json = repo.json(&["task", "risk", "--json"]);
    assert_eq!(json["data"]["tier"], "high");
    assert!(json["data"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file["path"] == "image"
            && file["reasons"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "kind:binary")));
}

#[test]
fn missing_promised_policy_blob_fails_without_lazy_fetching() {
    let repo = Repo::new(Some(LOW_DOCS));
    let oid = repo.git(&["rev-parse", "HEAD:.rabi/policy.toml"]);
    fs::remove_file(
        repo.repo
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]),
    )
    .unwrap();
    repo.git(&["init", "--bare", "--template=", "remote"]);
    repo.git(&["config", "remote.origin.url", "remote"]);
    repo.git(&["config", "remote.origin.promisor", "true"]);
    repo.git(&["config", "remote.origin.partialclonefilter", "blob:none"]);
    // A lazy fetch would invoke this local transport and create the marker.
    repo.git(&[
        "config",
        "remote.origin.uploadpack",
        "touch fetch-attempted",
    ]);
    let output = repo.rabi(&["task", "risk", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["error"]["code"], "git_failed");
    assert!(!repo.repo.join("fetch-attempted").exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_committed_paths_force_high_without_lossy_matching() {
    use std::os::unix::ffi::OsStringExt;
    let repo = Repo::new(Some(
        "schema = 1\ndefault-tier = 'low'\n[[rule]]\npaths = ['**']\ntier = 'low'",
    ));
    let name = std::ffi::OsString::from_vec(b"docs/non-utf8-\xff".to_vec());
    fs::write(repo.repo.join(name), "contents\n").unwrap();
    repo.commit();
    let json = repo.json(&["task", "risk", "--json"]);
    assert_eq!(json["data"]["tier"], "high");
    assert_eq!(json["data"]["floor"], true);
    let reasons = json["data"]["files"][0]["reasons"].as_array().unwrap();
    assert!(reasons
        .iter()
        .any(|reason| reason == "unknown:path-encoding"));
    assert!(reasons
        .iter()
        .any(|reason| reason == "floor:unknown:path-encoding"));
    assert!(!reasons.iter().any(|reason| reason == "rule:**"));
}

#[test]
fn risk_never_runs_clean_or_process_filters() {
    for (name, driver) in [
        ("driver", "clean"),
        ("driver", "process"),
        ("a=b", "clean"),
        ("a=b", "process"),
    ] {
        let repo = Repo::new(Some(LOW_DOCS));
        repo.write("docs/file.txt", "base\n");
        repo.write(".gitattributes", &format!("*.txt filter={name}\n"));
        repo.commit();
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&[
            "config",
            &format!("filter.{name}.{driver}"),
            "touch filter-executed; cat",
        ]);
        repo.git(&["config", &format!("filter.{name}.required"), "true"]);
        repo.write("docs/file.txt", "dirt\n");
        repo.json(&["task", "risk", "--json"]);
        assert!(!repo.repo.join("filter-executed").exists(), "{driver}");
    }
}

#[test]
fn risk_does_not_run_filters_from_worktree_configuration() {
    let repo = Repo::new(Some(LOW_DOCS));
    repo.write("docs/file.txt", "base\n");
    repo.write(".gitattributes", "*.txt filter=driver\n");
    repo.commit();
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let linked = repo.root.join("linked");
    repo.git(&["worktree", "add", "-b", "linked", linked.to_str().unwrap()]);
    repo.git(&["config", "extensions.worktreeConfig", "true"]);
    let output = repo
        .command(
            "git",
            &[
                "config",
                "--worktree",
                "filter.driver.clean",
                "touch filter-executed; cat",
            ],
            &linked,
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(linked.join("docs/file.txt"), "dirt\n").unwrap();
    repo.json(&["task", "risk", linked.to_str().unwrap(), "--json"]);
    assert!(!linked.join("filter-executed").exists());
}

#[test]
fn risk_does_not_inspect_nested_populated_submodules() {
    let inner = Repo::new(None);
    inner.write("docs/file.txt", "base\n");
    inner.write(".gitattributes", "*.txt filter=nested-driver\n");
    inner.commit();
    let outer = Repo::new(None);
    outer.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        inner.repo.to_str().unwrap(),
        "inner",
    ]);
    outer.commit();
    let repo = Repo::new(Some(LOW_DOCS));
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        outer.repo.to_str().unwrap(),
        "module",
    ]);
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "update",
        "--init",
        "--recursive",
    ]);
    repo.commit();
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    let nested = repo.repo.join("module/inner");
    let output = repo
        .command(
            "git",
            &[
                "config",
                "filter.nested-driver.clean",
                "touch filter-executed; cat",
            ],
            &nested,
        )
        .output()
        .unwrap();
    assert!(output.status.success());
    fs::write(nested.join("docs/file.txt"), "dirt\n").unwrap();
    repo.json(&["task", "risk", "--json"]);
    assert!(!nested.join("filter-executed").exists());
    // Untracked submodule contents are also irrelevant to committed inspection.
    fs::write(nested.join("docs/file.txt"), "base\n").unwrap();
    fs::write(nested.join("untracked"), "untracked\n").unwrap();
    repo.json(&["task", "risk", "--json"]);
    assert!(!nested.join("filter-executed").exists());
}

#[test]
fn tree_diff_never_executes_filters_from_inherited_configuration() {
    let repo = Repo::new(Some(LOW_DOCS));
    repo.write("docs/file.txt", "base\n");
    repo.write(".gitattributes", "*.txt filter=a=b\n");
    repo.commit();
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.write("docs/file.txt", "dirt\n");
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["task", "risk", "--json"],
            &repo.repo,
        )
        .env("GIT_CONFIG_COUNT", "4")
        .env("GIT_CONFIG_KEY_2", "filter.a=b.clean")
        .env("GIT_CONFIG_VALUE_2", "touch filter-executed; cat")
        .env("GIT_CONFIG_KEY_3", "filter.a=b.required")
        .env("GIT_CONFIG_VALUE_3", "true")
        .env(
            "GIT_CONFIG_PARAMETERS",
            "'filter.a=b.clean'='touch legacy-executed; cat'",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!repo.repo.join("filter-executed").exists());
    assert!(!repo.repo.join("legacy-executed").exists());
    // A representable legacy key also cannot introduce callbacks.
    repo.write(".gitattributes", "*.txt filter=driver\n");
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["task", "risk", "--json"],
            &repo.repo,
        )
        .env(
            "GIT_CONFIG_PARAMETERS",
            "'filter.driver.clean=touch legacy-executed; cat'",
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!repo.repo.join("legacy-executed").exists());
}

#[test]
fn status_free_selectors_keep_name_link_and_foreign_dot_rules() {
    let repo = Repo::new(Some(LOW_DOCS));
    repo.git(&["checkout", "main"]);
    let linked = repo.root.join("linked");
    repo.git(&["worktree", "add", linked.to_str().unwrap(), "topic"]);
    let set = repo.rabi(&["set", linked.to_str().unwrap(), "--pr", "7", "--issue", "9"]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    let expected = repo.json(&["task", "risk", linked.to_str().unwrap(), "--json"]);
    for selector in ["topic", "pr:7", "issue:9"] {
        assert_eq!(repo.json(&["task", "risk", selector, "--json"]), expected);
    }
    let other = Repo::new(None);
    let output = repo.rabi(&[
        "-R",
        other.repo.to_str().unwrap(),
        "task",
        "risk",
        ".",
        "--json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "usage");
}

#[test]
fn missing_base_policy_does_not_read_uncommitted_policy_or_index() {
    let repo = Repo::new(None);
    repo.write(".rabi/policy.toml", "not a committed policy");
    fs::write(repo.repo.join(".git/index"), b"not a git index").unwrap();
    let json = repo.json(&["task", "risk", "--json"]);
    assert_eq!(json["data"]["policy"]["present"], false);
    assert_eq!(json["data"]["tier"], "low");
}

#[cfg(unix)]
fn instrument_git(repo: &Repo) -> (PathBuf, String) {
    use std::os::unix::fs::PermissionsExt;
    let output = test_git::command("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let real = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("git");
    fs::write(
        &script,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$RABI_RISK_TEST_TRACE"
for arg; do
    case "$arg" in
        "$RABI_RISK_TEST_REJECT"*)
            printf 'unknown option: %s\n' "$arg" >&2
            exit 129
            ;;
    esac
done
exec "$RABI_RISK_TEST_GIT" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, real)
}

#[cfg(unix)]
#[test]
fn unsupported_git_options_fail_closed_with_clear_json_errors() {
    for rejected in ["--no-lazy-fetch", "--attr-source="] {
        let repo = Repo::new(Some(LOW_DOCS));
        let (bin, real) = instrument_git(&repo);
        let paths = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )))
        .unwrap();
        let trace = repo.root.join("git-trace");
        let output = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["task", "risk", "--json"],
                &repo.repo,
            )
            .env("PATH", paths)
            .env("RABI_RISK_TEST_GIT", real)
            .env("RABI_RISK_TEST_TRACE", &trace)
            .env("RABI_RISK_TEST_REJECT", rejected)
            .env_remove("GIT_NO_LAZY_FETCH")
            .env_remove("GIT_ATTR_SOURCE")
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["error"]["code"], "git_failed");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Git 2.44 or later"));
        assert!(json["data"].is_null());
        let calls = fs::read_to_string(trace).unwrap();
        assert!(calls.lines().any(|line| line.contains(rejected)));
        if rejected == "--no-lazy-fetch" {
            assert_eq!(
                calls.lines().count(),
                1,
                "unsupported Git must stop before selector discovery"
            );
        }
    }
}

#[cfg(unix)]
#[test]
fn object_queries_use_options_and_policy_lookup_never_probes_checkout_paths() {
    let repo = Repo::new(None);
    repo.write(".rabi/policy.toml", "invalid uncommitted policy");
    fs::write(repo.repo.join(".git/index"), b"not a git index").unwrap();
    let base = repo.git(&["rev-parse", "origin/main"]);
    let (bin, real) = instrument_git(&repo);
    let paths = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let trace = repo.root.join("git-trace");
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["task", "risk", "--json"],
            &repo.repo,
        )
        .env("PATH", paths)
        .env("RABI_RISK_TEST_GIT", real)
        .env("RABI_RISK_TEST_TRACE", &trace)
        .env("RABI_RISK_TEST_REJECT", "--never-rejected")
        .env_remove("GIT_NO_LAZY_FETCH")
        .env_remove("GIT_ATTR_SOURCE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let calls = fs::read_to_string(trace).unwrap();
    for line in calls.lines().filter(|line| {
        line.contains("^{commit}")
            || line.contains("cat-file")
            || line.contains("merge-base")
            || line.contains(" diff ")
    }) {
        assert!(line.starts_with("--no-lazy-fetch "), "{line}");
    }
    for line in calls.lines().filter(|line| line.contains(" diff ")) {
        assert!(
            line.starts_with(&format!("--no-lazy-fetch --attr-source={base} diff ")),
            "{line}"
        );
    }
    assert!(calls.contains("cat-file tree"));
    assert!(!calls.contains("cat-file -e"));
    assert!(!calls.contains(":.rabi/policy.toml"));
}

#[test]
fn missing_committed_policy_tree_is_an_error_not_defaults() {
    let repo = Repo::new(Some(LOW_DOCS));
    let oid = repo.git(&["rev-parse", "HEAD:.rabi"]);
    fs::remove_file(
        repo.repo
            .join(".git/objects")
            .join(&oid[..2])
            .join(&oid[2..]),
    )
    .unwrap();
    let output = repo.rabi(&["task", "risk", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["error"]["code"], "git_failed");
    assert!(json["data"].is_null());
}

#[test]
fn outside_a_repository_keeps_the_shared_not_repository_status() {
    let repo = Repo::new(None);
    let outside = repo.root.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["task", "risk", "--json"],
            &outside,
        )
        .env("GIT_CEILING_DIRECTORIES", &repo.root)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["error"]["code"], "not_repository");
}

#[cfg(unix)]
#[test]
fn a_policy_that_is_not_a_regular_file_fails() {
    let repo = Repo::new(None);
    repo.git(&["checkout", "main"]);
    fs::create_dir_all(repo.repo.join(".rabi")).unwrap();
    std::os::unix::fs::symlink("../policy-target.toml", repo.repo.join(".rabi/policy.toml"))
        .unwrap();
    repo.write("policy-target.toml", "schema = 1\n");
    repo.commit();
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&["checkout", "topic"]);
    repo.git(&["merge", "--ff-only", "main"]);
    let output = repo.rabi(&["task", "risk", "--json"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["error"]["code"], "failed");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("must be a regular file"));
}
