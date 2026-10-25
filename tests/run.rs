//! Fake-Codex integration coverage. No real provider or network is used.
#[path = "common/git.rs"]
mod test_git;
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
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
import json, os, pathlib, signal, subprocess, sys, uuid
args = sys.argv[1:]
root = pathlib.Path(os.environ['FAKE_ROOT'])
with (root/'argv.jsonl').open('a') as f: f.write(json.dumps(args)+'\n')
(root/'input').write_text(sys.stdin.read())
sid = args[args.index('resume')+1] if 'resume' in args else str(uuid.uuid4())
sid = os.environ.get('FAKE_OTHER_ID', sid)
if not os.environ.get('FAKE_NO_THREAD'):
    print(json.dumps({'type':'thread.started','thread_id':sid}), flush=True)
    rollout = pathlib.Path(os.environ['CODEX_HOME'])/'sessions/2026/10/25'/('rollout-test-'+sid+'.jsonl')
    rollout.parent.mkdir(parents=True, exist_ok=True)
    rollout.write_text('{}\n')
pathlib.Path(args[args.index('-o')+1]).write_text('Parent final answer.\n')
print('fake stderr', file=sys.stderr, flush=True)
if os.environ.get('FAKE_BLOCK'):
    signal.signal(signal.SIGTERM, signal.SIG_DFL if os.environ.get('FAKE_PARENT_TERM_DEFAULT') else signal.SIG_IGN)
    child = subprocess.Popen([sys.executable, '-c', 'import os,signal,time; signal.signal(signal.SIGTERM, signal.SIG_IGN); os.write(1,b"ready\\n"); time.sleep(600)'], stdout=subprocess.PIPE)
    assert child.stdout.readline() == b'ready\n'
    (root/'pids').write_text(str(os.getpid())+' '+str(child.pid))
    import time
    time.sleep(600)
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
    for contents in [format!("{}\n", std::process::id()), "unverifiable".into()] {
        fs::write(&lock, &contents).unwrap();
        assert_error(f.run(&["run", "--json"]), 4, "lock");
        assert_eq!(fs::read_to_string(&lock).unwrap(), contents);
    }
    fs::remove_file(lock).unwrap();
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
