//! Internal workflow progress and attempt records shared by linked checkouts.
#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::allocation::Allocation;
use crate::error::{Error, Result, NOT_FOUND};
use crate::workflow::{self, Provider, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum State {
    Running,
    Succeeded,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Workspace {
    pub id: Option<String>,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StopReason {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Workflow {
    pub name: String,
    pub version: u64,
    pub source: Source,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Loop {
    pub used: u64,
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Record {
    pub schema: u32,
    pub id: String,
    pub state: State,
    pub workflow: Workflow,
    pub current_step: Option<String>,
    pub steps_started: u64,
    pub loops: BTreeMap<String, Loop>,
    pub workspace: Workspace,
    /// Commit resolved from allocation's normalized base; older runs omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    pub pr: Option<crate::metadata::WorktreeLink>,
    pub stop_reason: Option<StopReason>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Agent {
    pub provider: Provider,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub session_id: Option<String>,
    pub continued_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Check {
    pub command: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Attempt {
    pub step: String,
    pub attempt: u64,
    pub result: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
    pub head: Option<String>,
    pub agent: Option<Agent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
}

impl Record {
    pub fn from_allocation(
        allocation: &Allocation,
        entry: &workflow::Entry,
        definition: &workflow::Definition,
    ) -> Self {
        Self::new(entry, definition, Workspace::from_allocation(allocation))
    }

    pub fn new(
        entry: &workflow::Entry,
        definition: &workflow::Definition,
        workspace: Workspace,
    ) -> Self {
        Self {
            schema: 1,
            id: format!("run_{}", uuid::Uuid::new_v4()),
            state: State::Running,
            workflow: Workflow {
                name: entry.name.clone(),
                version: definition.version,
                source: entry.source,
            },
            current_step: None,
            steps_started: 0,
            loops: definition
                .loops
                .iter()
                .map(|(name, max)| (name.clone(), Loop { used: 0, max: *max }))
                .collect(),
            workspace,
            base: None,
            pr: None,
            stop_reason: None,
            started_at: crate::metadata::iso_now(),
            finished_at: None,
        }
    }

    pub fn finish(&mut self, stop_reason: Option<StopReason>) {
        self.state = if stop_reason.is_some() {
            State::Stopped
        } else {
            State::Succeeded
        };
        self.stop_reason = stop_reason;
        self.current_step = None;
        self.finished_at = Some(crate::metadata::iso_now());
    }
}

impl Workspace {
    pub fn from_allocation(allocation: &Allocation) -> Self {
        let tree = &allocation.workspace;
        Self {
            id: tree.metadata.id.clone(),
            name: tree.name.clone(),
            path: tree.path.clone(),
            branch: (!tree.detached).then(|| tree.branch.clone()),
        }
    }
}

pub(crate) fn validate_id(id: &str) -> Result<()> {
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

pub(crate) fn directory(git_root: &Path, id: &str) -> Result<PathBuf> {
    validate_id(id)?;
    Ok(crate::git::git_common_dir(git_root)?
        .join("rabi/runs")
        .join(id))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let body = serde_json::to_vec_pretty(value).map_err(|error| Error::msg(error.to_string()))?;
    crate::atomic::write(path, &body)
        .map_err(|error| Error::msg(format!("Failed to save run record: {error}")))
}

/// Publish snapshots and the initial manifest in a fresh run directory. On
/// failure, remove only the newly allocated directory; existing runs stay intact.
pub(crate) fn create(git_root: &Path, record: &Record, task: &str, yaml: &str) -> Result<()> {
    let dir = directory(git_root, &record.id)?;
    let runs = dir
        .parent()
        .ok_or_else(|| Error::msg("Run directory has no parent"))?;
    fs::create_dir_all(runs)
        .map_err(|error| Error::msg(format!("Failed to create runs directory: {error}")))?;
    fs::create_dir(&dir)
        .map_err(|error| Error::msg(format!("Failed to create run directory: {error}")))?;
    let result = (|| {
        crate::atomic::write(&dir.join("task.md"), task.as_bytes())
            .map_err(|error| Error::msg(error.to_string()))?;
        crate::atomic::write(&dir.join("workflow.yaml"), yaml.as_bytes())
            .map_err(|error| Error::msg(error.to_string()))?;
        save(git_root, record)
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

pub(crate) fn save(git_root: &Path, record: &Record) -> Result<()> {
    if record.schema != 1 {
        return Err(Error::msg(
            "Unsupported run record schema: expected schema 1",
        ));
    }
    write_json(&directory(git_root, &record.id)?.join("run.json"), record)
}

pub(crate) fn attempt_directory(
    git_root: &Path,
    id: &str,
    step: &str,
    number: u64,
) -> Result<PathBuf> {
    // Step IDs are not restricted by workflow validation. Encode path separators
    // for storage, without imposing a naming policy on workflow definitions.
    let component = step
        .replace('%', "%25")
        .replace('/', "%2F")
        .replace('\0', "%00");
    Ok(directory(git_root, id)?
        .join("steps")
        .join(format!("{number:03}-{component}")))
}

/// Reserve a new run-wide attempt number and persist the count before execution.
/// A fresh directory prevents an attempt from replacing an earlier result.
pub(crate) fn begin_attempt(
    git_root: &Path,
    record: &mut Record,
    step: &str,
    agent: Option<Agent>,
) -> Result<Attempt> {
    let number = record
        .steps_started
        .checked_add(1)
        .ok_or_else(|| Error::msg("Run step count overflow"))?;
    record.steps_started = number;
    record.current_step = Some(step.into());
    save(git_root, record)?;
    let attempt = Attempt {
        step: step.into(),
        attempt: number,
        result: "running".into(),
        started_at: crate::metadata::iso_now(),
        finished_at: None,
        exit_code: None,
        head: None,
        agent,
        checks: Vec::new(),
    };
    let dir = attempt_directory(git_root, &record.id, &attempt.step, attempt.attempt)?;
    let parent = dir
        .parent()
        .ok_or_else(|| Error::msg("Attempt directory has no parent"))?;
    fs::create_dir_all(parent)
        .and_then(|()| fs::create_dir(&dir))
        .map_err(|error| Error::msg(format!("Failed to create attempt directory: {error}")))?;
    save_attempt(git_root, &record.id, &attempt)?;
    Ok(attempt)
}

pub(crate) fn save_attempt(git_root: &Path, id: &str, attempt: &Attempt) -> Result<()> {
    write_json(
        &attempt_directory(git_root, id, &attempt.step, attempt.attempt)?.join("result.json"),
        attempt,
    )
}

pub(crate) fn load_attempt(git_root: &Path, id: &str, step: &str, number: u64) -> Result<Attempt> {
    let body = fs::read(attempt_directory(git_root, id, step, number)?.join("result.json"))
        .map_err(|error| Error::msg(error.to_string()))?;
    serde_json::from_slice(&body)
        .map_err(|error| Error::msg(format!("Invalid attempt record: {error}")))
}

/// Never creates directories, updates progress, or rewrites stored records.
pub(crate) fn load(git_root: &Path, id: &str) -> Result<(Record, PathBuf)> {
    let dir = directory(git_root, id)?;
    let body = fs::read(dir.join("run.json")).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            Error::status(NOT_FOUND, format!("Run '{id}' not found"))
        } else {
            Error::msg(format!("Failed to read run record: {error}"))
        }
    })?;
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| Error::msg(format!("Invalid run record: {error}")))?;
    if value.get("schema").and_then(serde_json::Value::as_u64) != Some(1) {
        return Err(Error::msg(
            "Unsupported or missing run record schema: expected schema 1",
        ));
    }
    let record: Record = serde_json::from_value(value)
        .map_err(|error| Error::msg(format!("Invalid run record: {error}")))?;
    if record.id != id {
        return Err(Error::msg("Run record ID does not match its directory"));
    }
    Ok((record, dir))
}

/// Both built-in and repository workflows execute arbitrary commands under the
/// existing common-directory trust. This helper never prompts or writes trust.
pub(crate) fn require_trust(git_root: &Path) -> Result<()> {
    require_trust_with(crate::trust::trust_all(), || {
        crate::trust::is_trusted_root(git_root)
    })
}

fn require_trust_with(trust_all: bool, trusted: impl FnOnce() -> Result<bool>) -> Result<()> {
    if trust_all || trusted()? {
        Ok(())
    } else {
        Err(Error::Untrusted(
            "Workflow execution requires repository trust. Run `rabi trust add` first.".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("rabi-run-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&root).unwrap();
            crate::git::run(&["init", "--template=", "-b", "main"], Some(&root)).unwrap();
            crate::test_git::configure_repo(&root);
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn record() -> Record {
        Record {
            schema: 1,
            id: format!("run_{}", uuid::Uuid::new_v4()),
            state: State::Running,
            workflow: Workflow {
                name: "default".into(),
                version: 3,
                source: Source::Builtin,
            },
            current_step: None,
            steps_started: 0,
            loops: BTreeMap::from([("repair".into(), Loop { used: 0, max: 2 })]),
            workspace: Workspace {
                id: Some(format!("ws_{}", uuid::Uuid::new_v4())),
                name: "topic".into(),
                path: "/workspaces/topic".into(),
                branch: Some("topic".into()),
            },
            base: None,
            pr: None,
            stop_reason: None,
            started_at: crate::metadata::iso_now(),
            finished_at: None,
        }
    }

    #[test]
    fn run_snapshots_and_progress_roundtrip_without_rewriting_on_read() {
        let fixture = Fixture::new();
        let mut run = record();
        let dir = directory(&fixture.0, &run.id).unwrap();
        assert!(load(&fixture.0, &run.id).is_err());
        assert!(!dir.exists());
        create(&fixture.0, &run, "# Task\n", workflow::DEFAULT_YAML).unwrap();
        assert_eq!(fs::read_to_string(dir.join("task.md")).unwrap(), "# Task\n");
        assert_eq!(
            fs::read_to_string(dir.join("workflow.yaml")).unwrap(),
            workflow::DEFAULT_YAML
        );
        assert_eq!(load(&fixture.0, &run.id).unwrap().0, run);
        let bytes = fs::read(dir.join("run.json")).unwrap();
        assert!(create(&fixture.0, &run, "replacement", "replacement").is_err());
        assert_eq!(fs::read(dir.join("run.json")).unwrap(), bytes);
        assert_eq!(fs::read_to_string(dir.join("task.md")).unwrap(), "# Task\n");
        run.finish(Some(StopReason {
            kind: "step_failed".into(),
            message: "Check failed".into(),
        }));
        save(&fixture.0, &run).unwrap();
        let before = fs::read(dir.join("run.json")).unwrap();
        assert_eq!(load(&fixture.0, &run.id).unwrap().0, run);
        assert_eq!(fs::read(dir.join("run.json")).unwrap(), before);
        assert_eq!(run.state, State::Stopped);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    }

    #[test]
    fn numbered_attempts_preserve_prior_results_and_record_only_observed_head() {
        let fixture = Fixture::new();
        let mut run = record();
        create(&fixture.0, &run, "# Task", workflow::DEFAULT_YAML).unwrap();
        let mut first = begin_attempt(
            &fixture.0,
            &mut run,
            "implement",
            Some(Agent {
                provider: Provider::Codex,
                model: Some("example-model".into()),
                effort: Some("tool-defined-value".into()),
                session_id: None,
                continued_from: None,
            }),
        )
        .unwrap();
        assert_eq!(run.steps_started, 1);
        assert_eq!(
            load(&fixture.0, &run.id).unwrap().0.current_step.as_deref(),
            Some("implement")
        );
        first.result = "ok".into();
        first.finished_at = Some(crate::metadata::iso_now());
        first.exit_code = Some(0);
        first.head = Some("observed-head".into());
        first.agent.as_mut().unwrap().session_id = Some("session-1".into());
        save_attempt(&fixture.0, &run.id, &first).unwrap();
        let first_file = directory(&fixture.0, &run.id)
            .unwrap()
            .join("steps/001-implement/result.json");
        let bytes = fs::read(&first_file).unwrap();
        let attempt_dir = first_file.parent().unwrap();
        fs::set_permissions(attempt_dir, fs::Permissions::from_mode(0o500)).unwrap();
        let failure = save_attempt(&fixture.0, &run.id, &first);
        fs::set_permissions(attempt_dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(failure.is_err());
        assert_eq!(fs::read(&first_file).unwrap(), bytes);
        assert_eq!(fs::read_dir(attempt_dir).unwrap().count(), 1);
        let second = begin_attempt(&fixture.0, &mut run, "implement", None).unwrap();
        assert_eq!(second.attempt, 2);
        assert_eq!(run.steps_started, 2);
        assert_eq!(
            load_attempt(&fixture.0, &run.id, "implement", 1).unwrap(),
            first
        );
        assert_eq!(
            load_attempt(&fixture.0, &run.id, "implement", 2).unwrap(),
            second
        );
        assert_eq!(fs::read(&first_file).unwrap(), bytes);
        // A stale writer cannot replace an already allocated attempt directory.
        let mut stale = run.clone();
        stale.steps_started = 0;
        assert!(begin_attempt(&fixture.0, &mut stale, "implement", None).is_err());
        assert_eq!(fs::read(&first_file).unwrap(), bytes);
        let escaped = begin_attempt(&fixture.0, &mut run, "../step/with/slashes", None).unwrap();
        assert!(
            attempt_directory(&fixture.0, &run.id, &escaped.step, escaped.attempt)
                .unwrap()
                .parent()
                .unwrap()
                .ends_with("steps")
        );
    }

    #[test]
    fn save_failures_leave_no_partial_files_and_preserve_published_data() {
        let fixture = Fixture::new();
        let run = record();
        let dir = directory(&fixture.0, &run.id).unwrap();
        let runs = dir.parent().unwrap();
        fs::create_dir_all(runs).unwrap();
        fs::set_permissions(runs, fs::Permissions::from_mode(0o500)).unwrap();
        let failure = create(&fixture.0, &run, "# Task", workflow::DEFAULT_YAML);
        fs::set_permissions(runs, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(failure.is_err());
        assert!(!dir.join("run.json").exists());
        assert_eq!(fs::read_dir(runs).unwrap().count(), 0);
        let mut invalid = run.clone();
        invalid.schema = 2;
        assert!(create(&fixture.0, &invalid, "# Task", workflow::DEFAULT_YAML).is_err());
        assert!(
            !dir.exists(),
            "creation failure must not leave partial snapshots or manifest"
        );
        create(&fixture.0, &run, "# Task", workflow::DEFAULT_YAML).unwrap();
        let bytes = fs::read(dir.join("run.json")).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let failure = save(&fixture.0, &run);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(failure.is_err());
        assert_eq!(fs::read(dir.join("run.json")).unwrap(), bytes);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
    }

    #[test]
    fn unknown_or_missing_schema_is_rejected_without_modifying_files() {
        let fixture = Fixture::new();
        let run = record();
        create(&fixture.0, &run, "# Task", workflow::DEFAULT_YAML).unwrap();
        let file = directory(&fixture.0, &run.id).unwrap().join("run.json");
        for schema in [Some(2), None] {
            let mut value = serde_json::to_value(&run).unwrap();
            if let Some(schema) = schema {
                value["schema"] = serde_json::json!(schema);
            } else {
                value.as_object_mut().unwrap().remove("schema");
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            fs::write(&file, &bytes).unwrap();
            assert!(load(&fixture.0, &run.id)
                .unwrap_err()
                .to_string()
                .contains("schema 1"));
            assert_eq!(fs::read(&file).unwrap(), bytes);
        }
    }

    #[test]
    fn workflow_execution_requires_existing_common_directory_trust() {
        assert!(require_trust_with(true, || panic!("trust-all must bypass store reads")).is_ok());
        assert!(require_trust_with(false, || Ok(true)).is_ok());
        let refusal = require_trust_with(false, || Ok(false)).unwrap_err();
        assert_eq!(refusal.exit_code(), 4);
        assert!(refusal.to_string().contains("rabi trust add"));
        assert!(require_trust_with(false, || Err(Error::msg("store unreadable"))).is_err());
    }
}
