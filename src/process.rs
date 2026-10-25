use std::cell::Cell;
use std::io::Read;
use std::marker::PhantomData;
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

use crate::error::{Error, Result};

thread_local! {
    static GIT_HOOK_SUPPRESSION: Cell<usize> = const { Cell::new(0) };
}

// A scoped setting also covers Git calls made by shared worktree/git helpers.
// Keep it thread-local, without changing the user's config or process environment.
pub(crate) struct GitHookSuppression(PhantomData<Rc<()>>);

pub(crate) fn suppress_git_hooks() -> GitHookSuppression {
    GIT_HOOK_SUPPRESSION.with(|depth| depth.set(depth.get() + 1));
    GitHookSuppression(PhantomData)
}

impl Drop for GitHookSuppression {
    fn drop(&mut self) {
        GIT_HOOK_SUPPRESSION.with(|depth| depth.set(depth.get() - 1));
    }
}

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    if program == "git" && GIT_HOOK_SUPPRESSION.with(|depth| depth.get() > 0) {
        // Status queries during sync and read-only plans must not refresh the index.
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        cmd.args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ]);
    }
    if crate::runtime::non_interactive() {
        if program == "git" {
            cmd.env("GIT_TERMINAL_PROMPT", "0");
        }
        if program == "gh" || crate::runtime::is_hook() {
            cmd.stdin(Stdio::null());
        }
    }
    cmd
}

#[derive(Debug)]
pub struct Captured {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

pub fn raw(command: &str, args: &[&str], cwd: Option<&Path>) -> Result<Captured> {
    raw_with_env(command, args, cwd, &[])
}

pub fn raw_with_env(
    command: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
) -> Result<Captured> {
    raw_with_stdin(command, args, cwd, env, None)
}

pub(crate) fn raw_with_stdin(
    command: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    input: Option<&Path>,
) -> Result<Captured> {
    let output = raw_bytes(command, args, cwd, env, input)?;
    Ok(Captured {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        code: output.status.code().unwrap_or(1),
    })
}

pub(crate) fn raw_bytes(
    command: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    input: Option<&Path>,
) -> Result<std::process::Output> {
    let mut cmd = self::command(command);
    cmd.args(args);
    for (key, value) in env {
        cmd.env(key, value);
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    if let Some(input) = input {
        cmd.stdin(std::fs::File::open(input).map_err(|error| Error::msg(error.to_string()))?);
    }
    cmd.output().map_err(|error| spawn_error(command, &error))
}

pub fn capture(
    command: &str,
    args: &[&str],
    cwd: Option<&Path>,
    trim_stdout: bool,
) -> Result<Captured> {
    let mut cmd = self::command(command);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output().map_err(|error| spawn_error(command, &error))?;
    let stdout_raw = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr_raw = String::from_utf8_lossy(&output.stderr).into_owned();
    let code = output.status.code().unwrap_or(1);
    if output.status.success() {
        return Ok(Captured {
            stdout: if trim_stdout {
                stdout_raw.trim().to_string()
            } else {
                stdout_raw
            },
            stderr: stderr_raw.trim().to_string(),
            code,
        });
    }
    if !stderr_raw.trim().is_empty() {
        return Err(Error::msg(stderr_raw.trim().to_string()));
    }
    Ok(Captured {
        stdout: if trim_stdout {
            stdout_raw.trim().to_string()
        } else {
            stdout_raw
        },
        stderr: stderr_raw.trim().to_string(),
        code,
    })
}

pub fn inherit(
    command: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    redirect_stdout_to_stderr: bool,
) -> Result<i32> {
    let mut cmd = self::command(command);
    cmd.args(args)
        .stdin(
            if crate::runtime::non_interactive() && crate::runtime::is_hook() {
                Stdio::null()
            } else {
                Stdio::inherit()
            },
        )
        .stderr(Stdio::inherit());
    if redirect_stdout_to_stderr {
        cmd.stdout(Stdio::from(std::io::stderr()));
    } else {
        cmd.stdout(Stdio::inherit());
    }
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    let status = cmd.status().map_err(|error| spawn_error(command, &error))?;
    exit_code(command, status)
}

pub fn run_with_timeout(command: &str, args: &[&str], timeout: Duration) -> Result<Captured> {
    raw_with_timeout(command, args, None, &[], timeout)
}

/// Drain both pipes while waiting, and bound the entire capture (including EOF).
/// A timed-out gh must not leave a child holding stdout/stderr open indefinitely.
pub(crate) fn raw_with_timeout(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
    timeout: Duration,
) -> Result<Captured> {
    let mut cmd = self::command(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (key, value) in env {
        cmd.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|error| spawn_error(program, &error))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    // Separate readers avoid pipe-buffer deadlocks. Messages also let the caller
    // enforce the deadline when a descendant inherits an otherwise exited pipe.
    fn drain(
        pipe: impl Read + Send + 'static,
        sender: std::sync::mpsc::Sender<(bool, std::io::Result<String>)>,
        stdout: bool,
    ) {
        std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut text = String::new();
            let result = pipe.read_to_string(&mut text).map(|_| text);
            let _ = sender.send((stdout, result));
        });
    }
    if let Some(pipe) = child.stdout.take() {
        drain(pipe, sender.clone(), true);
    }
    if let Some(pipe) = child.stderr.take() {
        drain(pipe, sender, false);
    }
    let started = std::time::Instant::now();
    let mut stdout = None;
    let mut stderr = None;
    loop {
        while let Ok((is_stdout, result)) = receiver.try_recv() {
            if is_stdout {
                stdout = Some(result);
            } else {
                stderr = Some(result);
            }
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| Error::msg(error.to_string()))?
        {
            if stdout.is_some() && stderr.is_some() {
                return Ok(Captured {
                    stdout: stdout
                        .take()
                        .transpose()
                        .map_err(|error| Error::msg(error.to_string()))?
                        .unwrap_or_default(),
                    stderr: stderr
                        .take()
                        .transpose()
                        .map_err(|error| Error::msg(error.to_string()))?
                        .unwrap_or_default(),
                    code: status.code().unwrap_or(1),
                });
            }
        }
        if started.elapsed() >= timeout {
            #[cfg(unix)]
            if let Ok(pid) = i32::try_from(child.id()) {
                extern "C" {
                    fn kill(pid: i32, sig: i32) -> i32;
                }
                // SAFETY: the child was spawned in its own process group.
                unsafe {
                    kill(-pid, 9);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::msg(format!(
                "{program} timed out after {} ms",
                timeout.as_millis()
            )));
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn exit_code(command: &str, status: std::process::ExitStatus) -> Result<i32> {
    if let Some(code) = status.code() {
        return Ok(code);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            let name = signal_name(signal);
            return Err(Error::Signaled(name));
        }
    }
    Err(Error::Spawn {
        command: command.to_string(),
        details: "process exited without a status".to_string(),
    })
}

fn signal_name(signal: i32) -> String {
    match signal {
        15 => "SIGTERM".to_string(),
        2 => "SIGINT".to_string(),
        9 => "SIGKILL".to_string(),
        1 => "SIGHUP".to_string(),
        other => format!("SIG{other}"),
    }
}

fn spawn_error(command: &str, error: &std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::NotFound {
        return Error::Spawn {
            command: command.to_string(),
            details: "ENOENT".to_string(),
        };
    }
    Error::Spawn {
        command: command.to_string(),
        details: error.to_string(),
    }
}

pub(crate) fn process_is_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        // SAFETY: signal 0 does not deliver a signal. It only reports whether `pid` exists.
        let status = unsafe { kill(pid, 0) };
        // Only ESRCH proves absence. EPERM and other probe failures are unknown.
        status == 0 || std::io::Error::last_os_error().raw_os_error() != Some(3)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_capture_drains_both_pipes_without_deadlock() {
        let result = raw_with_timeout(
            "sh",
            &[
                "-c",
                "head -c 131072 /dev/zero; head -c 131072 /dev/zero >&2",
            ],
            None,
            &[],
            Duration::from_secs(3),
        )
        .unwrap();
        assert_eq!(result.code, 0);
        assert_eq!(result.stdout.len(), 131072);
        assert_eq!(result.stderr.len(), 131072);
    }
}
