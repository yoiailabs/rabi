//! Repeatable workspace preparation. Process information is observational only.
use std::path::Path;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::Context;
use crate::error::{Error, Result};
use crate::metadata::{self, MetadataInput};
use crate::output::visible_text;
use crate::{config, process, trust, worktree};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Pending,
    Running,
    Ok,
    Failed,
    Skipped,
    Interrupted,
    Unknown,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    pub state: State,
    pub at: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
}

impl Status {
    pub(crate) fn new(state: State) -> Self {
        Self {
            state,
            at: (state != State::Unknown).then(metadata::iso_now),
            error: None,
            pid: None,
            host: None,
        }
    }
}

fn host() -> Option<String> {
    static HOST: OnceLock<Option<String>> = OnceLock::new();
    HOST.get_or_init(|| {
        process::raw("uname", &["-n"], None)
            .ok()
            .filter(|out| out.code == 0 && !out.stdout.trim().is_empty())
            .map(|out| out.stdout.trim().to_owned())
    })
    .clone()
}

/// Never persists the derived interrupted state and never prevents an operation.
pub(crate) fn display(recorded: bool, saved: Option<&Status>) -> Option<Status> {
    if !recorded {
        return None;
    }
    let mut status = saved
        .cloned()
        .unwrap_or_else(|| Status::new(State::Unknown));
    if status.state == State::Running
        && status.host.is_some()
        && status.host == host()
        && status
            .pid
            .is_some_and(|pid| !process::process_is_alive(pid))
    {
        status.state = State::Interrupted;
    }
    status.pid = None;
    status.host = None;
    Some(status)
}

pub(crate) fn authorize(context: &Context, control: trust::HookControl) -> Result<Vec<String>> {
    trust::authorize(
        &context.git_root,
        config::setup_commands(context.config.as_ref()),
        config::pre_remove_commands(context.config.as_ref()),
        trust::HookKind::Setup,
        control,
    )
}

pub(crate) struct Report {
    pub status: Status,
    pub hooks: worktree::HookOutcome,
    pub copied: Vec<String>,
    pub copy_error: Option<String>,
    pub metadata_error: Option<String>,
}

impl Report {
    pub fn skipped() -> Self {
        Self {
            status: Status::new(State::Skipped),
            hooks: worktree::hook_outcome(false, None),
            copied: Vec::new(),
            copy_error: None,
            metadata_error: None,
        }
    }

    pub fn outcome(&self) -> Value {
        json!({
            "state": self.status.state, "at": self.status.at, "error": self.status.error,
            "ran": self.hooks.ran, "ok": self.status.state != State::Failed,
            "command": self.hooks.command, "exitCode": self.hooks.exit_code,
            "signal": self.hooks.signal,
        })
    }

    pub fn copy(&self) -> Value {
        json!({"copied": self.copied, "error": self.copy_error})
    }

    pub fn failed(&self) -> bool {
        self.status.state == State::Failed || self.metadata_error.is_some()
    }
}

fn save(context: &Context, path: &Path, status: &Status) -> Result<()> {
    metadata::write(
        &context.git_root,
        path,
        &MetadataInput {
            setup: Some(status.clone()),
            ..MetadataInput::default()
        },
    )?;
    Ok(())
}

/// Authorization must happen before this function: even copying is a mutation.
/// Creation keeps its established partial-result behavior if metadata cannot be saved.
pub(crate) fn run(
    context: &Context,
    tree: &worktree::WorktreeInfo,
    extra: &[String],
    commands: &[String],
    logger: &worktree::Logger,
    creating: bool,
) -> Result<Report> {
    let path = Path::new(&tree.path);
    let mut running = Status::new(State::Running);
    running.pid = Some(std::process::id());
    running.host = host();
    // If allocation could not create its record, keep that failure intact while
    // still reporting the copy/hook result. Do not invent creation details here.
    let record_status = !creating || tree.recorded;
    let mut metadata_error = match if record_status {
        save(context, path, &running)
    } else {
        Ok(())
    } {
        Ok(()) => None,
        Err(error) if creating => Some(error.to_string()),
        Err(error) => return Err(error),
    };
    let mut copied = Vec::new();
    let copy_error = worktree::copy_files_recorded(&context.git_root, path, extra, &mut copied)
        .err()
        .map(|error| error.to_string());
    let mut operational_error = None;
    let mut failure = None;
    // Hooks depend on the copied files. A retry will finish copying first.
    let ran = copy_error.is_none() && !commands.is_empty();
    if ran {
        logger.log("\nRunning setup commands...");
        match worktree::run_hook_commands(context, tree, commands, true, logger) {
            Ok(result) => failure = result,
            Err(error) => operational_error = Some(error.to_string()),
        }
    }
    let error = copy_error.clone().or(operational_error).or_else(|| {
        failure.as_ref().map(|failure| {
            failure.error.clone().unwrap_or_else(|| {
                format!(
                    "Setup command '{}' failed with exit code {}",
                    failure.command, failure.code
                )
            })
        })
    });
    let mut status = Status::new(if error.is_some() {
        State::Failed
    } else {
        State::Ok
    });
    status.error = error;
    if record_status {
        if let Err(error) = save(context, path, &status) {
            metadata_error.get_or_insert_with(|| error.to_string());
        }
    }
    let hooks = worktree::hook_outcome(ran, failure.as_ref());
    if let Some(error) = &copy_error {
        logger.warn(
            "copy_files",
            &format!(
                "Warning: Failed to copy some files: {}",
                visible_text(error)
            ),
        );
    }
    if let Some(failure) = failure {
        logger.warn("setup", &failure.warning(path));
    }
    Ok(Report {
        status,
        hooks,
        copied,
        copy_error,
        metadata_error,
    })
}

pub(crate) fn execute(
    context: &Context,
    selector: Option<&str>,
    extra: &[String],
    control: trust::HookControl,
    logger: &worktree::Logger,
) -> Result<i32> {
    let tree = crate::selectors::resolve_optional_current(context, selector, false)?;
    if tree.is_default {
        return Err(Error::Status {
            code: "primary",
            message: "Cannot set up the primary checkout: it is the copy source".into(),
        });
    }
    worktree::validate_checkout(context, &tree, "set up")?;
    metadata::load_strict(&context.git_root)?;
    let commands = authorize(context, control)?;
    // Preparing an unrecorded checkout records no invented creation details.
    if !tree.recorded {
        metadata::replace(
            &context.git_root,
            Path::new(&tree.path),
            &MetadataInput {
                name: Some(tree.name.clone()),
                ..MetadataInput::default()
            },
            metadata::BranchOrigin::Adopted,
        )?;
    }
    let tree = worktree::worktree_at_path(context, Path::new(&tree.path))?;
    let report = run(context, &tree, extra, &commands, logger, false)?;
    let tree = worktree::worktree_at_path(context, Path::new(&tree.path))?;
    crate::output::data(
        &json!({"workspace":tree.to_json(None), "setup":report.outcome(), "copy":report.copy()}),
    )?;
    if report.failed() {
        return Err(Error::msg(
            report
                .metadata_error
                .as_deref()
                .or(report.status.error.as_deref())
                .unwrap_or("Setup failed"),
        ));
    }
    logger.log(&format!(
        "Setup completed for workspace '{}'",
        crate::output::visible_text(&tree.name)
    ));
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn running_on_a_live_process_or_another_host_is_only_observed() {
        let mut saved = Status::new(State::Running);
        saved.pid = Some(std::process::id());
        saved.host = host();
        let view = display(true, Some(&saved)).unwrap();
        assert_eq!(view.state, State::Running);
        assert!(view.pid.is_none());
        assert!(view.host.is_none());
        assert_eq!(saved.pid, Some(std::process::id()));
        saved.pid = Some(i32::MAX as u32);
        saved.host = Some("another-host.example.invalid".into());
        assert_eq!(display(true, Some(&saved)).unwrap().state, State::Running);
        saved.host = None;
        assert_eq!(display(true, Some(&saved)).unwrap().state, State::Running);
        assert_eq!(display(true, None).unwrap().state, State::Unknown);
        assert!(display(false, Some(&saved)).is_none());
    }
}
