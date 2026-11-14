#[path = "common/git.rs"]
mod test_git;
#[path = "common/wait.rs"]
mod wait;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-task-report-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        for path in [&repo, &root.join("home"), &root.join("bin")] {
            fs::create_dir_all(path).unwrap();
        }
        test_git::write_config(&root.join("home"));
        for name in ["gh", "codex", "claude"] {
            let path = root.join("bin").join(name);
            fs::write(
                &path,
                "#!/bin/sh\necho unexpected external invocation >&2\nexit 99\n",
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let fixture = Self { root, repo };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "Fixture"]);
        fixture.git(&["config", "user.email", "fixture@example.invalid"]);
        fixture.git(&["commit", "--allow-empty", "-m", "fixture"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://example.invalid/project.git",
        ]);
        fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        fixture.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        fixture
    }
    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = test_git::command(program);
        command
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("RABI_TRUST_ALL")
            .env_remove("RABI_OUTPUT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
        command
    }
    fn run(command: &mut Command) -> Output {
        let mut capture = wait::OutputCapture::new();
        let mut child = capture.apply(command).spawn().unwrap();
        capture.finish(&mut child)
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Self::run(&mut self.command("git", args));
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn rabi(&self, args: &[&str]) -> Output {
        Self::run(&mut self.command(env!("CARGO_BIN_EXE_rabi"), args))
    }
    fn json(&self, args: &[&str]) -> Value {
        let output = self.rabi(args);
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    }
    fn tasks(&self) -> PathBuf {
        let common = self.repo.join(".git").canonicalize().unwrap();
        let hash = Sha256::digest(common.as_os_str().as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        self.root
            .join("state/rabi/repositories")
            .join(hash)
            .join("tasks")
    }
    fn create(&self, branch: &str, state: Value, events: Vec<Value>) -> String {
        let mut task = self.json(&[
            "task",
            "new",
            "Fixture intent",
            "--branch",
            branch,
            "--json",
        ])["data"]["task"]
            .clone();
        let id = task["id"].as_str().unwrap().to_owned();
        task["createdAt"] = json!("2026-01-01T00:00:00Z");
        task["state"] = state;
        if !matches!(
            task["state"]["name"].as_str().unwrap(),
            "merged" | "abandoned" | "new" | "blocked"
        ) {
            self.git(&["branch", branch]);
        }
        let directory = self.tasks().join(&id);
        fs::write(
            directory.join("task.json"),
            serde_json::to_vec(&task).unwrap(),
        )
        .unwrap();
        let history = events
            .into_iter()
            .map(|event| format!("{event}\n"))
            .collect::<String>();
        fs::write(directory.join("events.jsonl"), history).unwrap();
        id
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn event(at: &str, from: Option<&str>, to: &str, reason: &str) -> Value {
    json!({"at":at,"from":from.map(|state| json!({"name":state})),"to":{"name":to},"reason":reason})
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
fn empty_report_and_event_stream_create_no_state_or_locks() {
    let repo = Repo::new();
    let before = inventory(&repo.root);
    let report = repo.json(&["task", "report", "--json"]);
    assert_eq!(report["command"], "task report");
    assert_eq!(report["data"]["reportVersion"], 1);
    assert_eq!(report["data"]["taskCount"], 0);
    assert_eq!(report["data"]["tasks"], json!([]));
    assert_eq!(report["data"]["ciWait"]["meanMs"], Value::Null);
    let events = repo.rabi(&["task", "events", "--json"]);
    assert!(events.status.success());
    assert!(events.stdout.is_empty());
    assert_eq!(inventory(&repo.root), before);
    assert!(!repo.root.join("state").exists());
}

#[test]
fn report_reads_all_tasks_and_events_with_inclusive_since_and_full_durations() {
    let repo = Repo::new();
    let mut gate = event(
        "2026-01-02T00:00:00Z",
        Some("classifying"),
        "merging",
        "auto_tier",
    );
    gate["classification"] = json!({"tier":"low","floor":false});
    let id = repo.create(
        "merged",
        json!({"name":"merged"}),
        vec![
            event("2026-01-01T00:00:00Z", None, "new", "created"),
            event(
                "2026-01-01T23:00:00Z",
                Some("publishing"),
                "ci",
                "published",
            ),
            event("2026-01-02T00:00:00Z", Some("ci"), "reviewing", "ci_passed"),
            gate,
            event("2026-01-02T01:00:00Z", Some("merging"), "merged", "merged"),
        ],
    );
    repo.create(
        "abandoned",
        json!({"name":"abandoned"}),
        vec![event("2026-01-01T00:00:00Z", None, "new", "created")],
    );
    let before = inventory(&repo.root);
    let report = repo.json(&["task", "report", "--since", "2026-01-02", "--json"]);
    let data = &report["data"];
    assert_eq!(data["taskCount"], 1);
    assert_eq!(data["states"]["merged"], 1);
    assert_eq!(data["states"]["abandoned"], 0);
    assert_eq!(data["ciWait"]["totalMs"], 3_600_000);
    assert_eq!(data["timeToMerge"]["totalMs"], 90_000_000);
    assert_eq!(data["tierOutcomes"]["low"]["autoMerged"], 1);
    assert_eq!(data["tasks"][0]["taskId"], id);
    assert_eq!(
        repo.json(&["task", "report", "--json"])["data"]["taskCount"],
        2
    );
    let text = repo.rabi(&["task", "report"]);
    assert!(text.status.success());
    assert!(String::from_utf8(text.stdout).unwrap().contains("Tasks: 2"));
    assert_eq!(inventory(&repo.root), before);
}

#[test]
fn events_json_lines_select_prefix_or_all_preserving_append_order_and_read_only_state() {
    let repo = Repo::new();
    let id = repo.create(
        "one",
        json!({"name":"merged"}),
        vec![
            event("2026-01-01T00:00:00Z", None, "new", "created"),
            event("2026-01-02T00:00:00Z", Some("merging"), "merged", "merged"),
        ],
    );
    repo.create(
        "two",
        json!({"name":"abandoned"}),
        vec![event(
            "2026-01-03T00:00:00Z",
            Some("new"),
            "abandoned",
            "abandoned",
        )],
    );
    let before = inventory(&repo.root);
    let output = repo.rabi(&[
        "task",
        "events",
        &id[..12],
        "--since",
        "2026-01-02",
        "--json",
    ]);
    assert!(output.status.success());
    let row: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(row["taskId"], id);
    assert_eq!(row["reason"], "merged");
    assert!(row.get("schema").is_none());
    assert!(row.get("data").is_none());
    let output = repo.rabi(&["task", "events", "--json"]);
    let rows: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    let selected: Vec<_> = rows
        .iter()
        .filter(|row| row["taskId"] == id)
        .map(|row| row["reason"].as_str().unwrap())
        .collect();
    assert_eq!(selected, vec!["created", "merged"]);
    assert!(rows
        .windows(2)
        .all(|pair| pair[0]["taskId"].as_str() <= pair[1]["taskId"].as_str()));
    let text = repo.rabi(&["task", "events", &id]);
    assert!(String::from_utf8(text.stdout).unwrap().contains("created"));
    assert_eq!(inventory(&repo.root), before);
}

#[test]
fn corrupt_event_or_record_returns_one_error_envelope_before_any_json_lines() {
    for corrupt in ["record", "events", "timestamp"] {
        let repo = Repo::new();
        let first = repo.create(
            "one",
            json!({"name":"merged"}),
            vec![event("2026-01-01T00:00:00Z", None, "new", "created")],
        );
        let second = repo.create(
            "two",
            json!({"name":"merged"}),
            vec![event("2026-01-01T00:00:00Z", None, "new", "created")],
        );
        let id = first.max(second);
        let path = repo.tasks().join(&id);
        match corrupt {
            "record" => fs::write(path.join("task.json"), "{").unwrap(),
            "events" => fs::write(path.join("events.jsonl"), "{\n").unwrap(),
            _ => fs::write(
                path.join("events.jsonl"),
                format!("{}\n", event("invalid", None, "new", "created")),
            )
            .unwrap(),
        }
        let before = inventory(&repo.root);
        for command in ["report", "events"] {
            let output = repo.rabi(&["task", command, "--json"]);
            assert_eq!(output.status.code(), Some(1), "{corrupt} {command}");
            let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(envelope["ok"], false);
            assert!(envelope["error"]["message"].as_str().unwrap().contains(&id));
        }
        assert_eq!(inventory(&repo.root), before);
    }
}

#[test]
fn since_validation_and_event_selector_errors_preserve_usage_contract() {
    let repo = Repo::new();
    for command in ["report", "events"] {
        for value in ["0h", "2026-02-30", "1y", "18446744073709551615w"] {
            let output = repo.rabi(&["task", command, "--since", value, "--json"]);
            assert_eq!(output.status.code(), Some(2));
            let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(envelope["error"]["code"], "usage");
        }
        for value in [
            "1s",
            "1m",
            "1h",
            "1d",
            "1w",
            "2026-01-01",
            "2026-01-01T01:00:00+01:00",
        ] {
            assert!(repo
                .rabi(&["task", command, "--since", value, "--json"])
                .status
                .success());
        }
    }
    for (selector, code) in [("task_", 2), ("task_ffffff", 3)] {
        assert_eq!(
            repo.rabi(&["task", "events", selector, "--json"])
                .status
                .code(),
            Some(code)
        );
    }
    assert_eq!(
        repo.rabi(&["task", "report", "extra", "--json"])
            .status
            .code(),
        Some(2)
    );
}
