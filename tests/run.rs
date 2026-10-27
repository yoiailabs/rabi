//! Fake-provider integration coverage. No real provider or network is used.
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
    fn getpgrp() -> i32;
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
        let root = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("rabi-parent-{}", uuid::Uuid::new_v4()));
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
(root/'cwd').write_text(os.getcwd())
sid = args[args.index('resume')+1] if 'resume' in args else str(uuid.uuid4())
sid = os.environ.get('FAKE_OTHER_ID', sid)
if os.environ.get('FAKE_BLOCK'):
    signal.signal(signal.SIGTERM, signal.SIG_DFL if os.environ.get('FAKE_PARENT_TERM_DEFAULT') else signal.SIG_IGN)
    if os.environ.get('FAKE_TERM_STATE'):
        def capture_term_state(signum, frame):
            (root/'term-state.json').write_bytes(pathlib.Path(os.environ['FAKE_TERM_STATE']).read_bytes())
        signal.signal(signal.SIGTERM, capture_term_state)
    child = subprocess.Popen([sys.executable, '-c', 'import os,signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); os.write(1,b"ready\\n"); time.sleep(600)'], stdout=subprocess.PIPE)
    assert child.stdout.readline() == b'ready\n'
    (root/'pids').write_text(str(os.getpid())+' '+str(child.pid))
if os.environ.get('FAKE_WAIT_THREAD'):
    (root/'thread-ready').write_text('ready')
    while not (root/'emit-thread').exists(): time.sleep(.01)
if os.environ.get('FAKE_SESSION_DELAY'):
    time.sleep(float(os.environ['FAKE_SESSION_DELAY']))
if os.environ.get('FAKE_BAD_BYTES'):
    os.write(sys.stdout.fileno(), b'\xff\n')
if os.environ.get('FAKE_NOISE'):
    chunk = b'{"type":"noise"}\n' * 4096 if os.environ['FAKE_NOISE'] == 'json' else b'x' * 65536
    # Bound the fixture even if cancellation regresses; the reader unit test
    # supplies truly endless output to prove its per-poll limit deterministically.
    deadline = time.monotonic() + 5
    written = 0
    while written < 16 * 1024 * 1024 and time.monotonic() < deadline:
        written += os.write(sys.stdout.fileno(), chunk)
        if written >= 256 * 1024 and not (root/'noise-ready').exists():
            (root/'noise-ready').write_text('ready')
    time.sleep(600)
if not os.environ.get('FAKE_NO_THREAD'):
    event = {'type':'thread.started','thread_id':sid}
    if os.environ.get('FAKE_PADDED_THREAD'):
        event['padding'] = 'x' * 131072
    event = json.dumps(event)
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
    (root/'session-ready').write_text(str(os.getpid()))
pathlib.Path(args[args.index('-o')+1]).write_text(os.environ.get('FAKE_ANSWER', 'Parent final answer.\n'))
print('fake stderr', file=sys.stderr, flush=True)
if os.environ.get('FAKE_BLOCK'):
    time.sleep(600)
if os.environ.get('FAKE_WAIT_FINISH'):
    (root/'waiting-finish').write_text('ready')
    while not (root/'finish').exists(): time.sleep(.01)
(root/'completed').write_text('done')
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
        if mode == "mismatch" {
            fs::remove_file(f.root.join("completed")).unwrap();
        }
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
        if mode != "mismatch" {
            wait_for_file(&f.root.join("waiting-finish"));
        }
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
        if mode == "mismatch" {
            assert_eq!(
                state["sessionId"],
                previous.as_ref().unwrap()["run"]["sessionId"]
            );
            assert!(launch["message"]
                .as_str()
                .unwrap()
                .contains("different thread id"));
            assert_error(child.finish(), 1, "different thread id");
            let finished = f.state();
            assert_eq!(finished["sessionId"], state["sessionId"]);
            assert_eq!(finished["state"], "error");
            assert_eq!(
                finished["launches"].as_array().unwrap().last().unwrap()["sessionId"],
                expected
            );
            assert!(!f.root.join("completed").exists());
            f.assert_unlocked();
            continue;
        }
        assert_eq!(state["state"], "running");
        assert!(launch["finishedAt"].is_null());
        assert!(launch["stopReason"].is_null());
        assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
        assert_eq!(state["sessionId"], expected);
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
        assert_eq!(out.status.code(), Some(0));
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
fn mid_launch_session_mismatch_stops_and_reaps_the_child_group() {
    let f = Fixture::new();
    let first = f.json(&["run", "--skill", "x", "--json"]);
    fs::remove_file(f.root.join("completed")).unwrap();
    let child = Running(Some(
        f.command(&["run", "--timeout", "5s", "--json"])
            .env("FAKE_OTHER_ID", "different-thread")
            .env("FAKE_WAIT_THREAD", "1")
            .env("FAKE_BLOCK", "1")
            .env("FAKE_TERM_STATE", f.run_dir().join("state.json"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let pids = wait_for_file(&f.root.join("pids"));
    wait_for_file(&f.root.join("thread-ready"));
    let start = Instant::now();
    fs::write(f.root.join("emit-thread"), "continue").unwrap();
    let out = child.finish();
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "mismatched parent was left running until the launch timeout"
    );
    assert_error(out, 1, "different thread id (different-thread); expected");
    // TERM captures the durable mid-launch state before controlled stopping
    // escalates to KILL and the launcher writes its final state.
    let saved: Value =
        serde_json::from_slice(&fs::read(f.root.join("term-state.json")).unwrap()).unwrap();
    assert_eq!(saved["state"], "running");
    assert_eq!(saved["sessionId"], first["run"]["sessionId"]);
    assert_eq!(saved["launches"][1]["sessionId"], "different-thread");
    assert!(saved["launches"][1]["finishedAt"].is_null());
    assert!(saved["launches"][1]["stopReason"].is_null());
    for pid in pids.split_whitespace() {
        assert_dead(pid.parse().unwrap());
    }
    let parent: i32 = pids.split_whitespace().next().unwrap().parse().unwrap();
    assert_eq!(unsafe { kill(parent, 0) }, -1);
    assert!(!f.root.join("completed").exists());
    let state = f.state();
    assert_eq!(state["state"], "error");
    assert_eq!(state["sessionId"], first["run"]["sessionId"]);
    let launch = &state["launches"][1];
    assert_eq!(launch["sessionId"], "different-thread");
    assert_eq!(launch["stopReason"], "error");
    assert_eq!(launch["exitCode"], 137);
    assert!(launch["message"].as_str().unwrap().contains(
        "The recorded parent session was retained. Use `rabi run --fresh --skill <value>` to start over."
    ));
    f.assert_unlocked();
}

#[test]
fn mid_launch_session_save_failure_stops_and_reaps_the_child_group() {
    for mismatch in [false, true] {
        let f = Fixture::new();
        let mut command = if mismatch {
            f.json(&["run", "--skill", "x", "--json"]);
            let mut command = f.command(&["run", "--json"]);
            command.env("FAKE_OTHER_ID", "different-thread");
            command
        } else {
            f.command(&["run", "--skill", "x", "--json"])
        };
        let child = Running(Some(
            command
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
}

#[test]
fn malformed_event_bytes_do_not_stop_parent_or_replace_its_failure_status() {
    for parent_exit in [0, 42] {
        let f = Fixture::new();
        let mut child = Running(Some(
            f.command(&["run", "--skill", "x", "--json"])
                .env("FAKE_BAD_BYTES", "1")
                .env("FAKE_WAIT_FINISH", "1")
                .env("FAKE_EXIT", parent_exit.to_string())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        wait_for_file(&f.root.join("waiting-finish"));
        std::thread::sleep(Duration::from_millis(100));
        assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
        assert!(f.state()["sessionId"].is_null());
        fs::write(f.root.join("finish"), "finish").unwrap();
        let out = child.finish();
        assert_eq!(
            out.status.code(),
            Some(if parent_exit == 0 { 1 } else { parent_exit })
        );
        assert_eq!(
            fs::read_to_string(f.root.join("completed")).unwrap(),
            "done"
        );
        let state = f.state();
        assert_eq!(state["launches"][0]["exitCode"], parent_exit);
        assert_eq!(
            state["launches"][0]["stopReason"],
            if parent_exit == 0 {
                "error"
            } else {
                "parent_failed"
            }
        );
        assert!(state["sessionId"].is_null());
        f.assert_unlocked();
    }
}

#[test]
fn oversized_first_thread_event_defers_observation_without_trusting_later_ids() {
    let f = Fixture::new();
    let mut child = Running(Some(
        f.command(&["run", "--skill", "x", "--json"])
            .env("FAKE_OTHER_ID", "first-padded-thread")
            .env("FAKE_PADDED_THREAD", "1")
            .env("FAKE_SECOND_ID", "1")
            .env("FAKE_WAIT_FINISH", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    wait_for_file(&f.root.join("waiting-finish"));
    std::thread::sleep(Duration::from_millis(100));
    assert!(child.0.as_mut().unwrap().try_wait().unwrap().is_none());
    let state = f.state();
    assert!(state["sessionId"].is_null());
    assert!(state["launches"][0]["sessionId"].is_null());
    fs::write(f.root.join("finish"), "finish").unwrap();
    let out = child.finish();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let state = f.state();
    assert_eq!(state["sessionId"], "first-padded-thread");
    assert_eq!(state["launches"][0]["sessionId"], "first-padded-thread");
    f.assert_unlocked();
}

#[test]
fn noisy_output_does_not_delay_timeout_or_interrupt() {
    for noise in ["json", "unterminated"] {
        for interrupt in [false, true] {
            let f = Fixture::new();
            let timeout = if interrupt { "30s" } else { "1s" };
            let child = Running(Some(
                f.command(&["run", "--skill", "x", "--timeout", timeout, "--json"])
                    .env("FAKE_BLOCK", "1")
                    .env("FAKE_NOISE", noise)
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap(),
            ));
            let pids = wait_for_file(&f.root.join("pids"));
            wait_for_file(&f.root.join("noise-ready"));
            let start = Instant::now();
            if interrupt {
                send(child.0.as_ref().unwrap().id(), 2);
            }
            let out = child.finish();
            assert_eq!(
                out.status.code(),
                Some(if interrupt { 130 } else { 124 }),
                "{}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "{noise} delayed cancellation"
            );
            for pid in pids.split_whitespace() {
                assert_dead(pid.parse().unwrap());
            }
            let state = f.state();
            assert_eq!(state["state"], "stopped");
            assert_eq!(
                state["launches"][0]["stopReason"],
                if interrupt {
                    "interrupted"
                } else {
                    "timed_out"
                }
            );
            assert!(state["sessionId"].is_null());
            f.assert_unlocked();
        }
    }
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
        vec!["run", "--provider", "unknown", "--skill", "x"],
        vec!["run", "--provider", "Cursor", "--skill", "x"],
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
    f.json(&["list", "--json"]);
    assert!(
        f.root.join("fsmonitor-ran").exists(),
        "positive fsmonitor control"
    );
}

#[test]
fn text_answer_has_newline_and_progress_is_only_on_text_stderr() {
    let f = Fixture::new();
    f.record_workspace();
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
    let pids = wait_for_file(&f.root.join("pids"));
    wait_for_session(&f, &pids);
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
    let pids = wait_for_file(&f.root.join("pids"));
    wait_for_session(&f, &pids);
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
    fs::remove_file(f.root.join("completed")).unwrap();
    let out = f
        .command(&["run", "--json"])
        .env("FAKE_OTHER_ID", "different-thread")
        .env("FAKE_NO_NEWLINE", "1")
        .output()
        .unwrap();
    assert_error(out, 1, "different thread id");
    let state = f.state();
    assert_eq!(state["sessionId"], fresh["run"]["sessionId"]);
    assert_eq!(state["state"], "error");
    assert_eq!(state["launches"][1]["sessionId"], "different-thread");
    assert_eq!(state["launches"][1]["stopReason"], "error");
    assert_eq!(state["launches"][1]["exitCode"], 0);
    assert_eq!(
        fs::read_to_string(f.root.join("completed")).unwrap(),
        "done"
    );
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
fn wait_for_session(f: &Fixture, pids: &str) {
    let codex_pid = pids.split_whitespace().next().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if fs::read_to_string(f.root.join("session-ready")).is_ok_and(|ready| ready == codex_pid) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session and rollout were not ready"
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
            vec!["run", "--skill", "x", "--timeout", "5s", "--json"]
        } else {
            vec!["run", "--skill", "x", "--json"]
        };
        let child = Running(Some(
            f.command(&args)
                .env("FAKE_BLOCK", "1")
                .env("FAKE_SESSION_DELAY", "0.25")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let pids = wait_for_file(&f.root.join("pids"));
        wait_for_session(&f, &pids);
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
    let pids = wait_for_file(&f.root.join("pids"));
    wait_for_session(&f, &pids);
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
    until(lambda: (root/'session-ready').read_text() == str(pgid) if (root/'session-ready').exists() else None)
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
        for expected in [
            "--skill",
            "--fresh",
            "--no-handoff",
            "--timeout",
            "skill list",
            "handoff add",
            "--kind",
        ] {
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

#[test]
fn selector_runs_from_primary_and_shares_in_workspace_resumes() {
    let f = Fixture::new();
    let first = f
        .command_at(
            &f.repo,
            &[
                "run",
                "feat/parent",
                "--skill",
                "x",
                "--json",
                "--",
                "outside task",
            ],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let first: Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(
        fs::read_to_string(f.root.join("cwd")).unwrap(),
        f.workspace.to_str().unwrap()
    );
    assert!(fs::read_to_string(f.root.join("input"))
        .unwrap()
        .contains("outside task"));
    let second = f.json(&["run", "--json"]);
    assert_eq!(first["data"]["run"]["id"], second["run"]["id"]);
    assert_eq!(
        first["data"]["run"]["sessionId"],
        second["run"]["sessionId"]
    );
    let third = f
        .command_at(
            &f.repo,
            &["run", "feat/parent", "--json", "--"],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap();
    assert!(third.status.success());
    assert_eq!(
        fs::read_to_string(f.root.join("input")).unwrap(),
        "Continue."
    );
    assert_eq!(f.state()["launches"].as_array().unwrap().len(), 3);
}

#[test]
fn selector_with_repo_from_outside_and_refusals() {
    let f = Fixture::new();
    let repo = f.repo.to_str().unwrap();
    let out = f
        .command_at(
            &f.root,
            &["-R", repo, "run", "--skill=x", "feat/parent", "--", "task"],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(f.root.join("cwd")).unwrap(),
        f.workspace.to_str().unwrap()
    );
    for args in [
        vec!["-R", repo, "run"],
        vec!["run", "-R", repo, "--", "task"],
        vec!["run", "-R", repo, "task"],
    ] {
        assert_error(
            f.command_at(&f.root, &args, env!("CARGO_BIN_EXE_rabi"))
                .output()
                .unwrap(),
            2,
            "SEL --",
        );
    }
    assert_error(f.run(&["run", repo, "--"]), 2, "rabi create");
    let out = f.run(&["run", "missing-workspace", "--json", "--"]);
    assert_error(out, 3, "not_found");
    f.json(&["adopt", f.workspace.to_str().unwrap(), "--json"]);
    fs::remove_dir_all(&f.workspace).unwrap();
    assert_error(
        f.command_at(
            &f.repo,
            &["run", "feat/parent", "--"],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap(),
        1,
        "orphaned",
    );
}

#[test]
fn task_forms_and_option_values_keep_their_meaning() {
    let f = Fixture::new();
    for (args, skill, task) in [
        (
            vec!["run", "plain task", "--skill", "--"],
            "--",
            "plain task",
        ),
        (
            vec!["run", "--fresh", "--skill=--", "--", "boundary task"],
            "--",
            "boundary task",
        ),
        (
            vec![
                "run",
                "feat/parent",
                "--fresh",
                "--skill",
                "--",
                "--timeout",
                "1m",
                "--",
                "selected task",
            ],
            "--",
            "selected task",
        ),
        (
            vec!["run", "--fresh", "--skill", "x", "literal task"],
            "x",
            "literal task",
        ),
    ] {
        let out = f.run(&args);
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(f.state()["skill"], skill);
        assert!(fs::read_to_string(f.root.join("input"))
            .unwrap()
            .ends_with(task));
    }
    assert_error(f.run(&["run", "--timeout", "--"]), 2, "--timeout");
    assert_error(f.run(&["run", "one", "two", "--"]), 2, "Usage");
    assert_error(
        f.run(&["run", "feat/parent", "--", "one", "two"]),
        2,
        "at most one task",
    );
}

#[test]
fn selected_repository_must_be_trusted_before_status_inspection() {
    let f = Fixture::new();
    let monitor = f.root.join("bin/monitor-selected");
    fs::write(&monitor, "#!/usr/bin/env python3\nimport os,pathlib\n(pathlib.Path(os.environ['FAKE_ROOT'])/'selected-monitor-ran').touch()\n").unwrap();
    fs::set_permissions(&monitor, fs::Permissions::from_mode(0o755)).unwrap();
    f.git(&["config", "core.fsmonitor", monitor.to_str().unwrap()]);
    let out = f
        .command_at(
            &f.root,
            &[
                "run",
                "-R",
                f.repo.to_str().unwrap(),
                "--skill",
                "x",
                "feat/parent",
                "--",
            ],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .env("RABI_TRUST_ALL", "0")
        .output()
        .unwrap();
    assert_error(out, 4, "trust");
    assert!(!f.root.join("selected-monitor-ran").exists());
    assert_eq!(f.calls(), 0);
}

fn observed_run(f: &Fixture, command: &str) -> Value {
    let data = f.json(&[command, "--json"]);
    if command == "show" {
        data["workspace"]["run"].clone()
    } else {
        data["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tree| tree["path"] == f.workspace.to_str().unwrap())
            .unwrap()["run"]
            .clone()
    }
}

fn snapshot_run_files(path: &Path) -> Vec<(PathBuf, Vec<u8>, u64, i64, i64)> {
    let mut snapshot = Vec::new();
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            snapshot.extend(snapshot_run_files(&path));
        } else {
            let metadata = fs::metadata(&path).unwrap();
            snapshot.push((
                path.clone(),
                fs::read(&path).unwrap(),
                metadata.ino(),
                metadata.mtime(),
                metadata.mtime_nsec(),
            ));
        }
    }
    snapshot.sort_by(|a, b| a.0.cmp(&b.0));
    snapshot
}

#[test]
fn list_show_run_summary_is_read_only_and_exposes_latest_paths() {
    let f = Fixture::new();
    for command in ["list", "show"] {
        assert!(observed_run(&f, command).is_null());
    }
    assert!(!f.current().exists());
    let run = f.json(&["run", "--skill", "x", "--json"])["run"].clone();
    let before = snapshot_run_files(&f.repo.join(".git/rabi/runs"));
    for command in ["list", "show"] {
        let summary = observed_run(&f, command);
        assert_eq!(summary["id"], run["id"]);
        assert_eq!(summary["state"], "idle");
        assert_eq!(summary["sessionId"], run["sessionId"]);
        assert_eq!(summary["skill"], "x");
        assert_eq!(summary["launches"], 1);
        assert_eq!(summary["lastLaunch"]["n"], 1);
        assert_eq!(summary["lastLaunch"]["stopReason"], "completed");
        for key in [
            "dir",
            "promptPath",
            "eventsPath",
            "stderrPath",
            "answerPath",
        ] {
            let path = Path::new(summary["lastLaunch"][key].as_str().unwrap());
            assert!(path.is_absolute() && path.exists(), "{key}");
        }
    }
    assert_eq!(before, snapshot_run_files(&f.repo.join(".git/rabi/runs")));
    let text = String::from_utf8(f.run(&["show"]).stdout).unwrap();
    for expected in [
        "Run: idle (run_",
        "Session: ",
        "Last launch: 1 completed",
        "Answer: ",
        "Log: ",
    ] {
        assert!(text.contains(expected), "{text}");
    }
    assert!(String::from_utf8(f.run(&["list"]).stdout)
        .unwrap()
        .contains("run:idle"));
    for flag in ["--names", "--paths"] {
        assert!(!String::from_utf8(f.run(&["list", flag]).stdout)
            .unwrap()
            .contains("run:"));
    }
}

#[test]
fn run_probe_handles_dead_live_missing_locks_and_corrupt_state() {
    let f = Fixture::new();
    f.json(&["run", "--skill", "x", "--json"]);
    let state_path = f.run_dir().join("state.json");
    let mut state = f.state();
    state["state"] = "running".into();
    fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let lock = f.pointer().with_extension("lock");
    for (holder, expected) in [
        (None, "stopped"),
        (Some("2147483647".to_owned()), "stopped"),
        (Some(std::process::id().to_string()), "running"),
        (Some(format!("{} 0", std::process::id())), "running"),
        (Some("2147483647 0".to_owned()), "stopped"),
        (
            Some(format!("2147483647 {}", unsafe { getpgrp() })),
            "running",
        ),
    ] {
        if let Some(pid) = holder.as_deref() {
            fs::write(&lock, pid).unwrap();
        }
        let before = snapshot_run_files(&f.repo.join(".git/rabi/runs"));
        for command in ["list", "show"] {
            assert_eq!(observed_run(&f, command)["state"], expected);
        }
        assert_eq!(before, snapshot_run_files(&f.repo.join(".git/rabi/runs")));
    }
    fs::remove_file(f.run_dir().join("launches/1/answer.md")).unwrap();
    assert!(observed_run(&f, "show")["lastLaunch"]["answerPath"].is_null());
    let pointer = f.pointer();
    let id = fs::read(&pointer).unwrap();
    for (path, invalid) in [
        (&lock, "not-a-pid"),
        (&lock, "0"),
        (&lock, "1 2 3"),
        (&pointer, "../../outside"),
    ] {
        fs::write(path, invalid).unwrap();
        let before = snapshot_run_files(&f.repo.join(".git/rabi/runs"));
        for command in ["list", "show"] {
            let out = f.run(&[command, "--json"]);
            assert!(out.status.success());
            let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(envelope["warnings"][0]["code"], "run_read");
            assert!(observed_run(&f, command).is_null());
        }
        assert_eq!(before, snapshot_run_files(&f.repo.join(".git/rabi/runs")));
        fs::write(&pointer, &id).unwrap();
        fs::write(&lock, std::process::id().to_string()).unwrap();
    }
    fs::write(&state_path, b"{ broken").unwrap();
    let before = snapshot_run_files(&f.repo.join(".git/rabi/runs"));
    for command in ["list", "show"] {
        let out = f.run(&[command, "--json"]);
        assert!(out.status.success());
        let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(envelope["warnings"][0]["code"], "run_read");
        assert!(observed_run(&f, command).is_null());
    }
    assert_eq!(before, snapshot_run_files(&f.repo.join(".git/rabi/runs")));
}

#[test]
fn observing_live_run_does_not_take_its_lock() {
    let f = Fixture::new();
    let mut parent = Running(Some(
        f.command(&["run", "--skill", "x", "--json"])
            .env("FAKE_WAIT_FINISH", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    wait_for_file(&f.root.join("waiting-finish"));
    let lock = f.pointer().with_extension("lock");
    let body = fs::read(&lock).unwrap();
    let before = fs::metadata(&lock).unwrap();
    for command in ["list", "show"] {
        assert_eq!(observed_run(&f, command)["state"], "running");
    }
    assert_eq!(fs::read(&lock).unwrap(), body);
    assert_eq!(fs::metadata(&lock).unwrap().ino(), before.ino());
    fs::write(f.root.join("finish"), "done").unwrap();
    assert!(parent.0.as_mut().unwrap().id() > 0);
    assert!(parent.finish().status.success());
    assert_eq!(observed_run(&f, "show")["state"], "idle");
    assert!(f.run(&["run"]).status.success());
}

#[test]
fn run_refuses_foreign_replacement_and_skips_foreign_sibling_monitors() {
    let f = Fixture::new();
    f.json(&["trust", "add", "--json"]);
    fs::rename(&f.workspace, f.root.join("original-workspace")).unwrap();
    fs::create_dir(&f.workspace).unwrap();
    let init = f
        .command_at(
            &f.workspace,
            &["init", "--template=", "-b", "foreign"],
            "git",
        )
        .output()
        .unwrap();
    assert!(init.status.success());
    let monitor = f.root.join("bin/foreign-monitor");
    fs::write(&monitor, "#!/usr/bin/env python3\nimport os,pathlib\n(pathlib.Path(os.environ['FAKE_ROOT'])/'foreign-monitor-ran').touch()\n").unwrap();
    fs::set_permissions(&monitor, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(f
        .command_at(
            &f.workspace,
            &["config", "core.fsmonitor", monitor.to_str().unwrap()],
            "git"
        )
        .output()
        .unwrap()
        .status
        .success());
    let target = f
        .command_at(
            &f.root,
            &[
                "run",
                "-R",
                f.repo.to_str().unwrap(),
                "--skill",
                "x",
                "feat/parent",
                "--",
            ],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .env("RABI_TRUST_ALL", "0")
        .output()
        .unwrap();
    assert_error(target, 1, "does not belong");
    assert!(!f.root.join("foreign-monitor-ran").exists());
    assert_eq!(f.calls(), 0);
    assert!(!f.current().exists());
    let foreign = f.root.join("foreign");
    fs::rename(&f.workspace, &foreign).unwrap();
    std::os::unix::fs::symlink(&foreign, &f.workspace).unwrap();
    assert_error(
        f.command_at(
            &f.repo,
            &["run", "--skill", "x", "feat/parent", "--"],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .env("RABI_TRUST_ALL", "0")
        .output()
        .unwrap(),
        1,
        "symlink",
    );
    assert!(!f.root.join("foreign-monitor-ran").exists());
    assert_eq!(f.calls(), 0);
    let sibling = f.root.join("safe-sibling");
    f.git(&[
        "worktree",
        "add",
        "-b",
        "safe-sibling",
        sibling.to_str().unwrap(),
    ]);
    for (cwd, args) in [
        (&f.repo, vec!["run", "safe-sibling", "--skill", "x", "--"]),
        (&sibling, vec!["run"]),
    ] {
        let out = f
            .command_at(cwd, &args, env!("CARGO_BIN_EXE_rabi"))
            .env("RABI_TRUST_ALL", "0")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!f.root.join("foreign-monitor-ran").exists());
    }
    // Positive control: status in the foreign checkout would execute the monitor.
    assert!(f
        .command_at(&f.workspace, &["status", "--porcelain"], "git")
        .output()
        .unwrap()
        .status
        .success());
    assert!(f.root.join("foreign-monitor-ran").exists());
}

#[test]
fn run_read_diagnostics_escape_control_characters_from_invalid_records() {
    let f = Fixture::new();
    f.json(&["run", "--skill", "x", "--json"]);
    let state_path = f.run_dir().join("state.json");
    let original = f.state();
    let controls = "\u{1b}[2J\r\n\t\u{7}\u{202e}";
    for field in ["state", "stopReason", "launchNumber", "schema"] {
        let mut state = original.clone();
        match field {
            "state" => state["state"] = controls.into(),
            "stopReason" => state["launches"][0]["stopReason"] = controls.into(),
            "launchNumber" => state["launches"][0]["n"] = controls.into(),
            "schema" => state["schema"] = controls.into(),
            _ => unreachable!(),
        }
        fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
        for command in ["list", "show"] {
            for json in [false, true] {
                let args = if json {
                    vec![command, "--json"]
                } else {
                    vec![command]
                };
                let out = f.run(&args);
                assert!(out.status.success());
                let stderr = String::from_utf8(out.stderr).unwrap();
                assert!(stderr.contains("Cannot read workspace run"));
                assert_eq!(stderr.lines().count(), 1, "{field}: {stderr:?}");
                assert!(
                    stderr.chars().all(|ch| !ch.is_control() || ch == '\n'),
                    "{field}: {stderr:?}"
                );
                assert!(!stderr.contains('\u{202e}'), "{field}: {stderr:?}");
                if json {
                    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
                    let warning = &envelope["warnings"][0];
                    assert_eq!(warning["code"], "run_read");
                    let message = warning["message"].as_str().unwrap();
                    assert!(message.chars().all(|ch| !ch.is_control()));
                    assert!(!message.contains('\u{202e}'));
                    let run = if command == "show" {
                        &envelope["data"]["workspace"]["run"]
                    } else {
                        &envelope["data"]["workspaces"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .find(|tree| tree["path"] == f.workspace.to_str().unwrap())
                            .unwrap()["run"]
                    };
                    assert!(run.is_null());
                } else {
                    let stdout = String::from_utf8(out.stdout).unwrap();
                    assert!(!stdout.contains("Run:"));
                    assert!(!stdout.contains("run:"));
                }
            }
        }
    }
}

impl Fixture {
    fn fake_cursor(&self) {
        // Isolate PATH so tests cannot accidentally launch a real Cursor binary.
        for name in ["git", "python3"] {
            let source = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|path| path.join(name))
                .find(|path| path.is_file())
                .unwrap();
            std::os::unix::fs::symlink(source, self.root.join("bin").join(name)).unwrap();
        }
        let script = self.root.join("bin/cursor-agent");
        fs::write(&script, r#"#!/usr/bin/env python3
import hashlib, json, os, pathlib, signal, subprocess, sys, time, uuid
args = sys.argv[1:]
root = pathlib.Path(os.environ['FAKE_ROOT'])
with (root/'argv.jsonl').open('a') as f: f.write(json.dumps(args)+'\n')
(root/'input').write_bytes(sys.stdin.buffer.read())
(root/'cwd').write_text(os.getcwd())
(root/'binary').write_text(pathlib.Path(sys.argv[0]).name)
workspace = args[args.index('--workspace')+1]
sid = args[args.index('--resume')+1] if '--resume' in args else str(uuid.uuid4())
sid = os.environ.get('CURSOR_OTHER_ID', sid)
meta = pathlib.Path(os.environ['HOME'])/'.cursor/chats'/hashlib.md5(workspace.encode()).hexdigest()/sid/'meta.json'
meta.parent.mkdir(parents=True, exist_ok=True)
meta.write_text(json.dumps({'schemaVersion':1,'hasConversation':True,'cwd':workspace}))
(root/'meta-path').write_text(str(meta))
if os.environ.get('CURSOR_BLOCK'):
    def capture_signal(signum, frame):
        with (root/'signals').open('a') as f: f.write(str(signum)+'\n')
    signal.signal(signal.SIGINT, capture_signal)
    signal.signal(signal.SIGTERM, capture_signal)
    child = subprocess.Popen([sys.executable,'-c','import os,signal,time; signal.signal(signal.SIGINT,signal.SIG_IGN); signal.signal(signal.SIGTERM,signal.SIG_IGN); os.write(1,b"ready\\n"); time.sleep(600)'], start_new_session=True, stdout=subprocess.PIPE)
    assert child.stdout.readline() == b'ready\n'
    (root/'pids').write_text(str(os.getpid())+' '+str(child.pid))
if not os.environ.get('CURSOR_NO_INIT'):
    print(json.dumps({'type':'system','subtype':'init','apiKeySource':'login','cwd':workspace,'session_id':sid,'model':'Grok 4.7 256K High','permissionMode':'default'}), flush=True)
(root/'session-ready').write_text(str(os.getpid()))
if os.environ.get('CURSOR_AUDIT'):
    for subtype, policy in [('started','TYPE_INSECURE_NONE'), ('completed','TYPE_WORKSPACE_READWRITE'), ('completed','TYPE_INSECURE_NONE'), ('completed','TYPE_INSECURE_NONE')]:
        print(json.dumps({'type':'tool_call','subtype':subtype,'session_id':sid,'tool_call':{'shellToolCall':{'args':{'command':'curl https://example.com','requestedSandboxPolicy':{'type':policy},'skipApproval':True},'result':{'success':{'exitCode':0,'stdout':'200'}}}}}), flush=True)
if os.environ.get('CURSOR_BLOCK'): time.sleep(600)
if os.environ.get('CURSOR_FIRST_RESULT'):
    print(json.dumps({'type':'result','result':'Superseded result','is_error':False,'session_id':sid}), flush=True)
if not os.environ.get('CURSOR_NO_RESULT'):
    answer = 'x' * int(os.environ['CURSOR_ANSWER_SIZE']) if os.environ.get('CURSOR_ANSWER_SIZE') else 'Cursor final answer.\n'
    event = {'type':'result','subtype':'success','duration_ms':1,'duration_api_ms':1,'is_error':bool(os.environ.get('CURSOR_IS_ERROR')),'result':answer,'session_id':os.environ.get('CURSOR_RESULT_ID',sid),'request_id':'fake-request','usage':{'inputTokens':10,'outputTokens':5}}
    if os.environ.get('CURSOR_BAD_RESULT'): event['result'] = {'not':'text'}
    if os.environ.get('CURSOR_BAD_ERROR'): event['is_error'] = 'false'
    text = json.dumps(event)
    if os.environ.get('CURSOR_TRUNCATE'): text = text[:-5]
    print(text, end='' if os.environ.get('CURSOR_NO_NEWLINE') else '\n', flush=True)
if os.environ.get('CURSOR_TRAILING_TRUNCATED'):
    print('{"type":"result","result":"unfinished', end='', flush=True)
print('fake Cursor stderr', file=sys.stderr, flush=True)
(root/'completed').write_text('done')
sys.exit(int(os.environ.get('CURSOR_EXIT','0')))
"#).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn cursor_command(&self, args: &[&str]) -> Command {
        let mut command = self.command(args);
        command.env("PATH", self.root.join("bin"));
        command
    }

    fn cursor_json(&self, args: &[&str]) -> Value {
        let out = self.cursor_command(args).output().unwrap();
        assert!(
            out.status.success(),
            "{:?}: {} {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    }

    fn cursor_meta(&self) -> PathBuf {
        PathBuf::from(fs::read_to_string(self.root.join("meta-path")).unwrap())
    }
}

#[test]
fn cursor_fresh_and_resume_preserve_argv_prompt_answer_and_provider_summary() {
    let f = Fixture::new();
    f.fake_cursor();
    fs::copy(f.root.join("bin/cursor-agent"), f.root.join("bin/agent")).unwrap();
    let first = f.cursor_json(&[
        "run",
        "--provider",
        "cursor",
        "--skill",
        "x",
        "initial task",
        "--json",
    ]);
    assert_eq!(first["run"]["provider"], "cursor");
    assert_eq!(first["run"]["schema"], 2);
    assert_eq!(first["run"]["state"], "idle");
    assert_eq!(first["answer"], "Cursor final answer.\n");
    let dir = f.run_dir();
    assert_eq!(
        fs::read(f.root.join("input")).unwrap(),
        fs::read(dir.join("launches/1/prompt.md")).unwrap()
    );
    assert!(fs::read_to_string(f.root.join("input"))
        .unwrap()
        .contains("initial task"));
    assert_eq!(
        fs::read_to_string(dir.join("launches/1/answer.md")).unwrap(),
        "Cursor final answer.\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("cwd")).unwrap(),
        f.workspace.to_str().unwrap()
    );
    assert_eq!(
        fs::read_to_string(f.root.join("binary")).unwrap(),
        "cursor-agent"
    );
    let expected = serde_json::json!([
        "-p",
        "--output-format",
        "stream-json",
        "--workspace",
        f.workspace,
        "--trust",
        "--sandbox",
        "enabled",
        "--auto-review",
        "--model",
        "grok-4.7-high"
    ]);
    let calls = || {
        fs::read_to_string(f.root.join("argv.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(calls()[0], expected);
    let second = f.cursor_json(&["run", "--json"]);
    assert_eq!(second["run"]["id"], first["run"]["id"]);
    assert_eq!(second["run"]["sessionId"], first["run"]["sessionId"]);
    let mut resumed = expected.as_array().unwrap().clone();
    resumed.extend([Value::from("--resume"), first["run"]["sessionId"].clone()]);
    assert_eq!(calls()[1], Value::Array(resumed));
    assert_eq!(
        fs::read_to_string(f.root.join("input")).unwrap(),
        "Continue."
    );
    let third = f.cursor_json(&["run", "--provider", "cursor", "--json"]);
    assert_eq!(third["run"]["id"], first["run"]["id"]);
    assert_eq!(observed_run(&f, "show")["provider"], "cursor");
    let list = f.json(&["list", "--json"]);
    assert_eq!(
        list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ws| ws["path"] == f.workspace.to_str().unwrap())
            .unwrap()["run"]["provider"],
        "cursor"
    );
    let saved = fs::read(dir.join("state.json")).unwrap();
    let fresh = f.cursor_json(&[
        "run",
        "--fresh",
        "--provider",
        "cursor",
        "--skill",
        "x",
        "--json",
    ]);
    assert_ne!(fresh["run"]["id"], first["run"]["id"]);
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), saved);
    f.assert_unlocked();
}

#[test]
fn cursor_provider_mismatch_is_usage_before_persistent_mutation() {
    let f = Fixture::new();
    f.fake_cursor();
    f.cursor_json(&["run", "--provider", "cursor", "--skill", "x", "--json"]);
    let state = fs::read(f.run_dir().join("state.json")).unwrap();
    let pointer = fs::read(f.pointer()).unwrap();
    let exclude = fs::read(f.repo.join(".git/info/exclude")).unwrap();
    let sandbox = fs::read(f.workspace.join(".cursor/sandbox.json")).unwrap();
    // Provider mismatch takes precedence even if local Cursor history is broken.
    fs::remove_file(f.cursor_meta()).unwrap();
    assert_error(
        f.cursor_command(&["run", "--provider", "codex", "--json"])
            .output()
            .unwrap(),
        2,
        "--fresh --provider codex",
    );
    assert_eq!(fs::read(f.run_dir().join("state.json")).unwrap(), state);
    assert_eq!(fs::read(f.pointer()).unwrap(), pointer);
    assert_eq!(fs::read(f.repo.join(".git/info/exclude")).unwrap(), exclude);
    assert_eq!(
        fs::read(f.workspace.join(".cursor/sandbox.json")).unwrap(),
        sandbox
    );
    assert_eq!(
        fs::read_dir(f.run_dir().join("launches")).unwrap().count(),
        1
    );
    assert_eq!(f.calls(), 1);
    f.assert_unlocked();

    let g = Fixture::new();
    g.fake_cursor();
    g.json(&["run", "--skill", "x", "--json"]);
    let saved = fs::read(g.run_dir().join("state.json")).unwrap();
    assert_error(
        g.cursor_command(&["run", "--provider", "cursor", "--json"])
            .output()
            .unwrap(),
        2,
        "--fresh --provider cursor",
    );
    assert_eq!(fs::read(g.run_dir().join("state.json")).unwrap(), saved);
    assert!(!g.workspace.join(".cursor").exists());
    let switched = g.cursor_json(&[
        "run",
        "--fresh",
        "--provider",
        "cursor",
        "--skill",
        "x",
        "--json",
    ]);
    assert_eq!(switched["run"]["provider"], "cursor");
}

#[test]
fn cursor_probe_requires_valid_conversation_metadata_in_workspace_hash() {
    for mode in [
        "missing",
        "false",
        "malformed",
        "unknown-id",
        "wrong-workspace",
        "missing-field",
    ] {
        let f = Fixture::new();
        f.fake_cursor();
        f.cursor_json(&["run", "--provider", "cursor", "--skill", "x", "--json"]);
        let meta = f.cursor_meta();
        match mode {
            "missing" => fs::remove_file(meta).unwrap(),
            "false" => fs::write(meta, r#"{"hasConversation":false}"#).unwrap(),
            "malformed" => fs::write(meta, "{").unwrap(),
            "missing-field" => fs::write(meta, "{}").unwrap(),
            "unknown-id" => {
                let mut state = f.state();
                state["sessionId"] = Value::from(uuid::Uuid::new_v4().to_string());
                fs::write(f.run_dir().join("state.json"), state.to_string()).unwrap();
            }
            _ => {
                let elsewhere = f
                    .home
                    .join(".cursor/chats/incorrect-workspace-hash")
                    .join(meta.parent().unwrap().file_name().unwrap());
                fs::create_dir_all(&elsewhere).unwrap();
                fs::rename(&meta, elsewhere.join("meta.json")).unwrap();
            }
        }
        assert_error(
            f.cursor_command(&["run", "--json"]).output().unwrap(),
            1,
            "--fresh",
        );
        assert_eq!(
            f.calls(),
            1,
            "probe mode {mode} launched an empty conversation"
        );
        f.assert_unlocked();
    }
}

#[test]
fn cursor_sandbox_allowlist_is_excluded_idempotent_and_preserves_existing_content() {
    for existing in [false, true] {
        let f = Fixture::new();
        f.fake_cursor();
        let sandbox = f.workspace.join(".cursor/sandbox.json");
        if existing {
            fs::create_dir_all(sandbox.parent().unwrap()).unwrap();
            fs::write(
                &sandbox,
                "user sandbox content is preserved, even if not JSON\n",
            )
            .unwrap();
        }
        fs::create_dir_all(f.repo.join(".git/info")).unwrap();
        fs::write(
            f.repo.join(".git/info/exclude"),
            "# user exclusions\nexisting-pattern",
        )
        .unwrap();
        f.cursor_json(&["run", "--provider", "cursor", "--skill", "x", "--json"]);
        let bytes = fs::read(&sandbox).unwrap();
        if existing {
            assert_eq!(
                bytes,
                b"user sandbox content is preserved, even if not JSON\n"
            );
        } else {
            let value: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"networkPolicy":{"allow":["github.com","*.github.com","*.githubusercontent.com"]}})
            );
        }
        let exclude = fs::read(f.repo.join(".git/info/exclude")).unwrap();
        let text = String::from_utf8(exclude.clone()).unwrap();
        assert!(text.starts_with("# user exclusions\nexisting-pattern\n"));
        assert_eq!(
            text.lines()
                .filter(|line| [".cursor/sandbox.json", "/.cursor/sandbox.json"].contains(line))
                .count(),
            1
        );
        let status = f
            .command_at(&f.workspace, &["status", "--porcelain"], "git")
            .output()
            .unwrap();
        assert!(status.status.success());
        assert!(
            status.stdout.is_empty(),
            "{}",
            String::from_utf8_lossy(&status.stdout)
        );
        f.cursor_json(&["run", "--json"]);
        assert_eq!(fs::read(&sandbox).unwrap(), bytes);
        assert_eq!(fs::read(f.repo.join(".git/info/exclude")).unwrap(), exclude);
    }
}

#[test]
fn cursor_result_protocol_errors_and_parent_failures_keep_real_exit_code() {
    for (setting, exit, stop) in [
        ("CURSOR_NO_RESULT", 1, "error"),
        ("CURSOR_BAD_RESULT", 1, "error"),
        ("CURSOR_BAD_ERROR", 1, "error"),
        ("CURSOR_TRUNCATE", 1, "error"),
        ("CURSOR_TRAILING_TRUNCATED", 1, "error"),
        ("CURSOR_IS_ERROR", 1, "parent_failed"),
    ] {
        let f = Fixture::new();
        f.fake_cursor();
        let out = f
            .cursor_command(&["run", "--provider", "cursor", "--skill", "x", "--json"])
            .env(setting, "1")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(exit),
            "{setting}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let state = f.state();
        assert_eq!(
            state["state"],
            if stop == "parent_failed" {
                "stopped"
            } else {
                "error"
            },
            "{setting}"
        );
        assert_eq!(state["launches"][0]["stopReason"], stop, "{setting}");
        assert_eq!(state["launches"][0]["exitCode"], 0, "{setting}");
        assert!(state["launches"][0]["finishedAt"].is_string());
        f.assert_unlocked();
    }
    let f = Fixture::new();
    f.fake_cursor();
    let out = f
        .cursor_command(&["run", "--provider", "cursor", "--skill", "x", "--json"])
        .env("CURSOR_EXIT", "42")
        .env("CURSOR_NO_RESULT", "1")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(42),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert_eq!(f.state()["launches"][0]["stopReason"], "parent_failed");
    assert_eq!(f.state()["launches"][0]["exitCode"], 42);
}

#[test]
fn cursor_last_result_large_answer_and_missing_final_newline_are_supported() {
    for (size, no_init) in [(0, false), (131072, false), (131072, true)] {
        let f = Fixture::new();
        f.fake_cursor();
        let mut command =
            f.cursor_command(&["run", "--provider", "cursor", "--skill", "x", "--json"]);
        if no_init {
            command.env("CURSOR_NO_INIT", "1");
        } else {
            command.env("CURSOR_FIRST_RESULT", "1");
        }
        let out = command
            .env("CURSOR_NO_NEWLINE", "1")
            .env("CURSOR_ANSWER_SIZE", size.to_string())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let answer = fs::read(f.run_dir().join("launches/1/answer.md")).unwrap();
        assert_eq!(answer.len(), size);
        assert_eq!(answer, vec![b'x'; size]);
        f.assert_unlocked();
    }
}

#[test]
fn cursor_init_and_result_session_mismatch_names_cursor_and_keeps_original_id() {
    let f = Fixture::new();
    f.fake_cursor();
    let out = f
        .cursor_command(&["run", "--provider", "cursor", "--skill", "x", "--json"])
        .env("CURSOR_RESULT_ID", uuid::Uuid::new_v4().to_string())
        .output()
        .unwrap();
    assert_error(out, 1, "Cursor");
    assert_eq!(f.state()["launches"][0]["stopReason"], "error");
    let saved = f.cursor_json(&[
        "run",
        "--fresh",
        "--provider",
        "cursor",
        "--skill",
        "x",
        "--json",
    ]);
    fs::remove_file(f.root.join("completed")).unwrap();
    let child = Running(Some(
        f.cursor_command(&["run", "--json"])
            .env("CURSOR_OTHER_ID", uuid::Uuid::new_v4().to_string())
            .env("CURSOR_BLOCK", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    ));
    let pids = wait_for_file(&f.root.join("pids"));
    let out = child.finish();
    assert_error(out, 1, "Cursor");
    assert_eq!(f.state()["sessionId"], saved["run"]["sessionId"]);
    assert_ne!(
        f.state()["launches"][1]["sessionId"],
        saved["run"]["sessionId"]
    );
    assert_eq!(f.state()["launches"][1]["stopReason"], "error");
    assert!(!f.root.join("completed").exists());
    for pid in pids.split_whitespace() {
        assert_dead(pid.parse().unwrap());
    }
    f.assert_unlocked();
}

#[test]
fn cursor_binary_falls_back_to_agent_and_missing_binary_has_install_login_guidance() {
    for mode in ["missing", "fallback"] {
        let f = Fixture::new();
        f.fake_cursor();
        if mode == "fallback" {
            fs::rename(f.root.join("bin/cursor-agent"), f.root.join("bin/agent")).unwrap();
            let out = f.cursor_json(&["run", "--provider", "cursor", "--skill", "x", "--json"]);
            assert_eq!(out["run"]["provider"], "cursor");
            assert_eq!(fs::read_to_string(f.root.join("binary")).unwrap(), "agent");
        } else {
            fs::remove_file(f.root.join("bin/cursor-agent")).unwrap();
            let out = f
                .cursor_command(&["run", "--provider", "cursor", "--skill", "x", "--json"])
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(1));
            let error: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(error["error"]["code"], "spawn");
            let text = String::from_utf8_lossy(&out.stdout);
            assert!(text.contains("cursor-agent"), "{text}");
            assert!(text.to_ascii_lowercase().contains("install"), "{text}");
            assert!(text.contains("login") || text.contains("log in"), "{text}");
            assert_eq!(f.calls(), 0);
        }
    }
}

#[test]
fn cursor_completed_insecure_tool_calls_add_audit_without_failing() {
    let f = Fixture::new();
    f.fake_cursor();
    let out = f
        .cursor_command(&["run", "--provider", "cursor", "--skill", "x", "--json"])
        .env("CURSOR_AUDIT", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let data: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(data["data"]["launch"]["insecureToolCalls"], 2);
    assert!(data["data"]["launch"]["message"]
        .as_str()
        .unwrap()
        .contains("TYPE_INSECURE_NONE"));
    let summary = observed_run(&f, "show");
    assert_eq!(summary["lastLaunch"]["insecureToolCalls"], 2);
    assert!(summary["lastLaunch"]["message"]
        .as_str()
        .unwrap()
        .contains("sandbox"));
    assert_eq!(f.state()["state"], "idle");
}

#[cfg(target_os = "linux")]
#[test]
fn cursor_timeout_and_interrupt_kill_setsid_descendants_and_allow_resume() {
    for interrupt in [false, true] {
        let f = Fixture::new();
        f.fake_cursor();
        let timeout = if interrupt { "30s" } else { "2s" };
        let child = Running(Some(
            f.cursor_command(&[
                "run",
                "--provider",
                "cursor",
                "--skill",
                "x",
                "--timeout",
                timeout,
                "--json",
            ])
            .env("CURSOR_BLOCK", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        ));
        let pids = wait_for_file(&f.root.join("pids"));
        wait_for_session(&f, &pids);
        let ids: Vec<u32> = pids
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        let descendant_stat = fs::read_to_string(format!("/proc/{}/stat", ids[1])).unwrap();
        let stat: Vec<_> = descendant_stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(
            stat[2].parse::<u32>().unwrap(),
            ids[1],
            "fake shell must have its own process group"
        );
        if interrupt {
            send(child.0.as_ref().unwrap().id(), 2);
        }
        let out = child.finish();
        assert_eq!(
            out.status.code(),
            Some(if interrupt { 130 } else { 124 }),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        for pid in ids {
            assert_dead(pid);
        }
        let signals = fs::read_to_string(f.root.join("signals")).unwrap();
        assert_eq!(
            signals.lines().next(),
            Some("2"),
            "Cursor must receive INT before TERM"
        );
        assert!(
            signals.lines().any(|s| s == "15"),
            "stubborn Cursor must receive TERM after INT"
        );
        let saved = f.state();
        assert_eq!(saved["state"], "stopped");
        assert_eq!(
            saved["launches"][0]["stopReason"],
            if interrupt {
                "interrupted"
            } else {
                "timed_out"
            }
        );
        f.assert_unlocked();
        let resumed = f.cursor_json(&["run", "--json"]);
        assert_eq!(resumed["run"]["sessionId"], saved["sessionId"]);
    }
}

impl Fixture {
    fn fake_claude(&self) {
        // Restrict PATH so no test can invoke a real Claude installation.
        for name in ["git", "python3"] {
            let source = std::env::split_paths(&std::env::var_os("PATH").unwrap())
                .map(|path| path.join(name))
                .find(|path| path.is_file())
                .unwrap();
            std::os::unix::fs::symlink(source, self.root.join("bin").join(name)).unwrap();
        }
        let script = self.root.join("bin/claude");
        fs::write(&script, r#"#!/usr/bin/env python3
import json, os, pathlib, signal, subprocess, sys, time
args = sys.argv[1:]
root = pathlib.Path(os.environ['FAKE_ROOT'])
with (root/'argv.jsonl').open('a') as f: f.write(json.dumps(args)+'\n')
(root/'input').write_bytes(sys.stdin.buffer.read())
(root/'cwd').write_text(os.getcwd())
sid = args[args.index('--resume')+1] if '--resume' in args else args[args.index('--session-id')+1]
sid = os.environ.get('CLAUDE_OTHER_ID', sid)
if os.environ.get('CLAUDE_UNKNOWN_ID'):
    print(json.dumps({'type':'result','subtype':'error_during_execution','is_error':True,'session_id':sid,'errors':['No conversation found with session ID: '+sid]}), flush=True)
    print('No conversation found with session ID: '+sid, file=sys.stderr, flush=True)
    sys.exit(1)
if not os.environ.get('CLAUDE_NO_TRANSCRIPT'):
    config = pathlib.Path(os.environ.get('CLAUDE_CONFIG_DIR') or pathlib.Path(os.environ['HOME'])/'.claude')
    transcript = config/'projects/fake-lossy-workspace'/(sid+'.jsonl')
    transcript.parent.mkdir(parents=True, exist_ok=True)
    transcript.write_text('{}\n')
    (root/'transcript-path').write_text(str(transcript))
if os.environ.get('CLAUDE_BLOCK'):
    def capture_signal(signum, frame):
        with (root/'signals').open('a') as f: f.write(str(signum)+'\n')
        if signum == signal.SIGINT and not os.environ.get('CLAUDE_STUBBORN'):
            # Real Claude 2.1.310 exits zero on INT with an error result.
            print(json.dumps({'type':'result','subtype':'error_during_execution','is_error':True,'result':None,'session_id':sid}), flush=True)
            sys.exit(0)
    signal.signal(signal.SIGINT, capture_signal)
    signal.signal(signal.SIGTERM, capture_signal)
    child = subprocess.Popen([sys.executable,'-c','import os,signal,time; signal.signal(signal.SIGINT,signal.SIG_IGN); signal.signal(signal.SIGTERM,signal.SIG_IGN); os.write(1,b"ready\\n"); time.sleep(600)'], start_new_session=True, stdout=subprocess.PIPE)
    assert child.stdout.readline() == b'ready\n'
    (root/'pids').write_text(str(os.getpid())+' '+str(child.pid))
if os.environ.get('CLAUDE_EARLY_ID'):
    print(json.dumps({'type':'system','subtype':'status','session_id':sid}), flush=True)
if os.environ.get('CLAUDE_LARGE_EVENT'):
    if os.environ['CLAUDE_LARGE_EVENT'] == 'assistant':
        event = {'type':'assistant','session_id':sid,'message':{'role':'assistant','content':[{'type':'text','text':'x'*131072}]}}
    else:
        event = {'type':'user','session_id':sid,'message':{'role':'user','content':[{'type':'tool_result','tool_use_id':'fake-tool','content':'x'*131072}]}}
    print(json.dumps(event), flush=True)
if not os.environ.get('CLAUDE_NO_INIT'):
    event = {'type':'system','subtype':'init','cwd':os.getcwd(),'session_id':os.environ.get('CLAUDE_INIT_ID',sid),'model':'claude-opus-5-5','permissionMode':os.environ.get('CLAUDE_PERMISSION_MODE','auto')}
    if os.environ.get('CLAUDE_NO_PERMISSION_MODE'): del event['permissionMode']
    if os.environ.get('CLAUDE_INIT_PADDING'): event['padding'] = 'x'*int(os.environ['CLAUDE_INIT_PADDING'])
    print(json.dumps(event), flush=True)
(root/'session-ready').write_text(str(os.getpid()))
if os.environ.get('CLAUDE_CONTINUATION_DELAY'):
    time.sleep(float(os.environ['CLAUDE_CONTINUATION_DELAY']))
    (root/'continued-after-init').write_text('parent continued')
if os.environ.get('CLAUDE_BLOCK'): time.sleep(600)
if os.environ.get('CLAUDE_FIRST_RESULT'):
    print(json.dumps({'type':'result','result':'Superseded result','is_error':False,'session_id':sid}), flush=True)
if not os.environ.get('CLAUDE_NO_RESULT'):
    answer = 'x' * int(os.environ['CLAUDE_ANSWER_SIZE']) if os.environ.get('CLAUDE_ANSWER_SIZE') else 'Claude final answer.\n'
    event = {'type':'result','subtype':'success','duration_ms':1,'duration_api_ms':1,'is_error':bool(os.environ.get('CLAUDE_IS_ERROR')),'result':answer,'session_id':os.environ.get('CLAUDE_RESULT_ID',sid),'usage':{'input_tokens':10,'output_tokens':5}}
    if os.environ.get('CLAUDE_BAD_RESULT'): event['result'] = {'not':'text'}
    if os.environ.get('CLAUDE_NULL_RESULT'): event['result'] = None
    if os.environ.get('CLAUDE_BAD_ERROR'): event['is_error'] = 'false'
    text = json.dumps(event)
    if os.environ.get('CLAUDE_TRUNCATE'): text = text[:-5]
    print(text, end='' if os.environ.get('CLAUDE_NO_NEWLINE') else '\n', flush=True)
if os.environ.get('CLAUDE_TRAILING_TRUNCATED'):
    print('{"type":"result","result":"unfinished', end='', flush=True)
print('fake Claude stderr', file=sys.stderr, flush=True)
(root/'completed').write_text('done')
sys.exit(int(os.environ.get('CLAUDE_EXIT','0')))
"#).unwrap();
        fs::set_permissions(script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn claude_command(&self, args: &[&str]) -> Command {
        let mut command = self.command(args);
        command
            .env("PATH", self.root.join("bin"))
            .env_remove("CLAUDE_CONFIG_DIR");
        command
    }

    fn claude_json(&self, args: &[&str]) -> Value {
        let out = self.claude_command(args).output().unwrap();
        assert!(
            out.status.success(),
            "{:?}: {} {}",
            args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"].clone()
    }
}

#[test]
fn claude_fresh_and_resume_preserve_exact_argv_prompt_answer_and_provider_summary() {
    let f = Fixture::new();
    f.fake_claude();
    let exclude = fs::read(f.repo.join(".git/info/exclude")).ok();
    let first = f.claude_json(&[
        "run",
        "--provider",
        "claude",
        "--skill",
        "x",
        "initial task",
        "--json",
    ]);
    let sid = first["run"]["sessionId"].as_str().unwrap();
    let uuid = uuid::Uuid::parse_str(sid).unwrap();
    assert_eq!(uuid.get_version_num(), 4);
    assert_eq!(uuid.get_variant(), uuid::Variant::RFC4122);
    assert_eq!(uuid.hyphenated().to_string(), sid);
    assert_eq!(first["run"]["provider"], "claude");
    assert_eq!(first["run"]["schema"], 2);
    assert_eq!(first["run"]["state"], "idle");
    assert_eq!(first["answer"], "Claude final answer.\n");
    assert_eq!(first["launch"]["requestedSessionId"], sid);
    assert_eq!(first["launch"]["sessionId"], sid);
    let dir = f.run_dir();
    assert_eq!(
        fs::read(f.root.join("input")).unwrap(),
        fs::read(dir.join("launches/1/prompt.md")).unwrap()
    );
    assert!(fs::read_to_string(f.root.join("input"))
        .unwrap()
        .contains("initial task"));
    assert_eq!(
        fs::read_to_string(dir.join("launches/1/answer.md")).unwrap(),
        "Claude final answer.\n"
    );
    assert_eq!(
        fs::read_to_string(f.root.join("cwd")).unwrap(),
        f.workspace.to_str().unwrap()
    );
    let expected = serde_json::json!([
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--session-id",
        sid,
        "--permission-mode",
        "auto",
        "--permission-prompts",
        "none",
        "--model",
        "opus",
        "--effort",
        "high"
    ]);
    let calls = || {
        fs::read_to_string(f.root.join("argv.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(calls()[0], expected);
    let second = f.claude_json(&["run", "--json"]);
    assert_eq!(second["run"]["id"], first["run"]["id"]);
    assert_eq!(second["run"]["sessionId"], sid);
    assert!(second["launch"]["requestedSessionId"].is_null());
    let mut resumed = expected.as_array().unwrap().clone();
    resumed[4] = Value::from("--resume");
    assert_eq!(calls()[1], Value::Array(resumed));
    assert_eq!(
        fs::read_to_string(f.root.join("input")).unwrap(),
        "Continue."
    );
    let third = f.claude_json(&["run", "--provider", "claude", "--json"]);
    assert_eq!(third["run"]["sessionId"], sid);
    assert_eq!(observed_run(&f, "show")["provider"], "claude");
    let list = f.json(&["list", "--json"]);
    assert_eq!(
        list["workspaces"]
            .as_array()
            .unwrap()
            .iter()
            .find(|ws| ws["path"] == f.workspace.to_str().unwrap())
            .unwrap()["run"]["provider"],
        "claude"
    );
    let saved = fs::read(dir.join("state.json")).unwrap();
    let fresh = f.claude_json(&[
        "run",
        "--fresh",
        "--provider",
        "claude",
        "--skill",
        "x",
        "--json",
    ]);
    assert_ne!(fresh["run"]["id"], first["run"]["id"]);
    assert_ne!(fresh["run"]["sessionId"], sid);
    assert_eq!(fs::read(dir.join("state.json")).unwrap(), saved);
    assert_eq!(fs::read(f.repo.join(".git/info/exclude")).ok(), exclude);
    assert!(!f.workspace.join(".claude").exists());
    assert!(!f.workspace.join(".cursor").exists());
    f.assert_unlocked();
}

#[test]
fn claude_provider_mismatch_is_usage_before_any_persistent_mutation() {
    let f = Fixture::new();
    f.fake_claude();
    f.claude_json(&["run", "--provider", "claude", "--skill", "x", "--json"]);
    for provider in ["codex", "cursor"] {
        let before = snapshot_run_files(&f.repo.join(".git/rabi/runs"));
        assert_error(
            f.claude_command(&["run", "--provider", provider, "--json"])
                .output()
                .unwrap(),
            2,
            &format!("--fresh --provider {provider}"),
        );
        assert_eq!(snapshot_run_files(&f.repo.join(".git/rabi/runs")), before);
        assert_eq!(f.calls(), 1);
        f.assert_unlocked();
    }
    let g = Fixture::new();
    g.fake_claude();
    let first = g.json(&["run", "--skill", "x", "--json"]);
    assert_eq!(first["run"]["provider"], "codex");
    assert!(first["launch"]["requestedSessionId"].is_null());
    let before = snapshot_run_files(&g.repo.join(".git/rabi/runs"));
    assert_error(
        g.claude_command(&["run", "--provider", "claude", "--json"])
            .output()
            .unwrap(),
        2,
        "--fresh --provider claude",
    );
    assert_eq!(snapshot_run_files(&g.repo.join(".git/rabi/runs")), before);
    let fresh = g.claude_json(&[
        "run",
        "--fresh",
        "--provider",
        "claude",
        "--skill",
        "x",
        "--json",
    ]);
    assert_eq!(fresh["run"]["provider"], "claude");
}

#[test]
fn claude_unknown_local_session_attempts_resume_and_preserves_launch_failure() {
    let f = Fixture::new();
    f.fake_claude();
    let first = f.claude_json(&["run", "--provider", "claude", "--skill", "x", "--json"]);
    // Claude storage is an internal detail: missing local evidence must permit resume.
    fs::remove_dir_all(f.home.join(".claude")).unwrap();
    let resumed = f.claude_json(&["run", "--json"]);
    assert_eq!(resumed["run"]["sessionId"], first["run"]["sessionId"]);
    let mut state = f.state();
    let unknown_id = uuid::Uuid::new_v4().to_string();
    state["sessionId"] = Value::from(unknown_id.clone());
    fs::write(f.run_dir().join("state.json"), state.to_string()).unwrap();
    let out = f
        .claude_command(&["run", "--json"])
        .env("CLAUDE_UNKNOWN_ID", "1")
        .output()
        .unwrap();
    assert_error(out, 1, "claude");
    assert_eq!(f.calls(), 3);
    let saved = f.state();
    assert_eq!(saved["sessionId"], unknown_id);
    assert_eq!(saved["state"], "stopped");
    assert_eq!(saved["launches"][2]["stopReason"], "parent_failed");
    assert_eq!(saved["launches"][2]["exitCode"], 1);
    assert!(!f
        .home
        .join(".claude/projects/fake-lossy-workspace")
        .join(format!("{unknown_id}.jsonl"))
        .exists());
    f.assert_unlocked();
}

#[test]
fn claude_result_protocol_errors_and_parent_failures_keep_real_exit_code() {
    for (setting, stop) in [
        ("CLAUDE_NO_RESULT", "error"),
        ("CLAUDE_BAD_RESULT", "error"),
        ("CLAUDE_NULL_RESULT", "error"),
        ("CLAUDE_BAD_ERROR", "error"),
        ("CLAUDE_TRUNCATE", "error"),
        ("CLAUDE_TRAILING_TRUNCATED", "error"),
        ("CLAUDE_IS_ERROR", "parent_failed"),
    ] {
        let f = Fixture::new();
        f.fake_claude();
        let out = f
            .claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"])
            .env(setting, "1")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(1),
            "{setting}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let state = f.state();
        assert_eq!(
            state["state"],
            if stop == "parent_failed" {
                "stopped"
            } else {
                "error"
            },
            "{setting}"
        );
        assert_eq!(state["launches"][0]["stopReason"], stop, "{setting}");
        assert_eq!(state["launches"][0]["exitCode"], 0, "{setting}");
        assert!(state["launches"][0]["finishedAt"].is_string());
        f.assert_unlocked();
    }
    for result in ["CLAUDE_NO_RESULT", "CLAUDE_NULL_RESULT"] {
        let f = Fixture::new();
        f.fake_claude();
        let out = f
            .claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"])
            .env("CLAUDE_EXIT", "42")
            .env(result, "1")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(42),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(f.state()["launches"][0]["stopReason"], "parent_failed");
        assert_eq!(f.state()["launches"][0]["exitCode"], 42);
    }
}

#[test]
fn claude_last_result_large_answer_and_missing_final_newline_are_supported() {
    for size in [0, 131072] {
        let f = Fixture::new();
        f.fake_claude();
        let out = f
            .claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"])
            .env("CLAUDE_FIRST_RESULT", "1")
            .env("CLAUDE_NO_NEWLINE", "1")
            .env("CLAUDE_ANSWER_SIZE", size.to_string())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let answer = fs::read(f.run_dir().join("launches/1/answer.md")).unwrap();
        assert_eq!(answer, vec![b'x'; size]);
        f.assert_unlocked();
    }
}

#[test]
fn claude_permission_mode_must_be_auto_before_session_is_resumable() {
    for mode in [Some("default"), Some("plan"), None] {
        let f = Fixture::new();
        f.fake_claude();
        let mut command =
            f.claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"]);
        match mode {
            Some(mode) => {
                command.env("CLAUDE_PERMISSION_MODE", mode);
            }
            None => {
                command.env("CLAUDE_NO_PERMISSION_MODE", "1");
            }
        }
        let child = Running(Some(
            command
                .env("CLAUDE_BLOCK", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let pids = wait_for_file(&f.root.join("pids"));
        let out = child.finish();
        assert_error(out, 1, "auto");
        let state = f.state();
        assert_eq!(state["state"], "error");
        assert!(
            state["sessionId"].is_null(),
            "unvalidated permission mode {mode:?}"
        );
        assert_eq!(state["launches"][0]["stopReason"], "error");
        let message = state["launches"][0]["message"].as_str().unwrap();
        assert!(
            message.contains("Claude") && message.contains("permission mode"),
            "{message}"
        );
        assert!(!f.root.join("completed").exists());
        for pid in pids.split_whitespace() {
            assert_dead(pid.parse().unwrap());
        }
        let calls = f.calls();
        assert_error(
            f.claude_command(&["run", "--json"]).output().unwrap(),
            1,
            "--fresh",
        );
        assert_eq!(f.calls(), calls);
        f.assert_unlocked();
    }
}

#[test]
fn claude_oversized_init_permission_mode_is_checked_before_parent_continues() {
    for missing_mode in [false, true] {
        let f = Fixture::new();
        f.fake_claude();
        let mut command =
            f.claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"]);
        command
            .env("CLAUDE_INIT_PADDING", "131072")
            .env("CLAUDE_CONTINUATION_DELAY", ".3")
            .env("CLAUDE_BLOCK", "1");
        if missing_mode {
            command.env("CLAUDE_NO_PERMISSION_MODE", "1");
        } else {
            command.env("CLAUDE_PERMISSION_MODE", "default");
        }
        let child = Running(Some(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let pids = wait_for_file(&f.root.join("pids"));
        assert_error(child.finish(), 1, "permission mode");
        assert!(
            !f.root.join("continued-after-init").exists(),
            "oversized init escaped live permission validation (missing mode: {missing_mode})"
        );
        let state = f.state();
        assert_eq!(state["state"], "error");
        assert!(state["sessionId"].is_null());
        assert_eq!(state["launches"][0]["stopReason"], "error");
        for pid in pids.split_whitespace() {
            assert_dead(pid.parse().unwrap());
        }
        f.assert_unlocked();
    }
}

#[test]
fn claude_large_init_assistant_and_tool_events_preserve_valid_session() {
    for large_event in ["init", "assistant", "tool"] {
        let f = Fixture::new();
        f.fake_claude();
        let mut command =
            f.claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"]);
        if large_event == "init" {
            command.env("CLAUDE_INIT_PADDING", "131072");
        } else {
            command.env("CLAUDE_LARGE_EVENT", large_event);
        }
        let out = command.output().unwrap();
        assert!(
            out.status.success(),
            "large {large_event}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let state = f.state();
        assert_eq!(state["state"], "idle");
        assert_eq!(
            state["sessionId"],
            state["launches"][0]["requestedSessionId"]
        );
        assert_eq!(
            fs::read_to_string(f.run_dir().join("launches/1/answer.md")).unwrap(),
            "Claude final answer.\n"
        );
        f.assert_unlocked();
    }
}

#[test]
fn claude_first_observed_id_cannot_validate_a_different_init_id() {
    for resume in [false, true] {
        for live in [false, true] {
            let f = Fixture::new();
            f.fake_claude();
            let previous = resume
                .then(|| f.claude_json(&["run", "--provider", "claude", "--skill", "x", "--json"]));
            let _ = fs::remove_file(f.root.join("completed"));
            let init_id = uuid::Uuid::new_v4().to_string();
            let args = if resume {
                vec!["run", "--json"]
            } else {
                vec!["run", "--provider", "claude", "--skill", "x", "--json"]
            };
            let mut command = f.claude_command(&args);
            // The first event and final result use A (requested/resumed), while
            // init reports B. A valid auto mode alone must not validate A.
            command
                .env("CLAUDE_EARLY_ID", "1")
                .env("CLAUDE_INIT_ID", &init_id);
            let (out, pids) = if live {
                let child = Running(Some(
                    command
                        .env("CLAUDE_BLOCK", "1")
                        .env("CLAUDE_CONTINUATION_DELAY", ".3")
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()
                        .unwrap(),
                ));
                let pids = wait_for_file(&f.root.join("pids"));
                (child.finish(), pids)
            } else {
                (command.output().unwrap(), String::new())
            };
            assert_error(out, 1, "Claude");
            let state = f.state();
            let launch = state["launches"].as_array().unwrap().last().unwrap();
            assert_eq!(state["state"], "error");
            assert_eq!(launch["stopReason"], "error");
            if let Some(previous) = previous {
                assert_eq!(state["sessionId"], previous["run"]["sessionId"]);
            } else {
                assert!(
                    state["sessionId"].is_null(),
                    "a fresh session with inconsistent init identity remained resumable (live: {live})"
                );
                assert_ne!(launch["requestedSessionId"], init_id);
                let calls = f.calls();
                assert_error(
                    f.claude_command(&["run", "--json"]).output().unwrap(),
                    1,
                    "--fresh",
                );
                assert_eq!(f.calls(), calls);
            }
            if live {
                assert!(!f.root.join("continued-after-init").exists());
                assert!(!f.root.join("completed").exists());
            }
            for pid in pids.split_whitespace() {
                assert_dead(pid.parse().unwrap());
            }
            f.assert_unlocked();
        }
    }
}

#[test]
fn claude_init_requested_and_resumed_session_mismatches_stop_immediately() {
    for resume in [false, true] {
        let f = Fixture::new();
        f.fake_claude();
        let previous = resume
            .then(|| f.claude_json(&["run", "--provider", "claude", "--skill", "x", "--json"]));
        let observed = uuid::Uuid::new_v4().to_string();
        let args = if resume {
            vec!["run", "--json"]
        } else {
            vec!["run", "--provider", "claude", "--skill", "x", "--json"]
        };
        let _ = fs::remove_file(f.root.join("completed"));
        let child = Running(Some(
            f.claude_command(&args)
                .env("CLAUDE_OTHER_ID", &observed)
                .env("CLAUDE_BLOCK", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        ));
        let pids = wait_for_file(&f.root.join("pids"));
        assert_error(child.finish(), 1, "Claude");
        let state = f.state();
        let launch = state["launches"].as_array().unwrap().last().unwrap();
        assert_eq!(launch["stopReason"], "error");
        assert_eq!(launch["sessionId"], observed);
        if let Some(previous) = previous {
            assert_eq!(state["sessionId"], previous["run"]["sessionId"]);
        } else {
            assert_ne!(launch["requestedSessionId"], observed);
            assert!(state["sessionId"].is_null());
        }
        assert!(!f.root.join("completed").exists());
        for pid in pids.split_whitespace() {
            assert_dead(pid.parse().unwrap());
        }
        f.assert_unlocked();
    }
}

#[test]
fn claude_init_result_session_mismatch_names_claude() {
    let f = Fixture::new();
    f.fake_claude();
    let out = f
        .claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"])
        .env("CLAUDE_RESULT_ID", uuid::Uuid::new_v4().to_string())
        .output()
        .unwrap();
    assert_error(out, 1, "Claude");
    let state = f.state();
    assert_eq!(state["launches"][0]["stopReason"], "error");
    assert!(state["sessionId"].is_null());
    assert_eq!(
        state["launches"][0]["sessionId"],
        state["launches"][0]["requestedSessionId"]
    );
}

#[test]
fn claude_missing_binary_has_spawn_error_and_install_login_guidance() {
    let f = Fixture::new();
    f.fake_claude();
    fs::remove_file(f.root.join("bin/claude")).unwrap();
    let out = f
        .claude_command(&["run", "--provider", "claude", "--skill", "x", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "spawn");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("claude"), "{text}");
    assert!(text.to_ascii_lowercase().contains("install"), "{text}");
    assert!(text.contains("login") || text.contains("log in"), "{text}");
    assert_eq!(f.calls(), 0);
    assert!(f.state()["sessionId"].is_null());
    f.assert_unlocked();
}

#[cfg(target_os = "linux")]
#[test]
fn claude_timeout_and_interrupt_preserve_reason_after_zero_exit_and_cleanup_setsid_child() {
    for interrupt in [false, true] {
        let f = Fixture::new();
        f.fake_claude();
        let timeout = if interrupt { "30s" } else { "2s" };
        let child = Running(Some(
            f.claude_command(&[
                "run",
                "--provider",
                "claude",
                "--skill",
                "x",
                "--timeout",
                timeout,
                "--json",
            ])
            .env("CLAUDE_BLOCK", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        ));
        let pids = wait_for_file(&f.root.join("pids"));
        wait_for_session(&f, &pids);
        let ids: Vec<u32> = pids
            .split_whitespace()
            .map(|pid| pid.parse().unwrap())
            .collect();
        let stat = fs::read_to_string(format!("/proc/{}/stat", ids[1])).unwrap();
        let fields: Vec<_> = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .collect();
        assert_eq!(fields[2].parse::<u32>().unwrap(), ids[1]);
        if interrupt {
            send(child.0.as_ref().unwrap().id(), 2);
        }
        let out = child.finish();
        assert_eq!(
            out.status.code(),
            Some(if interrupt { 130 } else { 124 }),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        for pid in ids {
            assert_dead(pid);
        }
        let signals = fs::read_to_string(f.root.join("signals")).unwrap();
        assert_eq!(
            signals.lines().next(),
            Some("2"),
            "Claude must receive INT first"
        );
        let state = f.state();
        assert_eq!(state["state"], "stopped");
        assert_eq!(state["launches"][0]["exitCode"], 0);
        assert_eq!(
            state["launches"][0]["stopReason"],
            if interrupt {
                "interrupted"
            } else {
                "timed_out"
            }
        );
        let events = fs::read_to_string(f.run_dir().join("launches/1/events.jsonl")).unwrap();
        let result: Value = serde_json::from_str(events.lines().last().unwrap()).unwrap();
        assert_eq!(result["subtype"], "error_during_execution");
        assert_eq!(result["is_error"], true);
        assert!(result["result"].is_null());
        // A null result has no text to save, matching Cursor's malformed-result behavior.
        assert!(!f.run_dir().join("launches/1/answer.md").exists());
        f.assert_unlocked();
        let resumed = f.claude_json(&["run", "--json"]);
        assert_eq!(resumed["run"]["sessionId"], state["sessionId"]);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn claude_stubborn_parent_escalates_int_then_term_then_kill_with_setsid_cleanup() {
    let f = Fixture::new();
    f.fake_claude();
    let child = Running(Some(
        f.claude_command(&[
            "run",
            "--provider",
            "claude",
            "--skill",
            "x",
            "--timeout",
            "1s",
            "--json",
        ])
        .env("CLAUDE_BLOCK", "1")
        .env("CLAUDE_STUBBORN", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap(),
    ));
    let pids = wait_for_file(&f.root.join("pids"));
    assert_error(child.finish(), 124, "");
    assert_eq!(
        fs::read_to_string(f.root.join("signals"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        ["2", "15"]
    );
    for pid in pids.split_whitespace() {
        assert_dead(pid.parse().unwrap());
    }
    assert_eq!(f.state()["launches"][0]["stopReason"], "timed_out");
    f.assert_unlocked();
}

impl Fixture {
    fn record_workspace(&self) {
        self.json(&["adopt", self.workspace.to_str().unwrap(), "--json"]);
    }
    fn memory(&self) -> PathBuf {
        PathBuf::from(
            self.json(&["handoff", "path", "--json"])["path"]
                .as_str()
                .unwrap(),
        )
    }
    fn handoff(&self) -> Value {
        self.json(&["handoff", "show", "--json"])
    }
    fn provider_run(&self, provider: &str, args: &[&str]) -> Value {
        match provider {
            "cursor" => self.cursor_json(args),
            "claude" => self.claude_json(args),
            _ => self.json(args),
        }
    }
}

#[test]
fn handoff_cli_validates_selectors_json_stdin_and_private_storage() {
    let f = Fixture::new();
    assert_error(f.run(&["handoff", "show"]), 1, "rabi adopt");
    let unrecorded = f.run(&["run", "--skill", "x", "--json"]);
    let envelope: Value = serde_json::from_slice(&unrecorded.stdout).unwrap();
    assert!(envelope["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["code"] == "handoff_unrecorded"));
    assert!(envelope["data"]["launch"]["handoff"].is_null());
    f.record_workspace();
    let memory = f.memory();
    assert!(!memory.exists());
    assert!(f.handoff()["intent"].is_null());
    assert!(!f.root.join("state/rabi/repositories").exists());
    for args in [
        vec!["handoff", "add"],
        vec!["handoff", "add", "--kind", "request", "x"],
        vec!["handoff", "add", " "],
        vec!["handoff", "add", "--kind", "bad", "x"],
    ] {
        assert_error(f.run(&args), 2, "");
    }
    assert_error(f.run(&["handoff", "add", &"é".repeat(4097)]), 2, "8192");
    let entry = f.json(&[
        "handoff",
        "add",
        "--kind",
        "decision",
        &"é".repeat(4096),
        "--json",
    ]);
    assert!(entry["id"].as_str().unwrap().starts_with("ho_"));
    assert_eq!(entry["kind"], "decision");
    assert!(entry.get("source").is_none());
    let mut child = f
        .command(&["handoff", "add", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"stdin note\n\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert_eq!(f.handoff()["entries"][1]["text"], "stdin note");
    let out = f
        .command_at(
            &f.root,
            &[
                "-R",
                f.repo.to_str().unwrap(),
                "handoff",
                "show",
                f.workspace.to_str().unwrap(),
                "--json",
            ],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"],
        f.handoff()
    );
    let out = f
        .command_at(
            &f.repo,
            &["handoff", "show", "."],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap();
    assert_error(out, 4, "primary");
    for name in ["identity.json", "handoff.jsonl", ".lock"] {
        assert_eq!(
            fs::metadata(memory.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    for path in [
        memory.clone(),
        memory.parent().unwrap().to_path_buf(),
        memory.parent().unwrap().parent().unwrap().to_path_buf(),
    ] {
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let state = memory.parent().unwrap().parent().unwrap();
    let key = state.file_name().unwrap().to_str().unwrap();
    assert_eq!(key.len(), 64);
    assert!(key
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let repository: Value =
        serde_json::from_slice(&fs::read(state.join("repository.json")).unwrap()).unwrap();
    assert_eq!(
        repository["commonDir"],
        f.repo
            .join(".git")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(!state.join("runs").exists());
    assert!(!state.join("workspaces").exists());
}

#[test]
fn handoff_add_trims_only_trailing_whitespace_before_validation_and_storage() {
    let f = Fixture::new();
    f.record_workspace();
    let limit = "é".repeat(4096);
    let cases = [
        ("x\n".to_owned(), "x"),
        (
            "  first\n\n \tsecond\r\n\r\n \t\u{2003}".to_owned(),
            "  first\n\n \tsecond",
        ),
        ("- \n".to_owned(), "-"),
        (
            "  first\r\n  second\n\nlast\n".to_owned(),
            "  first\r\n  second\n\nlast",
        ),
        (format!("{limit}\r\n \t\u{2003}"), limit.as_str()),
        (" \r\n\t\u{2003}".to_owned(), ""),
        (format!("{limit}é\n"), ""),
    ];
    let mut stored = Vec::new();
    for (input, expected) in cases {
        for stdin in [false, true] {
            let out = if stdin {
                let mut child = f
                    .command(&["handoff", "add", "--kind", "checkpoint", "-", "--json"])
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap();
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(input.as_bytes())
                    .unwrap();
                child.wait_with_output().unwrap()
            } else {
                f.run(&[
                    "handoff",
                    "add",
                    "--kind",
                    "checkpoint",
                    "--json",
                    "--",
                    &input,
                ])
            };
            if expected.is_empty() {
                assert_error(out, 2, "nonblank UTF-8 and at most 8192 bytes");
            } else {
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                let entry: Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(entry["data"]["text"], expected);
                stored.push(expected.to_owned());
            }
        }
    }
    let snapshot = f.handoff();
    let texts: Vec<_> = snapshot["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(texts, stored);
}

#[test]
fn handoff_injected_heredoc_supplies_stdin_and_stores_a_checkpoint() {
    let f = Fixture::new();
    f.record_workspace();
    f.json(&["run", "--skill", "x", "task", "--json"]);
    let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
    let command = prompt
        .split_once("```sh\n")
        .unwrap()
        .1
        .split_once("\n```")
        .unwrap()
        .0;
    let out = f
        .command_at(&f.workspace, &["-c", command], "bash")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snapshot = f.handoff();
    assert_eq!(snapshot["entries"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["entries"][0]["kind"], "checkpoint");
    assert_eq!(
        snapshot["entries"][0]["text"],
        "Decisions, constraints, verification and remaining work."
    );
}

#[test]
fn handoff_concurrent_appends_and_torn_tail_preserve_history() {
    let f = Fixture::new();
    f.record_workspace();
    let mut children = Vec::new();
    for i in 0..12 {
        children.push(
            f.command(&[
                "handoff",
                "add",
                &format!("writer {i}: {}", "あ".repeat(2000)),
                "--json",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
        );
    }
    for child in children {
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(f.handoff()["entries"].as_array().unwrap().len(), 12);
    let ledger = f.memory().join("handoff.jsonl");
    let before = fs::read(&ledger).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(&ledger)
        .unwrap()
        .write_all(b"{torn")
        .unwrap();
    let out = f.run(&["handoff", "show", "--json"]);
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["data"]["entries"].as_array().unwrap().len(), 12);
    assert_eq!(result["warnings"][0]["code"], "handoff_invalid_line");
    f.json(&["handoff", "add", "after torn tail", "--json"]);
    let after = fs::read(&ledger).unwrap();
    assert!(after.starts_with(&before));
    assert!(String::from_utf8_lossy(&after).contains("{torn\x1e\n{"));
    assert_eq!(f.handoff()["entries"].as_array().unwrap().len(), 13);
    let valid = f.handoff()["entries"][0].clone();
    let mut unknown = valid.clone();
    unknown["schema"] = Value::from(99);
    let mut unknown_kind = valid;
    unknown_kind["kind"] = Value::from("unknown");
    let mut file = fs::OpenOptions::new().append(true).open(&ledger).unwrap();
    writeln!(file, "{unknown}\n{unknown_kind}").unwrap();
    assert_eq!(f.handoff()["entries"].as_array().unwrap().len(), 13);
}

#[test]
fn handoff_identity_mismatch_reads_empty_and_writes_archive() {
    let f = Fixture::new();
    f.record_workspace();
    f.json(&["handoff", "add", "old incarnation", "--json"]);
    let memory = f.memory();
    let records = f.repo.join(".git/rabi/workspaces");
    let record = fs::read_dir(records)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut value: Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    value["createdAt"] = Value::from("2026-10-27T12:00:00.000Z");
    fs::write(record, serde_json::to_vec(&value).unwrap()).unwrap();
    let before = fs::read(memory.join("identity.json")).unwrap();
    let out = f.run(&["handoff", "show", "--json"]);
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(result["data"]["entries"].as_array().unwrap().is_empty());
    assert_eq!(result["warnings"][0]["code"], "handoff_identity");
    assert_eq!(fs::read(memory.join("identity.json")).unwrap(), before);
    let out = f.run(&["run", "--skill", "x", "new task", "--json"]);
    assert!(out.status.success());
    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(envelope["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["code"] == "handoff_identity"));
    assert!(f.handoff()["entries"].as_array().unwrap().is_empty());
    assert_eq!(f.handoff()["intent"], "new task");
    let stale = fs::read_dir(memory.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_string_lossy().contains(".stale-"))
        .unwrap();
    assert!(fs::read_to_string(stale.join("handoff.jsonl"))
        .unwrap()
        .contains("old incarnation"));
}

#[test]
fn handoff_all_providers_capture_inject_grant_and_attribute_appends() {
    for provider in ["codex", "cursor", "claude"] {
        let f = Fixture::new();
        f.record_workspace();
        if provider == "cursor" {
            f.fake_cursor();
        }
        if provider == "claude" {
            f.fake_claude();
        }
        let binary = f.root.join("bin").join(match provider {
            "cursor" => "cursor-agent",
            "claude" => "claude",
            _ => "codex",
        });
        let script = fs::read_to_string(&binary).unwrap().replace("(root/'cwd').write_text(os.getcwd())", r#"(root/'cwd').write_text(os.getcwd())
(root/'run-env.json').write_text(json.dumps({k:os.environ.get(k) for k in ['RABI_BIN','RABI_RUN_ID','RABI_RUN_LAUNCH','RABI_RUN_PROVIDER','RABI_WORKSPACE_ID']}))
if os.environ.get('ADD_HANDOFF'):
    subprocess.run([os.environ['RABI_BIN'], 'handoff', 'add', '--kind', 'checkpoint', 'parent checkpoint'], check=True, stdout=subprocess.DEVNULL)
"#);
        fs::write(&binary, script).unwrap();
        let first = f.provider_run(
            provider,
            &[
                "run",
                "--provider",
                provider,
                "--skill",
                "x",
                "original task",
                "--json",
            ],
        );
        let memory = f.memory();
        let rabi_bin = fs::canonicalize(env!("CARGO_BIN_EXE_rabi")).unwrap();
        let command = if rabi_bin
            .as_os_str()
            .as_encoded_bytes()
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || b"/_-.@%+=:,".contains(b))
        {
            rabi_bin.to_str().unwrap().to_owned()
        } else {
            format!("'{}'", rabi_bin.to_str().unwrap().replace('\'', "'\\''"))
        };
        let reminder_command = format!("{command} handoff add --kind checkpoint - <<'EOF'\nDecisions, constraints, verification and remaining work.\nEOF");
        assert_eq!(f.handoff()["intent"], "original task");
        let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
        assert!(prompt.starts_with("Record durable context"));
        assert!(!prompt.contains("Original task"));
        assert_eq!(prompt.matches("original task").count(), 1);
        assert!(prompt.contains(&reminder_command));
        assert!(prompt.ends_with("Follow skill `x`.\n\noriginal task"));
        assert_eq!(
            first["launch"]["handoff"]["bytes"].as_u64().unwrap() as usize,
            prompt.find("You are the parent").unwrap()
        );
        let calls: Vec<Value> = fs::read_to_string(f.root.join("argv.jsonl"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect();
        let args = calls[0].as_array().unwrap();
        let grant = args.iter().position(|a| a == "--add-dir").unwrap();
        assert_eq!(args[grant + 1], memory.to_str().unwrap());
        let env: Value =
            serde_json::from_slice(&fs::read(f.root.join("run-env.json")).unwrap()).unwrap();
        assert_eq!(env["RABI_BIN"], rabi_bin.to_str().unwrap());
        assert_eq!(env["RABI_RUN_ID"], first["run"]["id"]);
        assert_eq!(env["RABI_RUN_LAUNCH"], "1");
        assert_eq!(env["RABI_RUN_PROVIDER"], provider);
        let mut cmd = f.command(&["run", "--json"]);
        if provider != "codex" {
            cmd.env("PATH", f.root.join("bin"));
        }
        let out = cmd.env("ADD_HANDOFF", "1").output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let source = f.handoff()["entries"][0]["source"].clone();
        assert_eq!(source["runId"], first["run"]["id"]);
        assert_eq!(source["launch"], 2);
        assert_eq!(source["provider"], provider);
        f.json(&["handoff", "add", "foreign human note", "--json"]);
        let resumed = f.provider_run(provider, &["run", "--json"]);
        let prompt = fs::read_to_string(f.run_dir().join("launches/3/prompt.md")).unwrap();
        assert!(prompt.contains(&reminder_command));
        assert!(prompt.contains("foreign human note"));
        assert!(!prompt.contains("parent checkpoint"));
        assert!(!prompt.contains("original task"));
        assert!(prompt.ends_with("Continue."));
        assert_eq!(
            resumed["launch"]["handoff"]["injectedEntryIds"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let fresh = f.provider_run(
            provider,
            &[
                "run",
                "--fresh",
                "--provider",
                provider,
                "--skill",
                "y",
                "new request",
                "--json",
            ],
        );
        assert_ne!(fresh["run"]["id"], first["run"]["id"]);
        let snapshot = f.handoff();
        assert_eq!(snapshot["intent"], "original task");
        assert_eq!(snapshot["entries"][2]["kind"], "request");
        assert_eq!(snapshot["entries"][2]["text"], "new request");
        let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
        assert!(prompt.contains("original task"));
        assert!(prompt.contains("parent checkpoint"));
        assert!(prompt.contains(&reminder_command));
        assert_eq!(prompt.matches("new request").count(), 1);
        let before = fs::read(memory.join("handoff.jsonl")).unwrap();
        let disabled = f.provider_run(
            provider,
            &["run", "--no-handoff", "disabled task", "--json"],
        );
        assert!(disabled["launch"]["handoff"].is_null());
        assert_eq!(
            fs::read_to_string(f.root.join("input")).unwrap(),
            "disabled task"
        );
        assert_eq!(fs::read(memory.join("handoff.jsonl")).unwrap(), before);
        let last: Value = serde_json::from_str(
            fs::read_to_string(f.root.join("argv.jsonl"))
                .unwrap()
                .lines()
                .last()
                .unwrap(),
        )
        .unwrap();
        assert!(!last.as_array().unwrap().contains(&Value::from("--add-dir")));
        let env: Value =
            serde_json::from_slice(&fs::read(f.root.join("run-env.json")).unwrap()).unwrap();
        assert!(env["RABI_BIN"].is_null());
        assert!(env["RABI_RUN_ID"].is_null());
    }
}

#[test]
fn handoff_resume_uses_snapshot_time_with_legacy_started_at_fallback() {
    for legacy in [false, true] {
        let f = Fixture::new();
        f.record_workspace();
        let old = f.json(&["handoff", "add", "already delivered", "--json"]);
        let first = f.json(&["run", "--skill", "x", "original task", "--json"]);
        let snapshot_at = first["launch"]["handoff"]["snapshotAt"].as_str().unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(snapshot_at).unwrap()
                <= chrono::DateTime::parse_from_rfc3339(
                    first["launch"]["startedAt"].as_str().unwrap()
                )
                .unwrap()
        );
        assert_eq!(
            first["launch"]["handoff"]["injectedEntryIds"],
            serde_json::json!([old["id"]])
        );
        let note = f.json(&["handoff", "add", "after snapshot before start", "--json"]);
        let mut state = f.state();
        let launch = &mut state["launches"][0];
        if legacy {
            launch["startedAt"] = snapshot_at.into();
            launch["handoff"]
                .as_object_mut()
                .unwrap()
                .remove("snapshotAt");
        } else {
            // Model a note appended after snapshot read but before prompt fsync
            // and startedAt publication, without a timing-sensitive race.
            launch["startedAt"] = note["createdAt"].clone();
        }
        fs::write(
            f.run_dir().join("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let resumed = f.json(&["run", "--json"]);
        assert_eq!(
            resumed["launch"]["handoff"]["injectedEntryIds"],
            serde_json::json!([note["id"]])
        );
        let prompt = fs::read_to_string(f.run_dir().join("launches/2/prompt.md")).unwrap();
        assert!(prompt.contains("after snapshot before start"));
        assert!(!prompt.contains("already delivered"));
        assert!(!prompt.contains("Original task"));
    }
}

#[test]
fn handoff_resume_retries_undelivered_snapshot_after_immediate_provider_failure() {
    for prior_delivery in [false, true] {
        let f = Fixture::new();
        f.record_workspace();
        f.json(&["run", "--skill", "x", "original task", "--json"]);
        if !prior_delivery {
            f.json(&["run", "--fresh", "--no-handoff", "--skill", "x", "--json"]);
        }
        let note = f.json(&["handoff", "add", "unseen human note", "--json"]);
        let calls = f.calls();
        let out = f
            .command(&["run", "--json"])
            .env("FAKE_NO_THREAD", "1")
            .env("FAKE_EXIT", "1")
            .output()
            .unwrap();
        assert_error(out, 1, "");
        assert_eq!(f.calls(), calls + 1); // Binary/preparation preflight succeeded.
        let state = f.state();
        let failed = &state["launches"][1];
        assert!(!state["sessionId"].is_null()); // A run-level session id is not evidence for this launch.
        assert!(failed["sessionId"].is_null());
        assert_eq!(failed["exitCode"], 1);
        assert_eq!(
            failed["handoff"]["injectedEntryIds"],
            serde_json::json!([note["id"]])
        );
        let resumed = f.json(&["run", "--json"]);
        assert_eq!(
            resumed["launch"]["handoff"]["injectedEntryIds"],
            serde_json::json!([note["id"]])
        );
        let prompt = fs::read_to_string(f.run_dir().join("launches/3/prompt.md")).unwrap();
        assert!(prompt.contains("unseen human note"));
        assert_eq!(prompt.contains("Original task"), !prior_delivery);
        let again = f.json(&["run", "--json"]);
        assert!(again["launch"]["handoff"]["injectedEntryIds"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn handoff_resume_accepts_observed_session_or_zero_exit_as_delivery() {
    for observed_session in [false, true] {
        let f = Fixture::new();
        f.record_workspace();
        f.json(&["run", "--skill", "x", "original task", "--json"]);
        let note = f.json(&["handoff", "add", "delivered note", "--json"]);
        let mut command = f.command(&["run", "--json"]);
        if observed_session {
            command.env("FAKE_EXIT", "1");
        } else {
            command.env("FAKE_NO_THREAD", "1");
        }
        // No-session exit 0 still reports a protocol error to the caller.
        assert_error(command.output().unwrap(), 1, "");
        let state = f.state();
        let delivered = &state["launches"][1];
        assert_eq!(delivered["sessionId"].is_string(), observed_session);
        assert_eq!(delivered["exitCode"], if observed_session { 1 } else { 0 });
        assert_eq!(
            delivered["handoff"]["injectedEntryIds"],
            serde_json::json!([note["id"]])
        );
        let resumed = f.json(&["run", "--json"]);
        assert!(resumed["launch"]["handoff"]["injectedEntryIds"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn handoff_heredoc_intent_and_requests_deduplicate_trailing_whitespace_verbatim() {
    let f = Fixture::new();
    f.record_workspace();
    let task = "  same task\nsecond line";
    let out = f
        .command_at(
            &f.workspace,
            &[
                "-c",
                "\"$1\" run --skill x - --json <<'EOF'\n  same task\nsecond line\nEOF\n",
                "handoff-test",
                env!("CARGO_BIN_EXE_rabi"),
            ],
            "bash",
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let intent = format!("{task}\n");
    for repeated in [task.to_owned(), format!("{task}\r\n \t\u{2003}")] {
        f.json(&["run", "--fresh", "--skill", "x", &repeated, "--json"]);
        assert_eq!(
            fs::read_to_string(f.memory().join("intent.md")).unwrap(),
            intent
        );
        assert!(f.handoff()["entries"].as_array().unwrap().is_empty());
        let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
        assert!(!prompt.contains("Original task"));
        assert_eq!(prompt.matches(task).count(), 1);
        assert!(prompt.ends_with(&repeated));
    }
    let request = "  next request\nkeep internal lines\n";
    f.json(&["run", request, "--json"]);
    f.json(&["handoff", "add", "intervening note", "--json"]);
    let ledger = fs::read(f.memory().join("handoff.jsonl")).unwrap();
    for repeated in [
        request.trim_end().to_owned(),
        format!("{request}\r\n \t\u{2003}"),
    ] {
        f.json(&["run", "--fresh", "--skill", "x", &repeated, "--json"]);
        assert_eq!(fs::read(f.memory().join("handoff.jsonl")).unwrap(), ledger);
        assert_eq!(f.handoff()["entries"][0]["text"], request);
        let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
        assert_eq!(prompt.matches(request.trim_end()).count(), 1);
        assert!(prompt.ends_with(&repeated));
    }
}

#[test]
fn handoff_resume_uses_last_injection_across_disabled_and_failed_launches() {
    for failed_preflight in [false, true] {
        let f = Fixture::new();
        f.record_workspace();
        f.json(&["run", "--skill", "x", "original task", "--json"]);
        let note = f.json(&["handoff", "add", "unseen human note", "--json"]);
        if failed_preflight {
            // Keep the fake provider isolated while making the binary check fail.
            f.fake_claude();
            let binary = f.root.join("bin/codex");
            let saved = f.root.join("saved-codex");
            fs::rename(&binary, &saved).unwrap();
            let out = f
                .command(&["run", "--json"])
                .env("PATH", f.root.join("bin"))
                .output()
                .unwrap();
            assert_error(out, 1, "not found on PATH");
            fs::rename(saved, binary).unwrap();
        } else {
            f.json(&["run", "--no-handoff", "--json"]);
        }
        assert!(f.state()["launches"][1]["handoff"].is_null());
        let resumed = f.json(&["run", "--json"]);
        assert_eq!(
            resumed["launch"]["handoff"]["injectedEntryIds"],
            serde_json::json!([note["id"]])
        );
        let prompt = fs::read_to_string(f.run_dir().join("launches/3/prompt.md")).unwrap();
        assert!(prompt.contains("unseen human note"));
        assert!(!prompt.contains("Original task"));
        let again = f.json(&["run", "--json"]);
        assert!(again["launch"]["handoff"]["injectedEntryIds"]
            .as_array()
            .unwrap()
            .is_empty());
    }
}

#[test]
fn handoff_resume_without_prior_injection_carries_full_snapshot() {
    let f = Fixture::new();
    f.record_workspace();
    f.json(&["run", "--skill", "x", "original task", "--json"]);
    let note = f.json(&["handoff", "add", "older human note", "--json"]);
    f.json(&["run", "--fresh", "--no-handoff", "--skill", "x", "--json"]);
    assert!(f.state()["launches"][0]["handoff"].is_null());
    let resumed = f.json(&["run", "--json"]);
    assert_eq!(
        resumed["launch"]["handoff"]["injectedEntryIds"],
        serde_json::json!([note["id"]])
    );
    let prompt = fs::read_to_string(f.run_dir().join("launches/2/prompt.md")).unwrap();
    assert!(prompt.contains("## Original task (verbatim)\n  original task\n"));
    assert!(prompt.contains("older human note"));
}

#[test]
fn handoff_fresh_retry_after_capture_does_not_duplicate_intent() {
    let f = Fixture::new();
    f.record_workspace();
    let task = "Run cmd < file and check Vec<String>";
    let out = f
        .command(&["run", "--skill", "x", task, "--json"])
        .env("FAKE_NO_THREAD", "1")
        .env("FAKE_EXIT", "1")
        .output()
        .unwrap();
    assert_error(out, 1, "");
    assert_eq!(f.handoff()["intent"], task);
    assert!(f.state()["sessionId"].is_null());
    let failed_run = f.state()["id"].clone();
    let fresh = f.json(&["run", "--fresh", "--skill", "x", task, "--json"]);
    assert_ne!(fresh["run"]["id"], failed_run);
    let snapshot = f.handoff();
    assert_eq!(snapshot["intent"], task);
    assert!(snapshot["entries"].as_array().unwrap().is_empty());
    let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
    assert!(!prompt.contains("Original task"));
    assert_eq!(prompt.matches(task).count(), 1);
    assert!(prompt.ends_with(task));

    // A different task still carries the original command and type verbatim.
    f.json(&["run", "--fresh", "--skill", "x", "different task", "--json"]);
    let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
    assert!(prompt.contains(&format!("## Original task (verbatim)\n  {task}\n")));
    assert_eq!(prompt.matches(task).count(), 1);
}

#[test]
fn handoff_cross_provider_switch_stdin_and_no_handoff_exact_prompt() {
    let f = Fixture::new();
    f.record_workspace();
    f.fake_claude();
    let task = "  original from stdin\ninternal line\r\n\n \t\u{2003}";
    let mut child = f
        .command(&[
            "run",
            "--provider",
            "claude",
            "--skill",
            "design",
            "-",
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(task.as_bytes())
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    assert_eq!(
        fs::read(f.memory().join("intent.md")).unwrap(),
        task.as_bytes()
    );
    f.json(&[
        "handoff",
        "add",
        "--kind",
        "decision",
        "design approved",
        "--json",
    ]);
    f.json(&[
        "run",
        "--fresh",
        "--skill",
        "implement",
        "  implement it\nnext line\r\n \t\u{2003}",
        "--json",
    ]);
    assert_eq!(f.state()["provider"], "codex");
    let snapshot = f.handoff();
    assert_eq!(snapshot["intent"], task);
    assert_eq!(
        snapshot["entries"][1]["text"],
        "  implement it\nnext line\r\n \t\u{2003}"
    );
    let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
    assert!(prompt.contains("design approved"));
    assert!(prompt.contains(&task.replace('\n', "\n  ")));
    assert!(!prompt.contains("\\u{a}"));
    let fresh = f.json(&[
        "run",
        "--fresh",
        "--no-handoff",
        "--skill",
        "legacy",
        "task",
        "--json",
    ]);
    assert!(fresh["launch"]["handoff"].is_null());
    assert_eq!(fs::read_to_string(f.root.join("input")).unwrap(), format!("You are the parent agent for this rabi run in workspace {} on branch feat/parent. You manage progress: delegate implementation and review to your own subagents, gather the information you need yourself (git, gh, tests), give follow-up instructions until the work is really done, and decide completion.\n\nFollow skill `legacy`.\n\ntask", f.workspace.display()));
}

#[test]
fn handoff_refuses_symlinks_and_repository_collision_before_spawn() {
    let f = Fixture::new();
    f.record_workspace();
    f.json(&["handoff", "add", "saved", "--json"]);
    let memory = f.memory();
    let target = f.root.join("untouched");
    fs::write(&target, "untouched").unwrap();
    for name in ["identity.json", "handoff.jsonl", "intent.md", ".lock"] {
        let file = memory.join(name);
        let backup = memory.join(format!("{name}.backup"));
        if file.exists() {
            fs::rename(&file, &backup).unwrap();
        }
        std::os::unix::fs::symlink(&target, &file).unwrap();
        assert_error(f.run(&["handoff", "add", "blocked"]), 1, "Handoff");
        let calls = f.calls();
        assert_error(
            f.run(&["run", "--fresh", "--skill", "x"]),
            1,
            "--no-handoff",
        );
        assert_eq!(f.calls(), calls);
        fs::remove_file(&file).unwrap();
        if backup.exists() {
            fs::rename(backup, file).unwrap();
        }
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched");
    let parent = memory.parent().unwrap();
    let real = parent.join("saved-memory");
    fs::rename(&memory, &real).unwrap();
    std::os::unix::fs::symlink(&real, &memory).unwrap();
    assert_error(f.run(&["handoff", "show"]), 1, "Handoff");
    assert_error(f.run(&["handoff", "path"]), 1, "Handoff");
    fs::remove_file(&memory).unwrap();
    fs::rename(&real, &memory).unwrap();
    let identity = memory.join("identity.json");
    let saved_identity = fs::read(&identity).unwrap();
    fs::remove_file(&identity).unwrap();
    assert!(f.handoff()["entries"].as_array().unwrap().is_empty());
    assert_error(
        f.run(&["run", "--fresh", "--skill", "x"]),
        1,
        "--no-handoff",
    );
    assert_error(f.run(&["handoff", "add", "blocked"]), 1, "no identity.json");
    fs::write(&identity, saved_identity).unwrap();
    let manifest = parent.parent().unwrap().join("repository.json");
    fs::write(&manifest, r#"{"schema":1,"commonDir":"/different/.git"}"#).unwrap();
    assert_error(f.run(&["handoff", "show"]), 1, "commonDir mismatch");
    assert_error(
        f.run(&["run", "--fresh", "--skill", "x"]),
        1,
        "--no-handoff",
    );
    f.json(&["run", "--fresh", "--no-handoff", "--skill", "x", "--json"]);
}

#[test]
fn handoff_retained_on_forget_and_invalid_source_omitted() {
    let f = Fixture::new();
    f.record_workspace();
    let out = f
        .command(&["handoff", "add", "human", "--json"])
        .env("RABI_RUN_ID", "run_00000000-0000-4000-8000-000000000000")
        .env("RABI_RUN_LAUNCH", "1")
        .env("RABI_RUN_PROVIDER", "codex")
        .output()
        .unwrap();
    assert!(out.status.success());
    let entry: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(entry["data"].get("source").is_none());
    let memory = f.memory();
    let before = fs::read(memory.join("handoff.jsonl")).unwrap();
    f.json(&["forget", ".", "--json"]);
    assert_eq!(fs::read(memory.join("handoff.jsonl")).unwrap(), before);
    assert_error(f.run(&["handoff", "show"]), 1, "rabi adopt");
}

#[test]
fn handoff_full_show_request_limits_output_env_and_no_handoff_first_launch() {
    let f = Fixture::new();
    f.record_workspace();
    let memory = f.memory();
    f.json(&["run", "--no-handoff", "--skill", "x", "omitted", "--json"]);
    assert!(!memory.exists());
    assert!(!f.root.join("state/rabi/repositories").exists());
    f.json(&["run", "--json"]);
    assert!(memory.exists());
    assert!(f.handoff()["intent"].is_null());
    f.json(&["run", "first nonblank", "--json"]);
    assert_eq!(f.handoff()["intent"], "first nonblank");
    let large = "q".repeat(65536);
    f.json(&["run", "--fresh", "--skill", "x", &large, "--json"]);
    assert_eq!(f.handoff()["entries"][0]["text"], large);
    let before = fs::read(memory.join("handoff.jsonl")).unwrap();
    let out = f.run(&["run", &"q".repeat(65537), "--json"]);
    assert!(out.status.success());
    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(envelope["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w["code"] == "handoff_request_skipped"));
    assert_eq!(fs::read(memory.join("handoff.jsonl")).unwrap(), before);
    let out = f.run(&["handoff", "show"]);
    assert!(out.status.success());
    assert!(String::from_utf8(out.stdout).unwrap().contains(&large));
    let out = f
        .command(&["handoff", "add", "--", "--literal text"])
        .env("RABI_OUTPUT", "json")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"]["text"],
        "--literal text"
    );
    let mut child = f
        .command(&["handoff", "add", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"\xff").unwrap();
    assert_error(child.wait_with_output().unwrap(), 2, "UTF-8");
    let alias = f.root.join("workspace-alias");
    std::os::unix::fs::symlink(&f.workspace, &alias).unwrap();
    let out = f
        .command_at(
            &alias,
            &["handoff", "path", "--json"],
            env!("CARGO_BIN_EXE_rabi"),
        )
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["data"]["path"],
        memory.to_str().unwrap()
    );
}

#[test]
fn handoff_complete_json_without_newline_never_becomes_committed() {
    let f = Fixture::new();
    f.record_workspace();
    let entry = f.json(&["handoff", "add", "uncommitted", "--json"]);
    let ledger = f.memory().join("handoff.jsonl");
    fs::write(&ledger, serde_json::to_vec(&entry).unwrap()).unwrap();
    assert!(f.handoff()["entries"].as_array().unwrap().is_empty());
    f.json(&["handoff", "add", "committed", "--json"]);
    let snapshot = f.handoff();
    assert_eq!(snapshot["entries"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["entries"][0]["text"], "committed");
    assert!(fs::read(&ledger)
        .unwrap()
        .windows(2)
        .any(|w| w == b"\x1e\n"));
}

#[test]
fn handoff_missing_provider_preserves_memory_and_repeated_requests_are_deduplicated() {
    for provider in ["codex", "cursor", "claude"] {
        let f = Fixture::new();
        f.record_workspace();
        // Installs git/python and isolates PATH for every missing-binary check.
        f.fake_claude();
        fs::remove_file(f.root.join("bin/claude")).unwrap();
        let memory = f.memory();
        if provider == "codex" {
            fs::remove_file(f.root.join("bin/codex")).unwrap();
        }
        let out = f
            .claude_command(&[
                "run",
                "--provider",
                provider,
                "--skill",
                "x",
                "failed intent",
                "--json",
            ])
            .env("PATH", f.root.join("bin"))
            .output()
            .unwrap();
        assert_error(out, 1, "not found on PATH");
        assert!(!memory.exists());
        f.json(&["handoff", "add", "existing note", "--json"]);
        let before = fs::read(memory.join("handoff.jsonl")).unwrap();
        assert!(!memory.join("intent.md").exists());
        fs::write(memory.join("intent.md"), "preserved intent").unwrap();
        let out = f
            .claude_command(&[
                "run",
                "--fresh",
                "--provider",
                provider,
                "--skill",
                "x",
                "failed task",
                "--json",
            ])
            .env("PATH", f.root.join("bin"))
            .output()
            .unwrap();
        assert_error(out, 1, "not found on PATH");
        assert_eq!(fs::read(memory.join("handoff.jsonl")).unwrap(), before);
        assert_eq!(
            fs::read_to_string(memory.join("intent.md")).unwrap(),
            "preserved intent"
        );
        assert_eq!(f.calls(), 0);
    }

    let f = Fixture::new();
    f.record_workspace();
    f.json(&["run", "--skill", "x", "intent", "--json"]);
    f.json(&["run", "request", "--json"]);
    f.json(&["handoff", "add", "intervening note", "--json"]);
    let before = fs::read(f.memory().join("handoff.jsonl")).unwrap();
    f.json(&["run", "--fresh", "--skill", "x", "request", "--json"]);
    assert_eq!(fs::read(f.memory().join("handoff.jsonl")).unwrap(), before);
    let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
    assert_eq!(prompt.matches("request").count(), 1);
}

#[test]
fn handoff_large_intent_is_verbatim_and_show_makes_control_characters_visible() {
    let f = Fixture::new();
    f.record_workspace();
    let intent = format!("\x1b]52;c;clipboard\x07{}\n", "original ".repeat(25000));
    let mut child = f
        .command(&["run", "--skill", "x", "-", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(intent.as_bytes())
        .unwrap();
    assert!(child.wait_with_output().unwrap().status.success());
    assert_eq!(
        fs::read(f.memory().join("intent.md")).unwrap(),
        intent.as_bytes()
    );
    let note = "note\x1b]0;title\x07\nnext";
    f.json(&["handoff", "add", note, "--json"]);
    let snapshot = f.handoff();
    assert_eq!(snapshot["intent"], intent);
    assert_eq!(snapshot["entries"][0]["text"], note);
    let out = f.run(&["handoff", "show"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains('\x1b'));
    assert!(!text.contains('\x07'));
    assert!(text.contains("\\u{1b}]52;c;clipboard\\u{7}"));
    assert!(text.contains("note\\u{1b}]0;title\\u{7}\n  next\n"));
    assert!(!text.contains("\\u{a}"));
    f.json(&["run", "--fresh", "--skill", "x", "next task", "--json"]);
    let prompt = fs::read_to_string(f.run_dir().join("launches/1/prompt.md")).unwrap();
    assert!(prompt.contains("intent truncated;"));
    assert!(prompt.find("You are the parent").unwrap() <= 24576);
}

#[test]
fn handoff_accepts_symlinks_through_state_root_but_refuses_descendant_symlinks() {
    for initialized in [false, true] {
        for component in ["state", "state/rabi"] {
            let f = Fixture::new();
            f.record_workspace();
            if initialized {
                f.json(&["handoff", "add", "before link", "--json"]);
            }
            let original = f.root.join(component);
            fs::create_dir_all(&original).unwrap();
            let original_memory = f.memory();
            let real = f.root.join("real-state");
            fs::rename(&original, &real).unwrap();
            std::os::unix::fs::symlink(&real, &original).unwrap();
            let memory = f.memory();
            assert_eq!(
                memory,
                real.join(original_memory.strip_prefix(&original).unwrap())
            );
            assert_eq!(memory.exists(), initialized);
            f.json(&["run", "--skill", "x", "task through link", "--json"]);
            let args: Value = serde_json::from_str(
                fs::read_to_string(f.root.join("argv.jsonl"))
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap(),
            )
            .unwrap();
            let args = args.as_array().unwrap();
            let grant = args.iter().position(|a| a == "--add-dir").unwrap();
            assert_eq!(args[grant + 1], memory.to_str().unwrap());
            assert_eq!(memory, memory.canonicalize().unwrap());
            f.json(&["handoff", "add", "after link", "--json"]);
            assert_eq!(
                f.handoff()["entries"].as_array().unwrap().len(),
                if initialized { 2 } else { 1 }
            );
            let repositories = f.root.join("state/rabi/repositories");
            let moved = f.root.join("real-repositories");
            fs::rename(&repositories, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &repositories).unwrap();
            assert_error(f.run(&["handoff", "show"]), 1, "Handoff");
            assert_error(f.run(&["handoff", "add", "blocked"]), 1, "Handoff");
        }
    }
}

#[test]
fn handoff_refuses_hard_links_without_modifying_targets() {
    let f = Fixture::new();
    f.record_workspace();
    f.json(&["run", "--skill", "x", "intent", "--json"]);
    f.json(&["handoff", "add", "note", "--json"]);
    let memory = f.memory();
    for path in [
        memory.join("identity.json"),
        memory.join("intent.md"),
        memory.join("handoff.jsonl"),
        memory.join(".lock"),
        memory
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("repository.json"),
    ] {
        let original = fs::read(&path).unwrap();
        let external = f.root.join("hard-link-target");
        fs::rename(&path, &external).unwrap();
        fs::hard_link(&external, &path).unwrap();
        assert_error(f.run(&["handoff", "show"]), 1, "hard link");
        assert_error(f.run(&["handoff", "add", "blocked"]), 1, "hard link");
        assert_error(f.run(&["run", "blocked"]), 1, "hard link");
        assert_eq!(fs::read(&external).unwrap(), original);
        fs::remove_file(&path).unwrap();
        fs::rename(&external, &path).unwrap();
    }
}

#[test]
fn handoff_busy_lock_refuses_with_bounded_wait_and_run_preserves_status() {
    let f = Fixture::new();
    f.record_workspace();
    f.json(&["handoff", "add", "note", "--json"]);
    let memory = f.memory();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(memory.join(".lock"))
        .unwrap();
    lock.lock().unwrap();
    let before = fs::read(memory.join("handoff.jsonl")).unwrap();
    let started = Instant::now();
    let out = f.run(&["run", "--skill", "x", "blocked", "--json"]);
    assert!(started.elapsed() >= Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(20));
    assert_eq!(out.status.code(), Some(4));
    let envelope: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "refused");
    assert!(envelope["error"]["message"]
        .as_str()
        .unwrap()
        .contains("busy"));
    assert_eq!(fs::read(memory.join("handoff.jsonl")).unwrap(), before);
    assert_eq!(f.calls(), 0);
    // Read-only operations do not wait for the append lock.
    assert_eq!(f.handoff()["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn handoff_workspace_preparation_failure_preserves_memory() {
    let f = Fixture::new();
    f.record_workspace();
    f.fake_cursor();
    f.json(&["handoff", "add", "preserved", "--json"]);
    let memory = f.memory();
    let before = fs::read(memory.join("handoff.jsonl")).unwrap();
    let exclude = f.repo.join(".git/info/exclude");
    fs::create_dir_all(exclude.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(f.root.join("unwritten"), &exclude).unwrap();
    let out = f
        .cursor_command(&[
            "run",
            "--provider",
            "cursor",
            "--skill",
            "x",
            "failed task",
            "--json",
        ])
        .output()
        .unwrap();
    assert_error(out, 1, "Refusing to replace");
    assert_eq!(fs::read(memory.join("handoff.jsonl")).unwrap(), before);
    assert!(!memory.join("intent.md").exists());
    assert_eq!(f.calls(), 0);
}
