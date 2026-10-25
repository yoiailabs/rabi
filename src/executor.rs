//! Foreground workflow engine.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::allocation;
use crate::context::Context;
use crate::error::{Error, Result};
use crate::run::{self, Attempt, Record, StopReason};
use crate::workflow::{self, Action, Commands, Output, Step, StepKind, Target};
use crate::{git, provider};

pub(crate) struct Request<'a> {
    pub workflow: &'a workflow::Inspection,
    pub task: &'a str,
    pub checks: &'a [String],
    pub base: &'a str,
    pub name: &'a str,
}

fn reason(kind: &str, message: impl Into<String>) -> StopReason {
    StopReason {
        kind: kind.into(),
        message: message.into(),
    }
}

fn io(error: std::io::Error) -> Error {
    Error::msg(error.to_string())
}

/// Invalid inputs/trust fail before any allocation. Runtime failures return a
/// stopped record; a persistence failure returns Err without claiming completion.
pub(crate) fn execute(context: &Context, request: Request<'_>) -> Result<Record> {
    let definition = request
        .workflow
        .definition
        .as_ref()
        .filter(|_| request.workflow.valid())
        .ok_or_else(|| Error::validation("A validated workflow is required"))?;
    // Revalidate at this boundary too: callers can construct Inspection values.
    if !definition.validate().is_empty() {
        return Err(Error::validation("A validated workflow is required"));
    }
    let agents = definition.agents();
    for agent in agents.values() {
        provider::supported(agent.provider)?;
    }
    let mut uses_cli = false;
    let mut uses_list = false;
    for step in definition.steps.values() {
        if let StepKind::Check { commands } = &step.kind {
            match commands {
                Commands::Cli(_) => uses_cli = true,
                Commands::List(commands) => {
                    uses_list = true;
                    if commands.is_empty() {
                        return Err(Error::validation("Check commands must not be empty"));
                    }
                }
            }
        }
    }
    if (uses_cli && request.checks.is_empty()) || (uses_list && !request.checks.is_empty()) {
        return Err(Error::validation(
            "commands: cli requires checks; definition commands cannot be combined with CLI checks",
        ));
    }
    crate::worktree::validate_worktree_name(request.name)?;
    run::require_trust(&context.git_root)?;
    // Before prepare this describes the requested destination. Allocation fills
    // the observed metadata, retaining the schema-1 workspace object shape.
    let record = Record::new(
        &request.workflow.entry,
        definition,
        run::Workspace {
            id: None,
            name: request.name.into(),
            path: crate::paths::worktree_path(&context.worktrees_directory, request.name)
                .to_string_lossy()
                .into_owned(),
            branch: Some(request.name.into()),
        },
    );
    run::create(
        &context.git_root,
        &record,
        request.task,
        &request.workflow.yaml,
    )?;
    let run_id = record.id.clone();
    execute_created(context, request, record, definition, agents)
        .map_err(|error| Error::status(crate::error::PARTIAL, format!("Run {run_id} could not persist progress: {error}. Inspect its records before any further action.")))
}

fn execute_created(
    context: &Context,
    request: Request<'_>,
    mut record: Record,
    definition: &workflow::Definition,
    agents: BTreeMap<String, workflow::Agent>,
) -> Result<Record> {
    let dir = run::directory(&context.git_root, &record.id)?;
    fs::File::create(dir.join("worklog.md")).map_err(io)?;
    let mut engine = Engine {
        context,
        request,
        definition,
        dir,
        prepared: false,
        sessions: BTreeMap::new(),
        checks: String::new(),
        review: None,
        reviews: BTreeMap::new(),
        feedback: String::new(),
        report: String::new(),
        base_branch: None,
    };
    let mut next = definition.start.clone();
    loop {
        if record.steps_started >= definition.limits.max_steps as u64 {
            record.finish(Some(reason(
                "limit",
                "max_steps reached before starting the next step",
            )));
            run::save(&context.git_root, &record)?;
            return Ok(record);
        }
        let step = definition
            .steps
            .get(&next)
            .ok_or_else(|| Error::msg("Validated step is missing"))?;
        let agent = agents.get(&next).map(|agent| run::Agent {
            provider: agent.provider,
            model: agent.model.clone(),
            effort: agent.effort.clone(),
            session_id: None,
            continued_from: match &step.kind {
                StepKind::Agent {
                    context: workflow::Context::Continue { step },
                    ..
                } => Some(step.clone()),
                _ => None,
            },
        });
        crate::output::errln!(
            "Run {}: starting {} ({}/{})",
            record.id,
            crate::output::visible_text(&next),
            record.steps_started + 1,
            definition.limits.max_steps
        );
        let mut attempt = run::begin_attempt(&context.git_root, &mut record, &next, agent)?;
        let attempt_dir =
            run::attempt_directory(&context.git_root, &record.id, &next, attempt.attempt)?;
        fs::File::create(attempt_dir.join("stdout.log")).map_err(io)?;
        fs::File::create(attempt_dir.join("stderr.log")).map_err(io)?;
        if let StepKind::Agent { instruction, .. } = &step.kind {
            fs::write(
                attempt_dir.join("prompt.md"),
                format!("{instruction}\n\n# Task\n{}", engine.request.task),
            )
            .map_err(io)?;
        }
        let outcome = engine.step(step, &mut record, &mut attempt, &attempt_dir);
        if attempt.agent.is_some() && !attempt_dir.join("answer.md").exists() {
            fs::File::create(attempt_dir.join("answer.md")).map_err(io)?;
        }
        let mut stop = match outcome {
            Ok(result) => {
                attempt.result = result.into();
                None
            }
            Err(stop) => {
                attempt.result = "error".into();
                Some(stop)
            }
        };
        // Observe, never gate on HEAD identity or compare it with past attempts.
        if engine.prepared {
            let path = Path::new(&record.workspace.path);
            attempt.head = git::head_commit(path);
            match dirty(path) {
                Ok(Some(message)) if !step.allow_dirty => {
                    stop = Some(reason("dirty_workspace", message))
                }
                Err(error) if stop.is_none() => {
                    stop = Some(reason("operation_failed", error.to_string()))
                }
                _ => {}
            }
        }
        attempt.finished_at = Some(crate::metadata::iso_now());
        run::save_attempt(&context.git_root, &record.id, &attempt)?;
        let mut worklog = OpenOptions::new()
            .append(true)
            .open(engine.dir.join("worklog.md"))
            .map_err(io)?;
        writeln!(
            worklog,
            "{}: {} -> {}{}",
            attempt.attempt,
            crate::output::visible_text(&attempt.step),
            attempt.result,
            stop.as_ref()
                .map(|s| format!(" ({})", s.kind))
                .unwrap_or_default()
        )
        .map_err(io)?;
        if stop.is_none() {
            if let Some(agent) = &attempt.agent {
                if let Some(session) = &agent.session_id {
                    engine.sessions.insert(next.clone(), session.clone());
                }
            }
            match step.transitions.get(&attempt.result) {
                None => {
                    stop = Some(reason(
                        "operation_failed",
                        format!("Step '{next}' has no on entry for '{}'", attempt.result),
                    ))
                }
                Some(target) => {
                    let to = match target {
                        Target::Step(to) => to,
                        Target::Loop { to, name } => {
                            let counter = record
                                .loops
                                .get_mut(name)
                                .ok_or_else(|| Error::msg("Validated loop is missing"))?;
                            if counter.used >= counter.max as u64 {
                                let message = match attempt.result.as_str() {
                                    "revise" => format!("Review limit reached: loop '{name}' exhausted. Reconsider the approach before starting a new run."),
                                    "fail" => format!("Check failure exhausted loop '{name}'. Reconsider the approach before starting a new run."),
                                    _ => format!("Loop '{name}' exhausted"),
                                };
                                stop = Some(reason("limit", message));
                            } else {
                                counter.used += 1;
                            }
                            to
                        }
                    };
                    next = to.clone();
                    if stop.is_none() && to == "stopped" {
                        stop = Some(reason("stopped", "Workflow selected stopped"));
                    }
                }
            }
        }
        if stop.is_some() || next == "succeeded" {
            record.finish(stop);
        } else {
            record.current_step = Some(next.clone());
        }
        run::save(&context.git_root, &record)?;
        if record.state != run::State::Running {
            return Ok(record);
        }
    }
}

struct Engine<'a> {
    context: &'a Context,
    request: Request<'a>,
    definition: &'a workflow::Definition,
    dir: PathBuf,
    prepared: bool,
    // Latest successful session for each step; resuming a thread appends to it.
    sessions: BTreeMap<String, String>,
    checks: String,
    review: Option<String>,
    reviews: BTreeMap<String, Review>,
    feedback: String,
    report: String,
    base_branch: Option<String>,
}

struct Review {
    round: u64,
    head: String,
    base: String,
    verdict: String,
}

type StepResult = std::result::Result<&'static str, StopReason>;

impl Engine<'_> {
    fn workspace<'a>(&self, record: &'a Record) -> Result<&'a Path> {
        if !self.prepared {
            return Err(Error::msg("Step requires a prepared workspace"));
        }
        Ok(Path::new(&record.workspace.path))
    }

    fn step(
        &mut self,
        step: &Step,
        record: &mut Record,
        attempt: &mut Attempt,
        dir: &Path,
    ) -> StepResult {
        self.perform(step, record, attempt, dir)
            .map_err(|error| reason("operation_failed", error.to_string()))?
    }

    fn perform(
        &mut self,
        step: &Step,
        record: &mut Record,
        attempt: &mut Attempt,
        dir: &Path,
    ) -> Result<StepResult> {
        match &step.kind {
            StepKind::System {
                action: Action::Prepare,
            } => {
                if self.prepared {
                    return Ok(Err(reason(
                        "operation_failed",
                        "Workspace is already prepared",
                    )));
                }
                let _logs = crate::process::log_inherited_output(dir);
                let allocation = allocation::create(
                    self.context,
                    allocation::Request {
                        name: Some(self.request.name),
                        base: Some(self.request.base),
                        branch: None,
                        pr: None,
                        issue: None,
                        detach: false,
                        reuse: false,
                        copy_files: &[],
                        metadata: crate::metadata::MetadataInput::default(),
                        hooks: crate::trust::HookControl {
                            no_hooks: false,
                            json: true,
                            prompt: false,
                        },
                        no_setup: false,
                    },
                    &crate::worktree::Logger {
                        enabled: false,
                        json: true,
                    },
                )?;
                attempt.exit_code = allocation.setup.hooks.exit_code;
                record.workspace = run::Workspace::from_allocation(&allocation);
                self.prepared = true;
                // Retain the allocated workspace even when setup failed.
                run::save(&self.context.git_root, record)?;
                let base = allocation.source["base"]
                    .as_str()
                    .ok_or_else(|| Error::msg("Allocation did not return a base"))?;
                self.base_branch = base
                    .strip_prefix("refs/heads/")
                    .or_else(|| base.strip_prefix("refs/remotes/origin/"))
                    .map(str::to_owned);
                record.base = Some(git::output(
                    &["rev-parse", "--verify", &format!("{base}^{{commit}}")],
                    Some(&self.context.source_checkout),
                    true,
                )?);
                if allocation.setup.failed() || allocation.metadata_error.is_some() {
                    return Ok(Err(reason(
                        "operation_failed",
                        allocation
                            .metadata_error
                            .or(allocation.setup.status.error)
                            .unwrap_or_else(|| "Workspace setup failed".into()),
                    )));
                }
                Ok(Ok("ok"))
            }
            StepKind::Check { commands } => {
                let workspace = self.workspace(record)?;
                let commands = match commands {
                    Commands::Cli(_) => self.request.checks,
                    Commands::List(list) => list,
                };
                self.checks.clear();
                for (index, command) in commands.iter().enumerate() {
                    let command_dir = dir.join(format!("command-{:03}", index + 1));
                    fs::create_dir(&command_dir).map_err(io)?;
                    let mut child = Command::new("sh");
                    child
                        .args(["-c", command])
                        .current_dir(workspace)
                        .stdin(Stdio::null());
                    attempt.checks.push(run::Check {
                        command: command.clone(),
                        exit_code: None,
                    });
                    let code = logged(&mut child, &command_dir)?;
                    if let Some(check) = attempt.checks.last_mut() {
                        check.exit_code = Some(code);
                    }
                    attempt.exit_code = Some(code);
                    for name in ["stdout.log", "stderr.log"] {
                        let data = fs::read(command_dir.join(name)).map_err(io)?;
                        OpenOptions::new()
                            .append(true)
                            .open(dir.join(name))
                            .map_err(io)?
                            .write_all(&data)
                            .map_err(io)?;
                    }
                    self.checks.push_str(&format!(
                        "Command: {command}\nExit: {code}\nstdout: {}\nstderr: {}\n",
                        String::from_utf8_lossy(
                            &fs::read(command_dir.join("stdout.log")).map_err(io)?
                        ),
                        String::from_utf8_lossy(
                            &fs::read(command_dir.join("stderr.log")).map_err(io)?
                        )
                    ));
                    if code != 0 {
                        return Ok(Ok("fail"));
                    }
                }
                Ok(Ok("pass"))
            }
            StepKind::Agent {
                mode,
                output,
                context,
                instruction,
                ..
            } => {
                let workspace = self.workspace(record)?;
                let config = self
                    .definition
                    .agents()
                    .remove(&attempt.step)
                    .ok_or_else(|| Error::msg("Resolved agent is missing"))?;
                let session = match context {
                    workflow::Context::Fresh(_) => None,
                    workflow::Context::Continue { step } => Some(
                        self.sessions
                            .get(step)
                            .ok_or_else(|| {
                                Error::msg(format!(
                                    "Continue target '{step}' has no successful session"
                                ))
                            })?
                            .as_str(),
                    ),
                };
                let merge_base = git::output(
                    &[
                        "merge-base",
                        record
                            .base
                            .as_deref()
                            .ok_or_else(|| Error::msg("Run base is missing"))?,
                        "HEAD",
                    ],
                    Some(workspace),
                    true,
                )?;
                let head = git::output(&["rev-parse", "HEAD"], Some(workspace), true)?;
                let previous = self.reviews.get(&attempt.step);
                let diff_base = if *output == Output::ReviewV1 {
                    previous
                        .map(|review| review.head.as_str())
                        .unwrap_or(&merge_base)
                } else {
                    &merge_base
                };
                let diff = if *output == Output::ReviewV1 && !step.allow_dirty {
                    git::output(&["diff", diff_base, &head, "--"], Some(workspace), false)?
                } else {
                    git::output(&["diff", diff_base, "--"], Some(workspace), false)?
                };
                let mut prompt = format!(
                    "{instruction}\n\n# Task\n{}\n\n# Diff\n{diff}\n",
                    self.request.task
                );
                if *output == Output::ReviewV1 {
                    let round = previous.map(|review| review.round + 1).unwrap_or(1);
                    let limit = step
                        .transitions
                        .get("revise")
                        .and_then(|target| match target {
                            Target::Loop { name, .. } => {
                                self.definition.loops.get(name).map(|max| *max as u64 + 1)
                            }
                            _ => None,
                        });
                    let merge_base = previous
                        .map(|review| review.base.as_str())
                        .unwrap_or(&merge_base);
                    prompt.push_str(&format!("\n# Review round {round} / {}\nInitial merge-base: {merge_base}\nReviewed HEAD: {head}\nReview command: git diff {diff_base} {head}\n", limit.map(|n| n.to_string()).unwrap_or_else(|| format!("max_steps {}", self.definition.limits.max_steps))));
                    for name in ["REVIEW.md", "AGENTS.md", "CLAUDE.md"] {
                        let path = workspace.join(name);
                        if path.is_file() {
                            prompt.push_str(&format!(
                                "Read {} when reviewing.\n",
                                crate::output::visible_text(&path.to_string_lossy())
                            ));
                        }
                    }
                    prompt.push_str(REVIEW_INSTRUCTION);
                    prompt.push_str(&format!(
                        "\n# Prior review answers and fix responses\n{}\n",
                        self.feedback
                    ));
                }
                let untracked = git::output(
                    &["ls-files", "--others", "--exclude-standard", "-z"],
                    Some(workspace),
                    false,
                )?;
                if !untracked.is_empty() {
                    prompt.push_str("\n# Untracked paths\n");
                    for path in untracked.split('\0').filter(|path| !path.is_empty()) {
                        prompt.push_str(&crate::output::visible_text(path));
                        prompt.push('\n');
                    }
                }
                if matches!(mode, workflow::Mode::ReadOnly) {
                    prompt.push_str("\nRead only: do not edit files.\n");
                }
                if *output == Output::ReviewV1
                    || (matches!(mode, workflow::Mode::Edit) && !self.checks.is_empty())
                {
                    prompt.push_str(&format!("\n# Latest checks\n{}\n", self.checks));
                }
                if matches!(mode, workflow::Mode::Edit) {
                    if let Some(review) = &self.review {
                        prompt.push_str(&format!("\n# Latest review\n{review}\n"));
                    }
                }
                prompt.push_str(&format!("\n# Run facts\nRun branch: {}\nBase branch: {}\nRequested base ref: {}\nRemote: origin\n",
                    record.workspace.branch.as_deref().unwrap_or("unknown"),
                    self.base_branch.as_deref().unwrap_or("unknown (resolve the requested base ref)"),
                    self.request.base));
                if matches!(mode, workflow::Mode::Edit) {
                    prompt.push_str(&format!(
                        "\n# Latest agent report\n{}\n\n# Review outcomes\n",
                        self.report
                    ));
                    if self.reviews.is_empty() {
                        prompt.push_str("No reviews performed.\n");
                    }
                    for (id, review) in &self.reviews {
                        prompt.push_str(&format!(
                            "Review {id}: verdict {}, reviewed HEAD {}, rounds {}\n",
                            review.verdict, review.head, review.round
                        ));
                    }
                    prompt.push_str(&format!(
                        "\n# Review history and fix responses\n{}\n",
                        self.feedback
                    ));
                    if self.checks.is_empty() {
                        prompt.push_str("\nChecks not run.\n");
                    }
                }
                // Keep the output contract last, after task, diff and all context.
                if *output == Output::ReviewV1 {
                    prompt.push_str(VERDICT_INSTRUCTION);
                }
                fs::write(dir.join("prompt.md"), prompt).map_err(io)?;
                let mut command = provider::codex(&config, mode, workspace, dir, session)?;
                let code = logged(&mut command, dir)?;
                attempt.exit_code = Some(code);
                // Preserve any reported session even on nonzero exit, without
                // treating it as a successful source for a later continuation.
                let session = provider::session_id(&dir.join("stdout.log"));
                if let Some(agent) = &mut attempt.agent {
                    agent.session_id = session.as_ref().ok().cloned();
                }
                if code != 0 {
                    return Ok(Err(reason(
                        "operation_failed",
                        format!("Codex exited with code {code}"),
                    )));
                }
                session?;
                let answer = fs::read_to_string(dir.join("answer.md")).map_err(io)?;
                if *output == Output::ReviewV1 {
                    let round = self
                        .reviews
                        .get(&attempt.step)
                        .map(|review| review.round + 1)
                        .unwrap_or(1);
                    let base = self
                        .reviews
                        .get(&attempt.step)
                        .map(|review| review.base.clone())
                        .unwrap_or(merge_base);
                    self.reviews.insert(
                        attempt.step.clone(),
                        Review {
                            round,
                            head: head.clone(),
                            base,
                            verdict: answer
                                .lines()
                                .rev()
                                .find(|line| !line.trim().is_empty())
                                .unwrap_or("")
                                .trim()
                                .into(),
                        },
                    );
                    self.feedback.push_str(&format!(
                        "\nReview {} round {round} at {head}:\n{answer}\n",
                        attempt.step
                    ));
                    self.review = Some(answer.clone());
                    return Ok(
                        match answer
                            .lines()
                            .rev()
                            .find(|line| !line.trim().is_empty())
                            .map(str::trim)
                        {
                            Some("判定: 問題なし") => Ok("pass"),
                            Some("判定: 要修正") => Ok("revise"),
                            _ => Err(reason(
                                "contract_invalid",
                                "Review answer has no valid final verdict line",
                            )),
                        },
                    );
                }
                self.report = answer.clone();
                if matches!(mode, workflow::Mode::Edit) && self.review.is_some() {
                    self.feedback
                        .push_str(&format!("\nFix response ({}):\n{answer}\n", attempt.step));
                }
                Ok(Ok("ok"))
            }
        }
    }
}

/// File-backed output keeps pipe buffers and large tool output out of the engine.
/// The guard kills and reaps the child if waiting fails.
fn logged(command: &mut Command, dir: &Path) -> Result<i32> {
    command
        .stdout(fs::File::create(dir.join("stdout.log")).map_err(io)?)
        .stderr(fs::File::create(dir.join("stderr.log")).map_err(io)?);
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let mut child = Child(command.spawn().map_err(io)?);
    let status = child.0.wait().map_err(io)?;
    Ok(status.code().unwrap_or(1))
}

fn dirty(workspace: &Path) -> Result<Option<String>> {
    let status = git::output(
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
        Some(workspace),
        false,
    )?;
    let mut entries = status.split('\0').filter(|entry| !entry.is_empty());
    let mut paths = Vec::new();
    while let Some(entry) = entries.next() {
        if let Some(path) = entry.get(3..) {
            paths.push(crate::output::visible_text(path));
        }
        if entry
            .as_bytes()
            .get(..2)
            .is_some_and(|s| s.iter().any(|b| matches!(b, b'R' | b'C')))
        {
            entries.next();
        }
    }
    if paths.is_empty() {
        return Ok(None);
    }
    let count = paths.len();
    paths.truncate(20);
    Ok(Some(format!(
        "Uncommitted workspace paths: {}{}",
        paths.join(", "),
        if count > 20 {
            format!(" (and {} more)", count - 20)
        } else {
            String::new()
        }
    )))
}

const REVIEW_INSTRUCTION: &str = "
Start with an actual count of actionable findings (Important / Nit / Pre-existing).
Tag findings [Important], [Nit], or [Pre-existing]; give each an ID and file:line
code evidence, triggering conditions, impact, confidence, and a fix direction.
Group instances by bug class. Pre-existing means present at the initial base.
Read prior findings and responses; update their status without duplicate findings.
Count unresolved prior Important findings too. Report unverified assumptions.
If the comparison cannot be interpreted (including rewritten history), request a
stop and reconsideration; do not silently broaden the diff. Every session is fresh.
Unresolved Important findings require the final line 判定: 要修正.
Only Nit/Pre-existing or no findings require the final line 判定: 問題なし.
";

const VERDICT_INSTRUCTION: &str = "\nEven when there are no findings, the final line must be exactly 判定: 問題なし or 判定: 要修正.\n";

#[cfg(test)]
mod tests;
