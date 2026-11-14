#![cfg(unix)]
#[path = "common/git.rs"]
mod test_git;
#[path = "common/scripts.rs"]
mod test_support;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PUSH_URL: &str = "https://github.com/fixture/project.git";

const GH: &str = r#"#!/bin/sh
set -eu
printf '%s\000' "$@" > "$RABI_PUBLISH_FIXTURE/gh-calls/args.$$"
case "$1 $2" in
  'repo view')
    [ ! -f "$RABI_PUBLISH_FIXTURE/fail-repository" ] || exit 1
    cat "$RABI_PUBLISH_FIXTURE/repository.json"
    ;;
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
        Self::with_url(commands, policy, PUSH_URL)
    }
    fn with_url(commands: Option<&[&str]>, policy: Option<&str>, url: &str) -> Self {
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
        fixture.git(&["push", fixture.remote.to_str().unwrap(), "main"]);
        fixture.git(&["remote", "add", "origin", url]);
        fs::write(
            fixture.root.join("repository.json"),
            serde_json::to_vec(&json!({"url": "https://github.com/fixture/project"})).unwrap(),
        )
        .unwrap();
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
        let recorded: Value = serde_json::from_slice(&fs::read(&fixture.record).unwrap()).unwrap();
        fixture.rewrite(recorded["pushRemoteUrl"].as_str().unwrap());
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
    fn rewrite(&self, url: &str) {
        // Only the wrapper's Git transport process reads this config. Task
        // creation must record the validated GitHub URL, not its local rewrite.
        fs::write(
            self.root.join("push.gitconfig"),
            format!(
                "[url \"{}\"]\n\tinsteadOf = {}\n",
                self.remote.display(),
                url
            ),
        )
        .unwrap();
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
for arg in "$@"; do
  case "$arg" in
    push|ls-remote|fetch) export GIT_CONFIG_GLOBAL="$RABI_PUBLISH_FIXTURE/push.gitconfig" ;;
  esac
  case "$arg" in
    fetch)
      [ ! -f "$RABI_PUBLISH_FIXTURE/fail-fetch" ] || exit 1
      if [ -f "$RABI_PUBLISH_FIXTURE/trace-fetch" ]; then
        export GIT_TRACE_PACKET="$RABI_PUBLISH_FIXTURE/fetch-packets"
      fi
      ;;
  esac
done
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
fn non_github_and_mismatched_destinations_block_before_any_push() {
    for url in [
        "/tmp/not-a-github-repository.git",
        "file:///tmp/not-a-github-repository.git",
        "https://gitlab.example/fixture/project.git",
        "git@gitlab.example:fixture/project.git",
        "https://github.com/another/project.git",
        "https://github.com/fixture/another.git",
        "http://github.com/fixture/project.git",
        "git://github.com/fixture/project.git",
        "https://github.com.evil.example/fixture/project.git",
    ] {
        let repo = Repo::with_url(None, None, url);
        let output = decode(repo.publish().output().unwrap(), 4);
        assert_eq!(output["error"]["code"], "destination_not_github", "{url}");
        assert_eq!(
            repo.view()["task"]["state"]["reason"],
            "destination_not_github"
        );
        assert!(repo.pushes().is_empty(), "{url}: destination was pushed");
        assert!(repo
            .git(&[
                "--git-dir",
                repo.remote.to_str().unwrap(),
                "for-each-ref",
                "refs/heads/topic"
            ])
            .is_empty());
        assert_eq!(repo.creates(), 0);
    }
}

#[test]
fn github_https_and_ssh_destinations_match_the_resolved_repository() {
    for url in [
        "https://github.com/fixture/project.git",
        "https://GitHub.com/Fixture/Project",
        "git@github.com:fixture/project.git",
        "ssh://git@github.com/fixture/project.git",
        "ssh://git@github.com:22/fixture/project.git",
    ] {
        let repo = Repo::with_url(None, None, url);
        decode(repo.publish().output().unwrap(), 0);
        assert_eq!(repo.pushes().len(), 1, "{url}");
        assert!(repo.pushes()[0]
            .iter()
            .any(|arg| arg.contains("github.com") || arg.contains("GitHub.com")));
    }
}

#[test]
fn unresolved_github_identity_is_retryable_without_changing_task_state() {
    for response in [
        "not JSON",
        "{}",
        "{\"url\":\"file:///tmp/repository\"}",
        "fail",
    ] {
        for state in ["implementing", "prechecking", "publishing"] {
            let repo = Repo::new(None, None);
            repo.state(state);
            let before = repo.view();
            let identity = fs::read(repo.root.join("repository.json")).unwrap();
            if response == "fail" {
                fs::write(repo.root.join("fail-repository"), "armed").unwrap();
            } else {
                fs::write(repo.root.join("repository.json"), response).unwrap();
            }
            decode(repo.publish().output().unwrap(), 1);
            assert_eq!(repo.view(), before, "{response}: {state}");
            assert!(repo.pushes().is_empty());
            assert_eq!(repo.creates(), 0);
            if response == "fail" {
                fs::remove_file(repo.root.join("fail-repository")).unwrap();
            }
            fs::write(repo.root.join("repository.json"), identity).unwrap();
            decode(repo.publish().output().unwrap(), 0);
        }
    }
}

#[test]
fn configured_host_does_not_authorize_a_different_push_host() {
    let repo = Repo::new(None, None);
    fs::write(
        repo.root.join("repository.json"),
        serde_json::to_vec(&json!({
            "url": "https://github.example/fixture/project"
        }))
        .unwrap(),
    )
    .unwrap();
    let output = decode(
        repo.publish()
            .env("GH_HOST", "github.example")
            .output()
            .unwrap(),
        4,
    );
    assert_eq!(output["error"]["code"], "destination_not_github");
    assert!(repo.pushes().is_empty());
}

#[test]
fn configured_github_enterprise_host_must_match_checkout_resolution() {
    let repo = Repo::with_url(None, None, "git@github.example:fixture/project.git");
    fs::write(
        repo.root.join("repository.json"),
        serde_json::to_vec(&json!({
            "url": "https://github.example/fixture/project"
        }))
        .unwrap(),
    )
    .unwrap();
    decode(
        repo.publish()
            .env("GH_HOST", "github.example")
            .output()
            .unwrap(),
        0,
    );
    assert_eq!(repo.pushes().len(), 1);
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
        "head -c 262144 /dev/zero | tr '\\000' x >&2; printf '\\033[31mfailed-tail\\007\\n' >&2; exit 7";
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
    assert!(failing_command.starts_with("head -c 262144"));
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

#[cfg(target_os = "linux")]
#[test]
fn escaped_child_holding_precheck_pipes_does_not_delay_tail_capture() {
    if Command::new("setsid").arg("--version").output().is_err() {
        rabi::output::stderr(format_args!(
            "SKIP escaped precheck pipe test: setsid unavailable\n"
        ));
        return;
    }
    struct EscapedChild(PathBuf);
    impl Drop for EscapedChild {
        fn drop(&mut self) {
            if let Some(pid) = fs::read_to_string(&self.0)
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
                .filter(|pid| *pid > 0)
            {
                // SAFETY: this fixture's setsid child owns its process group.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
        }
    }
    let repo = Repo::new(Some(&[
        "setsid sh -c 'echo $$ > \"$RABI_PUBLISH_FIXTURE/escaped-pid\"; exec sleep 60' & while [ ! -f \"$RABI_PUBLISH_FIXTURE/escaped-pid\" ]; do sleep 0.01; done; printf 'held-pipe-tail\\n' >&2; exit 7"
    ]), None);
    let _escaped = EscapedChild(repo.root.join("escaped-pid"));
    decode(repo.publish().output().unwrap(), 1);
    let view = repo.view();
    assert_eq!(precheck(&view)["reason"], "command_failed");
    assert!(precheck(&view)["outputTail"]
        .as_str()
        .unwrap()
        .contains("held-pipe-tail"));
    let pid = fs::read_to_string(repo.root.join("escaped-pid")).unwrap();
    let stat = fs::read_to_string(format!("/proc/{}/stat", pid.trim())).unwrap();
    assert_ne!(
        stat.rsplit_once(") ").unwrap().1.split_whitespace().next(),
        Some("Z"),
        "publication waited for the escaped child's pipe to close"
    );
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
        PUSH_URL,
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
    let events = view["events"].as_array().unwrap();
    assert!(events.iter().all(|event| event["to"]["name"] != "blocked"));
    assert_eq!(events.last().unwrap()["from"]["name"], "publishing");
    assert_eq!(events.last().unwrap()["to"]["name"], "implementing");
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
        repo.git(&["push", repo.remote.to_str().unwrap(), "main"]);
        for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
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
    // Repository resolution now precedes checks; let the gh fixture read its
    // identity while keeping the precheck shell unavailable on PATH.
    std::os::unix::fs::symlink("/bin/cat", repo.root.join("bin/cat")).unwrap();
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

#[test]
fn moving_local_base_refs_cannot_hide_floor_or_precheck_changes() {
    for (path, expected) in [
        (".github/example.txt", "floor_changed"),
        (".rabi/policy.toml", "floor_changed"),
        ("protected/file", "floor_changed"),
        (".rabi.toml", "precheck_changed"),
    ] {
        for base_ref in ["main", "refs/remotes/origin/main"] {
            let mut repo = Repo::new(
                Some(&["true"]),
                Some("schema = 1\n[[rule]]\npaths = ['protected/**']\nfloor = true\n"),
            );
            let target = repo.workspace.join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(
                &target,
                if path == ".rabi.toml" {
                    "[precheck]\ncommands = ['false']\n"
                } else if path == ".rabi/policy.toml" {
                    "schema = 1\n"
                } else {
                    "protected change\n"
                },
            )
            .unwrap();
            repo.commit("change protected task inputs");
            repo.git(&["update-ref", "refs/heads/main", &repo.head]);
            repo.git(&["update-ref", "refs/remotes/origin/main", &repo.head]);
            let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
            task["baseRef"] = json!(base_ref);
            fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
            let output = decode(repo.publish().output().unwrap(), 4);
            assert_eq!(output["error"]["code"], expected, "{path}: {base_ref}");
            assert!(repo.pushes().is_empty());
            assert_eq!(repo.creates(), 0);
        }
    }
}

#[test]
fn unavailable_verified_base_is_retryable_without_changing_task_state() {
    for mode in ["fetch", "missing", "unrelated"] {
        let repo = Repo::new(
            Some(&["touch \"$RABI_PUBLISH_FIXTURE/precheck-ran\""]),
            None,
        );
        if mode == "fetch" {
            fs::write(repo.root.join("fail-fetch"), "armed").unwrap();
        } else if mode == "missing" {
            repo.git(&[
                "--git-dir",
                repo.remote.to_str().unwrap(),
                "update-ref",
                "-d",
                "refs/heads/main",
            ]);
        } else {
            repo.git(&["checkout", "--orphan", "unrelated"]);
            repo.git(&["rm", "-rf", "."]);
            fs::write(repo.repo.join("unrelated"), "independent root\n").unwrap();
            repo.git(&["add", "."]);
            repo.git(&["commit", "-m", "independent root"]);
            repo.git(&[
                "push",
                "--force",
                repo.remote.to_str().unwrap(),
                "HEAD:refs/heads/main",
            ]);
            for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
                fs::remove_file(entry.unwrap().path()).unwrap();
            }
        }
        let before = repo.view();
        decode(repo.publish().output().unwrap(), 1);
        assert_eq!(repo.view(), before, "{mode}");
        assert!(!repo.root.join("precheck-ran").exists());
        assert!(repo.pushes().is_empty());
        assert_eq!(repo.creates(), 0);
    }
}

#[test]
fn remote_base_missing_locally_is_fetched_and_may_advance_independently() {
    let repo = Repo::new(None, None);
    let common_base = repo.git(&["rev-parse", "main"]);
    let writer = repo.root.join("base-writer");
    repo.git(&[
        "clone",
        "--no-local",
        "--branch",
        "main",
        repo.remote.to_str().unwrap(),
        writer.to_str().unwrap(),
    ]);
    repo.git(&[
        "-C",
        writer.to_str().unwrap(),
        "config",
        "user.name",
        "fixture",
    ]);
    repo.git(&[
        "-C",
        writer.to_str().unwrap(),
        "config",
        "user.email",
        "fixture",
    ]);
    fs::write(writer.join("remote-only"), "remote base update\n").unwrap();
    repo.git(&["-C", writer.to_str().unwrap(), "add", "."]);
    repo.git(&[
        "-C",
        writer.to_str().unwrap(),
        "commit",
        "-m",
        "advance remote base",
    ]);
    repo.git(&["-C", writer.to_str().unwrap(), "push", "origin", "main"]);
    let remote_base = repo.git(&["-C", writer.to_str().unwrap(), "rev-parse", "HEAD"]);
    let missing = repo
        .base_command("git")
        .args(["cat-file", "-e", &remote_base])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    // The wrapper enables tracing only after publication's environment scrub.
    // Local upload-pack uses the same fetch negotiation as a network transport.
    fs::write(repo.root.join("trace-fetch"), "armed").unwrap();
    decode(repo.publish().output().unwrap(), 0);
    let packets = fs::read_to_string(repo.root.join("fetch-packets")).unwrap();
    assert!(
        packets
            .lines()
            .any(|line| line.contains("fetch> have ") && line.ends_with(&common_base)),
        "fetch must negotiate the existing common history: {packets}"
    );
    assert_eq!(repo.git(&["cat-file", "-t", &remote_base]), "commit");
    assert_eq!(repo.pushes().len(), 1);
    assert_eq!(repo.view()["task"]["state"]["name"], "ci");
}

#[test]
fn agent_written_commit_graph_cannot_hide_floor_changes_or_remote_divergence() {
    for (path, expected) in [
        (".github/protected", "floor_changed"),
        ("ordinary", "diverged"),
    ] {
        let mut repo = Repo::new(None, None);
        for directory in [&repo.repo, &repo.workspace] {
            let file = directory.join(path);
            fs::create_dir_all(file.parent().unwrap()).unwrap();
            fs::write(file, "shared fixture content\n").unwrap();
        }
        repo.git(&["add", "."]);
        repo.git(&["commit", "-m", "temporary protected base content"]);
        let forged_base = repo.git(&["rev-parse", "HEAD"]);
        repo.git(&["rm", path]);
        repo.git(&["commit", "-m", "remove temporary protected base content"]);
        repo.git(&["push", repo.remote.to_str().unwrap(), "main"]);
        repo.commit("add protected task content");
        let forged_commit = repo.head.clone();
        fs::write(repo.workspace.join("tracked"), "additional task content\n").unwrap();
        repo.commit("add later task content");
        repo.git(&["commit-graph", "write", "--reachable"]);
        let graph_path = repo.repo.join(".git/objects/info/commit-graph");
        let mut graph = fs::read(&graph_path).unwrap();
        assert_eq!(&graph[..6], b"CGPH\x01\x01");
        let chunk_offset = |name: &[u8]| {
            let entry = graph[8..]
                .as_chunks::<12>()
                .0
                .iter()
                .take(usize::from(graph[6]))
                .find(|entry| &entry[..4] == name)
                .unwrap();
            usize::try_from(u64::from_be_bytes(entry[4..12].try_into().unwrap())).unwrap()
        };
        let oids = chunk_offset(b"OIDL");
        let data = chunk_offset(b"CDAT");
        let position = |sha: &str| {
            graph[oids..data]
                .as_chunks::<20>()
                .0
                .iter()
                .position(|oid| {
                    oid.iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                        == sha
                })
                .unwrap()
        };
        let commit_position = position(&forged_commit);
        let base_position = u32::try_from(position(&forged_base)).unwrap();
        // Each SHA-1 commit-data record has a 20-byte tree ID followed by its
        // parent positions. Forge a task ancestor without changing commit objects.
        let parent = data + commit_position * 36 + 20;
        graph[parent..parent + 4].copy_from_slice(&base_position.to_be_bytes());
        fs::remove_file(&graph_path).unwrap();
        fs::write(graph_path, graph).unwrap();
        // Verify the adversarial fixture affects Git's actual ancestry lookup.
        assert_eq!(
            repo.git(&["-c", "core.commitGraph=true", "merge-base", "main", "topic"]),
            forged_base
        );
        if expected == "diverged" {
            repo.git(&[
                "push",
                repo.remote.to_str().unwrap(),
                &format!("{forged_base}:refs/heads/topic"),
            ]);
        }
        for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let output = decode(repo.publish().output().unwrap(), 4);
        assert_eq!(output["error"]["code"], expected);
        assert!(repo.pushes().is_empty());
        assert_eq!(repo.creates(), 0);
    }
}

#[test]
fn symbolic_base_refs_cannot_redirect_the_remote_base_branch() {
    for (recorded, reference) in [
        ("main", "refs/heads/main"),
        ("refs/heads/main", "refs/heads/main"),
        ("origin/main", "refs/remotes/origin/main"),
        ("refs/remotes/origin/main", "refs/remotes/origin/main"),
    ] {
        let mut repo = Repo::new(None, None);
        fs::create_dir_all(repo.workspace.join(".github")).unwrap();
        fs::write(
            repo.workspace.join(".github/example.txt"),
            "protected change\n",
        )
        .unwrap();
        repo.commit("change protected task input");
        // Seed an existing remote branch, then let the agent redirect only
        // the local alias. The recorded main spelling must select remote main.
        repo.git(&["update-ref", "refs/heads/weaker", &repo.head]);
        repo.git(&[
            "push",
            repo.remote.to_str().unwrap(),
            "refs/heads/weaker:refs/heads/weaker",
        ]);
        repo.git(&["symbolic-ref", reference, "refs/heads/weaker"]);
        let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
        task["baseRef"] = json!(recorded);
        fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
        for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        let output = decode(repo.publish().output().unwrap(), 4);
        assert_eq!(output["error"]["code"], "floor_changed", "{recorded}");
        assert!(repo.pushes().is_empty());
        assert_eq!(repo.creates(), 0);
    }
}

#[test]
fn replacing_tracking_base_with_local_branch_cannot_choose_an_ambiguous_remote_base() {
    let mut repo = Repo::new(None, None);
    fs::create_dir_all(repo.workspace.join(".github")).unwrap();
    fs::write(
        repo.workspace.join(".github/example.txt"),
        "protected change\n",
    )
    .unwrap();
    repo.commit("change protected task input");
    // Seed both remote candidates, with the hierarchical branch already
    // containing the protected change, before simulating local ref tampering.
    repo.git(&[
        "push",
        repo.remote.to_str().unwrap(),
        "refs/heads/topic:refs/heads/origin/main",
    ]);
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    repo.git(&["update-ref", "refs/heads/origin/main", &repo.head]);
    let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
    task["baseRef"] = json!("origin/main");
    fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
    for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    let before = repo.view();
    let output = decode(repo.publish().output().unwrap(), 1);
    assert!(output["error"]["message"]
        .as_str()
        .unwrap()
        .contains("refs/heads/"));
    assert_eq!(repo.view(), before);
    assert!(repo.pushes().is_empty());
    assert_eq!(repo.creates(), 0);
}

#[test]
fn unambiguous_hierarchical_and_explicit_hex_bases_do_not_need_local_refs() {
    for (recorded, branch) in [
        ("release/stable".to_owned(), "release/stable".to_owned()),
        (
            "refs/heads/release/stable".to_owned(),
            "release/stable".to_owned(),
        ),
        (format!("refs/heads/{}", "a".repeat(40)), "a".repeat(40)),
    ] {
        let repo = Repo::new(None, None);
        repo.git(&[
            "push",
            repo.remote.to_str().unwrap(),
            &format!("refs/heads/main:refs/heads/{branch}"),
        ]);
        let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
        task["baseRef"] = json!(recorded);
        fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
        for entry in fs::read_dir(repo.root.join("git-calls")).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
        decode(repo.publish().output().unwrap(), 0);
        let create = fs::read_dir(repo.root.join("gh-calls"))
            .unwrap()
            .map(|entry| fs::read(entry.unwrap().path()).unwrap())
            .find(|args| args.starts_with(b"pr\0create\0"))
            .unwrap();
        let args: Vec<_> = create.split(|byte| *byte == 0).collect();
        assert!(args
            .windows(2)
            .any(|pair| pair == [b"--base".as_slice(), branch.as_bytes()]));
        assert_eq!(repo.pushes().len(), 1);
    }
}

#[test]
fn invalid_or_commit_only_base_spellings_refuse_without_task_changes() {
    for recorded in [
        "a".repeat(40),
        "a".repeat(64),
        "refs/tags/main".into(),
        "refs/remotes/origin".into(),
        "main~1".into(),
        "feature//invalid".into(),
        "refs/heads/".into(),
    ] {
        let repo = Repo::new(None, None);
        let mut task: Value = serde_json::from_slice(&fs::read(&repo.record).unwrap()).unwrap();
        task["baseRef"] = json!(recorded);
        fs::write(&repo.record, serde_json::to_vec(&task).unwrap()).unwrap();
        let before = repo.view();
        decode(repo.publish().output().unwrap(), 2);
        assert_eq!(repo.view(), before, "{recorded}");
        assert!(repo.pushes().is_empty());
        assert_eq!(repo.creates(), 0);
    }
}
