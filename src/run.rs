//! One resumable parent session per workspace. Task progress belongs to the parent.
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::provider::{Probe, Provider, Session};
use crate::worktree::WorktreeInfo;

mod inspect;
mod lifecycle;
pub(crate) use inspect::{summary, WorkspaceOutput};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    requested_session_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_zero")]
    insecure_tool_calls: u64,
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

fn is_zero(value: &u64) -> bool {
    *value == 0
}

pub(crate) struct Report {
    pub provider: Provider,
    pub run: Record,
    pub launch: Launch,
    pub answer: String,
    pub exit: i32,
    pub failure: Option<Error>,
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

fn observe_session(record: &mut Record, id: String, resuming: bool) -> Result<bool> {
    let provider = record.provider.parse::<Provider>()?;
    let launch = record
        .launches
        .last_mut()
        .ok_or_else(|| Error::msg("Missing launch record"))?;
    let mismatch = resuming && record.session_id.as_deref() != Some(&id);
    if mismatch {
        if launch.message.is_none() {
            launch.message = Some(format!("{} resumed a different thread id ({id}); expected {}. The recorded parent session was retained. Use `rabi run --fresh --skill <value>` to start over.", provider.display_name(), record.session_id.as_deref().unwrap_or_default()));
        }
    } else {
        record.session_id = Some(id.clone());
    }
    launch.session_id = Some(id);
    Ok(mismatch)
}

// Checkout paths stay stable across branch changes, including detached HEAD.
fn workspace_key(path: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(path.as_bytes()))
}

fn prepare_launch(dir: &Path, record: &mut Record, prompt: &str) -> Result<PathBuf> {
    let launches = dir.join("launches");
    io(fs::create_dir_all(&launches))?;
    let mut n = record.launches.last().map_or(0, |launch| launch.n);
    let launch_dir = loop {
        n = n
            .checked_add(1)
            .ok_or_else(|| Error::msg("Launch number overflow"))?;
        let path = launches.join(n.to_string());
        match fs::create_dir(&path) {
            Ok(()) => break path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(Error::msg(e.to_string())),
        }
    };
    let old_state = record.state.clone();
    let old_len = record.launches.len();
    let prepared = (|| {
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
            requested_session_id: None,
            insecure_tool_calls: 0,
            message: None,
        });
        record.state = State::Running;
        save(dir, record)
    })();
    if let Err(error) = prepared {
        // A failed preparation must not reserve this launch number forever.
        let _ = fs::remove_dir_all(&launch_dir);
        record.launches.truncate(old_len);
        record.state = old_state;
        return Err(error);
    }
    Ok(launch_dir)
}

fn read_previous(
    runs: &Path,
    pointer: &Path,
    tree: &WorktreeInfo,
    requested_provider: Option<Provider>,
) -> Result<Option<Record>> {
    let id = match fs::read_to_string(pointer) {
        Ok(id) => id,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(broken(error)),
    };
    validate_id(&id).map_err(broken)?;
    let bytes = fs::read(runs.join(&id).join("state.json")).map_err(broken)?;
    let record: Record = serde_json::from_slice(&bytes).map_err(broken)?;
    if record.schema != 2
        || record.id != id
        || record.provider.parse::<Provider>().is_err()
        || record.workspace.path != tree.path
    {
        return Err(broken(
            "invalid schema or mismatching run/workspace identity",
        ));
    }
    let provider = record.provider.parse::<Provider>().map_err(broken)?;
    if let Some(requested) = requested_provider.filter(|requested| *requested != provider) {
        return Err(Error::validation(format!(
            "Current parent uses {}; start a new parent with `rabi run --fresh --provider {} --skill <value>`",
            provider.name(), requested.name()
        )));
    }
    Ok(Some(record))
}

pub(crate) fn execute(
    root: &Path,
    tree: &WorktreeInfo,
    skill: Option<&str>,
    fresh: bool,
    requested_provider: Option<Provider>,
    task: Option<&str>,
    timeout: Duration,
) -> Result<Report> {
    let _signals = lifecycle::Signals::install()?;
    let common_dir = crate::git::git_common_dir(root)?;
    let runs = common_dir.join("rabi/runs");
    let current = runs.join("current");
    let key = workspace_key(&tree.path);
    let pointer = current.join(&key);
    // A conflicting provider is a usage error before creating directories,
    // taking over stale locks, probing a session, or updating any run files.
    if !fresh && requested_provider.is_some() {
        read_previous(&runs, &pointer, tree, requested_provider)?;
    }
    io(fs::create_dir_all(&current))?;
    let mut lock = lifecycle::Lock::acquire(current.join(format!("{key}.lock")))?;
    // Re-read under the lock: another launcher may have completed since the
    // read-only provider check above.
    let previous = if fresh {
        None
    } else {
        read_previous(&runs, &pointer, tree, requested_provider)?
    };
    let resuming = previous.is_some();
    let mut record = if let Some(record) = previous {
        let session = record
            .session_id
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| broken("no recorded session id"))?;
        let provider = record.provider.parse::<Provider>().map_err(broken)?;
        if provider
            .probe_session(session, Path::new(&record.workspace.path))
            .map_err(broken)?
            == Probe::Missing
        {
            return Err(broken(format!(
                "{} session rollout not found",
                provider.display_name()
            )));
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
            provider: requested_provider.unwrap_or(Provider::Codex).name().into(),
            session_id: None,
            skill: skill.into(),
            state: State::Idle,
            launches: Vec::new(),
        }
    };
    let provider = record.provider.parse::<Provider>().map_err(broken)?;
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
    let launch_dir = prepare_launch(&dir, &mut record, &prompt)?;
    io(crate::atomic::write(&pointer, record.id.as_bytes()))?;
    if !crate::output::json() {
        crate::output::stderr(format_args!("rabi: running parent session (run {}) in {}. Ctrl-C stops it; `rabi run \"...\"` resumes.\n", record.id, crate::output::visible_text(&tree.path)));
    }
    // Every fallible launch operation is enclosed so even spawn/protocol/IO failures
    // finalize the record, and the lifecycle guard reaps any live child first.
    let session = if resuming {
        record.session_id.clone()
    } else {
        None
    };
    let session = match session.as_deref() {
        Some(id) => Session::Resume { id },
        None => Session::New { requested: None },
    };
    let launched = provider
        .prepare_workspace(Path::new(&record.workspace.path), &common_dir)
        .and_then(|()| {
            lifecycle::launch(
                provider,
                Path::new(&tree.path),
                &launch_dir,
                session,
                timeout,
                &mut lock,
                |id| {
                    let mismatch = observe_session(&mut record, id, resuming)?;
                    save(&dir, &record).map_err(|e| {
                        Error::msg(format!(
                            "Failed to persist {} session id: {e}",
                            provider.display_name()
                        ))
                    })?;
                    Ok(mismatch)
                },
            )
        });
    // Cursor finalization scans the full log once after process reaping even
    // when the session id was already observed during launch.
    let cursor_final =
        (provider == Provider::Cursor).then(|| provider.finalize(&launch_dir.join("events.jsonl")));
    // Fast exits and a final line without a newline still use the complete log.
    // Once observed mid-launch, the first id is authoritative for this launch.
    let session = if let Some(finalized) = &cursor_final {
        finalized
            .as_ref()
            .ok()
            .and_then(|finalized| {
                (!finalized.session_id.is_empty()).then(|| Ok(finalized.session_id.clone()))
            })
            .unwrap_or_else(|| Err(Error::msg("Cursor did not report a session id")))
    } else {
        record
            .launches
            .last()
            .and_then(|launch| launch.session_id.clone())
            .map(Ok)
            .unwrap_or_else(|| {
                provider
                    .finalize(&launch_dir.join("events.jsonl"))
                    .map(|finalized| finalized.session_id)
            })
    };
    let launch = record
        .launches
        .last_mut()
        .ok_or_else(|| Error::msg("Missing launch record"))?;
    launch.finished_at = Some(crate::metadata::iso_now());
    let mut failure = None;
    let mut exit = match launched {
        Ok(result) => {
            launch.exit_code = Some(result.parent_exit);
            launch.stop_reason = Some(result.reason);
            result.exit
        }
        Err(e) => {
            launch.message = Some(e.to_string());
            launch.stop_reason = Some(StopReason::Error);
            if provider == Provider::Cursor {
                failure = Some(e);
            }
            1
        }
    };
    match session {
        Ok(id) => {
            if observe_session(&mut record, id, resuming)?
                && (provider == Provider::Codex || exit == 0)
            {
                record
                    .launches
                    .last_mut()
                    .ok_or_else(|| Error::msg("Missing launch record"))?
                    .stop_reason = Some(StopReason::Error);
                exit = 1;
            }
        }
        Err(e) if provider == Provider::Codex && launch.message.is_none() && exit == 0 => {
            launch.message = Some(e.to_string());
            launch.stop_reason = Some(StopReason::Error);
            exit = 1;
        }
        _ => {}
    }
    let launch = record
        .launches
        .last_mut()
        .ok_or_else(|| Error::msg("Missing launch record"))?;
    if let Some(reason) = &launch.stop_reason {
        launch.message = match reason {
            StopReason::Interrupted => Some("Run interrupted".into()),
            StopReason::TimedOut => Some("Run timed out".into()),
            _ => launch.message.take(),
        };
    }
    if let Some(finalized) = cursor_final {
        match finalized {
            Ok(finalized) => {
                if exit == 0 {
                    if let Some(error) = finalized.protocol_error {
                        launch.message = Some(error);
                        launch.stop_reason = Some(StopReason::Error);
                        exit = 1;
                    } else if finalized.is_error {
                        launch.message = Some("Cursor reported an error result".into());
                        launch.stop_reason = Some(StopReason::ParentFailed);
                        exit = 1;
                    }
                }
                launch.insecure_tool_calls = finalized.insecure_tool_calls;
            }
            Err(error) if exit == 0 => {
                launch.message = Some(error.to_string());
                launch.stop_reason = Some(StopReason::Error);
                exit = 1;
            }
            Err(_) => {}
        }
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
    if launch.insecure_tool_calls > 0 {
        let audit = format!("Cursor audit: {} completed tool call(s) requested TYPE_INSECURE_NONE (sandbox policy disabled).", launch.insecure_tool_calls);
        launch.message = Some(match launch.message.take() {
            Some(message) => format!("{message}; {audit}"),
            None => audit,
        });
    }
    record.state = match launch.stop_reason {
        Some(StopReason::Completed) => State::Idle,
        Some(StopReason::Interrupted | StopReason::TimedOut | StopReason::ParentFailed) => {
            State::Stopped
        }
        _ => State::Error,
    };
    let launch = launch.clone();
    save(&dir, &record).map_err(|e| match &launch.message {
        Some(message) => Error::msg(format!("{message}; failed to save final run state: {e}")),
        None => e,
    })?;
    Ok(Report {
        provider,
        run: record,
        launch,
        answer,
        exit,
        failure,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_record_loads_and_none_requested_session_id_is_omitted() {
        let value = serde_json::json!({
            "schema": 2, "id": format!("run_{}", uuid::Uuid::new_v4()),
            "workspace": {"id": null, "name": "test", "path": "/test", "branch": null},
            "provider": "codex", "sessionId": "test-session", "skill": "test", "state": "idle",
            "launches": [{
                "n": 1, "startedAt": "2026-10-26T00:00:00Z", "finishedAt": null,
                "exitCode": null, "stopReason": null, "skill": "test", "sessionId": null,
                "message": null
            }]
        });
        let mut record: Record = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(record.launches[0].requested_session_id, None);
        assert_eq!(record.launches[0].insecure_tool_calls, 0);
        assert_eq!(serde_json::to_value(&record).unwrap(), value);
        assert!(!serde_json::to_value(&record.launches[0])
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("requestedSessionId"));
        record.launches[0].insecure_tool_calls = 2;
        assert_eq!(
            serde_json::to_value(&record.launches[0]).unwrap()["insecureToolCalls"],
            2
        );
        record.launches[0].requested_session_id = Some("requested".into());
        assert_eq!(
            serde_json::to_value(&record.launches[0]).unwrap()["requestedSessionId"],
            "requested"
        );
    }

    #[test]
    fn failed_launch_preparation_removes_directory_and_allows_retry() {
        let dir = std::env::temp_dir().join(format!("rabi-launch-{}", uuid::Uuid::new_v4()));
        // A directory at the state destination deterministically rejects the
        // atomic rename, including when the test runs with elevated privileges.
        fs::create_dir_all(dir.join("state.json")).unwrap();
        let mut record: Record = serde_json::from_value(serde_json::json!({
            "schema": 2, "id": format!("run_{}", uuid::Uuid::new_v4()),
            "workspace": {"id": null, "name": "test", "path": "/test", "branch": null},
            "provider": "codex", "sessionId": "test-session", "skill": "test", "state": "idle", "launches": []
        })).unwrap();
        assert!(prepare_launch(&dir, &mut record, "test prompt").is_err());
        assert!(!dir.join("launches/1").exists());
        assert!(record.launches.is_empty());
        fs::remove_dir(dir.join("state.json")).unwrap();
        let path = prepare_launch(&dir, &mut record, "retry prompt").unwrap();
        assert_eq!(path, dir.join("launches/1"));
        assert_eq!(
            fs::read_to_string(path.join("prompt.md")).unwrap(),
            "retry prompt"
        );
        fs::remove_dir_all(dir).unwrap();
    }
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
