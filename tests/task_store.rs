#[path = "common/scripts.rs"]
mod scripts;
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
            .env_remove("RABI_TRUST_ALL")
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
fn task_assessment_selectors_accept_ids_and_preserve_workspace_fallback() {
    let repo = Repo::new();
    let task = repo.create("task/selected");
    let id = task["id"].as_str().unwrap();
    let workspace = task["workspacePath"].as_str().unwrap();
    fs::write(
        Path::new(workspace).join("selected.md"),
        "selected change\n",
    )
    .unwrap();
    let output = repo
        .command("git", &["add", "selected.md"])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = repo
        .command("git", &["commit", "-m", "selected change"])
        .current_dir(workspace)
        .output()
        .unwrap();
    assert!(output.status.success());
    for command in ["risk", "classify"] {
        let expected = repo.json(&["task", command, workspace, "--json"]);
        for selector in [id, &id[..6], &id[..12]] {
            let actual = repo.json(&["task", command, selector, "--json"]);
            assert_eq!(actual["data"]["head"], expected["data"]["head"]);
        }
    }
    repo.error(&["show", id, "--json"], 3, "not_found");
    let unknown = "task_unknown";
    repo.error(&["task", "risk", unknown, "--json"], 3, "not_found");
    repo.json(&["create", unknown, "--no-setup", "--json"]);
    repo.json(&["task", "risk", unknown, "--json"]);
    repo.json(&["create", "task_", "--no-setup", "--json"]);
    repo.error(&["task", "risk", "task_", "--json"], 2, "usage");

    let duplicate_id = format!(
        "{}{}",
        &id[..id.len() - 1],
        if id.ends_with('0') { '1' } else { '0' }
    );
    fs::create_dir(repo.tasks().join(&duplicate_id)).unwrap();
    repo.json(&["create", &id[..12], "--no-setup", "--json"]);
    let error = repo.error(&["task", "risk", &id[..12], "--json"], 2, "usage");
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Ambiguous task ID"));
}

#[test]
fn task_assessment_reports_missing_linked_workspaces_as_task_errors() {
    let repo = Repo::new();
    let task = repo.create("task/missing");
    let id = task["id"].as_str().unwrap();
    let workspace = task["workspacePath"].as_str().unwrap();
    for absent_binding in [false, true] {
        if absent_binding {
            let mut record = task.clone();
            record["workspacePath"] = Value::Null;
            fs::write(
                repo.directory(&task).join("task.json"),
                serde_json::to_vec(&record).unwrap(),
            )
            .unwrap();
        } else {
            fs::remove_dir_all(workspace).unwrap();
        }
        for command in ["risk", "classify", "status"] {
            let error = repo.error(&["task", command, id, "--json"], 3, "not_found");
            let message = error["error"]["message"].as_str().unwrap();
            assert!(message.contains(id));
            assert!(message.contains("no live linked workspace"));
        }
    }
}

#[test]
fn task_assessment_rejects_switched_and_detached_workspaces_without_classifying_other_commits() {
    let repo = Repo::new();
    let task = repo.create("task/selected");
    let id = task["id"].as_str().unwrap();
    let workspace = task["workspacePath"].as_str().unwrap();
    repo.git(&["-C", workspace, "switch", "-c", "other"]);
    fs::write(Path::new(workspace).join("other.md"), "other branch work\n").unwrap();
    repo.git(&["-C", workspace, "add", "other.md"]);
    repo.git(&["-C", workspace, "commit", "-m", "other branch work"]);
    let classifications = repo.repo.join(".git/rabi/classifications");
    for detached in [false, true] {
        if detached {
            repo.git(&["-C", workspace, "switch", "--detach"]);
        }
        for selector in [id, &id[..12]] {
            for args in [
                vec!["task", "risk", selector, "--json"],
                vec!["task", "classify", selector, "--json"],
                vec!["task", "status", selector, "--json"],
                vec!["task", "merge", selector, "--dry-run", "--json"],
            ] {
                let error = repo.error(&args, 3, "not_found");
                assert_eq!(
                    error["error"]["message"],
                    format!("Task {id} has no live linked workspace")
                );
            }
        }
        assert!(!classifications.exists());
    }
}

#[test]
fn new_creates_workspace_and_preserves_primary_git_while_store_commands_append_events() {
    let repo = Repo::new();
    let git_before = inventory(&repo.repo.join(".git"));
    let task = repo.create("task/example");
    let id = task["id"].as_str().unwrap();
    assert_eq!(task["schema"], 1);
    assert_eq!(task["state"], json!({"name":"new"}));
    assert_eq!(task["baseRef"], "origin/main");
    assert_eq!(task["pushRemoteUrl"], "https://example.invalid/project.git");
    let workspace = task["workspacePath"].as_str().unwrap();
    assert!(Path::new(workspace).join(".git").is_file());
    assert_eq!(
        repo.git(&["rev-parse", "task/example"]),
        repo.git(&["rev-parse", task["baseRef"].as_str().unwrap()])
    );
    assert_eq!(
        repo.git(&["-C", workspace, "branch", "--show-current"]),
        "task/example"
    );
    assert_eq!(
        repo.git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        2
    );
    let git_after_creation = inventory(&repo.repo.join(".git"));
    for (path, contents) in &git_before {
        assert_eq!(git_after_creation.get(path), Some(contents), "{path:?}");
    }
    assert_eq!(
        git_before.get(Path::new("index")),
        git_after_creation.get(Path::new("index"))
    );
    assert!(task["prNumber"].is_null());
    let list = repo.json(&["task", "list", "--json"]);
    assert_eq!(list["command"], "task list");
    assert_eq!(list["data"]["tasks"][0]["intent"], "Implement something");
    assert_eq!(list["data"]["tasks"][0]["id"], id);
    let path = repo.directory(&task);
    let stored: Value = serde_json::from_slice(&fs::read(path.join("task.json")).unwrap()).unwrap();
    assert_eq!(stored, task);
    for key in [
        "createdAt",
        "updatedAt",
        "baseRef",
        "pushRemoteUrl",
        "workspacePath",
        "prNumber",
    ] {
        assert!(stored.get(key).is_some(), "{key}");
    }
    assert!(stored
        .as_object()
        .unwrap()
        .keys()
        .all(|key| !key.contains('_')));
    let original = fs::read(path.join("events.jsonl")).unwrap();
    let before = inventory(&repo.tasks());
    let show = repo.json(&["task", "show", id, "--json"]);
    assert_eq!(show["data"]["task"], task);
    assert_eq!(show["data"]["events"][0]["from"], Value::Null);
    assert_eq!(show["data"]["events"][0]["reason"], "created");
    assert_eq!(
        serde_json::from_slice::<Value>(&original).unwrap(),
        show["data"]["events"][0]
    );
    assert_eq!(show["data"]["recordedState"], task["state"]);
    assert!(show["data"].get("recorded_state").is_none());
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
    assert_eq!(inventory(&repo.repo.join(".git")), git_after_creation);
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
        let git_before = inventory(&repo.repo.join(".git"));
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
        assert_eq!(inventory(&repo.repo.join(".git")), git_before, "{case}");
    }
}

#[test]
fn remote_tracking_branch_is_refused_before_the_task_record() {
    let repo = Repo::new();
    repo.git(&["update-ref", "refs/remotes/origin/reserved", "HEAD"]);
    let before = repo.tasks().exists().then(|| inventory(&repo.tasks()));
    let git_before = inventory(&repo.repo.join(".git"));
    let error = repo.error(
        &[
            "task", "new", "Intent", "--branch", "reserved", "--base", "main", "--json",
        ],
        4,
        "exists_remote",
    );
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("already exists as a remote-tracking branch"),
        "{message}"
    );
    assert!(
        !message.contains("was recorded")
            && !message.contains("Recover with")
            && !message.contains("rabi create --branch"),
        "{message}"
    );
    assert!(
        message.contains("Choose another --branch")
            || message.contains("delete the stale remote-tracking ref"),
        "{message}"
    );
    assert_eq!(
        repo.tasks().exists().then(|| inventory(&repo.tasks())),
        before
    );
    assert_eq!(inventory(&repo.repo.join(".git")), git_before);
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
    assert!(Path::new(task["workspacePath"].as_str().unwrap())
        .join(".git")
        .is_file());
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
    for (state, branch_exists, bound, mismatch) in [
        ("new", false, false, false),
        ("new", true, false, true),
        ("new", true, true, false),
        ("new", false, true, true),
        ("implementing", false, true, true),
        ("implementing", true, true, false),
        ("merged", false, true, false),
        ("abandoned", false, true, false),
    ] {
        let repo = Repo::new();
        let task = repo.create("topic");
        let id = task["id"].as_str().unwrap();
        repo.set_state(&task, state);
        if !bound {
            let path = repo.directory(&task).join("task.json");
            let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            record["workspacePath"] = Value::Null;
            fs::write(path, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        if !branch_exists {
            repo.git(&[
                "worktree",
                "remove",
                task["workspacePath"].as_str().unwrap(),
            ]);
            repo.git(&["branch", "-D", "topic"]);
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
        assert_eq!(show["data"]["recordedState"]["name"], state);
        assert_eq!(inventory(&repo.tasks()), before);
        if ["merged", "abandoned"].contains(&state) {
            repo.error(&["task", "abandon", id, "--json"], 2, "usage");
            assert_eq!(inventory(&repo.tasks()), before);
        }
    }
}

#[test]
fn blocked_reason_is_preserved_with_or_without_a_branch() {
    for branch_exists in [false, true] {
        let repo = Repo::new();
        let mut task = repo.create("topic");
        task["state"] = json!({"name":"blocked", "reason":"budget"});
        fs::write(
            repo.directory(&task).join("task.json"),
            serde_json::to_vec(&task).unwrap(),
        )
        .unwrap();
        if !branch_exists {
            repo.git(&[
                "worktree",
                "remove",
                task["workspacePath"].as_str().unwrap(),
            ]);
            repo.git(&["branch", "-D", "topic"]);
        }
        let before = inventory(&repo.tasks());
        let show = repo.json(&["task", "show", task["id"].as_str().unwrap(), "--json"]);
        let list = repo.json(&["task", "list", "--json"]);
        assert_eq!(show["data"]["task"]["state"], task["state"]);
        assert_eq!(show["data"]["recordedState"], task["state"]);
        assert_eq!(list["data"]["tasks"][0]["state"], task["state"]);
        assert_eq!(show["warnings"], json!([]));
        assert_eq!(list["warnings"], json!([]));
        assert_eq!(inventory(&repo.tasks()), before);
    }
}

#[test]
fn corrupt_records_fail_closed_with_operational_errors_naming_the_task() {
    for case in [
        "invalid json",
        "wrong schema",
        "wrong id",
        "unreadable",
        "missing",
    ] {
        let repo = Repo::new();
        let task = repo.create("broken");
        let path = repo.directory(&task).join("task.json");
        match case {
            "invalid json" => fs::write(&path, "{").unwrap(),
            "missing" => fs::remove_file(&path).unwrap(),
            "unreadable" => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            _ => {
                let mut record = task.clone();
                if case == "wrong schema" {
                    record["schema"] = json!(2);
                } else {
                    record["id"] = json!("task_different");
                }
                fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
        }
        let before = inventory(&repo.tasks());
        for args in [
            vec!["task", "show", task["id"].as_str().unwrap(), "--json"],
            vec!["task", "abandon", task["id"].as_str().unwrap(), "--json"],
            vec![
                "task",
                "new",
                "New intent",
                "--branch",
                "unrelated",
                "--json",
            ],
        ] {
            let output = repo.error(&args, 1, "failed");
            let message = output["error"]["message"].as_str().unwrap();
            assert!(
                message.contains(task["id"].as_str().unwrap()),
                "{case}: {message}"
            );
            if args[1] == "new" {
                assert!(message.contains("Cannot check branch reservations"));
            }
            assert_eq!(inventory(&repo.tasks()), before, "{case}: {args:?}");
        }
        assert_eq!(
            repo.json(&["task", "list", "--json"])["data"]["tasks"][0]["state"]["name"],
            "blocked"
        );
    }
}

#[test]
fn corrupt_events_are_operational_errors_before_abandonment() {
    for missing in [false, true] {
        let repo = Repo::new();
        let task = repo.create("broken-events");
        let path = repo.directory(&task).join("events.jsonl");
        if missing {
            fs::remove_file(&path).unwrap();
        } else {
            fs::write(&path, "{\n").unwrap();
        }
        let before = inventory(&repo.tasks());
        for command in ["show", "abandon"] {
            let output = repo.error(
                &["task", command, task["id"].as_str().unwrap(), "--json"],
                1,
                "failed",
            );
            assert!(output["error"]["message"]
                .as_str()
                .unwrap()
                .contains(task["id"].as_str().unwrap()));
            assert_eq!(inventory(&repo.tasks()), before);
        }
    }
}

#[test]
fn incomplete_staging_directories_are_ignored_and_successful_tasks_are_published() {
    for phase in ["empty", "lock", "events", "record"] {
        let repo = Repo::new();
        let mut task = repo.create("existing");
        let staging = repo
            .tasks()
            .join(format!(".staging-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&staging).unwrap();
        if phase != "empty" {
            fs::write(staging.join(".lock"), "").unwrap();
        }
        if ["events", "record"].contains(&phase) {
            fs::copy(
                repo.directory(&task).join("events.jsonl"),
                staging.join("events.jsonl"),
            )
            .unwrap();
        }
        if phase == "record" {
            task["branch"] = json!("next");
            fs::write(
                staging.join("task.json"),
                serde_json::to_vec(&task).unwrap(),
            )
            .unwrap();
        }
        let staged = inventory(&staging);
        let next = repo.create("next");
        assert_eq!(
            repo.json(&["task", "list", "--json"])["data"]["tasks"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(inventory(&staging), staged);
        let published: Value =
            serde_json::from_slice(&fs::read(repo.directory(&next).join("task.json")).unwrap())
                .unwrap();
        assert_eq!(published, next);
        assert_eq!(
            fs::read_dir(repo.tasks())
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_str()
                    .unwrap()
                    .starts_with(".staging-"))
                .count(),
            1
        );
    }
}

#[test]
fn task_creation_preserves_read_only_handoff_commands() {
    for initialized in [false, true] {
        let repo = Repo::new();
        let linked = repo.root.join("linked");
        let selector = linked.to_str().unwrap();
        repo.git(&["worktree", "add", "--detach", selector]);
        repo.json(&["adopt", selector, "--json"]);
        if initialized {
            repo.json(&["handoff", "add", selector, "Keep this note", "--json"]);
        }
        let show = repo.json(&["handoff", "show", selector, "--json"]);
        let path = repo.json(&["handoff", "path", selector, "--json"]);
        repo.create("task-after-handoff");
        let namespace = repo.tasks().parent().unwrap().to_path_buf();
        let manifest: Value =
            serde_json::from_slice(&fs::read(namespace.join("repository.json")).unwrap()).unwrap();
        assert_eq!(
            manifest,
            json!({"schema":1,"commonDir":repo.repo.join(".git").canonicalize().unwrap()})
        );
        let before = inventory(&namespace);
        assert_eq!(
            repo.json(&["handoff", "show", selector, "--json"])["data"],
            show["data"]
        );
        assert_eq!(
            repo.json(&["handoff", "path", selector, "--json"])["data"],
            path["data"]
        );
        assert_eq!(inventory(&namespace), before);
    }
}

#[test]
fn task_and_handoff_validate_the_shared_repository_manifest() {
    for contents in [
        "{",
        r#"{"schema":2,"commonDir":"/different/.git"}"#,
        r#"{"schema":1,"commonDir":"/different/.git"}"#,
    ] {
        let repo = Repo::new();
        let linked = repo.root.join("linked");
        let selector = linked.to_str().unwrap();
        repo.git(&["worktree", "add", "--detach", selector]);
        repo.json(&["adopt", selector, "--json"]);
        let task = repo.create("original");
        let namespace = repo.tasks().parent().unwrap().to_path_buf();
        fs::write(namespace.join("repository.json"), contents).unwrap();
        let before = inventory(&namespace);
        for args in [
            vec!["task", "list", "--json"],
            vec!["task", "show", task["id"].as_str().unwrap(), "--json"],
            vec!["task", "abandon", task["id"].as_str().unwrap(), "--json"],
            vec!["task", "new", "Unrelated intent", "--json"],
            vec!["handoff", "show", selector, "--json"],
            vec!["handoff", "path", selector, "--json"],
            vec!["handoff", "add", selector, "Cannot append", "--json"],
        ] {
            let output = repo.error(&args, 1, "failed");
            assert!(output["error"]["message"]
                .as_str()
                .unwrap()
                .contains("repository"));
            assert_eq!(inventory(&namespace), before, "{args:?}");
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
        assert_eq!(task["pushRemoteUrl"], expected);
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
                ["pushRemoteUrl"],
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
    repo.git(&[
        "worktree",
        "remove",
        task["workspacePath"].as_str().unwrap(),
    ]);
    repo.git(&["branch", "-D", "reserved"]);
    let replacement = repo.create("reserved");
    assert_ne!(replacement["id"], task["id"]);
}

#[test]
fn allocation_failure_keeps_published_task_and_reports_recovery() {
    for untrusted in [false, true] {
        let repo = Repo::new();
        if untrusted {
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\nsetup = [\"touch hook-ran\"]\n",
            )
            .unwrap();
        } else {
            let blocked = repo.root.join("blocked");
            fs::write(&blocked, "occupied").unwrap();
            repo.json(&[
                "config",
                "set",
                "workspace-root",
                blocked.to_str().unwrap(),
                "--json",
            ]);
        }
        let output = repo.rabi(&[
            "task",
            "new",
            "Retain intent",
            "--branch",
            "task/recover",
            "--base",
            "main",
            "--json",
        ]);
        assert!(!output.status.success());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if untrusted {
            assert_eq!(output.status.code(), Some(4));
            assert_eq!(value["error"]["code"], "untrusted");
        }
        let task = &value["data"]["task"];
        let id = task["id"].as_str().unwrap();
        let message = value["error"]["message"].as_str().unwrap();
        assert!(message.contains(id));
        assert!(
            message.contains("rabi create 'task/recover' --base 'main'"),
            "{message}"
        );
        assert!(task["workspacePath"].is_null());
        assert_eq!(
            repo.json(&["task", "show", id, "--json"])["data"]["task"],
            *task
        );
        assert_eq!(repo.git(&["branch", "--format=%(refname:short)"]), "main");
        assert_eq!(
            repo.git(&["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
        assert!(!repo.repo.join("hook-ran").exists());
        assert!(fs::read_dir(repo.tasks()).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging-")));
        repo.error(
            &[
                "task",
                "new",
                "Retry",
                "--branch",
                "task/recover",
                "--base",
                "main",
                "--json",
            ],
            2,
            "usage",
        );
        assert_eq!(
            repo.json(&["task", "list", "--json"])["data"]["tasks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        if untrusted {
            repo.json(&["trust", "add", "--json"]);
            let created = repo.json(&["create", "task/recover", "--base", "main", "--json"]);
            assert!(
                Path::new(created["data"]["workspace"]["path"].as_str().unwrap())
                    .join("hook-ran")
                    .is_file()
            );
        }
    }
}

#[test]
fn task_new_runs_ordinary_copy_and_hooks_and_binds_partial_setup() {
    for partial in [false, true] {
        let repo = Repo::new();
        fs::write(repo.repo.join(".gitignore"), ".env\n").unwrap();
        fs::write(repo.repo.join(".env"), "EXAMPLE=1\n").unwrap();
        fs::write(repo.repo.join(".worktreeinclude"), ".env\n").unwrap();
        fs::write(
            repo.repo.join(".rabi.toml"),
            if partial {
                "[hooks]\nsetup = [\"exit 7\"]\n"
            } else {
                "[hooks]\nsetup = [\"touch prepared\"]\n"
            },
        )
        .unwrap();
        repo.git(&["add", ".gitignore", ".worktreeinclude", ".rabi.toml"]);
        repo.git(&["commit", "-m", "fixture setup"]);
        repo.json(&["trust", "add", "--json"]);
        let output = repo.rabi(&[
            "task",
            "new",
            "Prepare task",
            "--branch",
            "prepared",
            "--base",
            "main",
            "--json",
        ]);
        assert_eq!(output.status.code(), Some(if partial { 6 } else { 0 }));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let task = &value["data"]["task"];
        let workspace = task["workspacePath"].as_str().unwrap();
        assert_eq!(value["data"]["workspace"]["path"], workspace);
        assert_eq!(
            fs::read_to_string(Path::new(workspace).join(".env")).unwrap(),
            "EXAMPLE=1\n"
        );
        assert_eq!(Path::new(workspace).join("prepared").exists(), !partial);
        assert_eq!(
            repo.json(&["task", "show", task["id"].as_str().unwrap(), "--json"])["data"]["task"],
            *task
        );
        if partial {
            assert_eq!(value["error"]["code"], "partial");
            let message = value["error"]["message"].as_str().unwrap();
            assert!(message.contains(task["id"].as_str().unwrap()));
            assert!(message.contains(workspace));
            assert!(message.contains("Retry with `"));
            assert!(message.contains("rabi setup 'prepared'"));
        }
    }
}

#[test]
fn attach_reuses_workspace_created_after_task_publication_and_binds_path() {
    let repo = Repo::new();
    let real_git = repo
        .command("sh", &["-c", "command -v git"])
        .output()
        .unwrap();
    let bin = repo.root.join("bin");
    fs::create_dir(&bin).unwrap();
    scripts::write_executable(
        bin.join("git"),
        r#"#!/bin/sh
set -eu
if [ ! -e "$RABI_REUSE_MARKER" ]; then
  for record in "$XDG_STATE_HOME"/rabi/repositories/*/tasks/task_*/task.json; do
    if [ -f "$record" ]; then
      touch "$RABI_REUSE_MARKER"
      "$RABI_REUSE_BIN" create task/reused --base main --json > "$RABI_REUSE_RESULT"
      break
    fi
  done
fi
exec "$RABI_REUSE_GIT" "$@"
"#,
    )
    .unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "task",
                "new",
                "Reuse task checkout",
                "--branch",
                "task/reused",
                "--base",
                "main",
                "--json",
            ],
        )
        .env("PATH", path)
        .env("RABI_REUSE_MARKER", repo.root.join("reused"))
        .env("RABI_REUSE_RESULT", repo.root.join("allocation.json"))
        .env("RABI_REUSE_BIN", env!("CARGO_BIN_EXE_rabi"))
        .env(
            "RABI_REUSE_GIT",
            String::from_utf8(real_git.stdout).unwrap().trim(),
        )
        .output()
        .unwrap();
    let value = Repo::decode(output);
    let created: Value =
        serde_json::from_slice(&fs::read(repo.root.join("allocation.json")).unwrap()).unwrap();
    assert_eq!(value["data"]["source"]["reused"], true);
    assert_eq!(
        value["data"]["task"]["workspacePath"],
        created["data"]["workspace"]["path"]
    );
    assert_eq!(
        repo.git(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        2
    );
    let task = &value["data"]["task"];
    assert_eq!(
        repo.json(&["task", "show", task["id"].as_str().unwrap(), "--json"])["data"]["task"],
        *task
    );
}

#[cfg(debug_assertions)]
#[test]
fn metadata_save_failure_still_binds_task_workspace_and_returns_partial() {
    let repo = Repo::new();
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "task",
                "new",
                "Retain checkout",
                "--branch",
                "topic",
                "--base",
                "main",
                "--json",
            ],
        )
        .env("RABI_TEST_FAIL_METADATA_SAVE", "1")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "partial");
    assert_eq!(value["data"]["metadata"]["ok"], false);
    assert_eq!(value["data"]["setup"]["ok"], true);
    let task = &value["data"]["task"];
    let workspace = task["workspacePath"].as_str().unwrap();
    assert!(Path::new(workspace).join(".git").is_file());
    assert_eq!(value["data"]["workspace"]["path"], workspace);
    assert_eq!(
        repo.json(&["task", "show", task["id"].as_str().unwrap(), "--json"])["data"]["task"],
        *task
    );
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains(task["id"].as_str().unwrap()));
    assert!(message.contains(workspace));
    assert!(message.contains("rabi setup 'topic'"));
}
