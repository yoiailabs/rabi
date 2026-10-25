//! One resumable parent session per workspace. Task progress belongs to Codex.
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::worktree::WorktreeInfo;

mod lifecycle;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Running,
    Idle,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StopReason {
    Completed,
    ParentFailed,
    Interrupted,
    TimedOut,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Workspace {
    id: Option<String>,
    name: String,
    path: String,
    branch: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Launch {
    n: usize,
    started_at: String,
    finished_at: Option<String>,
    exit_code: Option<i32>,
    stop_reason: Option<StopReason>,
    skill: String,
    session_id: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    schema: u32,
    id: String,
    workspace: Workspace,
    provider: String,
    session_id: Option<String>,
    skill: String,
    state: State,
    launches: Vec<Launch>,
}

pub(crate) struct Report {
    pub run: Record,
    pub launch: Launch,
    pub answer: String,
    pub exit: i32,
}

pub(crate) fn timeout(value: &str) -> Result<Duration> {
    let invalid = || {
        Error::validation(
            "--timeout requires a positive integer with suffix s, m, or h (for example 30m)",
        )
    };
    let (digits, unit) = value
        .split_at_checked(value.len().saturating_sub(1))
        .ok_or_else(invalid)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid());
    }
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(invalid()),
    };
    let seconds = digits
        .parse::<u64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| *n > 0)
        .ok_or_else(invalid)?;
    Ok(Duration::from_secs(seconds))
}

pub(crate) fn require_trust(root: &Path) -> Result<()> {
    if crate::trust::trust_all() || crate::trust::is_trusted_root(root)? {
        Ok(())
    } else {
        Err(Error::Untrusted(
            "Run requires repository trust. Run `rabi trust add` first or set RABI_TRUST_ALL=1."
                .into(),
        ))
    }
}

fn validate_id(id: &str) -> Result<()> {
    let valid = id
        .strip_prefix("run_")
        .and_then(|suffix| {
            uuid::Uuid::parse_str(suffix)
                .ok()
                .map(|uuid| (suffix, uuid))
        })
        .is_some_and(|(suffix, uuid)| {
            uuid.get_version_num() == 4
                && uuid.get_variant() == uuid::Variant::RFC4122
                && uuid.hyphenated().to_string() == suffix
        });
    if valid {
        Ok(())
    } else {
        Err(Error::validation(
            "Run ID must be the full run_<uuid v4> form",
        ))
    }
}

fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|e| Error::msg(e.to_string()))
}

fn save(dir: &Path, record: &Record) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(record).map_err(|e| Error::msg(e.to_string()))?;
    io(crate::atomic::write(&dir.join("state.json"), &bytes))
}

fn broken(reason: impl std::fmt::Display) -> Error {
    Error::msg(format!("Cannot resume current run: {reason}. Start a new parent with `rabi run --fresh --skill <value>`."))
}

// Bounded to one filename even for long hierarchical workspace names.
fn workspace_key(name: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(name.as_bytes()))
}

pub(crate) fn execute(
    root: &Path,
    tree: &WorktreeInfo,
    skill: Option<&str>,
    fresh: bool,
    task: Option<&str>,
    timeout: Duration,
) -> Result<Report> {
    let _signals = lifecycle::Signals::install()?;
    let runs = crate::git::git_common_dir(root)?.join("rabi/runs");
    let current = runs.join("current");
    io(fs::create_dir_all(&current))?;
    let key = workspace_key(&tree.name);
    let _lock = lifecycle::Lock::acquire(current.join(format!("{key}.lock")))?;
    let pointer = current.join(key);
    let previous = if fresh {
        None
    } else {
        match fs::read_to_string(&pointer) {
            Ok(id) => Some(id),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(broken(e)),
        }
    };
    let resuming = previous.is_some();
    let mut record = if let Some(id) = previous {
        validate_id(&id).map_err(broken)?;
        let bytes = fs::read(runs.join(&id).join("state.json")).map_err(broken)?;
        let record: Record = serde_json::from_slice(&bytes).map_err(broken)?;
        if record.schema != 2
            || record.id != id
            || record.provider != "codex"
            || record.workspace.name != tree.name
            || record.workspace.path != tree.path
        {
            return Err(broken(
                "invalid schema or mismatching run/workspace identity",
            ));
        }
        let session = record
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| broken("no recorded session id"))?;
        if !crate::provider::has_rollout(session).map_err(broken)? {
            return Err(broken("Codex session rollout not found"));
        }
        record
    } else {
        let skill = skill
            .ok_or_else(|| Error::validation("--skill is required for a first run or --fresh"))?;
        Record {
            schema: 2,
            id: format!("run_{}", uuid::Uuid::new_v4()),
            workspace: Workspace {
                id: tree.metadata.id.clone(),
                name: tree.name.clone(),
                path: tree.path.clone(),
                branch: (!tree.detached).then(|| tree.branch.clone()),
            },
            provider: "codex".into(),
            session_id: None,
            skill: skill.into(),
            state: State::Idle,
            launches: Vec::new(),
        }
    };
    if let Some(skill) = skill {
        record.skill = skill.into();
    }
    let prompt = if resuming {
        let mut message = skill
            .map(|s| format!("Follow skill `{s}` from now on.\n\n"))
            .unwrap_or_default();
        message.push_str(task.unwrap_or_default());
        if message.is_empty() {
            message.push_str("Continue.");
        }
        message
    } else {
        format!("You are the parent agent for this rabi run in workspace {} on branch {}. You manage progress: delegate implementation and review to your own subagents, gather the information you need yourself (git, gh, tests), give follow-up instructions until the work is really done, and decide completion.\n\nFollow skill `{}`.\n\n{}", tree.path, tree.branch, record.skill, task.unwrap_or_default())
    };
    let dir = runs.join(&record.id);
    if !resuming {
        io(fs::create_dir(&dir))?;
        save(&dir, &record)?;
    }
    let n = record.launches.len() + 1;
    let launch_dir = dir.join("launches").join(n.to_string());
    io(fs::create_dir_all(dir.join("launches")))?;
    io(fs::create_dir(&launch_dir))?;
    io(crate::atomic::write(
        &launch_dir.join("prompt.md"),
        prompt.as_bytes(),
    ))?;
    record.launches.push(Launch {
        n,
        started_at: crate::metadata::iso_now(),
        finished_at: None,
        exit_code: None,
        stop_reason: None,
        skill: record.skill.clone(),
        session_id: None,
        message: None,
    });
    record.state = State::Running;
    save(&dir, &record)?;
    io(crate::atomic::write(&pointer, record.id.as_bytes()))?;
    // Every fallible launch operation is enclosed so even spawn/protocol/IO failures
    // finalize the record, and the lifecycle guard reaps any live child first.
    let launched = lifecycle::launch(
        Path::new(&tree.path),
        &launch_dir,
        if resuming {
            record.session_id.as_deref()
        } else {
            None
        },
        timeout,
    );
    let launch = record
        .launches
        .last_mut()
        .ok_or_else(|| Error::msg("Missing launch record"))?;
    launch.finished_at = Some(crate::metadata::iso_now());
    let mut exit = match launched {
        Ok(result) => {
            launch.exit_code = Some(result.parent_exit);
            launch.stop_reason = Some(result.reason);
            result.exit
        }
        Err(e) => {
            launch.message = Some(e.to_string());
            launch.stop_reason = Some(StopReason::Error);
            1
        }
    };
    match crate::provider::session_id(&launch_dir.join("events.jsonl")) {
        Ok(id) => {
            launch.session_id = Some(id.clone());
            if resuming && record.session_id.as_deref() != Some(&id) {
                launch.message = Some(format!("Codex resumed a different thread id ({id}); expected {}. The recorded parent session was retained. Use `rabi run --fresh --skill <value>` to start over.", record.session_id.as_deref().unwrap_or_default()));
                launch.stop_reason = Some(StopReason::Error);
                exit = 1;
            } else {
                record.session_id = Some(id);
            }
        }
        Err(e) if launch.message.is_none() && exit == 0 => {
            launch.message = Some(e.to_string());
            launch.stop_reason = Some(StopReason::Error);
            exit = 1;
        }
        _ => {}
    }
    if let Some(reason) = &launch.stop_reason {
        launch.message = match reason {
            StopReason::Interrupted => Some("Run interrupted".into()),
            StopReason::TimedOut => Some("Run timed out".into()),
            _ => launch.message.take(),
        };
    }
    let answer = match fs::read_to_string(launch_dir.join("answer.md")) {
        Ok(answer) => answer,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            launch.message = Some(format!("Failed to read parent answer: {e}"));
            if exit == 0 {
                launch.stop_reason = Some(StopReason::Error);
                exit = 1;
            }
            String::new()
        }
    };
    record.state = match launch.stop_reason {
        Some(StopReason::Completed) => State::Idle,
        Some(StopReason::Interrupted | StopReason::TimedOut | StopReason::ParentFailed) => {
            State::Stopped
        }
        _ => State::Error,
    };
    let launch = launch.clone();
    save(&dir, &record)?;
    Ok(Report {
        run: record,
        launch,
        answer,
        exit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_validation_and_path_encoding() {
        for (s, seconds) in [("90s", 90), ("30m", 1800), ("2h", 7200)] {
            assert_eq!(timeout(s).unwrap().as_secs(), seconds);
        }
        for s in [
            "",
            "0s",
            "-1s",
            "+1h",
            "1",
            "1d",
            "1.5m",
            " 1s",
            "18446744073709551615h",
            "é",
        ] {
            assert!(timeout(s).is_err(), "{s}");
        }
        for name in ["../topic", "feat/run", "%2f", ".", ""] {
            assert!(!workspace_key(name).contains(['/', '.', '%']));
        }
        assert_ne!(workspace_key("a/b"), workspace_key("a%2fb"));
    }
}
