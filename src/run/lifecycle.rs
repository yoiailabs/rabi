//! Unix launch lifetime: signal flag, exclusive workspace lock, process group.
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{io, StopReason};
use crate::error::{Error, Result, REFUSED};
use crate::provider::{Provider, Session, StopPolicy};

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
    fn signal(sig: i32, handler: usize) -> usize;
    fn flock(fd: i32, operation: i32) -> i32;
}
static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}

pub(super) struct Signals {
    int: usize,
    term: usize,
}
impl Signals {
    pub fn install() -> Result<Self> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        // SAFETY: handlers only store to a lock-free atomic; no IO/allocation.
        let int = unsafe { signal(2, interrupted as *const () as usize) };
        if int == usize::MAX {
            return Err(Error::msg("Failed to install SIGINT handler"));
        }
        let term = unsafe { signal(15, interrupted as *const () as usize) };
        if term == usize::MAX {
            unsafe {
                signal(2, int);
            }
            return Err(Error::msg("Failed to install SIGTERM handler"));
        }
        Ok(Self { int, term })
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: restore the handlers returned by signal().
        unsafe {
            signal(2, self.int);
            signal(15, self.term);
        }
    }
}

pub(super) struct Lock {
    path: PathBuf,
    file: File,
    remove: bool,
}
impl Lock {
    pub fn acquire(path: PathBuf) -> Result<Self> {
        // A stale inode is flocked before unlinking. Concurrent stale takers
        // cannot remove a newly acquired lock after checking an old pid.
        loop {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(file) => {
                    let mut lock = Self {
                        path,
                        file,
                        remove: true,
                    };
                    lock.exclusive()?;
                    io(writeln!(lock.file, "{}", std::process::id()))?;
                    io(lock.file.sync_all())?;
                    return Ok(lock);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let file = match File::open(&path) {
                        Ok(file) => file,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                        Err(e) => return Err(Error::msg(e.to_string())),
                    };
                    let stale = Self {
                        path: path.clone(),
                        file,
                        remove: false,
                    };
                    stale.exclusive()?;
                    if !stale.same_inode() {
                        continue;
                    }
                    let body = fs::read_to_string(&path).map_err(|_| busy())?;
                    let mut ids = body.split_whitespace();
                    let pid = ids
                        .next()
                        .ok_or_else(busy)?
                        .parse::<u32>()
                        .map_err(|_| busy())?;
                    let pgid = ids
                        .next()
                        .map(str::parse::<u32>)
                        .transpose()
                        .map_err(|_| busy())?;
                    if ids.next().is_some()
                        || crate::process::process_is_alive(pid)
                        || pgid.is_some_and(group_is_alive)
                    {
                        return Err(busy());
                    }
                    // Keep the stale inode flocked through unlink. Propagate
                    // failures instead of retrying an unremovable lock forever.
                    if stale.same_inode() {
                        match fs::remove_file(&path) {
                            Ok(()) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => {
                                return Err(Error::msg(format!(
                                    "Failed to remove stale run lock: {e}"
                                )))
                            }
                        }
                    }
                    drop(stale);
                }
                Err(e) => return Err(Error::msg(e.to_string())),
            }
        }
    }
    fn record_group(&mut self, pgid: u32) -> Result<()> {
        let mut replacement = None;
        // Lock the replacement inode before publishing it, so contenders
        // cannot acquire an unlocked inode during atomic replacement.
        io(crate::atomic::write_with(&self.path, |file| {
            // SAFETY: valid open fd; nonblocking exclusive lock.
            if unsafe { flock(file.as_raw_fd(), 2 | 4) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
            writeln!(file, "{} {pgid}", std::process::id())?;
            replacement = Some(file.try_clone()?);
            Ok(())
        }))?;
        self.file = replacement.ok_or_else(|| Error::msg("Missing replacement run lock"))?;
        Ok(())
    }
    fn exclusive(&self) -> Result<()> {
        // SAFETY: valid open fd; LOCK_EX | LOCK_NB never waits.
        if unsafe { flock(self.file.as_raw_fd(), 2 | 4) } == 0 {
            Ok(())
        } else {
            Err(busy())
        }
    }
    fn same_inode(&self) -> bool {
        match (self.file.metadata(), fs::metadata(&self.path)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        if self.remove && self.same_inode() {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn busy() -> Error {
    Error::status(
        REFUSED,
        "A rabi run already holds this workspace lock (or its pid/process group cannot be verified)",
    )
}

pub(super) fn group_is_alive(pgid: u32) -> bool {
    let Some(pgid) = i32::try_from(pgid).ok().filter(|id| *id > 0) else {
        return true;
    };
    // SAFETY: positive pgid; signal 0 only probes the isolated process group.
    let status = unsafe { kill(-pgid, 0) };
    status == 0 || std::io::Error::last_os_error().raw_os_error() != Some(3)
}

struct Group(Child, StopPolicy);
impl Group {
    fn send(&self, sig: i32) {
        if let Ok(pid) = i32::try_from(self.0.id()) {
            // SAFETY: CommandExt::process_group(0) isolated the child group.
            unsafe {
                kill(-pid, sig);
            }
        }
    }
    fn stop(&mut self) {
        self.send(self.1.signal);
        // Keep the group alive through the grace period even if its leader
        // exits: grandchildren may ignore SIGTERM. Always escalate the group.
        let start = Instant::now();
        while start.elapsed() < self.1.grace {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.send(self.1.force_signal);
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        self.send(self.1.force_signal);
        let _ = self.0.wait();
    }
}

pub(super) struct Outcome {
    pub parent_exit: i32,
    pub exit: i32,
    pub reason: StopReason,
}

pub(super) fn launch(
    provider: Provider,
    workspace: &Path,
    dir: &Path,
    session: Session<'_>,
    timeout: Duration,
    lock: &mut Lock,
    on_session: impl FnMut(String) -> Result<bool>,
) -> Result<Outcome> {
    let mut cmd = provider.command(workspace, dir, session)?;
    cmd.stdout(io(File::create(dir.join("events.jsonl")))?)
        .stderr(io(File::create(dir.join("stderr.log")))?)
        .process_group(0);
    let events = crate::provider::SessionIdReader::open(provider, &dir.join("events.jsonl")).ok();
    if INTERRUPTED.load(Ordering::Relaxed) {
        return Ok(Outcome {
            parent_exit: 130,
            exit: 130,
            reason: StopReason::Interrupted,
        });
    }
    // Fail closed if the launcher dies between spawning and publishing the
    // child's pgid. Zero is unverifiable, never evidence of a dead group.
    lock.record_group(0)?;
    let group = Group(
        cmd.spawn().map_err(|e| Error::Spawn {
            command: provider.binary().into(),
            details: e.to_string(),
        })?,
        provider.stop_policy(),
    );
    lock.record_group(group.0.id())?;
    monitor(group, timeout, events, on_session)
}

fn monitor<R: std::io::Read>(
    mut group: Group,
    timeout: Duration,
    mut events: Option<crate::provider::SessionIdReader<R>>,
    mut on_session: impl FnMut(String) -> Result<bool>,
) -> Result<Outcome> {
    let start = Instant::now();
    loop {
        let reason = if INTERRUPTED.load(Ordering::Relaxed) {
            Some((StopReason::Interrupted, 130))
        } else if start.elapsed() >= timeout {
            Some((StopReason::TimedOut, 124))
        } else {
            None
        };
        if let Some((reason, exit)) = reason {
            group.stop();
            let status = io(group.0.wait())?;
            return Ok(Outcome {
                parent_exit: code(status),
                exit,
                reason,
            });
        }
        if let Some(reader) = events.as_mut() {
            match reader.poll() {
                Ok(Some(id)) => {
                    // A true callback result requests a controlled error stop
                    // after saving the observed session. Persistence errors
                    // still propagate, with Group reaping before finalization.
                    if on_session(id)? {
                        group.stop();
                        let status = io(group.0.wait())?;
                        return Ok(Outcome {
                            parent_exit: code(status),
                            exit: 1,
                            reason: StopReason::Error,
                        });
                    }
                    events = None;
                }
                // Observation is best effort: defer unreadable or oversized
                // events to the final scan, preserving parent work and status.
                Err(_) => events = None,
                Ok(None) => {}
            }
        }
        if let Some(status) = io(group.0.try_wait())? {
            let exit = code(status);
            return Ok(Outcome {
                parent_exit: exit,
                exit,
                reason: if exit == 0 {
                    StopReason::Completed
                } else {
                    StopReason::ParentFailed
                },
            });
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn code(status: ExitStatus) -> i32 {
    status
        .code()
        .unwrap_or_else(|| 128 + status.signal().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn session_callback_can_request_a_controlled_error_stop() {
        for stop in [false, true] {
            let path = std::env::temp_dir().join(format!("rabi-callback-{}", uuid::Uuid::new_v4()));
            let pause = if stop { "60" } else { ".05" };
            let child = std::process::Command::new("sh")
                .args([
                    "-c",
                    &format!(
                        "trap 'exit 42' TERM; printf ready; sleep {pause}; printf completed; exit 42"
                    ),
                ])
                .stdout(File::create(&path).unwrap())
                .stderr(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            let group = Group(child, Provider::Codex.stop_policy());
            let start = Instant::now();
            while fs::read_to_string(&path).unwrap().is_empty() {
                assert!(start.elapsed() < Duration::from_secs(5), "child not ready");
                std::thread::sleep(Duration::from_millis(10));
            }
            let event = b"{\"type\":\"thread.started\",\"thread_id\":\"observed-thread\"}\n";
            let mut calls = 0;
            let result = monitor(
                group,
                Duration::from_secs(5),
                Some(crate::provider::SessionIdReader::new(
                    Provider::Codex,
                    std::io::Cursor::new(event),
                )),
                |id| {
                    calls += 1;
                    assert_eq!(id, "observed-thread");
                    Ok(stop)
                },
            )
            .unwrap();
            assert_eq!(calls, 1);
            assert_eq!(result.parent_exit, 42, "preserve the observed TERM status");
            assert_eq!(result.exit, if stop { 1 } else { 42 });
            assert!(matches!(
                (stop, result.reason),
                (true, StopReason::Error) | (false, StopReason::ParentFailed)
            ));
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                if stop { "ready" } else { "readycompleted" }
            );
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn observer_read_failure_does_not_stop_child_or_change_its_status() {
        struct FailedRead(Rc<Cell<usize>>);
        impl std::io::Read for FailedRead {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0.set(self.0.get() + 1);
                Err(std::io::Error::other("injected event read failure"))
            }
        }
        for exit in [0, 42] {
            let path = std::env::temp_dir().join(format!("rabi-observer-{}", uuid::Uuid::new_v4()));
            let child = std::process::Command::new("sh")
                .args(["-c", &format!("sleep .05; printf finished; exit {exit}")])
                .stdout(File::create(&path).unwrap())
                .process_group(0)
                .spawn()
                .unwrap();
            let reads = Rc::new(Cell::new(0));
            let result = monitor(
                Group(child, Provider::Codex.stop_policy()),
                Duration::from_secs(5),
                Some(crate::provider::SessionIdReader::new(
                    Provider::Codex,
                    FailedRead(reads.clone()),
                )),
                |_| panic!("a failed observer must not report a session"),
            )
            .unwrap();
            assert_eq!(reads.get(), 1, "failed observation must be disabled");
            assert_eq!(result.parent_exit, exit);
            assert_eq!(result.exit, exit);
            assert!(matches!(
                (exit, result.reason),
                (0, StopReason::Completed) | (42, StopReason::ParentFailed)
            ));
            assert_eq!(fs::read_to_string(&path).unwrap(), "finished");
            fs::remove_file(path).unwrap();
        }
    }
}
