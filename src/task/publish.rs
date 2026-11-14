//! Deterministic committed checks and publication; agents never supply commands to GitHub.
use std::collections::VecDeque;
use std::fs;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::{ffi::OsStrExt, fs::symlink, process::CommandExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::boundary::{Boundary, Process, Signals};
use super::store::{BlockReason, Store, Task, TaskState};
use crate::error::{Error, Result, REFUSED};

const TAIL: usize = 4096;
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub task_id: String,
    head: String,
    pub pr_number: u64,
    state: TaskState,
}

fn event(
    store: &Store,
    task: &mut Task,
    to: TaskState,
    reason: &str,
    check: Option<serde_json::Value>,
    pr: Option<u64>,
) -> Result<()> {
    store.publication_event(task, to, reason, check, pr)?;
    task.state = to;
    task.pr_number = pr.or(task.pr_number);
    Ok(())
}
fn block(
    store: &Store,
    task: &mut Task,
    reason: BlockReason,
    code: &'static str,
    message: &str,
) -> Error {
    match event(store, task, TaskState::Blocked(reason), code, None, None) {
        Ok(()) => Error::Status {
            code,
            message: message.into(),
        },
        Err(error) => error,
    }
}

fn configuration(workspace: &Path, sha: &str) -> Result<Option<String>> {
    let Some((mode, blob)) = super::committed_entry(workspace, sha, ".rabi.toml")? else {
        return Ok(None);
    };
    if !matches!(mode, 0o100644 | 0o100755) {
        return Err(Error::msg("Committed .rabi.toml must be a regular file"));
    }
    let bytes = super::read(workspace, &["cat-file", "blob", &blob])?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| Error::msg("Committed .rabi.toml must be UTF-8"))?;
    Ok(Some(text.into()))
}

fn precheck_value(text: Option<&str>) -> Result<Option<serde_json::Value>> {
    let config: serde_json::Value = toml_edit::de::from_str(text.unwrap_or(""))
        .map_err(|_| Error::msg("Invalid committed .rabi.toml"))?;
    Ok(config.get("precheck").cloned())
}

fn sanitized(bytes: &[u8], workspace: &Path) -> String {
    let mut text: String = String::from_utf8_lossy(bytes)
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    for (key, value) in std::env::vars_os() {
        let key = key.to_string_lossy();
        let value = value.to_string_lossy();
        if (key.contains("TOKEN")
            || key.contains("SECRET")
            || key.contains("PASSWORD")
            || key == "HOME")
            && !value.is_empty()
        {
            text = text.replace(value.as_ref(), "[redacted]");
        }
    }
    if let Some(path) = workspace.to_str() {
        text = text.replace(path, "[workspace]");
    }
    // Absolute paths and URL userinfo can contain private checkout names or credentials.
    text = text
        .split_inclusive(char::is_whitespace)
        .map(|part| {
            if part.contains("://") && part.contains('@') {
                "[redacted-url] ".to_owned()
            } else if part.trim_start_matches(['\'', '"', '(']).starts_with('/') {
                "[path] ".to_owned()
            } else {
                part.to_owned()
            }
        })
        .collect();
    let start = text.len().saturating_sub(TAIL);
    let start = (start..=text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    text[start..].to_owned()
}

fn pr_title(intent: &str, workspace: &Path) -> String {
    let first = intent
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("Task");
    let first: String = first.trim().chars().take(128).collect();
    let title = sanitized(first.as_bytes(), workspace);
    let title = title.trim();
    if title.is_empty() {
        "Task".into()
    } else {
        title.chars().take(128).collect()
    }
}

fn quoted_data(value: &str, limit: usize) -> Result<String> {
    let mut end = value.len().min(limit);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let value = if end < value.len() {
        format!("{} [truncated]", &value[..end])
    } else {
        value.to_owned()
    };
    serde_json::to_string(&value).map_err(|_| Error::msg("Cannot encode PR metadata"))
}

fn pr_body(intent: &str, commits: &str) -> Result<String> {
    // Bound raw data before JSON encoding; even six-byte control escapes keep
    // the body below 32 KiB. Subjects are data, never Markdown instructions.
    let subjects = commits
        .lines()
        .take(30)
        .map(|line| quoted_data(line, 128))
        .collect::<Result<Vec<_>>>()?;
    Ok(format!(
        "Task intent (JSON string):\n```json\n{}\n```\n\nCommit subjects (JSON strings):\n```json\n{}\n```\n{}",
        quoted_data(intent, 1024)?,
        subjects.join("\n"),
        if commits.lines().count() > 30 {
            "Additional commit subjects omitted.\n"
        } else {
            ""
        },
    ))
}

struct Check {
    code: i32,
    tail: Vec<u8>,
    reason: &'static str,
}
fn nonblocking(pipe: &impl AsRawFd) -> Result<()> {
    // SAFETY: the pipe owns a live descriptor; fcntl only changes its status flags.
    let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(Error::msg("Cannot configure precheck output pipe"));
    }
    Ok(())
}

fn drain(pipe: &mut impl Read, tail: &mut VecDeque<u8>, mut budget: usize) -> Result<()> {
    let mut buffer = [0; 4096];
    while budget > 0 {
        let limit = budget.min(buffer.len());
        match pipe.read(&mut buffer[..limit]) {
            Ok(0) => break,
            Ok(count) => {
                budget -= count;
                tail.extend(&buffer[..count]);
                let excess = tail.len().saturating_sub(TAIL);
                tail.drain(..excess);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => return Err(Error::msg("Cannot read precheck output")),
        }
    }
    Ok(())
}

fn drain_pending(pipe: &mut (impl Read + AsRawFd), tail: &mut VecDeque<u8>) -> Result<()> {
    let mut pending: libc::c_int = 0;
    // SAFETY: FIONREAD writes one int through the valid pointer for this live pipe.
    if unsafe { libc::ioctl(pipe.as_raw_fd(), libc::FIONREAD, &mut pending) } < 0 {
        return Err(Error::msg("Cannot inspect pending precheck output"));
    }
    // Snapshot a finite byte count. An escaped child may retain the descriptor
    // or keep writing; neither may hold publication open indefinitely.
    drain(pipe, tail, pending.max(0) as usize)
}

fn check(workspace: &Path, script: &str, timeout: Duration) -> Result<Check> {
    let boundary = Boundary::new()?;
    let signals = Signals::install()?;
    let mut command = Command::new("sh");
    command
        .args(["-c", script])
        .current_dir(workspace)
        .env("PWD", workspace)
        .env_remove("OLDPWD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    boundary.prepare(&mut command);
    let child = command.spawn().map_err(|_| Error::Status {
        code: "environment",
        message: "Cannot start precheck shell".into(),
    })?;
    let mut process = Process(child);
    let mut stdout = process
        .0
        .stdout
        .take()
        .ok_or_else(|| Error::msg("Missing precheck stdout pipe"))?;
    let mut stderr = process
        .0
        .stderr
        .take()
        .ok_or_else(|| Error::msg("Missing precheck stderr pipe"))?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    let mut tail = VecDeque::with_capacity(TAIL);
    let started = Instant::now();
    let (code, reason) = loop {
        if signals.interrupted() {
            break (130, "interrupted");
        }
        if started.elapsed() >= timeout {
            break (124, "timeout");
        }
        // Bound each poll so even a continuously noisy child cannot delay
        // signal and timeout checks or starve the other pipe.
        drain(&mut stdout, &mut tail, 64 * 1024)?;
        drain(&mut stderr, &mut tail, 64 * 1024)?;
        if let Some(code) = process.exit_code()? {
            break (
                code,
                if code == 0 {
                    "passed"
                } else {
                    "command_failed"
                },
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    drop(process);
    // Reap and clean the group first, then consume the bytes already buffered.
    // No reader-thread scheduling deadline can discard the completed command's tail.
    drain_pending(&mut stdout, &mut tail)?;
    drain_pending(&mut stderr, &mut tail)?;
    let tail = tail.into_iter().collect();
    Ok(Check { code, tail, reason })
}

/// Do not run nested Git or accept unchecked files in populated gitlink directories.
fn populated_submodules(workspace: &Path) -> Result<bool> {
    let modules = super::text(workspace, &["rev-parse", "--git-path", "modules"])?;
    if workspace.join(modules).is_dir() {
        return Ok(true);
    }
    let entries = super::read(workspace, &["ls-files", "--stage", "-z"])?;
    for entry in entries
        .split(|byte| *byte == 0)
        .filter(|entry| entry.starts_with(b"160000 "))
    {
        let (_, path) = entry.split_at(
            entry
                .iter()
                .position(|byte| *byte == b'\t')
                .ok_or_else(|| Error::msg("Invalid indexed gitlink"))?
                + 1,
        );
        let path = workspace.join(std::ffi::OsStr::from_bytes(path));
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
            Ok(metadata) if metadata.is_dir() => {
                if fs::read_dir(path)
                    .map_err(|_| Error::msg("Cannot inspect gitlink workspace"))?
                    .next()
                    .is_some()
                {
                    return Ok(true);
                }
            }
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(Error::msg("Cannot inspect gitlink workspace")),
        }
    }
    Ok(false)
}

fn unchanged(
    workspace: &Path,
    metadata: &super::implement::GitMetadata,
    head: &str,
    branch: &str,
) -> Result<bool> {
    if super::implement::git_metadata_snapshot(workspace)? != *metadata {
        return Ok(false);
    }
    Ok(crate::git::current_branch(workspace)? == branch
        && super::commit(workspace, "HEAD")? == head
        && !super::dirty(workspace)?
        && !populated_submodules(workspace)?)
}

/// Disposable bare metadata excludes agent-writable repository config, hooks,
/// URL rewrites, transport commands and remote defaults from credentialed Git.
struct Publisher {
    temporary: Boundary,
    head: String,
}
impl Publisher {
    fn new(workspace: &Path, head: &str) -> Result<Self> {
        let temporary = Boundary::new()?;
        let path = temporary.path();
        fs::create_dir(path.join("refs"))
            .map_err(|_| Error::msg("Cannot prepare publication refs"))?;
        fs::write(path.join("HEAD"), format!("{head}\n"))
            .map_err(|_| Error::msg("Cannot prepare publication HEAD"))?;
        let object_format = super::text(workspace, &["rev-parse", "--show-object-format"])?;
        let config = if object_format == "sha256" {
            "[core]\nbare = true\ncommitGraph = false\nrepositoryformatversion = 1\n[extensions]\nobjectformat = sha256\n"
        } else {
            "[core]\nbare = true\ncommitGraph = false\nrepositoryformatversion = 0\n"
        };
        fs::write(path.join("config"), config)
            .map_err(|_| Error::msg("Cannot prepare publication config"))?;
        let common = crate::git::canonical_common_dir(workspace)?;
        symlink(common.join("objects"), path.join("objects"))
            .map_err(|_| Error::msg("Cannot prepare publication objects"))?;
        Ok(Self {
            temporary,
            head: head.into(),
        })
    }
    fn git(&self, args: &[&str]) -> Result<crate::process::Captured> {
        let mut environment = Command::new("git");
        for key in std::env::vars_os().map(|(key, _)| key) {
            if key.to_string_lossy().starts_with("GIT_") {
                environment.env_remove(key);
            }
        }
        environment
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_GRAFT_FILE", "/dev/null/rabi-grafts");
        let _scope = crate::process::task_git_environment(&environment);
        let git_dir = self
            .temporary
            .path()
            .to_str()
            .ok_or_else(|| Error::msg("Publication directory is not UTF-8"))?;
        let mut options = vec![
            "--no-lazy-fetch",
            "--git-dir",
            git_dir,
            "-c",
            "core.commitGraph=false",
            "-c",
            "fetch.writeCommitGraph=false",
            "-c",
            "gc.auto=0",
            "-c",
            "credential.interactive=false",
            "-c",
            "core.askPass=",
            "-c",
            "core.sshCommand=ssh -o BatchMode=yes -l git",
            "-c",
            "protocol.allow=never",
        ];
        options.extend([
            "-c",
            "protocol.file.allow=always",
            "-c",
            "protocol.http.allow=always",
            "-c",
            "protocol.https.allow=always",
            "-c",
            "protocol.ssh.allow=always",
            "-c",
            "protocol.git.allow=always",
        ]);
        options.extend_from_slice(args);
        crate::process::raw_with_timeout(
            "git",
            &options,
            Some(self.temporary.path()),
            &[],
            Duration::from_secs(120),
        )
    }
    fn remote_head(&self, url: &str, reference: &str) -> Result<Option<String>> {
        let response = self.git(&["ls-remote", "--refs", "--", url, reference])?;
        if response.code != 0 {
            return Err(Error::msg("Cannot inspect recorded push destination"));
        }
        let lines: Vec<_> = response.stdout.lines().collect();
        match lines.as_slice() {
            [] => Ok(None),
            [line] => {
                let (oid, name) = line
                    .split_once('\t')
                    .ok_or_else(|| Error::msg("Invalid remote branch response"))?;
                if name != reference
                    || oid.len() != self.head.len()
                    || !oid.bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(Error::msg("Invalid remote branch response"));
                }
                Ok(Some(oid.into()))
            }
            _ => Err(Error::msg("Ambiguous remote branch response")),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pull {
    number: u64,
    head_ref_name: String,
    head_ref_oid: String,
    is_cross_repository: bool,
    #[serde(default)]
    state: Option<String>,
}
/// Only unambiguous HTTPS and Git SSH repository addresses are eligible.
/// The host is authorized separately by gh's independent checkout resolution.
#[derive(Debug, PartialEq, Eq)]
struct Repository {
    host: String,
    owner: String,
    name: String,
}
impl Repository {
    fn parse(url: &str) -> Option<Self> {
        let (authority, path, ssh) = if let Some(rest) = url.strip_prefix("https://") {
            let (authority, path) = rest.split_once('/')?;
            (authority, path, false)
        } else if let Some(rest) = url.strip_prefix("ssh://") {
            let (authority, path) = rest.split_once('/')?;
            (authority, path, true)
        } else {
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            (authority, path, true)
        };
        let authority = if ssh {
            authority.strip_prefix("git@").unwrap_or(authority)
        } else {
            authority
        };
        let host = if let Some((host, port)) = authority.split_once(':') {
            if !ssh || port.is_empty() || !port.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            host
        } else {
            authority
        };
        if host.is_empty()
            || !host
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        {
            return None;
        }
        let path = path.strip_suffix('/').unwrap_or(path);
        let path = path.strip_suffix(".git").unwrap_or(path);
        let (owner, name) = path.split_once('/')?;
        if [owner, name].iter().any(|component| {
            component.is_empty()
                || matches!(*component, "." | "..")
                || !component
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        }) {
            return None;
        }
        Some(Self {
            host: host.to_ascii_lowercase(),
            owner: owner.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
        })
    }
    fn selector(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }
}

fn destination(workspace: &Path, url: &str) -> Result<Option<String>> {
    let Some(recorded) = Repository::parse(url) else {
        return Ok(None);
    };
    // Do not pass the recorded host as --repo: an arbitrary host must not be
    // allowed to authorize itself. gh resolves GitHub (including configured
    // Enterprise hosts) independently using the task checkout's context.
    let output = gh_command(workspace, &["repo", "view", "--json", "url"])?;
    let value: serde_json::Value = serde_json::from_str(&output)
        .map_err(|_| Error::msg("Cannot resolve task GitHub repository"))?;
    let resolved = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .filter(|url| url.starts_with("https://"))
        .and_then(Repository::parse)
        .ok_or_else(|| Error::msg("Cannot resolve task GitHub repository"))?;
    if recorded != resolved {
        return Ok(None);
    }
    Ok(Some(resolved.selector()))
}

fn gh(workspace: &Path, repo: &str, args: &[&str]) -> Result<String> {
    let mut options = args.to_vec();
    options.extend(["--repo", repo]);
    gh_command(workspace, &options)
}
fn gh_command(workspace: &Path, options: &[&str]) -> Result<String> {
    let mut command = Command::new("gh");
    command
        .args(options)
        .current_dir(workspace)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat");
    for key in std::env::vars_os().map(|(key, _)| key) {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command.env("GIT_TERMINAL_PROMPT", "0");
    let response =
        crate::process::raw_command_with_timeout(command, "gh", Duration::from_secs(30))?;
    if response.code != 0 {
        return Err(Error::msg(
            "GitHub publication command failed; rerun task publish to reconcile",
        ));
    }
    Ok(response.stdout)
}
fn pull_list(workspace: &Path, task: &Task, repository: &str) -> Result<Option<Pull>> {
    let output = gh(
        workspace,
        repository,
        &[
            "pr",
            "list",
            "--head",
            &task.branch,
            "--state",
            "open",
            "--limit",
            "1000",
            "--json",
            "number,headRefName,headRefOid,isCrossRepository",
        ],
    )?;
    let mut pulls: Vec<Pull> =
        serde_json::from_str(&output).map_err(|_| Error::msg("Invalid GitHub PR list"))?;
    if pulls.len() >= 1000 {
        return Err(Error::msg("Open PR search may be incomplete"));
    }
    pulls.retain(|pull| !pull.is_cross_repository && pull.head_ref_name == task.branch);
    if pulls.len() > 1 {
        return Err(Error::msg("Multiple open PRs match task branch"));
    }
    let pull = pulls.pop();
    if pull
        .as_ref()
        .is_some_and(|p| p.number == 0 || p.is_cross_repository || p.head_ref_name != task.branch)
    {
        return Err(Error::msg("PR does not match task repository and branch"));
    }
    Ok(pull)
}

/// Candidate names come only from the trusted record. Short names with a
/// slash may denote a local branch or a remote-tracking spelling, so only the
/// verified remote can disambiguate them; mutable local refs cannot choose.
fn base_candidates(workspace: &Path, recorded: &str) -> Result<Vec<String>> {
    let candidates = if let Some(branch) = recorded.strip_prefix("refs/heads/") {
        vec![branch.to_owned()]
    } else if let Some(tracking) = recorded.strip_prefix("refs/remotes/") {
        let (remote, branch) = tracking
            .split_once('/')
            .ok_or_else(|| Error::validation("Task publish requires a named base branch"))?;
        if remote.is_empty() {
            return Err(Error::validation(
                "Task publish requires a named base branch",
            ));
        }
        vec![branch.to_owned()]
    } else if recorded.starts_with("refs/")
        || (matches!(recorded.len(), 40 | 64)
            && recorded.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(Error::validation(
            "Task publish requires a named base branch; commit-only bases cannot open a PR",
        ));
    } else if let Some((_, branch)) = recorded.split_once('/') {
        vec![recorded.to_owned(), branch.to_owned()]
    } else {
        vec![recorded.to_owned()]
    };
    for branch in &candidates {
        let reference = format!("refs/heads/{branch}");
        let valid = super::capture(workspace, &["check-ref-format", &reference])?;
        if !valid.status.success() {
            return Err(Error::validation(
                "Task publish requires a valid named base branch",
            ));
        }
    }
    Ok(candidates)
}

pub(crate) fn execute(root: &Path, selector: Option<&str>) -> Result<Report> {
    crate::run::require_trust(root)?;
    let _read = super::boundary::git_read_scope();
    let store = Store::locate(root)?;
    let mut task = store.implementation_task(selector)?;
    let _operation = store.implementation_lock(&task)?;
    let _publication = store.publication_lock(&task)?;
    if !matches!(
        task.state,
        TaskState::Implementing | TaskState::Prechecking | TaskState::Publishing
    ) {
        return Err(Error::status(
            REFUSED,
            "Task must be implementing, prechecking or publishing",
        ));
    }
    let workspace = super::implement::workspace(root, &task)?;
    let metadata = super::implement::git_metadata_snapshot(&workspace)?;
    let head = super::commit(&workspace, "HEAD")?;
    let head = head.as_str();
    let base_candidates = base_candidates(&workspace, &task.base_ref)?;
    if populated_submodules(&workspace)? {
        return Err(Error::status(REFUSED, "Task publish cannot verify a clean tree with populated submodules or retained submodule data"));
    }
    if super::dirty(&workspace)? {
        return Err(Error::Status {
            code: "dirty",
            message: "Task publish requires a clean committed tree".into(),
        });
    }
    let repository = destination(&workspace, &task.push_remote_url)?.ok_or_else(|| {
        block(
            &store,
            &mut task,
            BlockReason::DestinationNotGithub,
            "destination_not_github",
            "Recorded push URL must match the GitHub repository resolved by gh for this task; no push performed",
        )
    })?;
    let publisher = Publisher::new(&workspace, head)?;
    let mut bases = Vec::new();
    for branch in base_candidates {
        let reference = format!("refs/heads/{branch}");
        if let Some(sha) = publisher.remote_head(&task.push_remote_url, &reference)? {
            bases.push((branch, sha));
        }
    }
    if bases.is_empty() {
        return Err(Error::msg(
            "Cannot resolve task base at recorded push destination; retry task publish",
        ));
    }
    if bases.len() != 1 {
        return Err(Error::msg(
            "Task base is ambiguous at recorded push destination; use a full refs/heads/ or refs/remotes/ base spelling",
        ));
    }
    let (base_branch, base) = bases.remove(0);
    // Fetch only the independently observed object ID into disposable metadata.
    // Neither local branch/tracking refs nor agent-writable config can select
    // the base, redirect this fetch, or change its ref destinations.
    // The checked commit supplies negotiation history without importing refs
    // from the agent-writable repository into the disposable metadata.
    let negotiation_tip = format!("--negotiation-tip={head}");
    let fetched = publisher.git(&[
        "fetch",
        &negotiation_tip,
        "--no-tags",
        "--no-write-fetch-head",
        "--no-recurse-submodules",
        "--no-auto-maintenance",
        "--",
        &task.push_remote_url,
        &base,
    ])?;
    if fetched.code != 0 {
        return Err(Error::msg(
            "Cannot obtain verified task base; retry task publish",
        ));
    }
    let inspection = publisher.temporary.path();
    let report = super::inspect(inspection, Some(&base))?;
    // Allow the remote base to have advanced since the task branched, but
    // require a verifiable common ancestor on both exact commit histories.
    for tip in [&base, &report.head] {
        let ancestor = publisher.git(&["merge-base", "--is-ancestor", &report.merge_base, tip])?;
        if ancestor.code != 0 {
            return Err(Error::msg(
                "Cannot verify task base ancestry; retry task publish",
            ));
        }
    }
    if task.state == TaskState::Implementing {
        event(
            &store,
            &mut task,
            TaskState::Prechecking,
            "precheck_started",
            None,
            None,
        )?;
    }
    let head_config = configuration(inspection, head)?;
    let base_config = configuration(inspection, &report.base.sha)?;
    if precheck_value(head_config.as_deref())? != precheck_value(base_config.as_deref())? {
        let current = task.state;
        event(
            &store,
            &mut task,
            current,
            "precheck",
            Some(
                json!({"head": head, "passed": false, "failingCommand": null, "outputTail": "", "reason": "precheck_changed"}),
            ),
            None,
        )?;
        return Err(block(
            &store,
            &mut task,
            BlockReason::PrecheckChanged,
            "precheck_changed",
            "Precheck configuration changed; a human must review it before publication",
        ));
    }
    let declared = crate::config::parse_config(head_config.as_deref().unwrap_or(""))?.precheck;
    if declared.as_ref().is_some_and(|declaration| {
        declaration
            .commands
            .iter()
            .any(|command| command.trim().is_empty() || command.contains('\0'))
    }) {
        return Err(block(
            &store,
            &mut task,
            BlockReason::PolicyInvalid,
            "failed",
            "Precheck commands must be nonempty and contain no NUL",
        ));
    }
    let timeout = declared
        .as_ref()
        .and_then(|d| d.timeout.as_deref())
        .map(|value| {
            crate::run::timeout(value).map_err(|_| {
                Error::msg(
                    "Invalid precheck timeout: expected a positive duration with s, m or h suffix",
                )
            })
        })
        .transpose()?
        .unwrap_or(Duration::from_secs(3600));
    let mut failure = None;
    for script in declared.as_ref().map_or(&[][..], |d| d.commands.as_slice()) {
        let result = match check(&workspace, script, timeout) {
            Ok(result) => result,
            Err(Error::Status {
                code: "environment",
                ..
            }) => Check {
                code: 127,
                reason: "environment",
                tail: Vec::new(),
            },
            Err(_) => Check {
                code: 1,
                reason: "execution_failed",
                tail: Vec::new(),
            },
        };
        let clean = unchanged(&workspace, &metadata, head, &task.branch).unwrap_or(false);
        let passed = result.code == 0 && clean;
        let reason = if !clean {
            "tree_changed"
        } else {
            result.reason
        };
        let current_state = task.state;
        event(
            &store,
            &mut task,
            current_state,
            "precheck",
            Some(
                json!({"head": head, "passed": passed, "failingCommand": if passed { None } else { Some(sanitized(script.as_bytes(), &workspace)) }, "outputTail": sanitized(&result.tail, &workspace), "reason": reason}),
            ),
            None,
        )?;
        if !passed {
            failure = Some((result.code, reason));
            break;
        }
    }
    if let Some((code, reason)) = failure {
        if code == 127 {
            return Err(block(
                &store,
                &mut task,
                BlockReason::Environment,
                "environment",
                "A precheck tool is missing; repair the environment",
            ));
        }
        event(
            &store,
            &mut task,
            TaskState::Implementing,
            reason,
            None,
            None,
        )?;
        return Err(Error::Status {
            code: "precheck_failed",
            message: format!("Precheck failed: {reason}; task returned to implementing"),
        });
    }
    let current_state = task.state;
    event(
        &store,
        &mut task,
        current_state,
        "precheck",
        Some(json!({"head": head, "passed": true, "failingCommand": null, "outputTail": ""})),
        None,
    )?;
    if report.classification.floor {
        return Err(block(
            &store,
            &mut task,
            BlockReason::FloorChanged,
            "floor_changed",
            "Floor paths changed; a human must push and approve these changes",
        ));
    }
    if !unchanged(&workspace, &metadata, head, &task.branch)? {
        return Err(block(
            &store,
            &mut task,
            BlockReason::RecordMismatch,
            "precheck_failed",
            "Task tree or HEAD changed before publication",
        ));
    }
    if task.state == TaskState::Prechecking {
        event(
            &store,
            &mut task,
            TaskState::Publishing,
            "precheck_passed",
            None,
            None,
        )?;
    }
    let reference = format!("refs/heads/{}", task.branch);
    let remote = publisher.remote_head(&task.push_remote_url, &reference)?;
    if let Some(remote) = remote.as_deref().filter(|remote| *remote != head) {
        let ancestor = publisher.git(&["merge-base", "--is-ancestor", remote, head])?;
        if ancestor.code != 0 {
            return Err(block(
                &store,
                &mut task,
                BlockReason::Diverged,
                "diverged",
                "Remote branch is not an ancestor of the checked HEAD; a human must reconcile it",
            ));
        }
    }
    if remote.as_deref() != Some(head) {
        let pushed = publisher
            .git(&[
                "push",
                "--",
                &task.push_remote_url,
                &format!("HEAD:{reference}"),
            ])
            .map_err(|_| {
                Error::status(
                    crate::error::PARTIAL,
                    "Push may have completed; rerun task publish to reconcile",
                )
            })?;
        if pushed.code != 0 {
            return Err(Error::status(
                crate::error::PARTIAL,
                "Push failed without force; rerun task publish to reconcile",
            ));
        }
    }
    let published = (|| -> Result<u64> {
        let gh_cwd = publisher.temporary.path();
        let number = if let Some(pull) = pull_list(gh_cwd, &task, &repository)? {
            pull.number
        } else {
            let commits = super::text(
                &workspace,
                &[
                    "log",
                    "--max-count=31",
                    "--format=%s",
                    &format!("{}..{head}", report.merge_base),
                    "--",
                ],
            )?;
            let body = pr_body(&task.intent, &commits)?;
            let body_path = publisher.temporary.path().join("body.txt");
            fs::write(&body_path, body).map_err(|_| Error::msg("Cannot prepare PR body"))?;
            let body_path = body_path
                .to_str()
                .ok_or_else(|| Error::msg("PR body path is not UTF-8"))?;
            let title = pr_title(&task.intent, &workspace);
            gh(
                gh_cwd,
                &repository,
                &[
                    "pr",
                    "create",
                    "--draft",
                    "--head",
                    &task.branch,
                    "--base",
                    &base_branch,
                    "--title",
                    &title,
                    "--body-file",
                    body_path,
                ],
            )?;
            pull_list(gh_cwd, &task, &repository)?
                .ok_or_else(|| {
                    Error::msg("Created PR not visible; rerun task publish to reconcile")
                })?
                .number
        };
        let output = gh(
            gh_cwd,
            &repository,
            &[
                "pr",
                "view",
                &number.to_string(),
                "--json",
                "number,headRefName,headRefOid,isCrossRepository,state",
            ],
        )?;
        let pull: Pull =
            serde_json::from_str(&output).map_err(|_| Error::msg("Invalid GitHub PR response"))?;
        if pull.number != number
            || pull.head_ref_name != task.branch
            || pull.is_cross_repository
            || pull.head_ref_oid != *head
            || pull.state.as_deref() != Some("OPEN")
        {
            return Err(Error::msg(
                "Published PR does not match the checked HEAD and repository",
            ));
        }
        Ok(number)
    })();
    let number = published.map_err(|_| {
        Error::status(
            crate::error::PARTIAL,
            "Branch publication may have completed; rerun task publish to reconcile the PR",
        )
    })?;
    if !unchanged(&workspace, &metadata, head, &task.branch).unwrap_or(false) {
        return Err(Error::status(
            crate::error::PARTIAL,
            "Task HEAD or tree changed during publication; human reconciliation required",
        ));
    }
    event(
        &store,
        &mut task,
        TaskState::Ci,
        "published",
        None,
        Some(number),
    ).map_err(|_| Error::status(crate::error::PARTIAL, "Publication completed but task record could not advance; rerun task publish to reconcile"))?;
    Ok(Report {
        task_id: task.id,
        head: head.to_owned(),
        pr_number: number,
        state: task.state,
    })
}

#[cfg(test)]
mod tests {
    use super::Repository;

    #[test]
    fn repository_addresses_reject_ambiguous_transports_and_authorities() {
        for url in [
            "/tmp/project.git",
            "file:///tmp/project.git",
            "http://github.com/owner/repo",
            "git://github.com/owner/repo",
            "https://github.com@elsewhere/owner/repo",
            "ssh://git@github.com@elsewhere/owner/repo",
            "ssh://other@github.com/owner/repo",
            "https://github.com:443/owner/repo",
            "ssh://git@github.com:not-a-port/owner/repo",
            "https://github.com/owner/repo?redirect=elsewhere",
            "https://github.com/owner/../repo",
            "https://github.com//repo",
            "git@github.com:/owner/repo",
        ] {
            assert!(Repository::parse(url).is_none(), "{url}");
        }
    }
}
