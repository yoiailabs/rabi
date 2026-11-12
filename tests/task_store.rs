#[path = "common/git.rs"]
mod test_git;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Repo {
    root: PathBuf,
    repo: PathBuf,
}
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-task-store-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        test_git::write_config(&root.join("home"));
        let fixture = Self { root, repo };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "Test"]);
        fixture.git(&["config", "user.email", "test@example.invalid"]);
        fixture.git(&["commit", "--allow-empty", "-m", "fixture"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://user:secret@example.invalid/project.git",
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
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("RABI_OUTPUT")
            .env_remove("GHQ_ROOT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
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
    fn decode(output: Output) -> Value {
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["schema"], 2);
        assert_eq!(value["ok"], true);
        value
    }
    fn json(&self, args: &[&str]) -> Value {
        Self::decode(self.rabi(args))
    }
    fn create(&self, branch: &str) -> Value {
        self.json(&[
            "task",
            "new",
            "Implement something\nMore detail",
            "--branch",
            branch,
            "--json",
        ])["data"]["task"]
            .clone()
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
    fn directory(&self, task: &Value) -> PathBuf {
        self.tasks().join(task["id"].as_str().unwrap())
    }
    fn set_state(&self, task: &Value, state: &str) {
        let mut record = task.clone();
        record["state"] = json!({"name": state});
        fs::write(
            self.directory(task).join("task.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    fn error(&self, args: &[&str], exit: i32, code: &str) -> Value {
        let output = self.rabi(args);
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["ok"], false);
        assert_eq!(value["error"]["code"], code);
        value
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
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
fn new_list_show_abandon_preserve_git_and_append_events() {
    let repo = Repo::new();
    let git_before = inventory(&repo.repo.join(".git"));
    let task = repo.create("task/example");
    let id = task["id"].as_str().unwrap();
    assert_eq!(task["schema"], 1);
    assert_eq!(task["state"], json!({"name":"new"}));
    assert_eq!(task["base_ref"], "origin/main");
    assert_eq!(
        task["push_remote_url"],
        "https://example.invalid/project.git"
    );
    assert!(task["workspace_path"].is_null());
    assert!(task["pr_number"].is_null());
    let list = repo.json(&["task", "list", "--json"]);
    assert_eq!(list["command"], "task list");
    assert_eq!(list["data"]["tasks"][0]["intent"], "Implement something");
    assert_eq!(list["data"]["tasks"][0]["id"], id);
    let path = repo.directory(&task);
    let original = fs::read(path.join("events.jsonl")).unwrap();
    let before = inventory(&repo.tasks());
    let show = repo.json(&["task", "show", id, "--json"]);
    assert_eq!(show["data"]["task"], task);
    assert_eq!(show["data"]["events"][0]["from"], Value::Null);
    assert_eq!(show["data"]["events"][0]["reason"], "created");
    assert_eq!(inventory(&repo.tasks()), before);
    let output = repo.json(&[
        "task",
        "abandon",
        id,
        "--reason",
        "No longer needed",
        "--json",
    ]);
    assert_eq!(output["data"]["task"]["state"]["name"], "abandoned");
    assert_eq!(output["data"]["events"][1]["reason"], "No longer needed");
    let appended = fs::read(path.join("events.jsonl")).unwrap();
    assert!(appended.starts_with(&original));
    assert_eq!(String::from_utf8(appended).unwrap().lines().count(), 2);
    assert_eq!(
        repo.json(&["task", "list", "--json"])["data"]["tasks"],
        json!([])
    );
    assert_eq!(
        repo.json(&["task", "list", "--all", "--json"])["data"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let before = inventory(&repo.tasks());
    repo.error(&["task", "abandon", id, "--json"], 2, "usage");
    assert_eq!(inventory(&repo.tasks()), before);
    assert_eq!(inventory(&repo.repo.join(".git")), git_before);
    for directory in [
        repo.root.join("state/rabi"),
        repo.tasks().parent().unwrap().to_path_buf(),
        repo.tasks(),
        path.clone(),
    ] {
        assert_eq!(
            fs::metadata(directory).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    for file in [
        path.join("task.json"),
        path.join("events.jsonl"),
        path.join(".lock"),
        repo.tasks().join(".lock"),
    ] {
        assert_eq!(
            fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    for (command, expected) in [
        (vec!["task", "list", "--all"], "abandoned"),
        (vec!["task", "show", id], "No longer needed"),
    ] {
        let output = repo.rabi(&command);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(expected));
        assert!(!text.contains("secret"));
    }
}

#[test]
fn creation_refusal_table_writes_nothing() {
    for case in [
        "local branch",
        "worktree",
        "unborn worktree",
        "duplicate task",
        "empty intent",
        "invalid branch",
        "missing base",
        "missing remote",
    ] {
        let repo = Repo::new();
        match case {
            "local branch" => {
                repo.git(&["branch", "reserved"]);
            }
            "worktree" => {
                repo.git(&[
                    "worktree",
                    "add",
                    "-b",
                    "reserved",
                    repo.root.join("linked").to_str().unwrap(),
                ]);
            }
            "unborn worktree" => {
                repo.git(&[
                    "worktree",
                    "add",
                    "--detach",
                    repo.root.join("linked").to_str().unwrap(),
                ]);
                repo.git(&[
                    "-C",
                    repo.root.join("linked").to_str().unwrap(),
                    "symbolic-ref",
                    "HEAD",
                    "refs/heads/reserved",
                ]);
            }
            "duplicate task" => {
                repo.create("reserved");
            }
            "missing remote" => {
                repo.git(&["remote", "remove", "origin"]);
            }
            _ => {}
        }
        let before = repo.tasks().exists().then(|| inventory(&repo.tasks()));
        let intent = if case == "empty intent" {
            " \n "
        } else {
            "Intent"
        };
        let branch = if case == "invalid branch" {
            "bad..name"
        } else {
            "reserved"
        };
        let base = if case == "missing base" {
            "missing"
        } else {
            "main"
        };
        repo.error(
            &[
                "task", "new", intent, "--branch", branch, "--base", base, "--json",
            ],
            if case == "missing remote" { 1 } else { 2 },
            if case == "missing remote" {
                "failed"
            } else {
                "usage"
            },
        );
        assert_eq!(
            repo.tasks().exists().then(|| inventory(&repo.tasks())),
            before,
            "{case}"
        );
    }
}

#[test]
fn stdin_slug_and_explicit_repository_selection() {
    let repo = Repo::new();
    let mut child = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["task", "new", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"Fix authentication!\nDetails\n")
        .unwrap();
    let value = Repo::decode(child.wait_with_output().unwrap());
    let task = &value["data"]["task"];
    assert_eq!(task["intent"], "Fix authentication!\nDetails\n");
    assert!(task["branch"]
        .as_str()
        .unwrap()
        .starts_with("fix-authentication-details-"));
    let other = repo.json(&["task", "new", "Fix authentication!\nDetails\n", "--json"]);
    assert_ne!(other["data"]["task"]["branch"], task["branch"]);
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["-R", repo.repo.to_str().unwrap(), "task", "list", "--json"],
        )
        .current_dir(&repo.root)
        .output()
        .unwrap();
    assert_eq!(
        Repo::decode(output)["data"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn corrupt_or_unreadable_records_do_not_break_listing() {
    for case in ["invalid json", "wrong schema", "wrong id", "unreadable"] {
        let repo = Repo::new();
        let broken = repo.create("broken");
        let valid = repo.create("valid");
        let record = repo.directory(&broken).join("task.json");
        match case {
            "invalid json" => fs::write(&record, "{").unwrap(),
            "unreadable" => {
                fs::remove_file(&record).unwrap();
                fs::create_dir(&record).unwrap();
            }
            _ => {
                let mut value = broken.clone();
                if case == "wrong schema" {
                    value["schema"] = json!(2);
                } else {
                    value["id"] = valid["id"].clone();
                }
                fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        let before = inventory(&repo.tasks());
        let list = repo.json(&["task", "list", "--json"]);
        let tasks = list["data"]["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2);
        let item = tasks
            .iter()
            .find(|task| task["id"] == broken["id"])
            .unwrap();
        let reason = if case == "unreadable" {
            "internal"
        } else {
            "record_mismatch"
        };
        assert_eq!(item["state"], json!({"name":"blocked", "reason":reason}));
        assert_eq!(list["warnings"][0]["code"], reason);
        assert_eq!(inventory(&repo.tasks()), before);
    }
}

#[test]
fn prefix_resolution_table() {
    let repo = Repo::new();
    let first = repo.create("one");
    let second = repo.create("two");
    let ids = [
        "task_aaaaaa00-0000-4000-8000-000000000001",
        "task_aaaaaa00-0000-4000-8000-000000000002",
    ];
    for (task, id) in [first, second].iter().zip(ids) {
        let old = repo.directory(task);
        let new = repo.tasks().join(id);
        fs::rename(old, &new).unwrap();
        let mut record = task.clone();
        record["id"] = json!(id);
        fs::write(new.join("task.json"), serde_json::to_vec(&record).unwrap()).unwrap();
    }
    for (prefix, status) in [(ids[0], 0), ("task_aaaaaa", 2), ("task_", 2), ("task_b", 3)] {
        if status == 0 {
            assert_eq!(
                repo.json(&["task", "show", prefix, "--json"])["data"]["task"]["id"],
                ids[0]
            );
        } else {
            repo.error(
                &["task", "show", prefix, "--json"],
                status,
                if status == 3 { "not_found" } else { "usage" },
            );
        }
    }
    fs::remove_dir_all(repo.tasks().join(ids[1])).unwrap();
    assert_eq!(
        repo.json(&["task", "show", "task_a", "--json"])["data"]["task"]["id"],
        ids[0]
    );
    repo.json(&["task", "abandon", "task_a", "--json"]);
}

#[test]
fn state_derivation_and_terminal_refusal_table_are_read_only() {
    for (state, branch_exists, mismatch) in [
        ("new", false, false),
        ("new", true, true),
        ("implementing", false, true),
        ("implementing", true, false),
        ("merged", false, false),
        ("abandoned", false, false),
    ] {
        let repo = Repo::new();
        let task = repo.create("topic");
        let id = task["id"].as_str().unwrap();
        repo.set_state(&task, state);
        if branch_exists {
            repo.git(&["branch", "topic"]);
        }
        let before = inventory(&repo.tasks());
        let show = repo.json(&["task", "show", id, "--json"]);
        let list = repo.json(&["task", "list", "--all", "--json"]);
        let expected = if mismatch {
            json!({"name":"blocked", "reason":"record_mismatch"})
        } else {
            json!({"name":state})
        };
        assert_eq!(show["data"]["task"]["state"], expected);
        assert_eq!(list["data"]["tasks"][0]["state"], expected);
        assert_eq!(show["data"]["recorded_state"]["name"], state);
        assert_eq!(inventory(&repo.tasks()), before);
        if ["merged", "abandoned"].contains(&state) {
            repo.error(&["task", "abandon", id, "--json"], 2, "usage");
            assert_eq!(inventory(&repo.tasks()), before);
        }
    }
}

#[test]
fn push_remote_precedence_and_capture_table() {
    for (keys, expected) in [
        (vec![], "https://example.invalid/project.git"),
        (
            vec![("branch.topic.remote", "upstream")],
            "ssh://example.invalid/upstream",
        ),
        (
            vec![
                ("branch.topic.remote", "upstream"),
                ("remote.pushDefault", "push"),
            ],
            "https://example.invalid/push",
        ),
        (
            vec![
                ("branch.topic.remote", "origin"),
                ("remote.pushDefault", "origin"),
                ("branch.topic.pushRemote", "upstream"),
            ],
            "ssh://example.invalid/upstream",
        ),
    ] {
        let repo = Repo::new();
        repo.git(&[
            "remote",
            "add",
            "upstream",
            "ssh://user:secret@example.invalid/upstream",
        ]);
        repo.git(&[
            "remote",
            "add",
            "push",
            "https://token@example.invalid/push?secret=yes",
        ]);
        for (key, value) in keys {
            repo.git(&["config", key, value]);
        }
        let task = repo.create("topic");
        assert_eq!(task["push_remote_url"], expected);
        let stored = fs::read_to_string(repo.directory(&task).join("task.json")).unwrap();
        assert!(!stored.contains("secret"));
        assert!(!stored.contains("token"));
        repo.git(&[
            "remote",
            "set-url",
            "origin",
            "https://different.invalid/project",
        ]);
        assert_eq!(
            repo.json(&["task", "show", task["id"].as_str().unwrap(), "--json"])["data"]["task"]
                ["push_remote_url"],
            expected
        );
    }
}

#[test]
fn abandonment_appends_from_each_non_terminal_state() {
    for state in [
        "new",
        "implementing",
        "prechecking",
        "publishing",
        "ci",
        "reviewing",
        "ready",
        "classifying",
        "awaiting_approval",
        "merging",
        "blocked",
    ] {
        let repo = Repo::new();
        let mut task = repo.create("topic");
        task["state"] = if state == "blocked" {
            json!({"name":"blocked", "reason":"budget"})
        } else {
            json!({"name":state})
        };
        let directory = repo.directory(&task);
        fs::write(
            directory.join("task.json"),
            serde_json::to_vec(&task).unwrap(),
        )
        .unwrap();
        let earlier = fs::read(directory.join("events.jsonl")).unwrap();
        let output = repo.json(&["task", "abandon", task["id"].as_str().unwrap(), "--json"]);
        assert_eq!(
            output["data"]["events"][1]["from"], task["state"],
            "{state}"
        );
        assert_eq!(output["data"]["events"][1]["reason"], "abandoned");
        assert_eq!(output["data"]["task"]["state"]["name"], "abandoned");
        assert!(fs::read(directory.join("events.jsonl"))
            .unwrap()
            .starts_with(&earlier));
    }
}

#[test]
fn concurrent_creation_serializes_branch_reservations() {
    let repo = Repo::new();
    let mut children: Vec<_> = (0..4)
        .map(|_| {
            repo.command(
                env!("CARGO_BIN_EXE_rabi"),
                &["task", "new", "Intent", "--branch", "one-claim", "--json"],
            )
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
        })
        .collect();
    let mut successes = 0;
    for child in children.drain(..) {
        let output = child.wait_with_output().unwrap();
        if output.status.success() {
            successes += 1;
        } else {
            assert_eq!(output.status.code(), Some(2));
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["error"]["code"], "usage");
        }
    }
    assert_eq!(successes, 1);
    assert_eq!(
        repo.json(&["task", "list", "--json"])["data"]["tasks"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn empty_store_reads_create_nothing() {
    let repo = Repo::new();
    assert_eq!(
        repo.json(&["task", "list", "--json"])["data"]["tasks"],
        json!([])
    );
    repo.error(&["task", "show", "task_ffffff", "--json"], 3, "not_found");
    assert!(!repo.root.join("state").exists());
}

#[test]
fn linked_checkouts_share_the_store_and_abandon_releases_reservation() {
    let repo = Repo::new();
    let linked = repo.root.join("linked");
    repo.git(&["worktree", "add", "--detach", linked.to_str().unwrap()]);
    let task = repo.create("reserved");
    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["task", "list", "--json"])
        .current_dir(&linked)
        .output()
        .unwrap();
    assert_eq!(Repo::decode(output)["data"]["tasks"][0]["id"], task["id"]);
    repo.json(&["task", "abandon", task["id"].as_str().unwrap(), "--json"]);
    let replacement = repo.create("reserved");
    assert_ne!(replacement["id"], task["id"]);
}
