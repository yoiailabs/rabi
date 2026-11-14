#![cfg(unix)]
#[path = "common/git.rs"]
mod test_git;
#[path = "common/scripts.rs"]
mod test_support;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const GH: &str = r#"#!/bin/sh
set -eu
printf '%s\000' "$@" > "$RABI_PUBLISH_FIXTURE/gh-calls/args.$$"
case "$1 $2" in
  'pr list')
    if [ -f "$RABI_PUBLISH_FIXTURE/pause-publication" ]; then
      touch "$RABI_PUBLISH_FIXTURE/publication-paused"
      while [ -f "$RABI_PUBLISH_FIXTURE/pause-publication" ]; do sleep 0.01; done
    fi
    if [ -f "$RABI_PUBLISH_FIXTURE/kill-before-pr" ]; then
      rm "$RABI_PUBLISH_FIXTURE/kill-before-pr"
      kill -KILL "$PPID"
      exit 1
    fi
    if [ -f "$RABI_PUBLISH_FIXTURE/pr-created" ]; then
      cat "$RABI_PUBLISH_FIXTURE/pr-list.json"
    else
      printf '[]\n'
    fi
    ;;
  'pr create')
    printf 'created\n' >> "$RABI_PUBLISH_FIXTURE/creates"
    touch "$RABI_PUBLISH_FIXTURE/pr-created"
    previous=''
    for arg in "$@"; do
      if [ "$previous" = --body-file ]; then
        cat "$arg" > "$RABI_PUBLISH_FIXTURE/pr-body"
      fi
      previous=$arg
    done
    if [ -f "$RABI_PUBLISH_FIXTURE/kill-after-pr" ]; then
      rm "$RABI_PUBLISH_FIXTURE/kill-after-pr"
      kill -KILL "$PPID"
      exit 1
    fi
    printf 'https://example.invalid/project/pull/7\n'
    ;;
  'pr view')
    cat "$RABI_PUBLISH_FIXTURE/pr.json"
    if [ -f "$RABI_PUBLISH_FIXTURE/fail-final-record" ]; then
      mv "$RABI_PUBLISH_RECORD" "$RABI_PUBLISH_FIXTURE/record.saved"
      rm "$RABI_PUBLISH_FIXTURE/fail-final-record"
    fi
    ;;
  *) printf 'unexpected gh invocation\n' >&2; exit 91 ;;
esac
"#;

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    workspace: PathBuf,
    remote: PathBuf,
    task: String,
    record: PathBuf,
    head: String,
}

impl Repo {
    fn new(commands: Option<&[&str]>, policy: Option<&str>) -> Self {
        let root = std::env::temp_dir().join(format!("rabi-publish-test-{}", uuid::Uuid::new_v4()));
        let mut fixture = Self {
            repo: root.join("repo"),
            workspace: root.join("workspace"),
            remote: root.join("remote.git"),
            root,
            task: String::new(),
            record: PathBuf::new(),
            head: String::new(),
        };
        for path in [
            &fixture.repo,
            &fixture.root.join("home"),
            &fixture.root.join("bin"),
            &fixture.root.join("git-calls"),
            &fixture.root.join("gh-calls"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        test_git::write_config(&fixture.root.join("home"));
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "fixture"]);
        fixture.git(&["config", "user.email", "fixture"]);
        fixture.git(&[
            "init",
            "--bare",
            "--template=",
            fixture.remote.to_str().unwrap(),
        ]);
        test_git::configure_repo(&fixture.remote);
        fs::write(fixture.repo.join("tracked"), "base\n").unwrap();
        if let Some(commands) = commands {
            fixture.config(&fixture.repo, commands);
        }
        if let Some(policy) = policy {
            fs::create_dir_all(fixture.repo.join(".rabi")).unwrap();
            fs::write(fixture.repo.join(".rabi/policy.toml"), policy).unwrap();
        }
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "fixture base"]);
        fixture.git(&["remote", "add", "origin", fixture.remote.to_str().unwrap()]);
        let created = decode(
            fixture
                .command(&[
                    "task",
                    "new",
                    "Publish recorded intent",
                    "--branch",
                    "topic",
                    "--base",
                    "main",
                    "--json",
                ])
                .output()
                .unwrap(),
            0,
        );
        fixture.task = created["data"]["task"]["id"].as_str().unwrap().into();
        fixture.git(&[
            "worktree",
            "add",
            "-b",
            "topic",
            fixture.workspace.to_str().unwrap(),
            "main",
        ]);
        let repository = fs::read_dir(fixture.root.join("state/rabi/repositories"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        fixture.record = repository
            .join("tasks")
            .join(&fixture.task)
            .join("task.json");
        fixture.state("implementing");
        fs::write(fixture.workspace.join("tracked"), "implemented\n").unwrap();
        fixture.commit("feat: implement fixture");
        test_support::write_executable(fixture.root.join("bin/gh"), GH).unwrap();
        fixture.install_git_wrapper();
        fixture
    }
    fn config(&self, directory: &Path, commands: &[&str]) {
        fs::write(
            directory.join(".rabi.toml"),
            format!(
                "[precheck]\ncommands = {}\ntimeout = '3s'\n",
                serde_json::to_string(commands).unwrap()
            ),
        )
        .unwrap();
    }
    fn base_command(&self, program: impl AsRef<std::ffi::OsStr>) -> Command {
        let mut command = test_git::command(program);
        let path = std::env::join_paths(std::iter::once(self.root.join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        command
            .current_dir(&self.repo)
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("PATH", path)
            .env("RABI_PUBLISH_FIXTURE", &self.root)
            .env("RABI_TRUST_ALL", "1")
            .env_remove("RABI_OUTPUT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
        command
    }
    fn command(&self, args: &[&str]) -> Command {
        let mut command = self.base_command(env!("CARGO_BIN_EXE_rabi"));
        command.args(args);
        command
    }
    fn publish(&self) -> Command {
        self.command(&["task", "publish", &self.task, "--json"])
    }
    fn git(&self, args: &[&str]) -> String {
        let output = self.base_command("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn commit(&mut self, subject: &str) {
        self.git(&["-C", self.workspace.to_str().unwrap(), "add", "."]);
        self.git(&[
            "-C",
            self.workspace.to_str().unwrap(),
            "commit",
            "-m",
            subject,
        ]);
        self.head = self.git(&["-C", self.workspace.to_str().unwrap(), "rev-parse", "HEAD"]);
        let pr = json!({"number": 7, "headRefName": "topic", "headRefOid": self.head, "isCrossRepository": false, "state": "OPEN"});
        fs::write(self.root.join("pr.json"), serde_json::to_vec(&pr).unwrap()).unwrap();
        fs::write(
            self.root.join("pr-list.json"),
            serde_json::to_vec(&json!([pr])).unwrap(),
        )
        .unwrap();
    }
    fn state(&self, state: &str) {
        let mut task: Value = serde_json::from_slice(&fs::read(&self.record).unwrap()).unwrap();
        task["state"] = json!({"name": state});
        task["workspacePath"] = json!(self.workspace);
        fs::write(&self.record, serde_json::to_vec(&task).unwrap()).unwrap();
    }
    fn view(&self) -> Value {
        decode(
            self.command(&["task", "show", &self.task, "--json"])
                .output()
                .unwrap(),
            0,
        )["data"]
            .clone()
    }
    fn pushes(&self) -> Vec<Vec<String>> {
        let mut calls = Vec::new();
        for entry in fs::read_dir(self.root.join("git-calls")).unwrap() {
            let args: Vec<_> = fs::read(entry.unwrap().path())
                .unwrap()
                .split(|byte| *byte == 0)
                .filter(|arg| !arg.is_empty())
                .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
                .collect();
            if args.iter().any(|arg| arg == "push") {
                calls.push(args);
            }
        }
        calls
    }
    fn creates(&self) -> usize {
        fs::read_to_string(self.root.join("creates"))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn install_git_wrapper(&self) {
        let real = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        assert!(real.status.success());
        let script = format!(
            r#"#!/bin/sh
set -eu
printf '%s\000' "$@" > "$RABI_PUBLISH_FIXTURE/git-calls/args.$$"
exec '{}' "$@"
"#,
            String::from_utf8(real.stdout)
                .unwrap()
                .trim()
                .replace('\'', "'\\''")
        );
        test_support::write_executable(self.root.join("bin/git"), script).unwrap();
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn decode(output: Output, exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], exit == 0);
    value
}
fn precheck(view: &Value) -> &Value {
    &view["events"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|event| event.get("precheck").is_some())
        .expect("precheck event")["precheck"]
}

#[test]
fn undeclared_precheck_publishes_with_explicit_refspec_and_no_hooks() {
    let repo = Repo::new(None, None);
    fs::create_dir_all(repo.repo.join(".git/hooks")).unwrap();
    test_support::write_executable(
        repo.repo.join(".git/hooks/pre-push"),
        "#!/bin/sh\nprintf executed > \"$RABI_PUBLISH_FIXTURE/hook-ran\"\nexit 1\n",
    )
    .unwrap();
    decode(repo.publish().output().unwrap(), 0);
    assert!(!repo.root.join("hook-ran").exists());
    let pushes = repo.pushes();
    assert_eq!(pushes.len(), 1);
    assert!(pushes[0]
        .windows(2)
        .any(|pair| pair == ["-c", "core.hooksPath=/dev/null"]));
    assert!(pushes[0]
        .windows(2)
        .any(|pair| pair == ["-c", "core.fsmonitor=false"]));
    assert!(pushes[0].iter().any(|arg| arg == "HEAD:refs/heads/topic"));
    assert!(!pushes[0]
        .iter()
        .any(|arg| arg.contains("force") || arg.starts_with('+')));
    assert_eq!(
        repo.git(&[
            "--git-dir",
            repo.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/topic"
        ]),
        repo.head
    );
    let view = repo.view();
    assert_eq!(view["task"]["state"]["name"], "ci");
    assert_eq!(view["task"]["prNumber"], 7);
    assert_eq!(precheck(&view)["head"], repo.head);
    assert_eq!(precheck(&view)["passed"], true);
    let states: Vec<_> = view["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["to"]["name"].as_str().unwrap())
        .collect();
    assert!(states
        .windows(2)
        .any(|pair| pair == ["prechecking", "publishing"]));
    assert_eq!(states.last(), Some(&"ci"));
    let body = fs::read_to_string(repo.root.join("pr-body")).unwrap();
    assert!(body.contains("Publish recorded intent"));
    assert!(body.contains("feat: implement fixture"));
    assert_eq!(repo.creates(), 1);
}

#[test]
fn precheck_runs_at_committed_head_in_workspace_with_credentials_stripped() {
    let repo = Repo::new(Some(&[
        "test \"$(cat tracked)\" = implemented && test \"$(git rev-parse HEAD)\" = \"$(cat \"$RABI_PUBLISH_FIXTURE/expected-head\")\"",
        "test -z \"${GH_TOKEN+x}${GITHUB_TOKEN+x}${GH_ENTERPRISE_TOKEN+x}${GITHUB_ENTERPRISE_TOKEN+x}${SSH_AUTH_SOCK+x}${GIT_ASKPASS+x}${SSH_ASKPASS+x}${GIT_UNKNOWN_FIXTURE+x}\" && test \"$RABI_AGENT\" = 1 && test -z \"$(ls -A \"$GH_CONFIG_DIR\")\"",
    ]), None);
    fs::write(repo.root.join("expected-head"), &repo.head).unwrap();
    let mut command = repo.publish();
    for key in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "SSH_AUTH_SOCK",
        "GIT_ASKPASS",
        "SSH_ASKPASS",
        "GIT_UNKNOWN_FIXTURE",
    ] {
        command.env(key, "credential-fixture");
    }
    decode(command.output().unwrap(), 0);
    assert_eq!(precheck(&repo.view())["passed"], true);
}

#[test]
fn failing_precheck_returns_to_implementing_and_retains_bounded_visible_tail() {
    // Keep ordered tail assertions on one pipe; separate stdout/stderr readers
    // cannot establish an ordering between bytes from different descriptors.
    let failure =
        "head -c 20000 /dev/zero | tr '\\000' x >&2; printf '\\033[31mfailed-tail\\007\\n' >&2; exit 7";
    let repo = Repo::new(
        Some(&[failure, "touch \"$RABI_PUBLISH_FIXTURE/later-command-ran\""]),
        None,
    );
    decode(repo.publish().output().unwrap(), 1);
    let view = repo.view();
    assert_eq!(view["task"]["state"]["name"], "implementing");
    let event = precheck(&view);
    assert_eq!(event["head"], repo.head);
    assert_eq!(event["passed"], false);
    let failing_command = event["failingCommand"].as_str().unwrap();
    assert!(failing_command.starts_with("head -c 20000"));
    assert!(failing_command.ends_with("exit 7"));
    let tail = event["outputTail"].as_str().unwrap();
    assert!(tail.len() <= 16384, "tail length {}", tail.len());
    assert!(tail.contains("failed-tail"));
    assert!(!tail
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t'));
    assert!(!repo.root.join("later-command-ran").exists());
    assert!(repo.pushes().is_empty());
}

#[test]
fn stored_precheck_diagnostics_redact_credentials_and_private_paths() {
    let repo = Repo::new(
        Some(&["cat \"$RABI_PUBLISH_FIXTURE/private-output\"; exit 1"]),
        None,
    );
    fs::write(
        repo.root.join("private-output"),
        format!(
            "credential-fixture-secret {} /private/fixture-output https://fixture-secret@example.invalid/project\n",
            repo.root.join("home").display()
        ),
    )
    .unwrap();
    decode(
        repo.publish()
            .env("GH_TOKEN", "credential-fixture-secret")
            .output()
            .unwrap(),
        1,
    );
    let view = repo.view();
    let output = precheck(&view)["outputTail"].as_str().unwrap();
    for private in [
        "credential-fixture-secret",
        repo.root.to_str().unwrap(),
        "/private/fixture-output",
        "fixture-secret@example.invalid",
    ] {
        assert!(!output.contains(private), "unredacted diagnostic: {output}");
    }
}

#[test]
fn changed_precheck_blocks_without_running_head_commands() {
    let mut repo = Repo::new(Some(&["true"]), None);
    repo.config(
        &repo.workspace,
        &["touch \"$RABI_PUBLISH_FIXTURE/changed-precheck-ran\""],
    );
    repo.commit("change precheck");
    let output = decode(repo.publish().output().unwrap(), 4);
    assert_eq!(output["error"]["code"], "precheck_changed");
    assert_eq!(repo.view()["task"]["state"]["reason"], "precheck_changed");
    assert!(!repo.root.join("changed-precheck-ran").exists());
    assert!(repo.pushes().is_empty());
    assert_eq!(repo.creates(), 0);
}

#[test]
fn unrelated_config_edits_preserve_precheck_comparison_and_hit_the_floor() {
    let mut repo = Repo::new(Some(&["true"]), None);
    let config = repo.workspace.join(".rabi.toml");
    let mut content = fs::read_to_string(&config).unwrap();
    content.push_str("\n[hooks]\nsetup = []\n");
    fs::write(config, content).unwrap();
    repo.commit("change unrelated configuration");
    let output = decode(repo.publish().output().unwrap(), 4);
    assert_eq!(output["error"]["code"], "floor_changed");
    assert_eq!(precheck(&repo.view())["passed"], true);
    assert!(repo.pushes().is_empty());
}

#[test]
fn tree_changes_during_precheck_fail_and_dirty_initial_tree_is_refused() {
    let repo = Repo::new(Some(&["printf changed > tracked"]), None);
    decode(repo.publish().output().unwrap(), 1);
    assert_eq!(repo.view()["task"]["state"]["name"], "implementing");
    assert_eq!(precheck(&repo.view())["passed"], false);
    assert!(repo.pushes().is_empty());
    let repo = Repo::new(
        Some(&["touch \"$RABI_PUBLISH_FIXTURE/precheck-ran\""]),
        None,
    );
    fs::write(repo.workspace.join("untracked"), "dirty").unwrap();
    decode(repo.publish().output().unwrap(), 4);
    assert!(!repo.root.join("precheck-ran").exists());
    assert!(repo.pushes().is_empty());
}

#[test]
fn missing_precheck_tool_blocks_environment_and_untrusted_repository_runs_nothing() {
    let repo = Repo::new(Some(&["rabi_missing_precheck_fixture_tool"]), None);
    decode(repo.publish().output().unwrap(), 4);
    assert_eq!(repo.view()["task"]["state"]["reason"], "environment");
    assert!(repo.pushes().is_empty());
    let repo = Repo::new(
        Some(&["touch \"$RABI_PUBLISH_FIXTURE/precheck-ran\""]),
        None,
    );
    decode(
        repo.publish()
            .env_remove("RABI_TRUST_ALL")
            .output()
            .unwrap(),
        4,
    );
    assert!(!repo.root.join("precheck-ran").exists());
    assert!(repo.pushes().is_empty());
}

#[test]
fn built_in_and_policy_floor_changes_block_before_any_push() {
    for path in [
        ".github/example.txt",
        ".rabi.toml",
        ".rabi/policy.toml",
        "protected/file",
    ] {
        let mut repo = Repo::new(
            None,
            Some("schema = 1\n[[rule]]\npaths = ['protected/**']\nfloor = true\n"),
        );
        let target = repo.workspace.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            if path == ".rabi.toml" {
                "[hooks]\n"
            } else {
                "changed\n"
            },
        )
        .unwrap();
        repo.commit("change protected file");
        let output = decode(repo.publish().output().unwrap(), 4);
        assert_eq!(output["error"]["code"], "floor_changed", "{path}");
        assert_eq!(repo.view()["task"]["state"]["reason"], "floor_changed");
        assert!(repo.pushes().is_empty(), "{path}");
        assert_eq!(repo.creates(), 0);
    }
}

#[test]
fn publishing_recovery_reuses_remote_head_and_pr_after_real_kill_points() {
    for kill in ["kill-before-pr", "kill-after-pr"] {
        let repo = Repo::new(None, None);
        fs::write(repo.root.join(kill), "armed").unwrap();
        let output = repo
            .publish()
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
            .wait_with_output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(
            repo.git(&[
                "--git-dir",
                repo.remote.to_str().unwrap(),
                "rev-parse",
                "refs/heads/topic"
            ]),
            repo.head
        );
        assert_eq!(repo.view()["task"]["state"]["name"], "publishing");
        decode(repo.publish().output().unwrap(), 0);
        assert_eq!(repo.pushes().len(), 1, "{kill}: duplicate push");
        assert_eq!(repo.creates(), 1, "{kill}: duplicate PR");
        assert_eq!(repo.view()["task"]["state"]["name"], "ci");
        assert_eq!(repo.view()["task"]["prNumber"], 7);
    }
}

#[test]
fn existing_pr_is_reused_and_diverged_remote_is_never_pushed() {
    let repo = Repo::new(None, None);
    fs::write(repo.root.join("pr-created"), "existing").unwrap();
    decode(repo.publish().output().unwrap(), 0);
    assert_eq!(repo.creates(), 0);
    let repo = Repo::new(None, None);
    repo.git(&["checkout", "-b", "elsewhere", "main"]);
    fs::write(repo.repo.join("other"), "other work\n").unwrap();
    repo.git(&["add", "other"]);
    repo.git(&["commit", "-m", "independent work"]);
    repo.git(&[
        "push",
        repo.remote.to_str().unwrap(),
        "HEAD:refs/heads/topic",
    ]);
    for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let output = decode(repo.publish().output().unwrap(), 4);
    assert_eq!(output["error"]["code"], "diverged");
    assert_eq!(repo.view()["task"]["state"]["reason"], "diverged");
    assert!(repo.pushes().is_empty());
    assert_eq!(repo.creates(), 0);
}

#[test]
fn recorded_push_url_survives_remote_changes_and_pr_head_must_match() {
    let repo = Repo::new(None, None);
    let other = repo.root.join("other.git");
    repo.git(&["init", "--bare", "--template=", other.to_str().unwrap()]);
    repo.git(&["remote", "set-url", "origin", other.to_str().unwrap()]);
    let mut pr: Value =
        serde_json::from_slice(&fs::read(repo.root.join("pr.json")).unwrap()).unwrap();
    pr["headRefOid"] = json!("a".repeat(40));
    fs::write(repo.root.join("pr.json"), serde_json::to_vec(&pr).unwrap()).unwrap();
    let output = repo.publish().output().unwrap();
    assert!(!output.status.success());
    assert_eq!(
        repo.git(&[
            "--git-dir",
            repo.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/topic"
        ]),
        repo.head
    );
    let remote_refs = repo.git(&["--git-dir", other.to_str().unwrap(), "for-each-ref"]);
    assert!(remote_refs.is_empty());
    assert_ne!(repo.view()["task"]["state"]["name"], "ci");
}

#[test]
fn agent_written_git_config_cannot_redirect_publication() {
    let repo = Repo::new(None, None);
    let decoy = repo.root.join("decoy.git");
    repo.git(&["init", "--bare", "--template=", decoy.to_str().unwrap()]);
    repo.git(&[
        "config",
        &format!("url.{}.pushInsteadOf", decoy.display()),
        repo.remote.to_str().unwrap(),
    ]);
    repo.git(&["config", "remote.origin.pushurl", decoy.to_str().unwrap()]);
    repo.git(&[
        "config",
        "credential.helper",
        "!printf executed > \"$RABI_PUBLISH_FIXTURE/helper-ran\"",
    ]);
    repo.git(&[
        "config",
        "core.sshCommand",
        "printf executed > \"$RABI_PUBLISH_FIXTURE/ssh-ran\"",
    ]);
    decode(repo.publish().output().unwrap(), 0);
    assert_eq!(
        repo.git(&[
            "--git-dir",
            repo.remote.to_str().unwrap(),
            "rev-parse",
            "refs/heads/topic"
        ]),
        repo.head
    );
    assert!(repo
        .git(&["--git-dir", decoy.to_str().unwrap(), "for-each-ref"])
        .is_empty());
    assert!(!repo.root.join("helper-ran").exists());
    assert!(!repo.root.join("ssh-ran").exists());
}

#[test]
fn publishing_retry_rechecks_the_current_head_after_a_kill() {
    let mut repo = Repo::new(
        Some(&[
            "printf '%s\\n' \"$(git rev-parse HEAD)\" >> \"$RABI_PUBLISH_FIXTURE/checked-heads\"",
        ]),
        None,
    );
    let first_head = repo.head.clone();
    fs::write(repo.root.join("kill-before-pr"), "armed").unwrap();
    let output = repo.publish().output().unwrap();
    assert!(!output.status.success());
    fs::write(repo.workspace.join("later"), "additional work\n").unwrap();
    repo.commit("feat: add subsequent change");
    decode(repo.publish().output().unwrap(), 0);
    let checked = fs::read_to_string(repo.root.join("checked-heads")).unwrap();
    assert_eq!(
        checked.lines().collect::<Vec<_>>(),
        [first_head, repo.head.clone()]
    );
    assert_eq!(precheck(&repo.view())["head"], repo.head);
    assert_eq!(repo.pushes().len(), 2);
    assert_eq!(repo.creates(), 1);
}

#[test]
fn changed_invalid_precheck_is_blocked_before_schema_validation() {
    let mut repo = Repo::new(Some(&["true"]), None);
    fs::write(
        repo.workspace.join(".rabi.toml"),
        "[precheck]\ncommands = 7\n",
    )
    .unwrap();
    repo.commit("change precheck schema");
    let output = decode(repo.publish().output().unwrap(), 4);
    assert_eq!(output["error"]["code"], "precheck_changed");
    assert!(repo.pushes().is_empty());
}

#[test]
fn resumed_publishing_precheck_failure_returns_to_implementing() {
    let repo = Repo::new(Some(&["false"]), None);
    repo.state("publishing");
    decode(repo.publish().output().unwrap(), 1);
    let view = repo.view();
    assert_eq!(view["task"]["state"]["name"], "implementing");
    assert_eq!(precheck(&view)["head"], repo.head);
    assert_eq!(precheck(&view)["passed"], false);
    assert!(repo.pushes().is_empty());
}

#[test]
fn named_base_refs_are_normalized_and_commit_only_base_refuses_before_push() {
    for base in ["refs/heads/main", "refs/remotes/custom/main", "commit"] {
        let repo = Repo::new(None, None);
        repo.git(&["update-ref", "refs/remotes/custom/main", "main"]);
        let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
        task["baseRef"] = json!(if base == "commit" {
            repo.git(&["rev-parse", "main"])
        } else {
            base.into()
        });
        fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
        decode(
            repo.publish().output().unwrap(),
            if base == "commit" { 2 } else { 0 },
        );
        if base == "commit" {
            assert!(repo.pushes().is_empty());
            assert_eq!(repo.creates(), 0);
        } else {
            let create = fs::read_dir(repo.root.join("gh-calls"))
                .unwrap()
                .map(|entry| fs::read(entry.unwrap().path()).unwrap())
                .find(|args| args.starts_with(b"pr\0create\0"))
                .unwrap();
            let args: Vec<_> = create.split(|byte| *byte == 0).collect();
            assert!(args
                .windows(2)
                .any(|pair| pair == [b"--base".as_slice(), b"main".as_slice()]));
        }
    }
}

#[test]
fn populated_submodule_content_cannot_supply_uncommitted_precheck_inputs() {
    for missing_metadata in [false, true] {
        let mut repo = Repo::new(None, None);
        let base = repo.git(&["rev-parse", "main"]);
        repo.git(&[
            "-C",
            repo.workspace.to_str().unwrap(),
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{base},module"),
        ]);
        repo.git(&[
            "-C",
            repo.workspace.to_str().unwrap(),
            "commit",
            "-m",
            "add gitlink",
        ]);
        let module = repo.workspace.join("module");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("source"), "uncommitted dependency input").unwrap();
        if !missing_metadata {
            fs::write(module.join(".git"), "gitdir: unavailable\n").unwrap();
        }
        // Task publishing must inspect the gitlink's filesystem without invoking
        // Git in it, including when the nested metadata is absent or invalid.
        repo.head = repo.git(&["-C", repo.workspace.to_str().unwrap(), "rev-parse", "HEAD"]);
        decode(repo.publish().output().unwrap(), 4);
        assert!(repo.pushes().is_empty());
        assert_eq!(repo.creates(), 0);
    }
}

#[test]
fn same_branch_fork_pr_does_not_prevent_reusing_same_repository_pr() {
    let repo = Repo::new(None, None);
    let same: Value =
        serde_json::from_slice(&fs::read(repo.root.join("pr.json")).unwrap()).unwrap();
    let mut fork = same.clone();
    fork["number"] = json!(8);
    fork["isCrossRepository"] = json!(true);
    fs::write(
        repo.root.join("pr-list.json"),
        serde_json::to_vec(&json!([fork, same])).unwrap(),
    )
    .unwrap();
    fs::write(repo.root.join("pr-created"), "existing").unwrap();
    decode(repo.publish().output().unwrap(), 0);
    assert_eq!(repo.creates(), 0);
    assert_eq!(repo.view()["task"]["prNumber"], 7);
}

#[test]
fn final_record_failure_reports_partial_and_retry_reconciles() {
    let repo = Repo::new(None, None);
    fs::write(repo.root.join("fail-final-record"), "armed").unwrap();
    let output = decode(
        repo.publish()
            .env("RABI_PUBLISH_RECORD", &repo.record)
            .output()
            .unwrap(),
        6,
    );
    assert_eq!(output["error"]["code"], "partial");
    fs::rename(repo.root.join("record.saved"), &repo.record).unwrap();
    decode(repo.publish().output().unwrap(), 0);
    assert_eq!(repo.pushes().len(), 1);
    assert_eq!(repo.creates(), 1);
    assert_eq!(repo.view()["task"]["state"]["name"], "ci");
}

#[test]
fn active_publication_refuses_concurrent_abandonment() {
    let repo = Repo::new(None, None);
    fs::write(repo.root.join("pause-publication"), "armed").unwrap();
    let mut publisher = repo
        .publish()
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !repo.root.join("publication-paused").exists() {
        if std::time::Instant::now() > deadline {
            let _ = publisher.kill();
            let output = publisher.wait_with_output().unwrap();
            panic!(
                "Publication did not reach network pause: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let rejected = repo
        .command(&["task", "abandon", &repo.task, "--json"])
        .output()
        .unwrap();
    fs::remove_file(repo.root.join("pause-publication")).unwrap();
    decode(publisher.wait_with_output().unwrap(), 0);
    decode(rejected, 4);
    assert_eq!(repo.view()["task"]["state"]["name"], "ci");
}

#[test]
fn same_sha_branch_switch_during_precheck_fails_publication() {
    let repo = Repo::new(Some(&["git checkout --detach"]), None);
    decode(repo.publish().output().unwrap(), 1);
    assert_eq!(repo.view()["task"]["state"]["name"], "implementing");
    assert!(repo.pushes().is_empty());
}

#[test]
fn precheck_timeout_and_interruption_record_failure_without_publication() {
    for (script, reason) in [
        ("sleep 5", "timeout"),
        ("kill -TERM \"$PPID\"; sleep 5", "interrupted"),
    ] {
        let repo = Repo::new(Some(&[script]), None);
        // Pin the same timeout in both committed configs to preserve the
        // precheck identity while exercising the command's own deadline.
        let config = fs::read_to_string(repo.repo.join(".rabi.toml"))
            .unwrap()
            .replace("3s", "1s");
        fs::write(repo.repo.join(".rabi.toml"), &config).unwrap();
        repo.git(&["add", ".rabi.toml"]);
        repo.git(&["commit", "-m", "shorten fixture timeout"]);
        // Create a matching table at HEAD without including .rabi.toml in the
        // merge-base diff, so only timeout/interruption is under test.
        repo.git(&[
            "-C",
            repo.workspace.to_str().unwrap(),
            "merge",
            "--no-edit",
            "main",
        ]);
        decode(repo.publish().output().unwrap(), 1);
        let view = repo.view();
        assert_eq!(view["task"]["state"]["name"], "implementing");
        assert_eq!(precheck(&view)["reason"], reason);
        assert!(repo.pushes().is_empty());
    }
}

#[test]
fn missing_precheck_shell_records_environment_block() {
    let repo = Repo::new(Some(&["true"]), None);
    let output = decode(
        repo.publish()
            .env("PATH", repo.root.join("bin"))
            .output()
            .unwrap(),
        4,
    );
    assert_eq!(output["error"]["code"], "environment");
    let view = repo.view();
    assert_eq!(view["task"]["state"]["reason"], "environment");
    assert_eq!(precheck(&view)["passed"], false);
    assert_eq!(precheck(&view)["reason"], "environment");
    assert!(repo.pushes().is_empty());
}

#[test]
fn generated_pr_title_and_body_bound_untrusted_intent_and_subjects() {
    let mut repo = Repo::new(None, None);
    let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
    task["intent"] = json!(format!(
        "\n\n{}\n{}",
        "x".repeat(1000),
        "\u{0001}".repeat(10000)
    ));
    fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
    fs::write(repo.workspace.join("tracked"), "additional committed data").unwrap();
    repo.commit(&"z".repeat(10000));
    decode(repo.publish().output().unwrap(), 0);
    let create = fs::read_dir(repo.root.join("gh-calls"))
        .unwrap()
        .map(|entry| fs::read(entry.unwrap().path()).unwrap())
        .find(|args| args.starts_with(b"pr\0create\0"))
        .unwrap();
    let args: Vec<_> = create.split(|byte| *byte == 0).collect();
    let title = args.windows(2).find(|pair| pair[0] == b"--title").unwrap()[1];
    assert_eq!(title, "x".repeat(128).as_bytes());
    let body = fs::read_to_string(repo.root.join("pr-body")).unwrap();
    assert!(body.len() < 32 * 1024);
    assert!(body.contains("[truncated]"));
    assert!(!body.chars().any(|c| c.is_control() && c != '\n'));
}

#[test]
fn non_utf8_inherited_environment_does_not_panic_during_publication() {
    use std::os::unix::ffi::OsStringExt;
    let repo = Repo::new(Some(&["true"]), None);
    let output = repo
        .publish()
        .env(
            "RABI_OPAQUE_FIXTURE",
            std::ffi::OsString::from_vec(vec![0xff]),
        )
        .env(
            std::ffi::OsString::from_vec(b"RABI_OPAQUE_\xff".to_vec()),
            "fixture",
        )
        .output()
        .unwrap();
    decode(output, 0);
    assert_eq!(repo.view()["task"]["state"]["name"], "ci");
}
