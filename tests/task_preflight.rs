#![cfg(unix)]
#[path = "common/git.rs"]
mod test_git;
#[path = "common/scripts.rs"]
mod test_support;

use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    base: String,
    head: String,
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-preflight-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        for dir in ["home", "bin", "responses"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        test_git::write_config(&root.join("home"));
        let mut fixture = Self {
            root,
            repo,
            base: String::new(),
            head: String::new(),
        };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "Fixture"]);
        fixture.git(&["config", "user.email", "fixture@example.invalid"]);
        fixture.write(
            ".rabi/policy.toml",
            "schema=1\ndefault-tier='low'\n[ci]\nrequired=['Rust test']\n",
        );
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "base"]);
        fixture.base = fixture.git(&["rev-parse", "HEAD"]);
        fixture.git(&["checkout", "-b", "topic"]);
        for index in 0..101 {
            fixture.write(&format!("docs/{index}.md"), "change\n");
        }
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "topic"]);
        fixture.head = fixture.git(&["rev-parse", "HEAD"]);
        fixture.serve(&fixture.responses());
        fixture
    }
    fn write(&self, path: &str, text: &str) {
        let path = self.repo.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = test_git::command(program);
        let paths = std::env::join_paths(std::iter::once(self.root.join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        command
            .args(args)
            .current_dir(&self.repo)
            .env("PATH", paths)
            .env("HOME", self.root.join("home"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("RABI_FACTS_RESPONSES", self.root.join("responses"))
            .env("RABI_FACTS_TRACE", self.root.join("trace"))
            .env_remove("RABI_OUTPUT")
            .env_remove("GHQ_ROOT")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("GIT_CONFIG_PARAMETERS");
        command
    }
    fn git(&self, args: &[&str]) -> String {
        let output = self.command("git", args).output().unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn rabi(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_rabi"), args)
            .output()
            .unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.rabi(args);
        assert!(
            output.status.success(),
            "{args:?}: {} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema"], 2);
        assert_eq!(value["ok"], true);
        assert!(value["error"].is_null());
        value
    }
    fn pull(&self) -> Value {
        json!({"number":7,"state":"open","draft":false,"base":{"ref":"main","sha":self.base,"repo":{"full_name":"example/project"}},"head":{"ref":"topic","sha":self.head,"repo":{"full_name":"example/project"}},"mergeable":true,"mergeable_state":"clean","changed_files":101,"additions":101,"deletions":0})
    }
    fn responses(&self) -> BTreeMap<String, Value> {
        let mut responses = BTreeMap::new();
        responses.insert(
            "repos/example/project".into(),
            json!({"default_branch":"main"}),
        );
        responses.insert("repos/example/project/pulls/7".into(), self.pull());
        responses.insert(
            "repos/example/project/pulls?state=open&head=example%3Atopic&per_page=100&page=1"
                .into(),
            json!([self.pull()]),
        );
        for page in 1..=2 {
            let files: Vec<_> = ((page-1)*100..usize::min(page*100,101)).map(|index| json!({"filename":format!("docs/{index}.md"),"status":"added","additions":1,"deletions":0})).collect();
            responses.insert(
                format!("repos/example/project/pulls/7/files?per_page=100&page={page}"),
                json!(files),
            );
            let runs: Vec<_> = ((page-1)*100..usize::min(page*100,101)).map(|index| json!({"id":index+1,"name":if index == 100 { "Rust test".into() } else { format!("Extra {index}") },"status":"completed","conclusion":"success","app":{"slug":"github-actions"},"started_at":"2026-01-01T00:00:00Z","completed_at":"2026-01-01T00:01:00Z"})).collect();
            responses.insert(format!("repos/example/project/commits/{}/check-runs?filter=all&per_page=100&page={page}", self.head), json!({"total_count":101,"check_runs":runs}));
        }
        responses.insert(
            format!(
                "repos/example/project/commits/{}/statuses?per_page=100&page=1",
                self.head
            ),
            json!([{"context":"legacy","state":"success","creator":{"login":"issuer-1"}}]),
        );
        responses.insert("repos/example/project/pulls/7/reviews?per_page=100&page=1".into(), json!([{"id":2,"state":"APPROVED","user":{"login":"reviewer-1"}},{"id":1,"state":"CHANGES_REQUESTED","user":{"login":"reviewer-1"}}]));
        responses
    }
    fn serve(&self, responses: &BTreeMap<String, Value>) {
        let mut script = String::from("#!/bin/sh\nset -eu\nprintf '%s\\n' \"$@\" '' >> \"$RABI_FACTS_TRACE\"\nif [ \"$*\" = 'repo view --json url,owner,name' ]; then\nprintf '%s\\n' '{\"url\":\"https://github.example.invalid/example/project\",\"owner\":{\"login\":\"example\"},\"name\":\"project\"}'\nexit 0\nfi\n[ \"$#\" = 4 ] && [ \"$1\" = api ] && [ \"$3\" = --hostname ] && [ \"$4\" = github.example.invalid ] || exit 91\ncase \"$2\" in\n");
        for (index, (path, response)) in responses.iter().enumerate() {
            fs::write(
                self.root.join("responses").join(index.to_string()),
                serde_json::to_vec(response).unwrap(),
            )
            .unwrap();
            script.push_str(&format!(
                "'{}') cat \"$RABI_FACTS_RESPONSES/{index}\" ;;\n",
                path.replace('\'', "'\\''")
            ));
        }
        script.push_str("*) echo \"unexpected gh invocation: $*\" >&2; exit 92 ;;\nesac\n");
        test_support::write_executable(self.root.join("bin/gh"), script).unwrap();
    }
    fn trace(&self) -> Vec<Vec<String>> {
        fs::read_to_string(self.root.join("trace"))
            .unwrap()
            .split("\n\n")
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.lines().map(str::to_string).collect())
            .collect()
    }
    fn assert_get_only(&self) {
        for argv in self.trace() {
            if argv[0] == "repo" {
                assert_eq!(argv, ["repo", "view", "--json", "url,owner,name"]);
                continue;
            }
            assert_eq!(argv.len(), 4);
            assert_eq!(argv[0], "api");
            assert!(argv[1].starts_with("repos/example/project"));
            assert_eq!(argv[2], "--hostname");
            assert_eq!(argv[3], "github.example.invalid");
            for forbidden in [
                "-X", "--method", "-f", "-F", "--field", "--input", "merge", "ready", "comment",
            ] {
                assert!(!argv.iter().any(|arg| arg == forbidden), "{argv:?}");
            }
        }
    }
}

fn inventory(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[test]
fn status_and_preflight_paginate_and_leave_git_unchanged() {
    let repo = Repo::new();
    let before = inventory(&repo.repo.join(".git"));
    let status = repo.json(&["task", "status", "pr:7", "--json"]);
    assert_eq!(status["command"], "task status");
    assert_eq!(status["data"]["files"].as_array().unwrap().len(), 101);
    assert_eq!(status["data"]["checkRuns"].as_array().unwrap().len(), 101);
    assert_eq!(status["data"]["filesTruncated"], false);
    assert_eq!(status["data"]["inconsistent"], false);
    assert_eq!(status["data"]["checksIncomplete"], false);
    assert_eq!(status["data"]["reviews"][0]["state"], "APPROVED");
    assert_eq!(status["data"]["statuses"][0]["creator"], "issuer-1");
    let merge = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
    assert_eq!(merge["command"], "task merge");
    assert_eq!(merge["data"]["verdict"], "incomplete");
    assert_eq!(merge["data"]["items"][1]["result"], "not_evaluated");
    assert_eq!(merge["data"]["items"][2]["result"], "pass");
    assert_eq!(merge["data"]["classification"]["tier"], "low");
    assert_eq!(inventory(&repo.repo.join(".git")), before);
    assert!(!repo.root.join("state").exists());
    let calls = repo.trace();
    assert!(calls
        .iter()
        .any(|args| args[1].contains("/files?per_page=100&page=2")));
    assert!(calls
        .iter()
        .any(|args| args[1].contains("/check-runs?filter=all&per_page=100&page=2")));
    repo.assert_get_only();
}

#[test]
fn workspace_branch_and_recorded_pr_and_global_repo_selection() {
    let repo = Repo::new();
    let merge = repo.json(&["task", "merge", "--dry-run", "--json"]);
    assert_eq!(merge["data"]["items"][1]["result"], "pass");
    assert_eq!(merge["data"]["verdict"], "incomplete");
    assert!(repo
        .trace()
        .iter()
        .any(|args| args[1].contains("/pulls?state=open&head=")));
    let set = repo.rabi(&["set", ".", "--pr", "7"]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );
    fs::write(repo.root.join("trace"), "").unwrap();
    let before = inventory(&repo.repo.join(".git"));
    let status = repo.json(&[
        "-R",
        repo.repo.to_str().unwrap(),
        "task",
        "status",
        ".",
        "--json",
    ]);
    assert_eq!(status["data"]["pr"]["number"], 7);
    assert!(!repo.trace().iter().any(|args| args[1].contains("/pulls?")));
    assert_eq!(inventory(&repo.repo.join(".git")), before);
    repo.assert_get_only();
    let text = repo.rabi(&["task", "merge", "--dry-run"]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(text.contains("PASS required_checks"));
    assert!(text.contains("N/A approval"));
    assert!(text.contains("Verdict: incomplete"));
    let text = repo.rabi(&["task", "status"]);
    assert!(text.status.success());
    assert!(String::from_utf8(text.stdout)
        .unwrap()
        .contains("PR #7: open"));
}

#[test]
fn merge_requires_dry_run_before_any_github_call() {
    let repo = Repo::new();
    let output = repo.rabi(&["task", "merge", "--json"]);
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "usage");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Only --dry-run is available"));
    assert!(!repo.root.join("trace").exists());
}

#[test]
fn branch_selection_errors_and_blocked_evaluation_keep_exit_contract() {
    let repo = Repo::new();
    let path = "repos/example/project/pulls?state=open&head=example%3Atopic&per_page=100&page=1";
    for (rows, code, cause) in [
        (json!([]), 3, "No open PR"),
        (json!([repo.pull(), repo.pull()]), 2, "Several open PRs"),
    ] {
        let mut responses = repo.responses();
        responses.insert(path.into(), rows);
        repo.serve(&responses);
        let output = repo.rabi(&["task", "status", "--json"]);
        assert_eq!(output.status.code(), Some(code));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value["error"]["message"].as_str().unwrap().contains(cause));
    }
    let mut responses = repo.responses();
    responses.get_mut("repos/example/project/pulls/7").unwrap()["draft"] = json!(true);
    repo.serve(&responses);
    let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
    assert_eq!(report["data"]["verdict"], "blocked");
    assert_eq!(report["data"]["items"][0]["result"], "fail");
    repo.assert_get_only();
}

#[test]
fn missing_local_objects_use_remote_base_policy_without_fetching() {
    let repo = Repo::new();
    let mut responses = repo.responses();
    let absent = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    responses.get_mut("repos/example/project/pulls/7").unwrap()["base"]["sha"] = json!(absent);
    responses.insert(format!("repos/example/project/git/trees/{absent}"),json!({"truncated":false,"tree":[{"path":".rabi","mode":"040000","sha":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}]}));
    responses.insert("repos/example/project/git/trees/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),json!({"truncated":false,"tree":[{"path":"policy.toml","mode":"100644","sha":"cccccccccccccccccccccccccccccccccccccccc"}]}));
    responses.insert("repos/example/project/git/blobs/cccccccccccccccccccccccccccccccccccccccc".into(),json!({"encoding":"base64","content":"c2NoZW1hPTEKW2NpXQpyZXF1aXJlZD1bJ1J1c3QgdGVzdCddCg=="}));
    repo.serve(&responses);
    let before = inventory(&repo.repo.join(".git"));
    let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
    assert_eq!(report["data"]["items"][2]["result"], "pass");
    assert_eq!(report["data"]["items"][3]["result"], "pass");
    assert_eq!(report["data"]["classification"]["tier"], "high");
    assert!(report["data"]["classification"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "unknown:kinds"));
    assert_eq!(inventory(&repo.repo.join(".git")), before);
    repo.assert_get_only();
}

#[test]
fn moved_head_and_file_count_mismatch_are_reported() {
    let repo = Repo::new();
    let mut responses = repo.responses();
    responses.get_mut("repos/example/project/pulls/7").unwrap()["changed_files"] = json!(102);
    repo.serve(&responses);
    let mut final_pr = repo.pull();
    final_pr["head"]["sha"] = json!("dddddddddddddddddddddddddddddddddddddddd");
    fs::write(
        repo.root.join("responses/final-head"),
        serde_json::to_vec(&final_pr).unwrap(),
    )
    .unwrap();
    let script = fs::read_to_string(repo.root.join("bin/gh")).unwrap().replace("case \"$2\" in", "if [ \"$2\" = repos/example/project/pulls/7 ]; then\nif [ -f \"$RABI_FACTS_RESPONSES/head-read\" ]; then cat \"$RABI_FACTS_RESPONSES/final-head\"; exit 0; fi\n: > \"$RABI_FACTS_RESPONSES/head-read\"\nfi\ncase \"$2\" in");
    test_support::write_executable(repo.root.join("bin/gh"), script).unwrap();
    let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
    assert_eq!(report["data"]["items"][1]["result"], "fail");
    assert!(report["data"]["items"][1]["reason"]
        .as_str()
        .unwrap()
        .contains("moved"));
    assert_eq!(report["data"]["items"][3]["result"], "fail");
    assert!(report["data"]["items"][3]["reason"]
        .as_str()
        .unwrap()
        .contains("truncated"));
    repo.assert_get_only();
}

#[test]
fn github_file_cap_is_truncated_even_when_counts_match() {
    let repo = Repo::new();
    let mut responses = repo.responses();
    responses.get_mut("repos/example/project/pulls/7").unwrap()["changed_files"] = json!(3000);
    for page in 1..=30 {
        responses.insert(format!("repos/example/project/pulls/7/files?per_page=100&page={page}"), json!(((page-1)*100..page*100).map(|i| json!({"filename":format!("docs/{i}.md"),"status":"added","additions":1,"deletions":0})).collect::<Vec<_>>()));
    }
    repo.serve(&responses);
    let status = repo.json(&["task", "status", "pr:7", "--json"]);
    assert_eq!(status["data"]["files"].as_array().unwrap().len(), 3000);
    assert_eq!(status["data"]["filesTruncated"], true);
    assert!(!repo
        .trace()
        .iter()
        .any(|argv| argv[1].contains("files?per_page=100&page=31")));
    repo.assert_get_only();
}

#[test]
fn api_failure_and_invalid_facts_return_operational_errors() {
    let repo = Repo::new();
    for malformed in [false, true] {
        let mut responses = repo.responses();
        if malformed {
            responses.insert("repos/example/project/pulls/7".into(), json!({"number":7}));
        } else {
            responses.remove("repos/example/project/pulls/7");
        }
        repo.serve(&responses);
        let output = repo.rabi(&["task", "status", "pr:7", "--json"]);
        assert_eq!(output.status.code(), Some(1));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert!(value["data"].is_null());
        assert_eq!(value["error"]["code"], "failed");
    }
    repo.assert_get_only();
}

#[test]
fn advanced_base_uses_merge_base_for_kinds_and_base_tip_for_policy() {
    let mut repo = Repo::new();
    repo.git(&["checkout", "main"]);
    repo.write("docs/base-only.md", "base advance\n");
    repo.write(
        ".rabi/policy.toml",
        "schema=1\ndefault-tier='medium'\n[ci]\nrequired=['Rust test']\n",
    );
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "advance base"]);
    repo.base = repo.git(&["rev-parse", "HEAD"]);
    repo.git(&["checkout", "topic"]);
    repo.serve(&repo.responses());
    let before = inventory(&repo.repo.join(".git"));
    let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
    assert_eq!(report["data"]["items"][3]["result"], "pass");
    assert_eq!(report["data"]["classification"]["tier"], "medium");
    assert_eq!(report["data"]["classification"]["totals"]["files"], 101);
    assert_eq!(report["data"]["verdict"], "incomplete");
    assert!(!report["data"]["classification"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "unknown:kinds"));
    assert_eq!(inventory(&repo.repo.join(".git")), before);
    repo.assert_get_only();
}

#[test]
fn unavailable_merge_base_leaves_kinds_unknown() {
    let mut repo = Repo::new();
    repo.head = repo.git(&["commit-tree", "HEAD^{tree}", "-m", "unrelated head"]);
    repo.serve(&repo.responses());
    let before = inventory(&repo.repo.join(".git"));
    let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
    assert_eq!(report["data"]["items"][3]["result"], "pass");
    assert_eq!(report["data"]["classification"]["tier"], "high");
    assert!(report["data"]["classification"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "unknown:kinds"));
    assert_eq!(inventory(&repo.repo.join(".git")), before);
    repo.assert_get_only();
}

#[test]
fn effective_review_state_table() {
    let repo = Repo::new();
    for (new_state, effective, result, verdict) in [
        ("COMMENTED", "CHANGES_REQUESTED", "fail", "blocked"),
        ("PENDING", "CHANGES_REQUESTED", "fail", "blocked"),
        ("DISMISSED", "DISMISSED", "pass", "incomplete"),
        ("APPROVED", "APPROVED", "pass", "incomplete"),
    ] {
        let mut responses = repo.responses();
        // Reverse API order so effective state must be chosen by ID.
        responses.insert(
            "repos/example/project/pulls/7/reviews?per_page=100&page=1".into(),
            json!([
                {"id":2,"state":new_state,"user":{"login":"reviewer-1"}},
                {"id":1,"state":"CHANGES_REQUESTED","user":{"login":"reviewer-1"}}
            ]),
        );
        repo.serve(&responses);
        let status = repo.json(&["task", "status", "pr:7", "--json"]);
        assert_eq!(
            status["data"]["reviews"][0]["state"], effective,
            "{new_state}"
        );
        let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
        assert_eq!(report["data"]["items"][7]["result"], result, "{new_state}");
        assert_eq!(report["data"]["verdict"], verdict, "{new_state}");
    }
    repo.assert_get_only();
}

#[test]
fn check_run_total_count_table() {
    let repo = Repo::new();
    for (first_count, second_count, incomplete) in [
        (Some(101), Some(101), false),
        (Some(102), Some(102), true),
        (Some(100), Some(100), true),
        (Some(102), Some(101), true),
        (None, Some(101), true),
    ] {
        let mut responses = repo.responses();
        for (page, total) in [(1, first_count), (2, second_count)] {
            let response = responses.get_mut(&format!("repos/example/project/commits/{}/check-runs?filter=all&per_page=100&page={page}", repo.head)).unwrap();
            if let Some(total) = total {
                response["total_count"] = json!(total);
            } else {
                response.as_object_mut().unwrap().remove("total_count");
            }
        }
        repo.serve(&responses);
        let status = repo.json(&["task", "status", "pr:7", "--json"]);
        assert_eq!(
            status["data"]["checksIncomplete"], incomplete,
            "{first_count:?}/{second_count:?}"
        );
        let report = repo.json(&["task", "merge", "--dry-run", "pr:7", "--json"]);
        assert_eq!(
            report["data"]["items"][2]["result"],
            if incomplete { "fail" } else { "pass" }
        );
        assert_eq!(
            report["data"]["items"][2]["detail"]["checksIncomplete"],
            incomplete
        );
        if incomplete {
            assert!(report["data"]["items"][2]["reason"]
                .as_str()
                .unwrap()
                .contains("incomplete"));
        }
    }
    repo.assert_get_only();
}
