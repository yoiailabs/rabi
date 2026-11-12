#![cfg(unix)]
#[path = "common/git.rs"]
mod test_git;
#[path = "common/scripts.rs"]
mod test_scripts;

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TOKEN: &str = "fixture-secret-token-never-display";
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        Self::with_files(&[(
            ".github/workflows/check.yml",
            b"Workflow YAML is not parsed",
        )])
    }
    fn with_files(files: &[(&str, &[u8])]) -> Self {
        let root = std::env::temp_dir().join(format!("rabi-doctor-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        fs::create_dir_all(repo.join(".github/workflows")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        fs::create_dir_all(root.join("bin")).unwrap();
        test_git::write_config(&root.join("home"));
        let fixture = Self { root, repo };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "Test"]);
        fixture.git(&["config", "user.email", "test@example.invalid"]);
        fs::write(fixture.repo.join("README.md"), "Fixture repository").unwrap();
        for (name, contents) in files {
            let path = fixture.repo.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "fixture"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.example/example/project.git",
        ]);
        fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        fixture.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        fixture.git(&["config", "branch.main.remote", "origin"]);
        fixture.git(&["config", "branch.main.merge", "refs/heads/main"]);
        fixture.install();
        fixture
    }
    fn command(&self, program: &str) -> Command {
        let mut command = test_git::command(program);
        command
            .current_dir(&self.repo)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1");
        command
    }
    fn git(&self, args: &[&str]) {
        let output = self.command("git").args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn install(&self) {
        let script = r#"#!/bin/sh
set -eu
printf '%s\t' "$@" >> "$DOCTOR_LOG"
printf '\n' >> "$DOCTOR_LOG"
if [ "$*" = 'repo view --json url,owner,name' ]; then
  if [ "$DOCTOR_MODE" = unauthenticated ]; then
    printf '%s\n' "$GH_TOKEN" >&2; exit 1
  fi
  printf '%s\n' '{"url":"https://github.example/example/project","owner":{"login":"example"},"name":"project"}'
  exit 0
fi
[ "$1" = api ] || exit 91
endpoint=$2
shift 2
[ "$1" = --hostname ] && [ "$2" = github.example ] || exit 92
shift 2
case "$endpoint" in
  user) [ "$#" = 1 ] && [ "$1" = --include ] || exit 93 ;;
  *) [ "$#" = 0 ] || exit 94 ;;
esac
case "$endpoint" in
  repos/example/project)
    if [ "$DOCTOR_MODE" = no-default ]; then printf '%s\n' '{"default_branch":null}'
    else printf '%s\n' '{"default_branch":"main"}'; fi ;;
  'repos/example/project/rules/branches/main?per_page=100&page=1')
    case "$DOCTOR_MODE" in
      403|404) printf 'HTTP %s %s\n' "$DOCTOR_MODE" "$GH_TOKEN" >&2; exit 1 ;;
      paginated)
        printf '['
        i=0
        while [ "$i" -lt 100 ]; do
          [ "$i" = 0 ] || printf ','
          printf '%s' '{"type":"required_status_checks"}'
          i=$((i + 1))
        done
        printf ']\n' ;;
      unsafe) printf '%s\n' '[{"type":"merge_queue"}]' ;;
      *) printf '%s\n' '[{"type":"required_status_checks"},{"type":"non_fast_forward"},{"type":"deletion"}]' ;;
    esac ;;
  'repos/example/project/rules/branches/main?per_page=100&page=2')
    [ "$DOCTOR_MODE" = paginated ] || exit 95
    printf '%s\n' '[{"type":"non_fast_forward"},{"type":"deletion"},{"type":"merge_queue"}]' ;;
  'repos/example/project/rulesets?per_page=100&page=1')
    if [ "$DOCTOR_MODE" = paginated ]; then
      printf '['
      i=0
      while [ "$i" -lt 100 ]; do
        [ "$i" = 0 ] || printf ','
        printf '%s' '{"enforcement":"disabled"}'
        i=$((i + 1))
      done
      printf ']\n'; exit 0
    fi
    if [ "$DOCTOR_MODE" = unsafe ]; then printf '%s\n' '[{"id":7,"name":"Protection","enforcement":"active"}]'
    else printf '%s\n' '[]'; fi ;;
  'repos/example/project/rulesets?per_page=100&page=2')
    [ "$DOCTOR_MODE" = paginated ] || exit 95
    printf '%s\n' '[{"id":7,"name":"Protection","enforcement":"active"}]' ;;
  repos/example/project/rulesets/7)
    printf '%s\n' '{"name":"Protection","enforcement":"active","target":"branch","current_user_can_bypass":"always","conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"],"exclude":[]}}}' ;;
  repos/example/project/actions/permissions/workflow)
    if [ "$DOCTOR_MODE" = unsafe ]; then printf '%s\n' '{"default_workflow_permissions":"write"}'
    else printf '%s\n' '{"default_workflow_permissions":"read"}'; fi ;;
  user)
    printf 'HTTP/2.0 200 OK\r\nAuthorization: bearer %s\r\n' "$GH_TOKEN"
    case "$DOCTOR_MODE" in
      unknown-scopes) : ;;
      unsafe) printf 'X-OAuth-Scopes: repo, workflow\r\n' ;;
      *) printf 'X-OAuth-Scopes: repo\r\n' ;;
    esac
    printf '\r\n{"login":"%s"}\n' "$GH_TOKEN" ;;
  *) exit 95 ;;
esac
"#;
        executable(&self.root.join("bin/gh"), script);
        let real_git = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|path| path.join("git"))
            .find(|path| path.is_file())
            .unwrap()
            .canonicalize()
            .unwrap();
        fs::write(
            self.root.join("real-git"),
            real_git.as_os_str().as_encoded_bytes(),
        )
        .unwrap();
        executable(
            &self.root.join("bin/git"),
            r#"#!/bin/sh
set -eu
printf '%s\t' "$@" >> "$DOCTOR_GIT_LOG"
printf '\n' >> "$DOCTOR_GIT_LOG"
# Only read commands are accepted; reject even an attempted fetch or mutation.
verify() {
while [ "$#" -gt 0 ]; do
  case "$1" in -c) shift 2 ;; --no-lazy-fetch) shift ;; *) break ;; esac
done
case "$1" in
  ls-tree)
    if [ "$DOCTOR_MODE" = workflow-enumeration-failed ]; then
      printf '%s\n' "$GH_TOKEN" >&2; exit 1
    fi ;;
  rev-parse|symbolic-ref) : ;;
  worktree) [ "$2" = list ] || exit 96 ;;
  config) [ "$2" = --get ] || exit 97 ;;
  remote) [ "$#" = 1 ] || [ "$2" = get-url ] || exit 98 ;;
  *) exit 99 ;;
esac
}
verify "$@"
exec "$DOCTOR_REAL_GIT" "$@"
"#,
        );
    }
    fn run(&self, mode: &str, args: &[&str], outside: bool) -> Output {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let output = self
            .command(env!("CARGO_BIN_EXE_rabi"))
            .current_dir(if outside { &self.root } else { &self.repo })
            .args(args)
            .env("PATH", path)
            .env("GH_TOKEN", TOKEN)
            .env("GH_PROMPT_DISABLED", "1")
            .env("DOCTOR_MODE", mode)
            .env("DOCTOR_LOG", self.root.join("gh.log"))
            .env("DOCTOR_GIT_LOG", self.root.join("git.log"))
            .env(
                "DOCTOR_REAL_GIT",
                fs::read_to_string(self.root.join("real-git")).unwrap(),
            )
            .output()
            .unwrap();
        assert!(!String::from_utf8_lossy(&output.stdout).contains(TOKEN));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(TOKEN));
        output
    }
    fn json(&self, mode: &str) -> Value {
        let output = self.run(mode, &["doctor", "--json"], false);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["schema"], 2);
        assert_eq!(document["command"], "doctor");
        assert_eq!(document["ok"], true);
        assert_eq!(document["warnings"], serde_json::json!([]));
        assert!(document["error"].is_null());
        document
    }
    fn assert_get_only(&self) {
        let git_calls = fs::read_to_string(self.root.join("git.log")).unwrap();
        assert!(!git_calls.contains("cat-file"), "{git_calls}");
        let calls = fs::read_to_string(self.root.join("gh.log")).unwrap();
        assert!(!calls.is_empty());
        for call in calls.lines() {
            let args = call
                .split('\t')
                .filter(|arg| !arg.is_empty())
                .collect::<Vec<_>>();
            if args[0] == "repo" {
                assert_eq!(args, ["repo", "view", "--json", "url,owner,name"]);
                continue;
            }
            assert_eq!(args[0], "api", "{call}");
            assert!(
                args.iter().all(|arg| !matches!(
                    *arg,
                    "-X" | "--method" | "-f" | "-F" | "--field" | "--raw-field" | "--input"
                )),
                "{call}"
            );
            assert_eq!(&args[2..4], ["--hostname", "github.example"]);
            assert!(
                args.len() == 4 || (args.len() == 5 && args[1] == "user" && args[4] == "--include")
            );
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn executable(path: &Path, contents: &str) {
    test_scripts::write_executable(path, contents).unwrap();
}
fn codes(document: &Value) -> Vec<&str> {
    document["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["code"].as_str().unwrap())
        .collect()
}
fn snapshot(directory: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = vec![];
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(snapshot(&path));
        } else {
            files.push((path.clone(), fs::read(path).unwrap()));
        }
    }
    files.sort();
    files
}

#[test]
fn unsafe_snapshot_uses_only_get_and_committed_default_workflows() {
    let fixture = Fixture::new();
    fixture.git(&[
        "remote",
        "set-url",
        "--push",
        "origin",
        "git@github.example:example/project.git",
    ]);
    fixture.git(&["checkout", "-b", "topic"]);
    fs::rename(
        fixture.repo.join(".github/workflows/check.yml"),
        fixture.repo.join(".github/workflows/topic.yml"),
    )
    .unwrap();
    fixture.git(&["add", "."]);
    fixture.git(&["commit", "-m", "topic workflow"]);
    fs::write(fixture.repo.join(".github/workflows/staged.yml"), "staged").unwrap();
    fixture.git(&["add", "."]);
    fs::write(
        fixture.repo.join(".github/workflows/untracked.yml"),
        "untracked",
    )
    .unwrap();
    fs::write(
        fixture.repo.join(".rabi.toml"),
        "invalid checkout configuration",
    )
    .unwrap();
    let before = snapshot(&fixture.repo);
    let document = fixture.json("unsafe");
    assert_eq!(
        document["data"]["repository"],
        "github.example/example/project"
    );
    assert_eq!(document["data"]["defaultBranch"], "main");
    for code in [
        "required_checks_not_enforced",
        "force_push_allowed",
        "deletion_allowed",
        "merge_queue",
        "ruleset_bypass",
        "token_workflow_scope",
        "default_workflow_permissions",
        "remote_push_ssh",
        "workflow_review_manually",
    ] {
        assert!(codes(&document).contains(&code), "{code}: {document}");
    }
    let workflow = document["data"]["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["code"] == "workflow_review_manually")
        .collect::<Vec<_>>();
    assert_eq!(workflow.len(), 1);
    assert!(workflow[0]["message"]
        .as_str()
        .unwrap()
        .starts_with(".github/workflows/check.yml:"));
    assert_eq!(document["data"]["counts"]["warning"], 7);
    assert_eq!(document["data"]["counts"]["info"], 2);
    fixture.assert_get_only();
    assert_eq!(snapshot(&fixture.repo), before);
    let calls = fs::read_to_string(fixture.root.join("git.log")).unwrap();
    for call in calls.lines().filter(|call| call.contains("ls-tree")) {
        assert!(call.contains("--no-lazy-fetch"), "{call}");
    }
}

#[test]
fn inaccessible_apis_and_unknown_token_scopes_are_informational() {
    for mode in [
        "403",
        "404",
        "unauthenticated",
        "no-default",
        "unknown-scopes",
    ] {
        let fixture = Fixture::new();
        let document = fixture.json(mode);
        assert_eq!(
            document["data"]["counts"]["warning"], 0,
            "{mode}: {document}"
        );
        let expected = if mode == "unknown-scopes" {
            "token_workflow_scope"
        } else {
            "could_not_check"
        };
        assert!(codes(&document).contains(&expected), "{mode}: {document}");
        fixture.assert_get_only();
    }
}

#[test]
fn empty_text_json_repository_selection_and_usage() {
    let fixture = Fixture::with_files(&[]);
    let document = fixture.json("safe");
    assert_eq!(document["data"]["findings"], serde_json::json!([]));
    assert_eq!(
        document["data"]["counts"],
        serde_json::json!({"warning":0,"info":0})
    );
    let output = fixture.run("safe", &["doctor"], false);
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "No findings\n0 warning(s), 0 info finding(s)\n"
    );
    for args in [
        vec!["doctor", "-R", fixture.repo.to_str().unwrap(), "--json"],
        vec!["-R", fixture.repo.to_str().unwrap(), "doctor", "--json"],
    ] {
        let output = fixture.run("safe", &args, true);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    for args in [
        vec!["doctor", "extra", "--json"],
        vec!["doctor", "--unknown", "--json"],
    ] {
        assert_eq!(fixture.run("safe", &args, false).status.code(), Some(2));
    }
    assert_eq!(
        fixture
            .run("safe", &["doctor", "--json"], true)
            .status
            .code(),
        Some(3)
    );
    let output = fixture.run(
        "safe",
        &["doctor", "-R", "/nonexistent-doctor-repository", "--json"],
        true,
    );
    assert!(!output.status.success());
    fixture.assert_get_only();
}

#[test]
fn effective_rules_and_rulesets_are_paginated() {
    let fixture = Fixture::new();
    let document = fixture.json("paginated");
    assert_eq!(
        codes(&document),
        ["merge_queue", "ruleset_bypass", "workflow_review_manually"]
    );
    let calls = fs::read_to_string(fixture.root.join("gh.log")).unwrap();
    assert!(calls.contains("rules/branches/main?per_page=100&page=2"));
    assert!(calls.contains("rulesets?per_page=100&page=2"));
    fixture.assert_get_only();
}

#[test]
fn ssh_uses_push_remote_precedence_without_changing_workflow_source() {
    for (name, push, global, fetch, ssh, expected) in [
        (
            "branch push override",
            Some("branch-push"),
            Some("global-push"),
            Some("fetch"),
            "branch-push",
            true,
        ),
        (
            "global push override",
            None,
            Some("global-push"),
            Some("fetch"),
            "global-push",
            true,
        ),
        ("fetch fallback", None, None, Some("fetch"), "fetch", true),
        ("origin fallback", None, None, None, "origin", true),
        (
            "branch HTTPS overrides global SSH",
            Some("branch-push"),
            Some("global-push"),
            Some("fetch"),
            "global-push",
            false,
        ),
        (
            "global HTTPS overrides fetch SSH",
            None,
            Some("global-push"),
            Some("fetch"),
            "fetch",
            false,
        ),
    ] {
        let fixture = Fixture::new();
        for remote in ["branch-push", "global-push", "fetch"] {
            fixture.git(&[
                "remote",
                "add",
                remote,
                "https://github.example/example/project.git",
            ]);
        }
        fixture.git(&[
            "remote",
            "set-url",
            "--push",
            ssh,
            "git@github.example:example/project.git",
        ]);
        fixture.git(&["config", "--unset", "branch.main.remote"]);
        for (key, value) in [
            ("branch.main.pushRemote", push),
            ("remote.pushDefault", global),
            ("branch.main.remote", fetch),
        ] {
            if let Some(value) = value {
                fixture.git(&["config", key, value]);
            }
        }
        // Give fetch its committed workflow, but no tracking tip to either
        // push-only remote. Push overrides must not redirect enumeration.
        fixture.git(&["update-ref", "refs/remotes/fetch/main", "HEAD"]);
        fs::rename(
            fixture.repo.join(".github/workflows/check.yml"),
            fixture.repo.join(".github/workflows/local.yml"),
        )
        .unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "local workflow differs from tracking tip"]);
        let before = snapshot(&fixture.repo);
        let document = fixture.json("safe");
        assert_eq!(
            codes(&document).contains(&"remote_push_ssh"),
            expected,
            "{name}: {document}"
        );
        let workflow = document["data"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["code"] == "workflow_review_manually")
            .unwrap();
        assert!(
            workflow["message"]
                .as_str()
                .unwrap()
                .starts_with(".github/workflows/check.yml:"),
            "{name}: {document}"
        );
        assert!(
            !codes(&document).contains(&"could_not_check"),
            "{name}: {document}"
        );
        fixture.assert_get_only();
        assert_eq!(snapshot(&fixture.repo), before, "{name}");
        let selected = push.or(global).or(fetch).unwrap_or("origin");
        let calls = fs::read_to_string(fixture.root.join("git.log")).unwrap();
        assert!(
            calls.contains(&format!("remote\tget-url\t--push\t--all\t{selected}\t")),
            "{name}: {calls}"
        );
    }
}

#[test]
fn missing_or_local_push_remotes_have_a_specific_unavailable_reason() {
    for (name, key, value, remove_origin) in [
        ("missing origin fallback", None, None, true),
        (
            "local fetch remote",
            Some("branch.main.remote"),
            Some("."),
            false,
        ),
        (
            "missing push override",
            Some("branch.main.pushRemote"),
            Some("missing"),
            false,
        ),
        (
            "local global push override",
            Some("remote.pushDefault"),
            Some("."),
            false,
        ),
    ] {
        let fixture = Fixture::new();
        fixture.git(&["config", "--unset", "branch.main.remote"]);
        if remove_origin {
            fixture.git(&["remote", "remove", "origin"]);
        }
        if let (Some(key), Some(value)) = (key, value) {
            fixture.git(&["config", key, value]);
        }
        let document = fixture.json("safe");
        let finding = document["data"]["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["detail"] == "remote_push_ssh")
            .unwrap();
        assert_eq!(finding["code"], "could_not_check", "{name}");
        assert_eq!(
            finding["message"],
            "Could not check remote_push_ssh: push remote could not be determined",
            "{name}"
        );
        assert!(!codes(&document).contains(&"remote_push_ssh"), "{name}");
        fixture.assert_get_only();
    }
}

#[test]
fn workflow_manual_review_enumeration_table() {
    for (name, files, mode, expected_paths, unavailable) in [
        ("no workflows", vec![], "safe", vec![], false),
        (
            "yml",
            vec![(".github/workflows/check.yml", b"not YAML".as_slice())],
            "safe",
            vec![".github/workflows/check.yml"],
            false,
        ),
        (
            "yaml",
            vec![(
                ".github/workflows/check.yaml",
                b"\xff\0not UTF-8 or YAML".as_slice(),
            )],
            "safe",
            vec![".github/workflows/check.yaml"],
            false,
        ),
        (
            "multiple workflows",
            vec![
                (".github/workflows/a.yml", TOKEN.as_bytes()),
                (".github/workflows/b.yaml", b"on: *events".as_slice()),
            ],
            "safe",
            vec![".github/workflows/a.yml", ".github/workflows/b.yaml"],
            false,
        ),
        (
            "non-workflow files",
            vec![
                ("elsewhere/check.yml", b"".as_slice()),
                (".github/check.yaml", b"".as_slice()),
                (".github/workflows/check.txt", b"".as_slice()),
                (".github/workflows/check.YML", b"".as_slice()),
            ],
            "safe",
            vec![],
            false,
        ),
        (
            "nested directories ignored",
            vec![
                (".github/workflows/nested/check.yml", b"".as_slice()),
                (".github/workflows/nested/deep/check.yaml", b"".as_slice()),
            ],
            "safe",
            vec![],
            false,
        ),
        (
            "enumeration failure",
            vec![(".github/workflows/check.yml", b"".as_slice())],
            "workflow-enumeration-failed",
            vec![],
            true,
        ),
    ] {
        let fixture = Fixture::with_files(&files);
        let before = snapshot(&fixture.repo);
        let document = fixture.json(mode);
        let findings = document["data"]["findings"].as_array().unwrap();
        if unavailable {
            assert_eq!(findings.len(), 1, "{name}: {document}");
            assert_eq!(findings[0]["code"], "could_not_check", "{name}");
            assert_eq!(findings[0]["level"], "info", "{name}");
            assert_eq!(findings[0]["detail"], "workflow_review_manually", "{name}");
            assert_eq!(findings[0]["message"], "Could not check workflow_review_manually: Git data is unavailable locally (Git 2.44 or later is required; no fetch was attempted)", "{name}");
        } else {
            assert_eq!(findings.len(), expected_paths.len(), "{name}: {document}");
            for (finding, path) in findings.iter().zip(&expected_paths) {
                assert_eq!(finding["code"], "workflow_review_manually", "{name}");
                assert_eq!(finding["level"], "info", "{name}");
                assert_eq!(finding["message"], format!("{path}: review its triggers and permissions manually; rabi doctor does not parse workflow YAML"), "{name}");
            }
        }
        assert_eq!(document["data"]["counts"]["warning"], 0, "{name}");
        assert_eq!(document["data"]["counts"]["info"], findings.len(), "{name}");
        fixture.assert_get_only();
        assert_eq!(snapshot(&fixture.repo), before, "{name}");
    }
}
