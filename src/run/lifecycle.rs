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

// Cursor tool shells may call setsid(), escaping their parent's process group.
// Linux /proc snapshots retain their identities before signaling the leader.
// This is best effort: children that reparent before a snapshot may be missed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct Descendant {
    pid: i32,
    started: u64,
}

#[cfg(target_os = "linux")]
fn process_identity(pid: i32) -> Option<(i32, u64)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm may contain spaces and parentheses; fields after its final ')' are
    // state (3), ppid (4), ... starttime (22).
    let fields: Vec<_> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

// Snapshot admission is separate from signaling so PID reuse can be tested
// without depending on kernel PID allocation or sending signals in a test.
#[cfg(target_os = "linux")]
fn refresh_saved_descendants(
    saved: &mut Vec<Descendant>,
    leader: Option<Descendant>,
    reaped: bool,
    processes: &[(Descendant, i32)],
) {
    saved.retain(|saved| processes.iter().any(|(current, _)| saved == current));
    // Reaping frees the leader PID for reuse. Afterwards only descendants saved
    // before reap and still matching their starttime may remain signal targets;
    // neither a replacement leader nor saved descendants admit new processes.
    if reaped {
        return;
    }
    let Some(leader) =
        leader.filter(|leader| processes.iter().any(|(current, _)| leader == current))
    else {
        return;
    };
    loop {
        let mut added = false;
        for (process, parent) in processes {
            if process.pid != leader.pid
                && (*parent == leader.pid || saved.iter().any(|saved| saved.pid == *parent))
                && !saved.iter().any(|saved| saved.pid == process.pid)
            {
                saved.push(*process);
                added = true;
            }
        }
        if !added {
            break;
        }
    }
}

struct Group {
    child: Child,
    policy: StopPolicy,
    descendants: Vec<Descendant>,
    reaped: bool,
    #[cfg(target_os = "linux")]
    leader: Option<Descendant>,
}
impl Group {
    fn new(child: Child, policy: StopPolicy) -> Self {
        #[cfg(target_os = "linux")]
        let leader = i32::try_from(child.id()).ok().and_then(|pid| {
            let (_, started) = process_identity(pid)?;
            Some(Descendant { pid, started })
        });
        Self {
            child,
            policy,
            descendants: Vec::new(),
            reaped: false,
            #[cfg(target_os = "linux")]
            leader,
        }
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    // All Child reaping goes through these methods, including Drop, so later
    // cleanup cannot accidentally treat a released numeric PID as the leader.
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        let status = self.child.wait()?;
        self.reaped = true;
        Ok(status)
    }

    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        let status = self.child.try_wait()?;
        if status.is_some() {
            self.reaped = true;
        }
        Ok(status)
    }

    fn refresh_descendants(&mut self) {
        if self.policy.term_grace.is_none() {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            let Ok(entries) = fs::read_dir("/proc") else {
                return;
            };
            let processes: Vec<_> = entries
                .filter_map(|entry| {
                    let pid = entry.ok()?.file_name().to_str()?.parse::<i32>().ok()?;
                    let (parent, started) = process_identity(pid)?;
                    Some((Descendant { pid, started }, parent))
                })
                .collect();
            refresh_saved_descendants(&mut self.descendants, self.leader, self.reaped, &processes);
        }
    }

    fn send(&self, sig: i32) {
        // Signal the process group for every provider, even after the leader is
        // reaped: surviving members keep the group (and so its id) alive, which
        // is what lets the existing Codex cleanup reach them. Only the /proc
        // ancestry root is dropped after reap; see refresh_saved_descendants.
        if let Ok(pid) = i32::try_from(self.id()) {
            // SAFETY: CommandExt::process_group(0) isolated the child group.
            unsafe {
                kill(-pid, sig);
            }
        }
        #[cfg(target_os = "linux")]
        for process in &self.descendants {
            // Check starttime to avoid signaling a different process after PID
            // reuse. Linux does not offer an atomic check-and-kill via /proc;
            // this narrow race is why descendant cleanup remains best effort.
            if process_identity(process.pid).is_some_and(|(_, started)| started == process.started)
            {
                // SAFETY: identity checked above; pid is a child observed in /proc.
                unsafe {
                    kill(process.pid, sig);
                }
            }
        }
    }

    fn grace(&mut self, duration: Duration) {
        // Keep escalating even if the leader exits: descendants may ignore INT
        // or TERM. Refresh snapshots to include newly spawned tool children.
        let start = Instant::now();
        while start.elapsed() < duration {
            self.refresh_descendants();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) {
        self.refresh_descendants();
        self.send(self.policy.signal);
        self.grace(self.policy.grace);
        if let Some(grace) = self.policy.term_grace {
            self.send(15);
            self.grace(grace);
        }
        self.send(self.policy.force_signal);
    }
}
impl Drop for Group {
    fn drop(&mut self) {
        self.refresh_descendants();
        self.send(self.policy.force_signal);
        let _ = self.wait();
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
    let group = Group::new(
        cmd.spawn().map_err(|e| Error::Spawn {
            command: provider.binary().into(),
            details: e.to_string(),
        })?,
        provider.stop_policy(),
    );
    lock.record_group(group.id())?;
    monitor(group, timeout, events, on_session)
}

fn monitor<R: std::io::Read>(
    mut group: Group,
    timeout: Duration,
    mut events: Option<crate::provider::SessionIdReader<R>>,
    mut on_session: impl FnMut(String) -> Result<bool>,
) -> Result<Outcome> {
    let start = Instant::now();
    let mut last_snapshot = None;
    loop {
        if last_snapshot.is_none_or(|at: Instant| at.elapsed() >= Duration::from_millis(50)) {
            group.refresh_descendants();
            last_snapshot = Some(Instant::now());
        }
        let reason = if INTERRUPTED.load(Ordering::Relaxed) {
            Some((StopReason::Interrupted, 130))
        } else if start.elapsed() >= timeout {
            Some((StopReason::TimedOut, 124))
        } else {
            None
        };
        if let Some((reason, exit)) = reason {
            group.stop();
            let status = io(group.wait())?;
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
                        let status = io(group.wait())?;
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
        if let Some(status) = io(group.try_wait())? {
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

    #[cfg(target_os = "linux")]
    #[test]
    fn descendant_snapshots_reject_reused_leader_pid() {
        let leader = Descendant {
            pid: 10,
            started: 100,
        };
        let child = Descendant {
            pid: 20,
            started: 200,
        };
        let grandchild = Descendant {
            pid: 30,
            started: 300,
        };
        let mut saved = Vec::new();
        // Snapshot order must not matter for multi-generation discovery.
        refresh_saved_descendants(
            &mut saved,
            Some(leader),
            false,
            &[(grandchild, 20), (child, 10), (leader, 1)],
        );
        assert_eq!(saved, vec![child, grandchild]);
        let replacement = Descendant {
            pid: 10,
            started: 400,
        };
        let unrelated = Descendant {
            pid: 40,
            started: 500,
        };
        for reaped in [false, true] {
            let mut saved = vec![child, grandchild];
            refresh_saved_descendants(
                &mut saved,
                Some(leader),
                reaped,
                &[
                    (replacement, 1),
                    (unrelated, 10),
                    (child, 1),
                    (grandchild, 20),
                ],
            );
            assert_eq!(saved, vec![child, grandchild], "reaped: {reaped}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reaped_leader_freezes_admission_and_rejects_reused_saved_pids() {
        let leader = Descendant {
            pid: 10,
            started: 100,
        };
        let child = Descendant {
            pid: 20,
            started: 200,
        };
        let new_child = Descendant {
            pid: 30,
            started: 300,
        };
        let new_grandchild = Descendant {
            pid: 40,
            started: 350,
        };
        let mut saved = vec![child];
        // Even a matching leader snapshot and a verified saved descendant must
        // not admit new targets after wait/try_wait have reaped the leader.
        refresh_saved_descendants(
            &mut saved,
            Some(leader),
            true,
            &[
                (leader, 1),
                (child, 1),
                (new_child, 10),
                (new_grandchild, 20),
            ],
        );
        assert_eq!(saved, vec![child]);
        let replacement = Descendant {
            pid: 20,
            started: 400,
        };
        refresh_saved_descendants(
            &mut saved,
            Some(leader),
            true,
            &[(replacement, 1), (new_child, 20)],
        );
        assert!(saved.is_empty());
    }

    #[test]
    fn child_wait_methods_mark_reaped() {
        for provider in [Provider::Codex, Provider::Cursor] {
            for poll in [false, true] {
                let child = std::process::Command::new("sh")
                    .args(["-c", "exit 0"])
                    .process_group(0)
                    .spawn()
                    .unwrap();
                let mut group = Group::new(child, provider.stop_policy());
                assert!(!group.reaped);
                if poll {
                    let start = Instant::now();
                    while group.try_wait().unwrap().is_none() {
                        assert!(start.elapsed() < Duration::from_secs(5));
                        std::thread::sleep(Duration::from_millis(10));
                    }
                } else {
                    group.wait().unwrap();
                }
                assert!(group.reaped);
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn running(pid: i32) -> bool {
        // Zombies awaiting their new parent's reap count as stopped.
        fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .and_then(|(_, rest)| rest.split_whitespace().next())
                .is_some_and(|state| state != "Z" && state != "X")
        })
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn group_members_are_killed_after_the_leader_exits_and_is_reaped() {
        for provider in [Provider::Codex, Provider::Cursor] {
            let path =
                std::env::temp_dir().join(format!("rabi-group-survivor-{}", uuid::Uuid::new_v4()));
            // The leader leaves a same-group child behind and exits normally.
            let child = std::process::Command::new("sh")
                .arg("-c")
                .arg(format!(
                    "sh -c 'trap \"\" INT TERM; sleep 30' & echo $! > {}; exit 0",
                    path.display()
                ))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .process_group(0)
                .spawn()
                .unwrap();
            let mut group = Group::new(child, provider.stop_policy());
            let start = Instant::now();
            while group.try_wait().unwrap().is_none() {
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            let pid: i32 = loop {
                if let Ok(text) = fs::read_to_string(&path) {
                    if let Ok(pid) = text.trim().parse() {
                        break pid;
                    }
                }
                assert!(start.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            };
            assert!(running(pid), "survivor should still run");
            drop(group);
            let start = Instant::now();
            while running(pid) {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "{provider:?} left a group member running after leader reap"
                );
                std::thread::sleep(Duration::from_millis(10));
            }
            fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn cursor_stop_sends_int_then_term_before_kill() {
        let path =
            std::env::temp_dir().join(format!("rabi-cursor-signals-{}", uuid::Uuid::new_v4()));
        let child = std::process::Command::new("sh")
            .args([
                "-c",
                "trap 'printf INT' INT; trap 'printf TERM' TERM; printf ready; while :; do :; done",
            ])
            .stdout(File::create(&path).unwrap())
            .stderr(std::process::Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut group = Group::new(child, Provider::Cursor.stop_policy());
        let start = Instant::now();
        while fs::read_to_string(&path).unwrap().is_empty() {
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(10));
        }
        group.stop();
        assert_eq!(code(group.wait().unwrap()), 137);
        assert_eq!(fs::read_to_string(&path).unwrap(), "readyINTTERM");
        fs::remove_file(path).unwrap();
    }

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
            let group = Group::new(child, Provider::Codex.stop_policy());
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
                Group::new(child, Provider::Codex.stop_policy()),
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
