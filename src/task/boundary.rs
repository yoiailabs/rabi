//! Shared launch environment and lifetime for task agents, not a same-user sandbox.
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Error, Result};

pub(crate) struct Boundary {
    path: PathBuf,
}
impl Boundary {
    pub fn new() -> Result<Self> {
        let path = std::env::temp_dir().join(format!("rabi-task-agent-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .map_err(|_| Error::msg("Cannot create task agent temporary directory"))?;
        let boundary = Self { path };
        fs::create_dir(boundary.path.join("github"))
            .map_err(|_| Error::msg("Cannot create empty task agent GitHub configuration"))?;
        Ok(boundary)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Apply last, after any caller environment. The owner must outlive the child.
    pub fn prepare(&self, command: &mut Command) {
        prepare_environment(command);
        command.env("GH_CONFIG_DIR", self.path.join("github"));
    }

    #[cfg(test)]
    pub fn git_scope(&self) -> crate::process::TaskGitEnvironment {
        let mut command = Command::new("git");
        self.prepare(&mut command);
        crate::process::task_git_environment(&command)
    }
}

/// Rabi reads need credential isolation, but never invoke gh or need temporary files.
pub(crate) fn git_read_scope() -> crate::process::TaskGitEnvironment {
    let mut command = Command::new("git");
    prepare_environment(&mut command);
    crate::process::task_git_environment(&command)
}

fn prepare_environment(command: &mut Command) {
    let explicit: Vec<_> = command
        .get_envs()
        .map(|(key, _)| key.to_os_string())
        .collect();
    for key in std::env::vars_os().map(|(key, _)| key).chain(explicit) {
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
        "SSH_ASKPASS",
    ] {
        command.env_remove(key);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("RABI_AGENT", "1");
    // Command-scoped config takes precedence over system, user and repository
    // config. Empty helper resets the complete accumulated helper list.
    let settings = [
        ("credential.helper", ""),
        ("core.askPass", ""),
        ("credential.interactive", "false"),
        // OpenSSH options supported on Linux and macOS. No agents or prompts;
        // on-disk keys remain accessible, as do all other same-user files.
        (
            "core.sshCommand",
            "ssh -o BatchMode=yes -o IdentitiesOnly=yes -o IdentityAgent=none",
        ),
    ];
    // Codex's default secret filter removes names containing KEY. Use
    // Git's quoted parameter format so these settings reach its tool shells
    // without disabling the provider's default environment exclusions.
    let parameters = settings
        .into_iter()
        .map(|(key, value)| format!("'{}'", format!("{key}={value}").replace('\'', "'\\''")))
        .collect::<Vec<_>>()
        .join(" ");
    command.env("GIT_CONFIG_PARAMETERS", parameters);
}

impl Drop for Boundary {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

static INTERRUPTED: AtomicBool = AtomicBool::new(false);
extern "C" fn interrupted(_: i32) {
    INTERRUPTED.store(true, Ordering::Relaxed);
}
pub(super) struct Signals {
    int: libc::sighandler_t,
    term: libc::sighandler_t,
}
impl Signals {
    pub fn install() -> Result<Self> {
        INTERRUPTED.store(false, Ordering::Relaxed);
        // SAFETY: handlers only store to a lock-free atomic; restore on drop.
        let int =
            unsafe { libc::signal(libc::SIGINT, interrupted as *const () as libc::sighandler_t) };
        if int == libc::SIG_ERR {
            return Err(Error::msg("Cannot install task agent signal handler"));
        }
        let term = unsafe {
            libc::signal(
                libc::SIGTERM,
                interrupted as *const () as libc::sighandler_t,
            )
        };
        if term == libc::SIG_ERR {
            unsafe {
                libc::signal(libc::SIGINT, int);
            }
            return Err(Error::msg("Cannot install task agent signal handler"));
        }
        Ok(Self { int, term })
    }
    pub fn interrupted(&self) -> bool {
        INTERRUPTED.load(Ordering::Relaxed)
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        // SAFETY: restore handlers returned by signal.
        unsafe {
            libc::signal(libc::SIGINT, self.int);
            libc::signal(libc::SIGTERM, self.term);
        }
    }
}

/// Reserve the leader PID until group cleanup, including after a normal exit.
pub(super) struct Process(pub Child);
impl Process {
    pub fn exit_code(&self) -> Result<Option<i32>> {
        // SAFETY: zero initializes siginfo_t; waitid fills it for our child.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
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
                Err(Error::msg("Cannot observe task agent exit"))
            };
        }
        // SAFETY: waitid initializes these fields, zero PID means still running.
        if unsafe { info.si_pid() } == 0 {
            return Ok(None);
        }
        let status = unsafe { info.si_status() };
        Ok(Some(if info.si_code == libc::CLD_EXITED {
            status
        } else {
            128 + status
        }))
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        if let Ok(group) = i32::try_from(self.0.id()) {
            // SAFETY: caller spawns the child with process_group(0).
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        let _ = self.0.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::OsStr;

    #[test]
    fn boundary_overrides_explicit_credentials_and_git_configuration() {
        let boundary = Boundary::new().unwrap();
        let mut command = Command::new("unused");
        for key in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "SSH_AUTH_SOCK",
            "GIT_ASKPASS",
            "SSH_ASKPASS",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_KEY_70",
            "GIT_CONFIG_VALUE_70",
            "GIT_UNKNOWN_SELECTOR",
        ] {
            command.env(key, "fixture");
        }
        boundary.prepare(&mut command);
        let environment: BTreeMap<_, _> = command.get_envs().collect();
        for key in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
            "SSH_AUTH_SOCK",
            "GIT_ASKPASS",
            "SSH_ASKPASS",
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_CONFIG_KEY_70",
            "GIT_CONFIG_VALUE_70",
            "GIT_UNKNOWN_SELECTOR",
        ] {
            assert_eq!(environment.get(OsStr::new(key)), Some(&None), "{key}");
        }
        assert_eq!(
            environment.get(OsStr::new("GIT_CONFIG_PARAMETERS")),
            Some(&Some(OsStr::new("'credential.helper=' 'core.askPass=' 'credential.interactive=false' 'core.sshCommand=ssh -o BatchMode=yes -o IdentitiesOnly=yes -o IdentityAgent=none'")))
        );
    }
}
