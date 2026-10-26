//! Read-only projection of the existing parent run records for list and show.
use std::fs;
use std::path::Path;

use serde::Serialize;

use super::{io, lifecycle, validate_id, workspace_key, Record, State, StopReason};
use crate::error::{Error, Result};

#[derive(Serialize)]
pub(crate) struct WorkspaceOutput {
    #[serde(flatten)]
    pub workspace: crate::worktree::WorktreeJson,
    pub run: Option<Summary>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Summary {
    pub id: String,
    pub state: &'static str,
    pub session_id: Option<String>,
    skill: String,
    launches: usize,
    pub last_launch: Option<LastLaunch>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LastLaunch {
    pub n: usize,
    started_at: String,
    finished_at: Option<String>,
    exit_code: Option<i32>,
    pub stop_reason: Option<&'static str>,
    message: Option<String>,
    dir: Option<String>,
    prompt_path: Option<String>,
    pub events_path: Option<String>,
    stderr_path: Option<String>,
    pub answer_path: Option<String>,
}

pub(crate) fn summary(root: &Path, path: &str) -> Option<Summary> {
    match read(root, path) {
        Ok(run) => run,
        Err(error) => {
            crate::output::workspace_warning(
                "run_read",
                &crate::output::visible_text(&format!("Cannot read workspace run: {error}")),
                path,
            );
            None
        }
    }
}

fn read(root: &Path, path: &str) -> Result<Option<Summary>> {
    let runs = crate::git::git_common_dir(root)?.join("rabi/runs");
    let key = workspace_key(path);
    let pointer = runs.join("current").join(&key);
    let id = match fs::read_to_string(pointer) {
        Ok(id) => id,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::msg(error.to_string())),
    };
    validate_id(&id)?;
    let dir = runs.join(&id);
    let record: Record = serde_json::from_slice(&io(fs::read(dir.join("state.json")))?)
        .map_err(|error| Error::msg(error.to_string()))?;
    if record.schema != 2
        || record.id != id
        || record
            .provider
            .parse::<crate::provider::Provider>()
            .is_err()
        || record.workspace.path != path
        || record.launches.iter().any(|launch| launch.n == 0)
        || record
            .launches
            .windows(2)
            .any(|pair| pair[0].n >= pair[1].n)
    {
        return Err(Error::msg("Invalid run schema or workspace identity"));
    }
    let state = match record.state {
        State::Running if holder_alive(&runs.join("current").join(format!("{key}.lock")))? => {
            "running"
        }
        State::Running | State::Stopped => "stopped",
        State::Idle => "idle",
        State::Error => "error",
    };
    let last_launch = record.launches.last().map(|launch| {
        let dir = dir.join("launches").join(launch.n.to_string());
        LastLaunch {
            n: launch.n,
            started_at: launch.started_at.clone(),
            finished_at: launch.finished_at.clone(),
            exit_code: launch.exit_code,
            stop_reason: launch.stop_reason.as_ref().map(|reason| match reason {
                StopReason::Completed => "completed",
                StopReason::ParentFailed => "parent_failed",
                StopReason::Interrupted => "interrupted",
                StopReason::TimedOut => "timed_out",
                StopReason::Error => "error",
            }),
            message: launch.message.clone(),
            dir: dir.is_dir().then(|| dir.to_string_lossy().into_owned()),
            prompt_path: existing_file(&dir.join("prompt.md")),
            events_path: existing_file(&dir.join("events.jsonl")),
            stderr_path: existing_file(&dir.join("stderr.log")),
            answer_path: existing_file(&dir.join("answer.md")),
        }
    });
    Ok(Some(Summary {
        id,
        state,
        session_id: record.session_id,
        skill: record.skill,
        launches: record.launches.len(),
        last_launch,
    }))
}

fn existing_file(path: &Path) -> Option<String> {
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

fn holder_alive(path: &Path) -> Result<bool> {
    // Never acquire a lock, including flock: observation must not interfere
    // with either a live launch or a contender taking over a stale lock.
    let body = match fs::read_to_string(path) {
        Ok(body) => body,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(Error::msg(error.to_string())),
    };
    let ids = body
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| Error::msg("Invalid run lock holder"))?;
    if ids.is_empty()
        || ids.len() > 2
        || ids[0] == 0
        || ids.iter().any(|id| i32::try_from(*id).is_err())
    {
        return Err(Error::msg("Invalid run lock holder"));
    }
    Ok(crate::process::process_is_alive(ids[0])
        || ids
            .get(1)
            .is_some_and(|pgid| *pgid > 0 && lifecycle::group_is_alive(*pgid)))
}
