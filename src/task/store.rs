//! Local bookkeeping claims, independent of GitHub evidence and workspace records.
use crate::error::{Error, Result};
use crate::{git, output};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BlockReason {
    NoProgress,
    NoCi,
    CiTimeout,
    Conflict,
    Diverged,
    PolicyInvalid,
    ProviderFailed,
    EvidenceMissing,
    FloorChanged,
    PrecheckChanged,
    ReviewInvalid,
    Budget,
    Internal,
    RecordMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "name", content = "reason", rename_all = "snake_case")]
pub(crate) enum TaskState {
    New,
    Implementing,
    Prechecking,
    Publishing,
    Ci,
    Reviewing,
    Ready,
    Classifying,
    AwaitingApproval,
    Merging,
    Merged,
    Blocked(BlockReason),
    Abandoned,
}

// Every allowed edge lives here, including blocked re-derivation and abandonment.
const TRANSITIONS: &[(&str, &[&str])] = &[
    ("new", &["implementing", "blocked", "abandoned"]),
    ("implementing", &["prechecking", "blocked", "abandoned"]),
    (
        "prechecking",
        &["publishing", "implementing", "blocked", "abandoned"],
    ),
    ("publishing", &["ci", "blocked", "abandoned"]),
    ("ci", &["reviewing", "implementing", "blocked", "abandoned"]),
    (
        "reviewing",
        &["ready", "implementing", "blocked", "abandoned"],
    ),
    ("ready", &["classifying", "blocked", "abandoned"]),
    (
        "classifying",
        &["merging", "awaiting_approval", "blocked", "abandoned"],
    ),
    (
        "awaiting_approval",
        &["merging", "implementing", "blocked", "abandoned"],
    ),
    ("merging", &["merged", "blocked", "abandoned"]),
    (
        "blocked",
        &[
            "new",
            "implementing",
            "prechecking",
            "publishing",
            "ci",
            "reviewing",
            "ready",
            "classifying",
            "awaiting_approval",
            "merging",
            "blocked",
            "abandoned",
        ],
    ),
];

impl TaskState {
    pub fn name(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Implementing => "implementing",
            Self::Prechecking => "prechecking",
            Self::Publishing => "publishing",
            Self::Ci => "ci",
            Self::Reviewing => "reviewing",
            Self::Ready => "ready",
            Self::Classifying => "classifying",
            Self::AwaitingApproval => "awaiting_approval",
            Self::Merging => "merging",
            Self::Merged => "merged",
            Self::Blocked(_) => "blocked",
            Self::Abandoned => "abandoned",
        }
    }
    pub fn terminal(self) -> bool {
        matches!(self, Self::Merged | Self::Abandoned)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Task {
    schema: u32,
    pub id: String,
    created_at: String,
    updated_at: String,
    pub state: TaskState,
    pub intent: String,
    pub branch: String,
    base_ref: String,
    push_remote_url: String,
    pub workspace_path: Option<PathBuf>,
    pr_number: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Event {
    at: String,
    from: Option<TaskState>,
    to: TaskState,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct View {
    pub task: Task,
    recorded_state: TaskState,
    events: Vec<Event>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    id: String,
    state: TaskState,
    branch: Option<String>,
    intent: Option<String>,
}

fn io<T>(result: std::io::Result<T>) -> Result<T> {
    result.map_err(|e| Error::msg(format!("Task store: {e}")))
}
fn encode(value: &impl Serialize) -> Result<Vec<u8>> {
    serde_json::to_vec(value).map_err(|_| Error::msg("Cannot encode task record"))
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn valid_id(id: &str) -> bool {
    id.strip_prefix("task_")
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .is_some_and(|uuid| id == format!("task_{uuid}") && uuid.get_version_num() == 4)
}
fn private_dir(path: &Path) -> Result<()> {
    io(fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path))?;
    if !io(fs::symlink_metadata(path))?.is_dir() {
        return Err(Error::msg("Task store directory must not be a symlink"));
    }
    Ok(())
}
fn open(path: &Path, append: bool) -> Result<File> {
    let file = io(OpenOptions::new()
        .read(true)
        .append(append)
        .create(append)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path))?;
    let metadata = io(file.metadata())?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(Error::msg(
            "Task store files must be regular files without hard links",
        ));
    }
    Ok(file)
}
// File::lock uses flock on Unix; the lock is released by closing the file.
fn lock(path: &Path) -> Result<File> {
    let file = open(path, true)?;
    io(file.lock())?;
    Ok(file)
}
fn bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    io(open(path, false)?.read_to_end(&mut bytes))?;
    Ok(bytes)
}

pub(crate) struct Store {
    root: PathBuf,
    repository: crate::repository_state::RepositoryState,
    path: PathBuf,
}

impl Store {
    pub fn locate(root: &Path) -> Result<Self> {
        let repository = crate::repository_state::RepositoryState::locate(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            path: repository.path().join("tasks"),
            repository,
        })
    }

    fn ids(&self) -> Result<Vec<String>> {
        if self.repository.directory(false)?.is_none() {
            return Ok(Vec::new());
        }
        let entries = match fs::read_dir(&self.path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return io(Err(e)),
        };
        let mut ids = Vec::new();
        for entry in entries {
            let entry = io(entry)?;
            if let Some(id) = entry.file_name().to_str().filter(|id| valid_id(id)) {
                ids.push(id.to_owned());
            }
        }
        ids.sort();
        Ok(ids)
    }

    fn load(&self, id: &str) -> Result<Task> {
        self.load_record(id)
            .map_err(|error| Error::msg(format!("Cannot read task {id}: {error}")))
    }

    fn load_record(&self, id: &str) -> Result<Task> {
        if !io(fs::symlink_metadata(self.path.join(id)))?.is_dir() {
            return Err(Error::validation(
                "Task directory is not a regular directory",
            ));
        }
        let record: Task = serde_json::from_slice(&bytes(&self.path.join(id).join("task.json"))?)
            .map_err(|_| Error::validation("Invalid task.json"))?;
        if record.schema != 1
            || record.id != id
            || record.intent.trim().is_empty()
            || !valid_branch(&self.root, &record.branch)?
            || record.base_ref.trim().is_empty()
            || record.push_remote_url != strip_credentials(&record.push_remote_url)
        {
            return Err(Error::validation(
                "Task record does not match schema or directory",
            ));
        }
        Ok(record)
    }

    fn resolve(&self, prefix: &str) -> Result<String> {
        if prefix.len() < 6 {
            return Err(Error::validation(
                "Task ID prefixes require at least 6 characters",
            ));
        }
        let ids = self.ids()?;
        if let Some(id) = ids.iter().find(|id| id.as_str() == prefix) {
            return Ok(id.clone());
        }
        let matching: Vec<_> = ids
            .into_iter()
            .filter(|id| id.starts_with(prefix))
            .collect();
        match matching.as_slice() {
            [id] => Ok(id.clone()),
            [] => Err(Error::Status {
                code: "not_found",
                message: "Task ID not found".into(),
            }),
            _ => Err(Error::validation("Ambiguous task ID prefix")),
        }
    }

    fn derived(&self, task: &Task) -> Result<TaskState> {
        if !task.state.terminal() && !matches!(task.state, TaskState::Blocked(_)) {
            let exists = git::reference_exists(&self.root, &format!("refs/heads/{}", task.branch))?;
            if exists == (task.state == TaskState::New) {
                output::warning(
                    "record_mismatch",
                    format!("Task {} branch disagrees with its recorded state", task.id),
                );
                return Ok(TaskState::Blocked(BlockReason::RecordMismatch));
            }
        }
        Ok(task.state)
    }

    pub fn list(&self, all: bool) -> Result<Vec<Entry>> {
        let mut entries = Vec::new();
        for id in self.ids()? {
            match self.load_record(&id) {
                Ok(task) => {
                    if !all && task.state.terminal() {
                        continue;
                    }
                    entries.push(Entry {
                        id,
                        state: self.derived(&task)?,
                        branch: Some(task.branch),
                        intent: Some(task.intent.lines().next().unwrap_or("").to_owned()),
                    });
                }
                Err(error) => {
                    let reason = if matches!(error, Error::Validation(_)) {
                        BlockReason::RecordMismatch
                    } else {
                        BlockReason::Internal
                    };
                    // Never include record contents or decoder/Git diagnostics in warnings.
                    output::warning(
                        reason_code(reason),
                        format!("Cannot read task {id}; displaying blocked"),
                    );
                    entries.push(Entry {
                        id,
                        state: TaskState::Blocked(reason),
                        branch: None,
                        intent: None,
                    });
                }
            }
        }
        Ok(entries)
    }

    fn events(&self, id: &str) -> Result<Vec<Event>> {
        bytes(&self.path.join(id).join("events.jsonl"))
            .map_err(|error| Error::msg(format!("Cannot read task {id} events: {error}")))?
            .split(|b| *b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| {
                serde_json::from_slice(line)
                    .map_err(|_| Error::msg(format!("Task {id}: invalid events.jsonl")))
            })
            .collect()
    }

    pub fn show(&self, prefix: &str) -> Result<View> {
        let id = self.resolve(prefix)?;
        let mut task = self.load(&id)?;
        let events = self.events(&id)?;
        let recorded_state = task.state;
        task.state = self.derived(&task)?;
        Ok(View {
            task,
            recorded_state,
            events,
        })
    }

    fn branch_available(&self, branch: &str) -> Result<()> {
        if git::reference_exists(&self.root, &format!("refs/heads/{branch}"))?
            || git::list_worktrees(&self.root)?
                .iter()
                .any(|tree| tree.branch.as_deref() == Some(branch))
        {
            return Err(Error::validation(
                "Task branch already exists or is checked out in a checkout",
            ));
        }
        Ok(())
    }

    fn available(&self, branch: &str) -> Result<()> {
        self.branch_available(branch)?;
        for id in self.ids()? {
            let task = self.load(&id).map_err(|error| {
                Error::msg(format!("Cannot check branch reservations: {error}"))
            })?;
            if !task.state.terminal() && task.branch == branch {
                return Err(Error::validation(
                    "Task branch is reserved by another non-terminal task",
                ));
            }
        }
        Ok(())
    }

    pub fn create(&self, intent: String, branch: Option<&str>, base: Option<&str>) -> Result<View> {
        if intent.trim().is_empty() {
            return Err(Error::validation("Task intent must not be empty"));
        }
        let id = format!("task_{}", uuid::Uuid::new_v4());
        let branch = branch
            .map(str::to_owned)
            .unwrap_or_else(|| slug(&intent, &id[id.len() - 8..]));
        if !valid_branch(&self.root, &branch)? {
            return Err(Error::validation("Invalid task branch name"));
        }
        let base_ref = match base {
            Some(base) => base.to_owned(),
            None => super::default_branch(&self.root)?,
        };
        if !super::resolves_commit(&self.root, &base_ref)? {
            return Err(Error::validation(
                "Task base must resolve to a local commit",
            ));
        }
        self.branch_available(&branch)?;
        let push_remote_url = push_url(&self.root, &branch)?;
        // Serialize branch reservations as well as individual task updates.
        self.repository.directory(true)?;
        private_dir(&self.path)?;
        let _creation = lock(&self.path.join(".lock"))?;
        self.available(&branch)?;
        let directory = self.path.join(format!(".staging-{}", uuid::Uuid::new_v4()));
        io(fs::DirBuilder::new().mode(0o700).create(&directory))?;
        let published = self.publish(&directory, id, intent, branch, base_ref, push_remote_url);
        if published.is_err() {
            // Best effort: a crash can still leave an ignored staging directory.
            let _ = fs::remove_dir_all(&directory);
        }
        published
    }

    // Caller holds the creation lock and has created the private staging directory.
    fn publish(
        &self,
        directory: &Path,
        id: String,
        intent: String,
        branch: String,
        base_ref: String,
        push_remote_url: String,
    ) -> Result<View> {
        let _lock = lock(&directory.join(".lock"))?;
        let at = now();
        let task = Task {
            schema: 1,
            id,
            created_at: at.clone(),
            updated_at: at.clone(),
            state: TaskState::New,
            intent,
            branch,
            base_ref,
            push_remote_url,
            workspace_path: None,
            pr_number: None,
        };
        let event = Event {
            at,
            from: None,
            to: TaskState::New,
            reason: "created".into(),
            detail: None,
            agent: None,
        };
        self.save(directory, &task, &event)?;
        io(File::open(directory).and_then(|file| file.sync_all()))?;
        io(fs::rename(directory, self.path.join(&task.id)))?;
        io(File::open(&self.path).and_then(|file| file.sync_all()))?;
        Ok(View {
            recorded_state: task.state,
            task,
            events: vec![event],
        })
    }

    // Caller holds the per-task lock. Append before atomic replacement: an
    // interrupted replacement may leave an extra event, never erase history.
    fn save(&self, directory: &Path, task: &Task, event: &Event) -> Result<()> {
        let mut line = encode(event)?;
        line.push(b'\n');
        let mut file = open(&directory.join("events.jsonl"), true)?;
        io(file.write_all(&line))?;
        io(file.sync_all())?;
        io(crate::atomic::write(
            &directory.join("task.json"),
            &encode(task)?,
        ))
    }

    pub fn implementation_task(&self, selector: Option<&str>) -> Result<Task> {
        let id = if let Some(selector) = selector.filter(|value| *value != ".") {
            self.resolve(selector)?
        } else {
            let branch = git::current_branch(&self.root)?;
            let mut matching = Vec::new();
            for id in self.ids()? {
                let task = self.load(&id)?;
                if task.branch == branch && !task.state.terminal() {
                    matching.push(id);
                }
            }
            match matching.as_slice() {
                [id] => id.clone(),
                [] => {
                    return Err(Error::Status {
                        code: "not_found",
                        message: "No active task matches the current branch; specify a task ID"
                            .into(),
                    })
                }
                _ => {
                    return Err(Error::validation(
                        "Ambiguous current task; specify a task ID",
                    ))
                }
            }
        };
        self.events(&id)?;
        self.load(&id)
    }

    pub fn implementation_lock(&self, task: &Task) -> Result<File> {
        let file = open(&self.path.join(&task.id).join(".implement.lock"), true)?;
        file.try_lock().map_err(|_| {
            Error::status(crate::error::REFUSED, "Task implementer is already running")
        })?;
        Ok(file)
    }

    pub fn implementation_event(
        &self,
        id: &str,
        workspace: &Path,
        start: bool,
        data: serde_json::Value,
    ) -> Result<()> {
        let directory = self.path.join(id);
        if !io(fs::symlink_metadata(&directory))?.is_dir() {
            return Err(Error::msg("Task directory must not be a symlink"));
        }
        let _lock = lock(&directory.join(".lock"))?;
        let mut task = self.load(id)?;
        self.events(id)?;
        let mut event = if start {
            match task.state {
                TaskState::New => {
                    transition(&mut task, TaskState::Implementing, "implementation_started")?
                }
                TaskState::Implementing => Event {
                    at: now(),
                    from: Some(task.state),
                    to: task.state,
                    reason: "implementation_started".into(),
                    detail: None,
                    agent: None,
                },
                _ => {
                    return Err(Error::status(
                        crate::error::REFUSED,
                        "Task must be new or implementing",
                    ))
                }
            }
        } else {
            Event {
                at: now(),
                from: Some(task.state),
                to: task.state,
                reason: "implementation_finished".into(),
                detail: None,
                agent: None,
            }
        };
        event.agent = Some(data);
        task.workspace_path = Some(workspace.to_path_buf());
        task.updated_at = event.at.clone();
        self.save(&directory, &task, &event)
    }

    pub fn transition(&self, prefix: &str, to: TaskState, reason: &str) -> Result<View> {
        let id = self.resolve(prefix)?;
        let directory = self.path.join(&id);
        if !io(fs::symlink_metadata(&directory))
            .map_err(|error| Error::msg(format!("Cannot read task {id}: {error}")))?
            .is_dir()
        {
            return Err(Error::msg(format!(
                "Task {id}: directory must not be a symlink"
            )));
        }
        let _lock = lock(&directory.join(".lock"))
            .map_err(|error| Error::msg(format!("Cannot lock task {id}: {error}")))?;
        let mut task = self.load(&id)?;
        let event = transition(&mut task, to, reason)?;
        let mut events = self.events(&id)?;
        self.save(&directory, &task, &event)?;
        events.push(event);
        Ok(View {
            recorded_state: task.state,
            task,
            events,
        })
    }
}

fn transition(task: &mut Task, to: TaskState, reason: &str) -> Result<Event> {
    if !TRANSITIONS
        .iter()
        .any(|(from, targets)| *from == task.state.name() && targets.contains(&to.name()))
    {
        return Err(Error::validation(format!(
            "Invalid task transition: {} -> {}",
            task.state.name(),
            to.name()
        )));
    }
    let event = Event {
        at: now(),
        from: Some(task.state),
        to,
        reason: reason.into(),
        detail: None,
        agent: None,
    };
    task.state = to;
    task.updated_at = event.at.clone();
    Ok(event)
}

fn valid_branch(root: &Path, branch: &str) -> Result<bool> {
    if branch.is_empty() || branch.starts_with('-') || branch == "HEAD" {
        return Ok(false);
    }
    Ok(
        super::capture(root, &["check-ref-format", &format!("refs/heads/{branch}")])?
            .status
            .success(),
    )
}
fn slug(intent: &str, suffix: &str) -> String {
    let mut slug = String::new();
    for character in intent.chars().take(80) {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    format!("{}-{suffix}", if slug.is_empty() { "task" } else { slug })
}

fn push_url(root: &Path, branch: &str) -> Result<String> {
    let failed = || Error::msg("Cannot resolve task push remote URL");
    let remote = git::push_remote(root, branch).map_err(|_| failed())?;
    let result = super::capture(root, &["remote", "get-url", "--push", "--", &remote])
        .map_err(|_| failed())?;
    if !result.status.success() {
        return Err(failed());
    }
    let url = std::str::from_utf8(&result.stdout)
        .map_err(|_| failed())?
        .trim();
    if url.is_empty() {
        return Err(failed());
    }
    Ok(strip_credentials(url))
}

fn strip_credentials(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = rest[..end].rsplit('@').next().unwrap_or("");
        let path = rest[end..].split(['?', '#']).next().unwrap_or("");
        return format!("{scheme}://{authority}{path}");
    }
    // scp-style SSH destinations may contain a user/password before @.
    if !url.starts_with(['/', '.']) {
        if let Some((_, destination)) = url.rsplit_once('@') {
            if destination
                .split_once(':')
                .is_some_and(|(host, _)| !host.contains('/'))
            {
                return destination.into();
            }
        }
    }
    url.into()
}

fn reason_code(reason: BlockReason) -> &'static str {
    if reason == BlockReason::RecordMismatch {
        "record_mismatch"
    } else {
        "internal"
    }
}
fn state_text(state: TaskState) -> String {
    match state {
        TaskState::Blocked(reason) => format!(
            "blocked({})",
            serde_json::to_value(reason)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| "internal".into())
        ),
        _ => state.name().into(),
    }
}
impl Entry {
    pub fn print(&self) {
        output::outln!(
            "{} {} {} {}",
            self.id,
            state_text(self.state),
            output::visible_text(self.branch.as_deref().unwrap_or("?")),
            output::visible_text(self.intent.as_deref().unwrap_or("?"))
        );
    }
}
impl View {
    pub fn print(&self) {
        output::outln!("{} {}", self.task.id, state_text(self.task.state));
        output::outln!("Branch: {}", output::visible_text(&self.task.branch));
        output::outln!("Base: {}", output::visible_text(&self.task.base_ref));
        output::outln!("Intent: {}", output::visible_text(&self.task.intent));
        for event in &self.events {
            output::outln!(
                "{} {} -> {}: {}",
                output::visible_text(&event.at),
                event.from.map_or("null", TaskState::name),
                state_text(event.to),
                output::visible_text(&event.reason)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_table_covers_all_state_pairs() {
        use TaskState::*;
        let states = [
            New,
            Implementing,
            Prechecking,
            Publishing,
            Ci,
            Reviewing,
            Ready,
            Classifying,
            AwaitingApproval,
            Merging,
            Merged,
            Blocked(BlockReason::Budget),
            Abandoned,
        ];
        let forward_and_back = [
            (New, Implementing),
            (Implementing, Prechecking),
            (Prechecking, Publishing),
            (Publishing, Ci),
            (Ci, Reviewing),
            (Reviewing, Ready),
            (Ready, Classifying),
            (Classifying, Merging),
            (Classifying, AwaitingApproval),
            (AwaitingApproval, Merging),
            (Merging, Merged),
            (Prechecking, Implementing),
            (Ci, Implementing),
            (Reviewing, Implementing),
            (AwaitingApproval, Implementing),
        ];
        for from in states {
            for to in states {
                let allowed = !from.terminal()
                    && (matches!(to, Blocked(_) | Abandoned)
                        || (matches!(from, Blocked(_)) && !to.terminal())
                        || forward_and_back.contains(&(from, to)));
                let mut task = Task {
                    schema: 1,
                    id: "test".into(),
                    created_at: "old".into(),
                    updated_at: "old".into(),
                    state: from,
                    intent: "intent".into(),
                    branch: "topic".into(),
                    base_ref: "main".into(),
                    push_remote_url: "example.invalid:repo".into(),
                    workspace_path: None,
                    pr_number: None,
                };
                let result = transition(&mut task, to, "test transition");
                assert_eq!(result.is_ok(), allowed, "{from:?} -> {to:?}");
                if let Ok(event) = result {
                    assert_eq!(event.from, Some(from));
                    assert_eq!(event.to, to);
                    assert_eq!(event.reason, "test transition");
                    assert_eq!(task.state, to);
                    assert_eq!(task.updated_at, event.at);
                } else {
                    assert_eq!(task.state, from);
                    assert_eq!(task.updated_at, "old");
                }
            }
        }
    }

    #[test]
    fn credentials_and_slug_table() {
        for (url, expected) in [
            (
                "https://user:secret@example.invalid/repo.git",
                "https://example.invalid/repo.git",
            ),
            (
                "HTTPS://token@example.invalid/repo?token=secret#secret",
                "HTTPS://example.invalid/repo",
            ),
            (
                "ssh://user:secret@example.invalid:22/repo",
                "ssh://example.invalid:22/repo",
            ),
            ("git:secret@example.invalid:repo", "example.invalid:repo"),
            ("git:sec@ret@example.invalid:repo", "example.invalid:repo"),
            ("git@example.invalid:repo", "example.invalid:repo"),
            ("/local/repo@name:copy", "/local/repo@name:copy"),
        ] {
            assert_eq!(strip_credentials(url), expected);
        }
        for (intent, expected) in [
            ("Fix Authentication!", "fix-authentication-12345678"),
            ("...", "task-12345678"),
            ("  Multiple   spaces\nNext", "multiple-spaces-next-12345678"),
        ] {
            assert_eq!(slug(intent, "12345678"), expected);
        }
    }
}
