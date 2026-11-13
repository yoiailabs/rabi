//! A fresh classifier invocation, separate from resumable workspace agents.
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::{fs::DirBuilderExt, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Provider {
    #[default]
    Claude,
}

impl Provider {
    pub(super) fn parse_cli(value: &str) -> crate::error::Result<Self> {
        match value {
            "claude" => Ok(Self::Claude),
            _ => Err(crate::error::Error::validation(
                "Classifier provider must be claude",
            )),
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(10 * 60);
const ANSWER_BYTES: u64 = 64 * 1024;
type Failure = &'static str;

struct Temporary(PathBuf);
impl Temporary {
    fn new() -> Result<Self, Failure> {
        let path = std::env::temp_dir().join(format!("rabi-classifier-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|_| "classifier:temporary-directory")?;
        let temporary = Self(path);
        for directory in ["workspace", "github"] {
            fs::create_dir(temporary.0.join(directory))
                .map_err(|_| "classifier:temporary-directory")?;
        }
        Ok(temporary)
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Every early return terminates and reaps the whole child group. Output uses
// files, so a blocked stdin writer or inherited pipe can never outlive a timeout.
struct Process(Child);
impl Process {
    fn exited_successfully(&self) -> Result<Option<bool>, Failure> {
        // SAFETY: zero is a valid initial siginfo_t; waitid fills it for this child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // WNOWAIT leaves the exited leader as a zombie, reserving its PID/PGID
        // until Drop kills the group and reaps the leader, in that order.
        // SAFETY: info is writable and P_PID selects our unreaped child.
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                self.0.id(),
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 {
            return if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                Ok(None)
            } else {
                Err("classifier:wait-failed")
            };
        }
        // SAFETY: waitid initialized these fields (zero PID means still running).
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        // SAFETY: WEXITED returned an exit or signal status for this child.
        Ok(Some(
            info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0,
        ))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Ok(group) = i32::try_from(self.0.id()) {
            // SAFETY: process_group(0) creates this child's isolated group.
            unsafe { libc::kill(-group, libc::SIGKILL) };
        }
        let _ = self.0.wait();
    }
}

fn command(provider: Provider, binary: &Path, temporary: &Path, model: Option<&str>) -> Command {
    let mut command = Command::new(binary);
    match provider {
        Provider::Claude => {
            command.args([
                "-p",
                "--tools",
                "",
                "--safe-mode",
                "--output-format",
                "text",
                "--strict-mcp-config",
                "--mcp-config",
                "{\"mcpServers\":{}}",
                "--setting-sources",
                "",
                "--settings",
                "{\"disableAllHooks\":true}",
                "--disable-slash-commands",
                "--no-session-persistence",
                "--permission-prompts",
                "none",
            ]);
        }
    }
    if let Some(model) = model {
        command.arg("--model").arg(model);
    }
    let cwd = temporary.join("workspace");
    command
        .current_dir(&cwd)
        .env("PWD", &cwd)
        .env_remove("OLDPWD")
        .env("RABI_AGENT", "1")
        .env("GH_CONFIG_DIR", temporary.join("github"))
        .stderr(Stdio::null())
        .process_group(0);
    // Remove all Git selectors and configuration, including inherited alternate
    // object stores, rather than merely clearing GIT_DIR and GIT_WORK_TREE.
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    for key in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "SSH_AUTH_SOCK",
        "GIT_ASKPASS",
        "RABI_REPO_PATH",
        "RABI_WORKSPACE_PATH",
        "RABI_WORKSPACE",
        "RABI_BIN",
    ] {
        command.env_remove(key);
    }
    command
}

pub(super) fn run(
    provider: Provider,
    prompt: &str,
    model: Option<&str>,
) -> Result<String, Failure> {
    let binary = crate::provider::resolve_binary_in(
        std::env::var_os("PATH").as_deref(),
        &[provider.binary()],
    )
    .ok_or("classifier:provider-not-found")?;
    let temporary = Temporary::new()?;
    let input = temporary.0.join("prompt.txt");
    fs::write(&input, prompt).map_err(|_| "classifier:prompt-write")?;
    let mut command = command(provider, &binary, &temporary.0, model);
    command.stdin(File::open(input).map_err(|_| "classifier:prompt-read")?);
    let answer = temporary.0.join("answer.json");
    command.stdout(File::create(&answer).map_err(|_| "classifier:answer-write")?);
    let process = Process(command.spawn().map_err(|_| "classifier:launch-failed")?);
    // Test-only override can shorten the deadline, never extend it or disable it.
    let timeout = std::env::var("RABI_TEST_CLASSIFIER_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(TIMEOUT)
        .min(TIMEOUT);
    let started = Instant::now();
    loop {
        if started.elapsed() >= timeout {
            return Err("classifier:timeout");
        }
        if fs::metadata(&answer).is_ok_and(|metadata| metadata.len() > ANSWER_BYTES) {
            return Err("classifier:answer-too-large");
        }
        if let Some(success) = process.exited_successfully()? {
            if !success {
                return Err("classifier:non-zero-exit");
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Kill any surviving descendants before reading a finalized answer.
    drop(process);
    let mut bytes = Vec::new();
    File::open(answer)
        .map_err(|_| "classifier:missing-answer")?
        .take(ANSWER_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "classifier:answer-read")?;
    if bytes.len() as u64 > ANSWER_BYTES {
        return Err("classifier:answer-too-large");
    }
    String::from_utf8(bytes).map_err(|_| "classifier:invalid-answer-encoding")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observing_exit_keeps_the_leader_waitable_until_group_cleanup() {
        for (script, success) in [
            ("exit 0", true),
            ("exit 7", false),
            ("kill -TERM $$", false),
        ] {
            let mut command = Command::new("sh");
            command.args(["-c", script]).process_group(0);
            let process = Process(command.spawn().expect("Spawn fixture leader"));
            let pid = process.0.id() as libc::pid_t;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(observed) = process.exited_successfully().expect("Observe leader") {
                    assert_eq!(observed, success);
                    break;
                }
                assert!(Instant::now() < deadline, "Fixture leader did not exit");
                std::thread::sleep(Duration::from_millis(10));
            }
            // A second observation succeeds only while the zombie is unreaped.
            assert_eq!(process.exited_successfully(), Ok(Some(success)));
            drop(process);
            let mut status = 0;
            // SAFETY: pid identifies the fixture child; status is writable.
            assert_eq!(
                unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) },
                -1
            );
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ECHILD)
            );
        }
    }
}
