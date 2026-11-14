#![cfg(unix)]
#[path = "common/scripts.rs"]
mod scripts;
#[path = "common/git.rs"]
mod test_git;
#[path = "common/wait.rs"]
mod wait;

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GH: &str = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$RABI_BATCH_FIXTURE/gh-trace"
head=$(git rev-parse HEAD)
base=$(git rev-parse main)
case "$1 $2" in
  'repo view') printf '%s\n' '{"url":"https://github.com/fixture/project","owner":{"login":"fixture"},"name":"project"}' ;;
  *)
    [ "$1" = api ] || exit 90
    if [ "$2" = --hostname ]; then shift 2; fi
    case "$2" in
      repos/fixture/project) printf '{"default_branch":"main"}\n' ;;
      repos/fixture/project/pulls/7)
        printf '{"number":7,"state":"open","draft":true,"merged":false,"base":{"ref":"main","sha":"%s","repo":{"full_name":"fixture/project"}},"head":{"ref":"waiting","sha":"%s","repo":{"full_name":"fixture/project"}},"mergeable":true,"mergeable_state":"clean","changed_files":0,"additions":0,"deletions":0}\n' "$base" "$head" ;;
      repos/fixture/project/pulls/7/files*|repos/fixture/project/pulls/7/reviews*|repos/fixture/project/commits/*/statuses*) printf '[]\n' ;;
      repos/fixture/project/commits/*/check-runs*) printf '{"total_count":1,"check_runs":[{"id":1,"name":"test","status":"in_progress","conclusion":null,"app":{"slug":"github-actions"}}]}\n' ;;
      repos/fixture/project/git/ref/heads/main) printf '{"ref":"refs/heads/main","object":{"type":"commit","sha":"%s"}}\n' "$base" ;;
      repos/fixture/project/git/trees/1111111111111111111111111111111111111111) printf '{"truncated":false,"tree":[{"path":"policy.toml","mode":"100644","sha":"2222222222222222222222222222222222222222"}]}\n' ;;
      repos/fixture/project/git/trees/*) printf '{"truncated":false,"tree":[{"path":".rabi","mode":"040000","sha":"1111111111111111111111111111111111111111"}]}\n' ;;
      repos/fixture/project/git/blobs/*)
        content=$(printf "schema=1\n[ci]\nrequired=['test']\n" | base64 | tr -d '\n')
        printf '{"encoding":"base64","content":"%s"}\n' "$content" ;;
      *) exit 91 ;;
    esac ;;
esac
"#;

struct Repo {
    root: PathBuf,
    repo: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-batch-test-{}", uuid::Uuid::new_v4()));
        for directory in ["repo", "home", "bin"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        test_git::write_config(&root.join("home"));
        let fixture = Self {
            repo: root.join("repo"),
            root,
        };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "fixture"]);
        fixture.git(&["config", "user.email", "fixture"]);
        fixture.git(&["commit", "--allow-empty", "-m", "fixture base"]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/project.git",
        ]);
        scripts::write_executable(fixture.root.join("bin/gh"), GH).unwrap();
        scripts::write_executable(
            fixture.root.join("bin/codex"),
            "#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' \"$PWD\" >> \"$RABI_BATCH_FIXTURE/provider-trace\"\n",
        )
        .unwrap();
        // Refuse every transport operation rather than allowing a test failure
        // to contact a real remote accidentally.
        let real_git = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        assert!(real_git.status.success());
        scripts::write_executable(
            fixture.root.join("bin/git"),
            format!(
                "#!/bin/sh\nfor arg in \"$@\"; do case \"$arg\" in fetch|push|ls-remote) exit 92 ;; esac; done\nexec '{}' \"$@\"\n",
                String::from_utf8(real_git.stdout).unwrap().trim().replace('\'', "'\\''")
            ),
        ).unwrap();
        fixture
    }

    fn command(&self, binary: impl AsRef<std::ffi::OsStr>, args: &[&str]) -> Command {
        let mut command = test_git::command(binary);
        let path = std::env::join_paths(std::iter::once(self.root.join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        command
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("PATH", path)
            .env("RABI_BATCH_FIXTURE", &self.root)
            .env("RABI_TRUST_ALL", "1")
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_AGENT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
        command
    }

    fn run(&self, mut command: Command) -> Output {
        let mut capture = wait::OutputCapture::new();
        let mut child = capture.apply(&mut command).spawn().unwrap();
        capture.finish(&mut child)
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.run(self.command("git", args));
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn rabi(&self, args: &[&str], exit: i32) -> Value {
        let output = self.run(self.command(env!("CARGO_BIN_EXE_rabi"), args));
        assert_eq!(
            output.status.code(),
            Some(exit),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn task(&self, index: u32, branch: &str, state: &str, checkout: bool) -> (String, PathBuf) {
        let created = self.rabi(
            &[
                "task",
                "new",
                "Fixture task",
                "--branch",
                branch,
                "--base",
                "main",
                "--json",
            ],
            0,
        );
        let old_id = created["data"]["task"]["id"].as_str().unwrap();
        let tasks = fs::read_dir(self.root.join("state/rabi/repositories"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("tasks");
        let id = format!("task_00000000-0000-4000-8000-{index:012}");
        let path = tasks.join(&id);
        fs::rename(tasks.join(old_id), &path).unwrap();
        let record = path.join("task.json");
        let mut value: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
        value["id"] = json!(id);
        value["state"] = json!({"name":state});
        if state == "blocked" {
            value["state"]["reason"] = json!("budget");
        }
        if checkout {
            let workspace = self.root.join(branch);
            self.git(&[
                "worktree",
                "add",
                "-b",
                branch,
                workspace.to_str().unwrap(),
                "main",
            ]);
            value["workspacePath"] = json!(workspace);
            value["advance"]["evaluatedHead"] = json!(self.git(&["rev-parse", "main"]));
        }
        fs::write(&record, serde_json::to_vec(&value).unwrap()).unwrap();
        (id, record)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn state(path: &Path) -> Value {
    serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap()["state"].clone()
}

#[test]
fn batch_empty_store_succeeds_without_initializing_task_storage() {
    let repo = Repo::new();
    let result = repo.rabi(&["task", "advance", "--all", "--wait", "--json"], 0);
    assert_eq!(result["data"]["items"], json!([]));
    assert!(!repo.root.join("state").exists());
}

#[test]
fn batch_continues_after_errors_and_skips_terminal_blocked_and_mismatched_tasks() {
    let repo = Repo::new();
    let (corrupt, corrupt_record) = repo.task(1, "corrupt", "implementing", true);
    let (missing, missing_record) = repo.task(2, "missing", "implementing", false);
    repo.git(&["branch", "missing"]);
    let (progress, progress_record) = repo.task(3, "progress", "implementing", true);
    repo.git(&[
        "-C",
        repo.root.join("progress").to_str().unwrap(),
        "commit",
        "--allow-empty",
        "-m",
        "fixture progress",
    ]);
    let (_, terminal_record) = repo.task(4, "terminal", "merged", false);
    let (_, blocked_record) = repo.task(5, "blocked", "blocked", false);
    let (_, mismatch_record) = repo.task(6, "mismatch", "implementing", false);
    let (new, _) = repo.task(7, "new", "new", false);
    // Corrupt last, after creation has validated all earlier reservations.
    fs::write(&corrupt_record, "invalid").unwrap();
    let result = repo.rabi(&["task", "advance", "--all", "--json"], 1);
    let items = result["data"]["items"].as_array().unwrap();
    assert_eq!(
        items
            .iter()
            .map(|item| item["taskId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [corrupt, missing, progress, new]
    );
    assert_eq!(
        items
            .iter()
            .map(|item| item["exitCode"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        [1, 4, 0, 0]
    );
    assert_eq!(items[0]["report"], Value::Null);
    assert_eq!(items[0]["error"]["code"], "failed");
    assert_eq!(items[2]["report"]["state"]["name"], "prechecking");
    assert_eq!(items[3]["report"]["reason"], "skipped_new");
    assert_eq!(items[3]["error"], Value::Null);
    assert_eq!(state(&progress_record)["name"], "prechecking");
    assert_eq!(state(&missing_record)["name"], "implementing");
    for path in [&terminal_record, &blocked_record, &mismatch_record] {
        assert!(!path.with_file_name(".advance.lock").exists());
    }
    assert!(!repo.root.join("provider-trace").exists());
    assert!(!repo.root.join("gh-trace").exists());
}

#[test]
fn batch_new_tasks_are_non_failing_skips_without_launches_or_record_changes() {
    let repo = Repo::new();
    let (id, record) = repo.task(1, "new", "new", false);
    let events = record.with_file_name("events.jsonl");
    let before_record = fs::read(&record).unwrap();
    let before_events = fs::read(&events).unwrap();
    for flags in [vec!["--all", "--json"], vec!["--all", "--wait", "--json"]] {
        let args: Vec<_> = ["task", "advance"].into_iter().chain(flags).collect();
        let result = repo.rabi(&args, 0);
        assert_eq!(result["ok"], true);
        assert_eq!(result["data"]["items"].as_array().unwrap().len(), 1);
        let item = &result["data"]["items"][0];
        assert_eq!(item["taskId"], id);
        assert_eq!(item["exitCode"], 0);
        assert_eq!(item["report"]["state"]["name"], "new");
        assert_eq!(item["report"]["reason"], "skipped_new");
        assert_eq!(item["report"]["fixBudget"], 3);
        assert_eq!(item["report"]["fixesUsed"], 0);
        assert_eq!(item["error"], Value::Null);
    }
    let single = repo.rabi(&["task", "advance", &id, "--json"], 4);
    assert_eq!(single["error"]["code"], "refused");
    assert_eq!(fs::read(record).unwrap(), before_record);
    assert_eq!(fs::read(events).unwrap(), before_events);
    assert!(!repo.root.join("provider-trace").exists());
    assert!(!repo.root.join("gh-trace").exists());
}

#[test]
fn batch_new_task_does_not_turn_other_tasks_progress_into_a_refusal() {
    let repo = Repo::new();
    let (new, _) = repo.task(1, "new", "new", false);
    let (progress, record) = repo.task(2, "progress", "implementing", true);
    repo.git(&[
        "-C",
        repo.root.join("progress").to_str().unwrap(),
        "commit",
        "--allow-empty",
        "-m",
        "fixture progress",
    ]);
    let result = repo.rabi(&["task", "advance", "--all", "--json"], 0);
    assert_eq!(result["data"]["items"][0]["taskId"], new);
    assert_eq!(
        result["data"]["items"][0]["report"]["reason"],
        "skipped_new"
    );
    assert_eq!(result["data"]["items"][1]["taskId"], progress);
    assert_eq!(result["data"]["items"][1]["exitCode"], 0);
    assert_eq!(state(&record)["name"], "prechecking");
    assert!(!repo.root.join("provider-trace").exists());
    assert!(!repo.root.join("gh-trace").exists());
}

#[test]
fn batch_wait_stops_at_waiting_and_continues_to_the_next_task() {
    let repo = Repo::new();
    let (waiting, waiting_record) = repo.task(1, "waiting", "ci", true);
    let mut record: Value = serde_json::from_slice(&fs::read(&waiting_record).unwrap()).unwrap();
    record["prNumber"] = json!(7);
    fs::write(&waiting_record, serde_json::to_vec(&record).unwrap()).unwrap();
    let (no_progress, no_progress_record) = repo.task(2, "no-progress", "implementing", true);
    let result = repo.rabi(&["task", "advance", "--all", "--wait", "--json"], 4);
    let items = result["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["taskId"], waiting);
    assert_eq!(items[0]["exitCode"], 10);
    assert_eq!(items[0]["report"]["reason"], "waiting");
    assert_eq!(items[1]["taskId"], no_progress);
    assert_eq!(items[1]["exitCode"], 4);
    assert_eq!(
        state(&no_progress_record),
        json!({"name":"blocked","reason":"no_progress"})
    );
    let trace = fs::read_to_string(repo.root.join("gh-trace")).unwrap();
    assert_eq!(
        trace
            .lines()
            .filter(|line| line.contains("check-runs"))
            .count(),
        1
    );
    assert_eq!(
        fs::read_to_string(repo.root.join("provider-trace"))
            .unwrap()
            .lines()
            .count(),
        1
    );
}

#[test]
fn batch_preserves_per_task_lock_and_continues_after_contention() {
    let repo = Repo::new();
    let (locked, locked_record) = repo.task(1, "locked", "implementing", true);
    let (progress, progress_record) = repo.task(2, "progress", "implementing", true);
    repo.git(&[
        "-C",
        repo.root.join("progress").to_str().unwrap(),
        "commit",
        "--allow-empty",
        "-m",
        "fixture progress",
    ]);
    let lock = fs::File::create(locked_record.with_file_name(".advance.lock")).unwrap();
    lock.lock().unwrap();
    let result = repo.rabi(&["task", "advance", "--all", "--json"], 4);
    assert_eq!(result["data"]["items"][0]["taskId"], locked);
    assert_eq!(result["data"]["items"][0]["exitCode"], 4);
    assert_eq!(result["data"]["items"][1]["taskId"], progress);
    assert_eq!(state(&progress_record)["name"], "prechecking");
    assert_eq!(state(&locked_record)["name"], "implementing");
}

#[test]
fn batch_all_rejects_a_selector() {
    let repo = Repo::new();
    let result = repo.rabi(&["task", "advance", "--all", "task_000000", "--json"], 2);
    assert_eq!(result["error"]["code"], "usage");
}

#[test]
fn batch_wait_drives_multiple_steps_and_retains_partial_results_after_error() {
    let repo = Repo::new();
    let (progress, progress_record) = repo.task(1, "progress", "implementing", true);
    repo.git(&[
        "-C",
        repo.root.join("progress").to_str().unwrap(),
        "commit",
        "--allow-empty",
        "-m",
        "fixture progress",
    ]);
    let (later, later_record) = repo.task(2, "later", "implementing", true);
    // First step accepts the clean descendant commit; the next publication
    // step reaches the fixture's transport refusal without contacting a remote.
    let result = repo.rabi(&["task", "advance", "--all", "--wait", "--json"], 1);
    assert_eq!(result["data"]["items"][0]["taskId"], progress);
    assert_eq!(result["data"]["items"][0]["exitCode"], 1);
    assert_eq!(state(&progress_record)["name"], "prechecking");
    assert!(repo.root.join("gh-trace").exists());
    assert_eq!(result["data"]["items"][1]["taskId"], later);
    assert_eq!(result["data"]["items"][1]["exitCode"], 4);
    assert_eq!(state(&later_record)["name"], "blocked");
}

#[test]
fn batch_interrupt_stops_before_launching_later_tasks() {
    let repo = Repo::new();
    let (first, _) = repo.task(1, "first", "implementing", true);
    let (_, later_record) = repo.task(2, "later", "implementing", true);
    scripts::write_executable(
        repo.root.join("bin/codex"),
        "#!/bin/sh\ncat >/dev/null\nkill -TERM \"$PPID\"\nprintf 'interrupted\\n' >> \"$RABI_BATCH_FIXTURE/provider-trace\"\n",
    )
    .unwrap();
    let result = repo.rabi(&["task", "advance", "--all", "--wait", "--json"], 130);
    assert_eq!(result["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(result["data"]["items"][0]["taskId"], first);
    assert_eq!(result["data"]["items"][0]["exitCode"], 130);
    assert_eq!(state(&later_record)["name"], "implementing");
    assert!(!later_record.with_file_name(".advance.lock").exists());
}

#[test]
fn batch_continues_from_primary_after_the_invoking_checkout_is_removed() {
    let repo = Repo::new();
    let (first, _) = repo.task(1, "first", "implementing", true);
    let (progress, progress_record) = repo.task(2, "progress", "implementing", true);
    repo.git(&[
        "-C",
        repo.root.join("progress").to_str().unwrap(),
        "commit",
        "--allow-empty",
        "-m",
        "fixture progress",
    ]);
    // Reproduce a vanished invocation directory without authorizing a real
    // remote merge. The task boundary detects the removed checkout, while
    // batch discovery and the later task must keep using the primary checkout.
    scripts::write_executable(
        repo.root.join("bin/codex"),
        "#!/bin/sh\ncat >/dev/null\n/bin/rm -rf -- \"$RABI_BATCH_FIXTURE/first\"\n",
    )
    .unwrap();
    let mut command = repo.command(
        env!("CARGO_BIN_EXE_rabi"),
        &["task", "advance", "--all", "--json"],
    );
    command.current_dir(repo.root.join("first"));
    let output = repo.run(command);
    assert_eq!(
        output.status.code(),
        Some(4),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["data"]["items"][0]["taskId"], first);
    assert_eq!(result["data"]["items"][0]["exitCode"], 4);
    assert_eq!(result["data"]["items"][1]["taskId"], progress);
    assert_eq!(result["data"]["items"][1]["exitCode"], 0);
    assert_eq!(state(&progress_record)["name"], "prechecking");
}
