//! Fake-Codex integration coverage. No real provider or network is used.
#[path = "common/git.rs"]
mod test_git;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

extern "C" {
    fn kill(pid: i32, signal: i32) -> i32;
}
fn send(pid: u32, signal: i32) {
    unsafe {
        kill(pid as i32, signal);
    }
}

struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    workspace: PathBuf,
    home: PathBuf,
    codex: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-parent-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        let home = root.join("home");
        let codex = root.join("codex-home");
        let workspace = root.join("workspace");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&home).unwrap();
        test_git::write_config(&home);
        let f = Self {
            root,
            repo,
            workspace,
            home,
            codex,
        };
        f.git(&["init", "--template=", "-b", "main"]);
        f.git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ]);
        f.git(&[
            "worktree",
            "add",
            "-b",
            "feat/parent",
            f.workspace.to_str().unwrap(),
        ]);
        fs::create_dir(f.root.join("bin")).unwrap();
        let script = f.root.join("bin/codex");
        fs::write(&script, r#"#!/usr/bin/env python3
import json, os, pathlib, signal, subprocess, sys, time, uuid
args = sys.argv[1:]
root = pathlib.Path(os.environ['FAKE_ROOT'])
with (root/'argv.jsonl').open('a') as f: f.write(json.dumps(args)+'\n')
(root/'input').write_text(sys.stdin.read())
sid = args[args.index('resume')+1] if 'resume' in args else str(uuid.uuid4())
sid = os.environ.get('FAKE_OTHER_ID', sid)
if os.environ.get('FAKE_BLOCK'):
    signal.signal(signal.SIGTERM, signal.SIG_DFL if os.environ.get('FAKE_PARENT_TERM_DEFAULT') else signal.SIG_IGN)
    child = subprocess.Popen([sys.executable, '-c', 'import os,signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); os.write(1,b"ready\\n"); time.sleep(600)'], stdout=subprocess.PIPE)
    assert child.stdout.readline() == b'ready\n'
    (root/'pids').write_text(str(os.getpid())+' '+str(child.pid))
if os.environ.get('FAKE_WAIT_THREAD'):
    (root/'thread-ready').write_text('ready')
    while not (root/'emit-thread').exists(): time.sleep(.01)
if not os.environ.get('FAKE_NO_THREAD'):
    event = json.dumps({'type':'thread.started','thread_id':sid})
    if os.environ.get('FAKE_SPLIT_THREAD'):
        middle = len(event)//2
        print(event[:middle], end='', flush=True)
        (root/'partial-thread').write_text('ready')
        while not (root/'complete-thread').exists(): time.sleep(.01)
        event = event[middle:]
    print(event, end='' if os.environ.get('FAKE_NO_NEWLINE') else '\n', flush=True)
    if os.environ.get('FAKE_SECOND_ID'):
        print(json.dumps({'type':'thread.started','thread_id':'ignored-second-thread'}), flush=True)
    rollout = pathlib.Path(os.environ['CODEX_HOME'])/'sessions/2026/10/25'/('rollout-test-'+sid+'.jsonl')
    rollout.parent.mkdir(parents=True, exist_ok=True)
    rollout.write_text('{}\n')
pathlib.Path(args[args.index('-o')+1]).write_text(os.environ.get('FAKE_ANSWER', 'Parent final answer.\n'))
print('fake stderr', file=sys.stderr, flush=True)
if os.environ.get('FAKE_BLOCK'):
    time.sleep(600)
if os.environ.get('FAKE_WAIT_FINISH'):
    (root/'waiting-finish').write_text('ready')
    while not (root/'finish').exists(): time.sleep(.01)
sys.exit(int(os.environ.get('FAKE_EXIT','0')))
"#).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
        f
    }
    fn git(&self, args: &[&str]) {
        let out = self.command_at(&self.repo, args, "git").output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn command_at(&self, cwd: &Path, args: &[&str], binary: &str) -> Command {
        let mut c = Command::new(binary);
        c.current_dir(cwd)
            .args(args)
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex)
            .env("FAKE_ROOT", &self.root)
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .env("RABI_TRUST_ALL", "1")
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            );
        c
    }
    fn command(&self, args: &[&str]) -> Command {
        self.command_at(&self.workspace, args, env!("CARGO_BIN_EXE_rabi"))
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }
    fn json(&self, args: &[&str]) -> Value {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{:?}: {} {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    }
    fn current(&self) -> PathBuf {
        self.repo.join(".git/rabi/runs/current")
    }
    fn pointer(&self) -> PathBuf {
        fs::read_dir(self.current())
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| !p.to_string_lossy().ends_with(".lock"))
            .unwrap()
    }
    fn run_dir(&self) -> PathBuf {
        self.repo
            .join(".git/rabi/runs")
            .join(fs::read_to_string(self.pointer()).unwrap())
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.run_dir().join("state.json")).unwrap()).unwrap()
    }
    fn calls(&self) -> usize {
        fs::read_to_string(self.root.join("argv.jsonl"))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn assert_unlocked(&self) {
        assert!(!fs::read_dir(self.current()).unwrap().any(|e| e
            .unwrap()
            .path()
            .to_string_lossy()
            .ends_with(".lock")));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn assert_error(out: Output, code: i32, hint: &str) {
    assert_eq!(
        out.status.code(),
        Some(code),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
    .contains(hint));
}

#[test]
fn parent_resumes_same_session_and_fresh_retains_old_record() {
    let f = Fixture::new();
    let skill = "nonexistent/skill `verbatim`\nextra";
    let first = f.json(&["run", "--skill", skill, "initial task", "--json"]);
    assert_eq!(first["run"]["schema"], 2);
    assert_eq!(first["run"]["skill"], skill);
    assert_eq!(first["answer"], "Parent final answer.\n");
    assert_eq!(first["run"]["state"], "idle");
    let dir = f.run_dir();
    let prompt = fs::read_to_string(dir.join("launches/1/prompt.md")).unwrap();
    assert!(prompt.contains(&format!("Follow skill `{skill}`.")));
    assert!(prompt.contains("initial task"));
    let second = f.json(&["run", "--json"]);
    assert_eq!(first["run"]["id"], second["run"]["id"]);
    assert_eq!(first["run"]["sessionId"], second["run"]["sessionId"]);
    assert_eq!(second["run"]["launches"].as_array().unwrap().len(), 2);
    assert_eq!(
        fs::read_to_string(dir.join("launches/2/prompt.md")).unwrap(),
        "Continue."
    );
    let calls: Vec<Value> = fs::read_to_string(f.root.join("argv.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let argv = calls[1].as_array().unwrap();
    let index = argv.iter().position(|a| a == "resume").unwrap();
    assert_eq!(argv[index + 1], first["run"]["sessionId"]);
    assert!(argv.contains(&Value::from("--approve-for-me")));
    assert!(!argv.contains(&Value::from("--sandbox")));
    assert!(argv.contains(&Value::from("gpt-6.1-sol")));
    assert!(argv.contains(&Value::from("model_reasoning_effort=\"high\"")));
    f.json(&["run", "--skill", "new value", "follow up", "--json"]);
    assert_eq!(
        fs::read_to_string(dir.join("launches/3/prompt.md")).unwrap(),
        "Follow skill `new value` from now on.\n\nfollow up"
    );
    let old_bytes = fs::read(dir.join("state.json")).unwrap();
    let fresh = f.json(&["run", "--fresh", "--skill", "new parent", "--json"]);
    assert_ne!(fresh["run"]["id"], first["run"]["id"]);
    assert_ne!(fresh["run"]["sessionId"], first["run"]["sessionId"]);
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), old_bytes);
    assert_eq!(fresh["run"]["launches"].as_array().unwrap().len(), 1);
    f.assert_unlocked();
}

#[test]
fn session_id_is_saved_once_while_first_fresh_and_resumed_parents_are_running() {
    for mode in ["first", "fresh", "resume", "mismatch"] {
        let f = Fixture::new();
        let previous = (mode != "first").then(|| f.json(&["run", "--skill", "x", "--json"]));
        let expected = if mode == "resume" {
            previous.as_ref().unwrap()["run"]["sessionId"]
                .as_str()
                .unwrap()
        } else {
            "mid-launch-thread"
        };
        let args = match mode {
            "first" => vec!["run", "--skill", "x", "--json"],
            "fresh" => vec!["run", "--fresh", "--skill", "x", "--json"],
            _ => vec!["run", "--json"],
        };
        let mut child = Running(Some(
            f.command(&args)
                .env("FAKE_OTHER_ID", expected)
                .env("FAKE_SECOND_ID", "1")
                .env("FAKE_SPLIT_THREAD", "1")
                .env("FAKE_WAIT_FINISH", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        wait_for_file(&f.root.join("partial-thread"));
        let partial = f.state();
        assert!(partial["launches"].as_array().unwrap().last().unwrap()["sessionId"].is_null());
        fs::write(f.root.join("complete-thread"), "continue").unwrap();
        wait_for_file(&f.root.join("waiting-finish"));
        let deadline = Instant::now() + Duration::from_secs(10);
        let state = loop {
            let state = f.state();
            if state["launches"].as_array().unwrap().last().unwrap()["sessionId"] == expected {
                break state;
            }
            assert!(
                Instant::now() < deadline,
                "session id was not saved during {mode}"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        let launch = state["launches"].as_array().unwrap().last().unwrap();
        assert_eq!(state["state"], "running");
        assert!(launch["finishedAt"].is_null());
        assert!(launch["stopReason"].is_null());
        assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
        if mode == "mismatch" {
            assert_eq!(
                state["sessionId"],
                previous.as_ref().unwrap()["run"]["sessionId"]
            );
            assert!(launch["message"]
                .as_str()
                .unwrap()
                .contains("different thread id"));
        } else {
            assert_eq!(state["sessionId"], expected);
        }
        // Atomic writes replace the inode. Further polls and duplicate events
        // must leave the first observation intact until finalization.
        let path = f.run_dir().join("state.json");
        let inode = fs::metadata(&path).unwrap().ino();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        assert_eq!(f.state(), state);
        assert_error(f.run(&["run", "--json"]), 4, "lock");
        fs::write(f.root.join("finish"), "finish").unwrap();
        let out = child.finish();
        assert_eq!(
            out.status.code(),
            Some(if mode == "mismatch" { 1 } else { 0 })
        );
        let finished = f.state();
        assert_eq!(finished["sessionId"], state["sessionId"]);
        assert_eq!(
            finished["launches"].as_array().unwrap().last().unwrap()["sessionId"],
            expected
        );
        f.assert_unlocked();
    }
}

#[test]
fn final_unterminated_thread_event_still_records_a_session() {
    let f = Fixture::new();
    let out = f
        .command(&["run", "--skill", "x", "--json"])
        .env("FAKE_NO_NEWLINE", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let state = f.state();
    assert!(state["sessionId"].is_string());
    assert_eq!(state["launches"][0]["sessionId"], state["sessionId"]);
    f.assert_unlocked();
}

#[test]
fn mid_launch_session_save_failure_stops_and_reaps_the_child_group() {
    let f = Fixture::new();
    let child = Running(Some(
        f.command(&["run", "--skill", "x", "--json"])
            .env("FAKE_WAIT_THREAD", "1")
            .env("FAKE_BLOCK", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let pids = wait_for_file(&f.root.join("pids"));
    wait_for_file(&f.root.join("thread-ready"));
    let state = f.run_dir().join("state.json");
    fs::remove_file(&state).unwrap();
    fs::create_dir(&state).unwrap();
    fs::write(f.root.join("emit-thread"), "continue").unwrap();
    assert_error(child.finish(), 1, "Failed to persist Codex session id");
    for pid in pids.split_whitespace() {
        assert_dead(pid.parse().unwrap());
    }
    // A reaped direct child has no pid, even on systems that retain orphan
    // grandchildren as zombies. Group::drop must wait before releasing lock.
    let parent: i32 = pids.split_whitespace().next().unwrap().parse().unwrap();
    assert_eq!(unsafe { kill(parent, 0) }, -1);
    f.assert_unlocked();
}

#[test]
fn validation_trust_stdin_and_removed_commands() {
    let f = Fixture::new();
    assert_error(f.run(&["run"]), 2, "--skill");
    assert_error(f.run(&["run", "--fresh"]), 2, "--skill");
    for duration in ["0s", "1", "-1m", "10d", "999999999999999999999h"] {
        assert_error(
            f.run(&["run", "--skill", "x", "--timeout", duration]),
            2,
            "--timeout",
        );
    }
    assert_error(
        f.command_at(
            &f.repo,
            &["run", "--skill", "x"],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap(),
        2,
        "rabi create",
    );
    assert_error(
        f.command(&["run", "--skill", "x", "--json"])
            .env("RABI_TRUST_ALL", "0")
            .output()
            .unwrap(),
        4,
        "untrusted",
    );
    for args in [
        vec!["workflow"],
        vec!["workflow", "list"],
        vec!["workflow", "show"],
        vec!["workflow", "check"],
        vec!["run", "start"],
        vec!["run", "show"],
        vec!["run", "--provider", "codex"],
        vec!["run", "a", "b"],
        vec!["run", "-R", "."],
    ] {
        assert_error(f.run(&args), 2, "");
    }
    assert_eq!(f.calls(), 0);
    let mut child = f
        .command(&["run", "--skill", "x", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"task from stdin\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "Parent final answer.\n"
    );
    assert!(fs::read_to_string(f.root.join("input"))
        .unwrap()
        .contains("task from stdin\n"));
    assert_eq!(
        fs::read_to_string(f.run_dir().join("launches/1/stderr.log")).unwrap(),
        "fake stderr\n"
    );
    f.assert_unlocked();
}

#[test]
fn untrusted_run_does_not_execute_fsmonitor() {
    let f = Fixture::new();
    let monitor = f.root.join("bin/monitor");
    fs::write(&monitor, "#!/usr/bin/env python3\nimport os, pathlib\n(pathlib.Path(os.environ['FAKE_ROOT'])/'fsmonitor-ran').touch()\n").unwrap();
    fs::set_permissions(&monitor, fs::Permissions::from_mode(0o755)).unwrap();
    f.git(&["config", "core.fsmonitor", monitor.to_str().unwrap()]);
    for args in [
        vec!["run", "--skill", "x"],
        vec!["run", "--skill", "x", "--json"],
    ] {
        assert_error(
            f.command(&args)
                .env("RABI_TRUST_ALL", "0")
                .output()
                .unwrap(),
            4,
            "trust",
        );
        assert!(!f.root.join("fsmonitor-ran").exists());
        assert_eq!(f.calls(), 0);
    }
    f.json(&["run", "--skill", "x", "--json"]);
    assert!(
        f.root.join("fsmonitor-ran").exists(),
        "positive fsmonitor control"
    );
}

#[test]
fn text_answer_has_newline_and_progress_is_only_on_text_stderr() {
    let f = Fixture::new();
    for answer in ["answer without newline", "answer with newline\n", ""] {
        let out = f
            .command(&["run", "--fresh", "--skill", "x"])
            .env("FAKE_ANSWER", answer)
            .output()
            .unwrap();
        assert!(out.status.success());
        let expected = if answer.ends_with('\n') {
            answer.to_owned()
        } else {
            format!("{answer}\n")
        };
        assert_eq!(out.stdout, expected.as_bytes());
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert_eq!(stderr.lines().count(), 1);
        assert!(stderr.contains(&format!("rabi: running parent session (run {}) in {}. Ctrl-C stops it; `rabi run \"...\"` resumes.", f.state()["id"].as_str().unwrap(), f.workspace.display())));
    }
    let out = f
        .command(&["run", "--json"])
        .env("FAKE_ANSWER", "no newline")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(out.stderr.is_empty());
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["answer"], "no newline");
}

#[test]
fn resume_skips_incomplete_launch_directories() {
    let f = Fixture::new();
    let first = f.json(&["run", "--skill", "x", "--json"]);
    let leftover = f.run_dir().join("launches/2");
    fs::create_dir(&leftover).unwrap();
    fs::write(leftover.join("prompt.md"), "incomplete previous attempt").unwrap();
    let resumed = f.json(&["run", "--json"]);
    assert_eq!(resumed["run"]["id"], first["run"]["id"]);
    assert_eq!(resumed["run"]["sessionId"], first["run"]["sessionId"]);
    assert_eq!(resumed["launch"]["n"], 3);
    assert_eq!(
        fs::read_to_string(leftover.join("prompt.md")).unwrap(),
        "incomplete previous attempt"
    );
    assert_eq!(f.json(&["run", "--json"])["launch"]["n"], 4);
}

#[test]
fn branch_changes_resume_same_checkout_and_cannot_bypass_lock() {
    let f = Fixture::new();
    let first = f.json(&["run", "--skill", "x", "--json"]);
    let pointer = f.pointer();
    let out = f
        .command_at(&f.workspace, &["switch", "-c", "renamed"], "git")
        .output()
        .unwrap();
    assert!(out.status.success());
    let resumed = f.json(&["run", "--json"]);
    assert_eq!(resumed["run"]["id"], first["run"]["id"]);
    assert_eq!(resumed["run"]["sessionId"], first["run"]["sessionId"]);
    assert_eq!(f.pointer(), pointer);
    let child = running(&f, &["run", "--json"]);
    wait_for_file(&f.root.join("pids"));
    let out = f
        .command_at(&f.workspace, &["switch", "--detach"], "git")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_error(
        f.run(&["run", "--skill", "cannot bypass", "--json"]),
        4,
        "lock",
    );
    send(child.0.as_ref().unwrap().id(), 2);
    assert_eq!(child.finish().status.code(), Some(130));
    let out = f
        .command_at(
            &f.workspace,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "detached commit",
            ],
            "git",
        )
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        f.json(&["run", "--json"])["run"]["sessionId"],
        first["run"]["sessionId"]
    );
}

#[test]
fn detached_worktrees_at_same_commit_have_distinct_sessions_and_locks() {
    let f = Fixture::new();
    let out = f
        .command_at(&f.workspace, &["switch", "--detach"], "git")
        .output()
        .unwrap();
    assert!(out.status.success());
    let first = f.json(&["run", "--skill", "x", "--json"]);
    let second = f.root.join("second");
    f.git(&[
        "worktree",
        "add",
        "--detach",
        second.to_str().unwrap(),
        "main",
    ]);
    let child = running(&f, &["run", "--json"]);
    wait_for_file(&f.root.join("pids"));
    let out = f
        .command(&["run", "--skill", "y", "--json"])
        .current_dir(&second)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let other: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        first["run"]["workspace"]["name"],
        other["data"]["run"]["workspace"]["name"]
    );
    assert_ne!(first["run"]["id"], other["data"]["run"]["id"]);
    assert_ne!(first["run"]["sessionId"], other["data"]["run"]["sessionId"]);
    send(child.0.as_ref().unwrap().id(), 2);
    assert_eq!(child.finish().status.code(), Some(130));
    assert_eq!(
        f.json(&["run", "--json"])["run"]["sessionId"],
        first["run"]["sessionId"]
    );
}

#[test]
fn broken_session_never_launches_and_mismatched_thread_is_recorded() {
    let f = Fixture::new();
    f.json(&["run", "--skill", "x", "--json"]);
    let dir = f.run_dir();
    let original = f.state();
    for mode in [
        "missing-id",
        "invalid-json",
        "missing-file",
        "wrong-schema",
        "invalid-state",
        "missing-rollout",
    ] {
        fs::write(
            dir.join("state.json"),
            serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        match mode {
            "missing-id" => {
                let mut state = original.clone();
                state["sessionId"] = Value::Null;
                fs::write(dir.join("state.json"), state.to_string()).unwrap();
            }
            "invalid-json" => fs::write(dir.join("state.json"), "{").unwrap(),
            "missing-file" => fs::remove_file(dir.join("state.json")).unwrap(),
            "wrong-schema" => {
                let mut state = original.clone();
                state["schema"] = Value::from(1);
                fs::write(dir.join("state.json"), state.to_string()).unwrap();
            }
            "invalid-state" => {
                let mut state = original.clone();
                state["state"] = Value::from("nonsense");
                fs::write(dir.join("state.json"), state.to_string()).unwrap();
            }
            _ => fs::remove_dir_all(f.codex.join("sessions")).unwrap(),
        }
        assert_error(f.run(&["run", "--json"]), 1, "--fresh");
        assert_eq!(f.calls(), 1);
        f.assert_unlocked();
    }
    // Fresh bypasses broken state, and a mismatched resume never replaces its id.
    let fresh = f.json(&["run", "--fresh", "--skill", "x", "--json"]);
    let out = f
        .command(&["run", "--json"])
        .env("FAKE_OTHER_ID", "different-thread")
        .output()
        .unwrap();
    assert_error(out, 1, "different thread id");
    let state = f.state();
    assert_eq!(state["sessionId"], fresh["run"]["sessionId"]);
    assert_eq!(state["state"], "error");
    assert_eq!(state["launches"][1]["sessionId"], "different-thread");
    f.assert_unlocked();
}

#[test]
fn parent_failure_and_spawn_or_protocol_errors_preserve_results_and_release_lock() {
    for (mode, expected) in [("exit", 42), ("protocol", 1), ("spawn", 1)] {
        let f = Fixture::new();
        let mut c = f.command(&["run", "--skill", "x", "--json"]);
        match mode {
            "exit" => {
                c.env("FAKE_EXIT", "42");
            }
            "protocol" => {
                c.env("FAKE_NO_THREAD", "1");
            }
            _ => {
                fs::write(
                    f.root.join("bin/codex"),
                    "#!/nonexistent/codex-interpreter\n",
                )
                .unwrap();
            }
        }
        let out = c.output().unwrap();
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["ok"], false);
        assert!(!doc["data"]["launch"].is_null());
        assert!(f.state()["launches"][0]["finishedAt"].is_string());
        f.assert_unlocked();
    }
}

fn wait_for_file(path: &Path) -> String {
    let start = Instant::now();
    loop {
        if let Ok(s) = fs::read_to_string(path) {
            if !s.is_empty() {
                return s;
            }
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
struct Running(Option<Child>);
impl Running {
    fn finish(mut self) -> Output {
        let start = Instant::now();
        let child = self.0.as_mut().unwrap();
        while child.try_wait().unwrap().is_none() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "launcher did not exit"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for Running {
    fn drop(&mut self) {
        if let Some(child) = self.0.as_mut() {
            send(child.id(), 15);
            let start = Instant::now();
            while matches!(child.try_wait(), Ok(None)) && start.elapsed() < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn running(f: &Fixture, args: &[&str]) -> Running {
    Running(Some(
        f.command(args)
            .env("FAKE_BLOCK", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ))
}
fn assert_dead(pid: u32) {
    // A killed orphan can remain a zombie until the container init reaps it.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let exists = unsafe { kill(pid as i32, 0) } == 0;
        #[cfg(target_os = "linux")]
        let zombie = fs::read_to_string(format!("/proc/{pid}/stat"))
            .ok()
            .is_some_and(|s| {
                s.split(')')
                    .nth(1)
                    .unwrap_or("")
                    .trim_start()
                    .starts_with('Z')
            });
        #[cfg(not(target_os = "linux"))]
        let zombie = false;
        if !exists || zombie {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "process {pid} survived group termination"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn timeout_and_interrupt_kill_group_release_lock_and_allow_resume() {
    for signal in [None, Some(2), Some(15)] {
        let f = Fixture::new();
        let args = if signal.is_none() {
            vec!["run", "--skill", "x", "--timeout", "2s", "--json"]
        } else {
            vec!["run", "--skill", "x", "--json"]
        };
        let child = running(&f, &args);
        let pids = wait_for_file(&f.root.join("pids"));
        if let Some(signal) = signal {
            send(child.0.as_ref().unwrap().id(), signal);
        }
        let out = child.finish();
        assert_eq!(
            out.status.code(),
            Some(if signal.is_none() { 124 } else { 130 }),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        for pid in pids.split_whitespace() {
            assert_dead(pid.parse().unwrap());
        }
        let state = f.state();
        assert_eq!(state["state"], "stopped");
        assert_eq!(
            state["launches"][0]["stopReason"],
            if signal.is_none() {
                "timed_out"
            } else {
                "interrupted"
            }
        );
        assert!(state["sessionId"].is_string());
        f.assert_unlocked();
        assert_eq!(f.json(&["run", "--json"])["run"]["id"], state["id"]);
    }
}
#[test]
fn concurrent_run_refused_and_dead_pid_lock_taken_over() {
    let f = Fixture::new();
    let child = running(&f, &["run", "--skill", "x", "--json"]);
    wait_for_file(&f.root.join("pids"));
    for _ in 0..2 {
        assert_error(f.run(&["run", "--skill", "second", "--json"]), 4, "lock");
    }
    assert_eq!(f.calls(), 1);
    send(child.0.as_ref().unwrap().id(), 2);
    assert_eq!(child.finish().status.code(), Some(130));
    f.assert_unlocked();
    let lock = f.pointer().with_file_name(format!(
        "{}.lock",
        f.pointer().file_name().unwrap().to_string_lossy()
    ));
    let dead = Command::new("true").spawn().unwrap();
    let pid = dead.id();
    let mut dead = dead;
    dead.wait().unwrap();
    fs::write(&lock, format!("{pid}\n")).unwrap();
    f.json(&["run", "--json"]);
    f.assert_unlocked();
    for contents in [
        format!("{}\n", std::process::id()),
        format!("{pid} 0\n"), // Launcher died before the spawned group could be recorded.
        "unverifiable".into(),
    ] {
        fs::write(&lock, &contents).unwrap();
        assert_error(f.run(&["run", "--json"]), 4, "lock");
        assert_eq!(fs::read_to_string(&lock).unwrap(), contents);
    }
    fs::remove_file(lock).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn killed_launcher_keeps_lock_until_codex_process_group_is_gone() {
    let f = Fixture::new();
    // A separate subreaper owns only this test's orphaned processes. Reaping
    // them explicitly makes kill(-pgid, 0) independent of container PID 1.
    let script = r#"
import ctypes, json, os, pathlib, signal, subprocess, sys, time
assert ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) == 0
root = pathlib.Path(os.environ['FAKE_ROOT'])
current = pathlib.Path(sys.argv[2])
def until(probe):
    deadline = time.monotonic() + 10
    while True:
        result = probe()
        if result:
            return result
        assert time.monotonic() < deadline, 'timed out waiting for process handshake'
        time.sleep(.01)
env = dict(os.environ, FAKE_BLOCK='1')
launcher = subprocess.Popen([sys.argv[1], 'run', '--skill', 'x', '--json'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
pgid = None
try:
    pids = until(lambda: (root/'pids').read_text().split() if (root/'pids').exists() else None)
    pgid = int(pids[0])
    lock = next(current.glob('*.lock'))
    until(lambda: lock.read_text().split() == [str(launcher.pid), str(pgid)])
    pointer = next(path for path in current.iterdir() if path.suffix != '.lock')
    state_path = current.parent/pointer.read_text()/'state.json'
    first = until(lambda: (state if state['sessionId'] and state['launches'][0]['sessionId'] else None) if (state := json.loads(state_path.read_text())) else None)
    assert first['state'] == 'running'
    assert first['launches'][0]['finishedAt'] is None
    launcher.kill()
    launcher.communicate(timeout=10)
    assert launcher.returncode == -signal.SIGKILL
    os.killpg(pgid, 0)
    second = subprocess.run([sys.argv[1], 'run', '--json'], capture_output=True, timeout=10)
    assert second.returncode == 4, (second.stdout, second.stderr)
    assert b'lock' in second.stdout
    # Reap the group leader first: the lock must also protect surviving
    # descendants when the original Codex pid itself no longer exists.
    os.kill(pgid, signal.SIGKILL)
    assert os.waitpid(pgid, 0)[0] == pgid
    os.killpg(pgid, 0)
    second = subprocess.run([sys.argv[1], 'run', '--json'], capture_output=True, timeout=10)
    assert second.returncode == 4, (second.stdout, second.stderr)
finally:
    if launcher.poll() is None:
        launcher.kill()
        launcher.communicate(timeout=10)
    if pgid is not None:
        try: os.killpg(pgid, signal.SIGKILL)
        except ProcessLookupError: pass
        for pid in pids:
            try: os.waitpid(int(pid), 0)
            except ChildProcessError: pass
    try:
        os.killpg(pgid, 0)
        raise AssertionError('group survived reaping')
    except ProcessLookupError: pass
resumed = subprocess.run([sys.argv[1], 'run', '--json'], capture_output=True, timeout=10)
assert resumed.returncode == 0, (resumed.stdout, resumed.stderr)
data = json.loads(resumed.stdout)['data']
assert data['run']['id'] == first['id']
assert data['run']['sessionId'] == first['sessionId']
calls = [json.loads(line) for line in (root/'argv.jsonl').read_text().splitlines()]
assert calls[-1][calls[-1].index('resume')+1] == first['sessionId']
print(resumed.stdout.decode(), end='')
"#;
    let out = f
        .command_at(
            &f.workspace,
            &[
                "-c",
                script,
                env!("CARGO_BIN_EXE_rabi"),
                f.current().to_str().unwrap(),
            ],
            "python3",
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(doc["data"]["run"]["sessionId"].is_string());
    assert_eq!(doc["data"]["run"]["launches"].as_array().unwrap().len(), 2);
    assert_eq!(f.calls(), 2);
    f.assert_unlocked();
}
#[test]
fn skill_list_orders_sources_and_names_and_keeps_duplicates_without_parsing() {
    let f = Fixture::new();
    let sources = [
        (f.workspace.join(".agents/skills"), "checkout:.agents"),
        (f.workspace.join(".codex/skills"), "checkout:.codex"),
        (f.workspace.join(".claude/skills"), "checkout:.claude"),
        (f.home.join(".agents/skills"), "user:.agents"),
        (f.codex.join("skills"), "user:.codex"),
        (f.home.join(".claude/skills"), "user:.claude"),
    ];
    assert_eq!(
        f.json(&["skill", "list", "--json"])["skills"],
        serde_json::json!([])
    );
    for (root, _) in &sources {
        for name in ["z", "a"] {
            fs::create_dir_all(root.join(name)).unwrap();
            fs::write(root.join(name).join("SKILL.md"), "--- invalid frontmatter").unwrap();
        }
        fs::create_dir(root.join("empty")).unwrap();
    }
    let data = f.json(&["skill", "list", "--json"]);
    let skills = data["skills"].as_array().unwrap();
    assert_eq!(skills.len(), 12);
    for (i, (root, source)) in sources.iter().enumerate() {
        for (j, name) in ["a", "z"].iter().enumerate() {
            let s = &skills[i * 2 + j];
            assert_eq!(s["name"], *name);
            assert_eq!(s["source"], *source);
            assert_eq!(s["path"], root.join(name).to_str().unwrap());
        }
    }
    let text = f.run(&["skill", "list"]);
    assert!(text.status.success());
    let expected = skills
        .iter()
        .map(|s| {
            format!(
                "{}\t{}\t{}\n",
                s["name"].as_str().unwrap(),
                s["source"].as_str().unwrap(),
                s["path"].as_str().unwrap()
            )
        })
        .collect::<String>();
    assert_eq!(String::from_utf8(text.stdout).unwrap(), expected);
    // CODEX_HOME unset falls back to ~/.codex; a nested cwd uses checkout root.
    fs::create_dir_all(f.home.join(".codex/skills/fallback")).unwrap();
    fs::write(f.home.join(".codex/skills/fallback/SKILL.md"), "x").unwrap();
    let nested = f.workspace.join("nested");
    fs::create_dir(&nested).unwrap();
    let out = f
        .command(&["skill", "list", "--json"])
        .current_dir(nested)
        .env_remove("CODEX_HOME")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("fallback"));
    assert_eq!(f.calls(), 0);
}

#[test]
fn different_workspaces_keep_distinct_parents_and_long_names_fit_pointer_filenames() {
    let f = Fixture::new();
    let first = f.json(&["run", "--skill", "x", "--json"]);
    let pointer = f.pointer();
    let second = f.root.join("second");
    let branch = format!("feat/{}", "x".repeat(180));
    f.git(&["worktree", "add", "-b", &branch, second.to_str().unwrap()]);
    let out = f
        .command(&["run", "--skill", "y", "--json"])
        .current_dir(&second)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let second: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_ne!(
        first["run"]["sessionId"],
        second["data"]["run"]["sessionId"]
    );
    assert_eq!(
        fs::read_to_string(pointer).unwrap(),
        first["run"]["id"].as_str().unwrap()
    );
    assert_eq!(fs::read_dir(f.current()).unwrap().count(), 2);
    for entry in fs::read_dir(f.current()).unwrap() {
        assert_eq!(entry.unwrap().file_name().len(), 64);
    }
    assert_eq!(
        f.json(&["run", "--json"])["run"]["sessionId"],
        first["run"]["sessionId"]
    );
}

#[test]
fn old_records_and_user_workflows_are_untouched_and_trust_lists_only_hooks() {
    let f = Fixture::new();
    let old = f
        .repo
        .join(".git/rabi/runs")
        .join(format!("run_{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&old).unwrap();
    fs::write(old.join("run.json"), "old schema 1 record").unwrap();
    let workflows = f.workspace.join(".rabi/workflows");
    fs::create_dir_all(&workflows).unwrap();
    fs::write(workflows.join("user.yaml"), "invalid yaml from old engine").unwrap();
    let trusted = f.command(&["trust", "add", "--json"]).output().unwrap();
    assert!(trusted.status.success());
    assert!(!String::from_utf8_lossy(&trusted.stderr).contains("workflow"));
    let trusted: Value = serde_json::from_slice(&trusted.stdout).unwrap();
    let keys = trusted["data"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(keys, vec!["path", "trustedAt"]);
    // Recorded trust also works without the environment bypass.
    let out = f
        .command(&["run", "--skill", "x", "--json"])
        .env("RABI_TRUST_ALL", "0")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(
        fs::read_to_string(old.join("run.json")).unwrap(),
        "old schema 1 record"
    );
    assert_eq!(
        fs::read_to_string(workflows.join("user.yaml")).unwrap(),
        "invalid yaml from old engine"
    );
}

#[test]
fn group_escalation_reaches_grandchild_after_parent_exits_on_term() {
    let f = Fixture::new();
    let child = Running(Some(
        f.command(&["run", "--skill", "x", "--json"])
            .env("FAKE_BLOCK", "1")
            .env("FAKE_PARENT_TERM_DEFAULT", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let pids = wait_for_file(&f.root.join("pids"));
    send(child.0.as_ref().unwrap().id(), 2);
    assert_eq!(child.finish().status.code(), Some(130));
    for pid in pids.split_whitespace() {
        assert_dead(pid.parse().unwrap());
    }
    assert_eq!(f.state()["launches"][0]["exitCode"], 143);
    f.assert_unlocked();
}

#[test]
fn nonzero_parent_statuses_pass_through_without_rabi_partial_semantics() {
    for status in [1, 2, 4, 6, 42, 130] {
        let f = Fixture::new();
        let out = f
            .command(&["run", "--skill", "x", "--json"])
            .env("FAKE_EXIT", status.to_string())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(status));
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"]["code"], "command_failed");
        assert_eq!(doc["data"]["launch"]["exitCode"], status);
    }
}

#[test]
fn unremovable_stale_lock_reports_error_without_spinning() {
    let f = Fixture::new();
    f.json(&["run", "--skill", "x", "--json"]);
    let pointer = f.pointer();
    let lock = pointer.with_file_name(format!(
        "{}.lock",
        pointer.file_name().unwrap().to_string_lossy()
    ));
    let mut dead = Command::new("true").spawn().unwrap();
    let pid = dead.id();
    dead.wait().unwrap();
    fs::write(&lock, pid.to_string()).unwrap();
    fs::set_permissions(f.current(), fs::Permissions::from_mode(0o500)).unwrap();
    let child = Running(Some(
        f.command(&["run", "--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let out = child.finish();
    fs::set_permissions(f.current(), fs::Permissions::from_mode(0o700)).unwrap();
    assert_error(out, 1, "Failed to remove stale run lock");
    assert_eq!(f.calls(), 1);
}

#[test]
fn new_help_and_completions_come_from_the_contract_without_retired_commands() {
    let f = Fixture::new();
    for shell in ["bash", "zsh", "fish"] {
        let out = f.run(&["completions", shell]);
        assert!(out.status.success());
        let script = String::from_utf8(out.stdout).unwrap();
        for expected in ["--skill", "--fresh", "--timeout", "skill list"] {
            assert!(script.contains(expected), "{shell}: {expected}");
        }
        for removed in ["workflow", "run start", "run show"] {
            assert!(!script.contains(removed), "{shell}: {removed}");
        }
    }
    let out = f.run(&["run", "--help"]);
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("[TASK | -]"));
    assert_eq!(f.calls(), 0);
}
