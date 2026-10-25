use std::path::Path;

use crate::args::{self, Opt, Parsed};
use crate::error::{Error, Result, GENERAL, NOT_FOUND, PARTIAL, SUCCESS, VALIDATION};
use crate::help;
use crate::metadata::{self, MetadataInput};
use crate::output::{errln, outln, visible_text};
use crate::preferences;
use crate::projects::{self, ProjectListEntry, ProjectRecord, ProjectRegistryStore};
use crate::repository;
use crate::trust::{self, HookControl};
use crate::worktree::{self, Logger};

struct Command {
    name: &'static str,
    help_key: &'static str,
    run: fn(&[String]) -> Result<i32>,
    children: &'static [Command],
}

pub fn run(args: &[String]) -> Result<i32> {
    let result = run_inner(args);
    // JSON diagnostics are already reflected in finish()'s envelope and status.
    // Check its stdout write here, or all command output in text mode.
    crate::output::check_io()?;
    result
}

fn run_inner(args: &[String]) -> Result<i32> {
    // Common options may precede the command as well as follow it. Keep the
    // command path in front so exec/remainder boundaries retain their meaning.
    let mut prefix = 0;
    while let Some(arg) = args.get(prefix) {
        let name = arg.split('=').next().unwrap_or(arg);
        if arg.starts_with("-R") && arg != "-R" {
            prefix += 1;
            continue;
        }
        if !matches!(name, "--json" | "--output" | "--no-input" | "--repo" | "-R") {
            break;
        }
        prefix += 1;
        if matches!(name, "--output" | "--repo" | "-R") && !arg.contains('=') && prefix < args.len()
        {
            prefix += 1;
        }
    }
    if prefix > 0 {
        if prefix == args.len() {
            return unresolved_usage(args, "", "A command is required after common options");
        }
        let rest = &args[prefix..];
        let consumed = find(rest, COMMANDS)
            .map(|(_, remaining)| rest.len() - remaining.len())
            .unwrap_or(1);
        let mut reordered = rest[..consumed].to_vec();
        reordered.extend_from_slice(&args[..prefix]);
        reordered.extend_from_slice(&rest[consumed..]);
        return run_inner(&reordered);
    }
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        let prepared = crate::runtime::prepare(args, true, "", &command_options(""), true);
        if let Some(error) = prepared.error {
            return unresolved_usage(args, "", &error.to_string());
        }
        outln!("{}", help::main_help()?);
        return Ok(SUCCESS);
    }
    match find(args, COMMANDS) {
        Some((command, remaining)) => {
            let key = command.help_key;
            let supports_json = !matches!(key, "exec" | "completions" | "shell-init");
            let prepared = crate::runtime::prepare(
                remaining,
                supports_json || (!command.children.is_empty() && !remaining.is_empty()),
                key,
                &command_options(key),
                true,
            );
            let missing_subcommand = !command.children.is_empty() && prepared.args.is_empty();
            if prepared.error.is_none() && (prepared.help || (missing_subcommand && !prepared.json))
            {
                match help::command_help(key)? {
                    Some(text) => outln!("{text}"),
                    None => outln!("Help not available for command '{}'", command.name),
                }
                return Ok(SUCCESS);
            }
            crate::output::begin(key, prepared.json);
            let result = match prepared.error {
                Some(error) => Err(error),
                None if missing_subcommand => Err(Error::validation(format!(
                    "A subcommand is required for '{key}'"
                ))),
                None => (command.run)(&prepared.args),
            };
            let exit = match result {
                Ok(exit) => exit,
                Err(error) => fail_with(&error, error.exit_code())?,
            };
            crate::output::finish(exit)
        }
        None => unresolved_usage(
            args,
            &args[0],
            &if args[0].starts_with('-') {
                format!("Unknown option '{}'", args[0])
            } else {
                format!("Unknown command '{}'", args.join(" "))
            },
        ),
    }
}

fn unresolved_usage(args: &[String], command: &str, message: &str) -> Result<i32> {
    let prepared = crate::runtime::prepare(args, true, command, &command_options(command), false);
    crate::output::begin(command, prepared.json);
    let error = prepared.error.unwrap_or_else(|| Error::validation(message));
    let exit = fail_with(&error, VALIDATION)?;
    if !crate::output::json() {
        errln!("{}", help::main_help()?);
    }
    crate::output::finish(exit)
}

fn find<'a>(args: &'a [String], commands: &'a [Command]) -> Option<(&'a Command, &'a [String])> {
    let (name, rest) = args.split_first()?;
    let command = commands.iter().find(|command| command.name == name)?;
    if !command.children.is_empty() && !rest.is_empty() {
        if let Some(found) = find(rest, command.children) {
            return Some(found);
        }
    }
    Some((command, rest))
}

fn completions(args: &[String]) -> Result<i32> {
    shell_script(args, "completions")
}

fn shell_init(args: &[String]) -> Result<i32> {
    shell_script(args, "shell-init")
}

fn shell_script(args: &[String], key: &str) -> Result<i32> {
    let parsed = args::parse(args, &command_options(key), true)?;
    let [shell] = parsed.positionals.as_slice() else {
        return Err(Error::validation(format!(
            "Usage: rabi {key} bash|zsh|fish"
        )));
    };
    let shell = crate::shell::Shell::parse(shell)?;
    let script = if key == "completions" {
        crate::shell::completions(shell)?
    } else {
        crate::shell::init(shell).to_string()
    };
    outln!("{script}");
    Ok(SUCCESS)
}

fn sync(args: &[String]) -> Result<i32> {
    let _hooks = crate::process::suppress_git_hooks();
    let json = crate::output::json();
    let parsed = args::parse(args, &command_options("sync"), true)?;
    let request = crate::sync::Request {
        names: parsed.positionals.clone(),
        all: parsed.flag("all"),
        main: parsed.flag("main"),
        auto: parsed.flag("auto"),
        dry_run: parsed.flag("dry-run"),
        rebase: parsed.flag("rebase"),
        merge: parsed.flag("merge"),
        rewrite_published: parsed.flag("rewrite-published"),
        keep_conflicts: parsed.flag("keep-conflicts"),
    };
    request.validate()?;
    let context = match command_context(&parsed) {
        Ok(context) => context,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let report = match crate::sync::run(&context, &request) {
        Ok(report) => report,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if json {
        emit_json(&report)?;
        if report.exit != 0 {
            crate::output::failure(
                Error::Status {
                    code: if report.exit == crate::error::REFUSED {
                        "published"
                    } else {
                        crate::error::status_code(report.exit)
                    },
                    message: "Sync did not complete successfully".into(),
                }
                .to_json(report.exit)
                .error,
            );
        }
    } else {
        for item in report.default.iter().chain(&report.worktrees) {
            // Git diagnostics can contain newlines; human output stays one line per item.
            let mut detail = item
                .detail
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !item.conflicts.is_empty() {
                let files = item.conflicts.join(", ").replace(['\n', '\r'], " ");
                detail = format!("{files} ({detail})");
            }
            outln!("{}  {}  {}", item.name, item.status, detail);
        }
        let mut counts = std::collections::BTreeMap::new();
        for item in report.default.iter().chain(&report.worktrees) {
            *counts.entry(item.status.as_str()).or_insert(0usize) += 1;
        }
        outln!(
            "Sync complete: {}",
            counts
                .into_iter()
                .map(|(status, count)| format!("{count} {status}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(report.exit)
}

fn create(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("create"), true)?;
    if parsed.positionals.len() > 1 {
        return Err(Error::validation("Usage: rabi create [NAME] [options]"));
    }
    if parsed.flag("no-setup") && parsed.flag("copy-file") {
        return Err(Error::validation(
            "--no-setup cannot be combined with --copy-file",
        ));
    }
    let context = command_context(&parsed)?;
    let allocation = crate::allocation::create(
        &context,
        crate::allocation::Request {
            name: parsed.positionals.first().map(String::as_str),
            base: single_value(&parsed, "base")?,
            branch: single_value(&parsed, "branch")?,
            pr: single_value(&parsed, "pr")?,
            issue: single_value(&parsed, "issue")?,
            detach: parsed.flag("detach"),
            reuse: parsed.flag("reuse"),
            copy_files: &parsed.values("copy-file"),
            metadata: metadata_input(&parsed)?,
            no_setup: parsed.flag("no-setup"),
            hooks: hook_control(&parsed, crate::output::json(), true),
        },
        &logger_for(crate::output::json()),
    )?;
    render_create(&allocation, &logger_for(crate::output::json()))
}

fn render_create(allocation: &crate::allocation::Allocation, logger: &Logger) -> Result<i32> {
    let tree = &allocation.workspace;
    let metadata_error = allocation.metadata_error.as_deref();
    let setup = &allocation.setup;
    logger.log(&format!(
        "{} workspace '{}' at {}",
        if allocation.source["reused"] == true {
            "Reused"
        } else {
            "Created"
        },
        visible_text(&tree.name),
        visible_text(&tree.path)
    ));
    if let Some(error) = metadata_error {
        logger.warn(
            "metadata_save",
            &format!("Warning: {}", visible_text(error)),
        );
    }
    crate::output::data(&serde_json::json!({
        "workspace":tree.to_json(None), "source":allocation.source,
        "setup":setup.outcome(), "copy":setup.copy(),
        "metadata":worktree::metadata_save_outcome(metadata_error)
    }))?;
    if let Some(retry) = &allocation.retry_command {
        return Err(Error::status(
            PARTIAL,
            format!("Workspace created but setup did not complete. Retry with `{retry}`."),
        ));
    }
    Ok(SUCCESS)
}

fn setup(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("setup"), true)?;
    if parsed.positionals.len() > 1 {
        return Err(Error::validation(
            "Usage: rabi setup [SEL] [--copy-file PAT] [--no-hooks] [--source-checkout PATH]",
        ));
    }
    let context = if let Some(source) = single_value(&parsed, "source-checkout")? {
        crate::context::open_for_hooks(selected_repository(&parsed)?.as_deref(), Path::new(source))?
    } else {
        command_context(&parsed)?
    };
    crate::setup::execute(
        &context,
        parsed
            .positionals
            .first()
            .map(String::as_str)
            .unwrap_or("."),
        &parsed.values("copy-file"),
        hook_control(&parsed, crate::output::json(), true),
        &logger_for(crate::output::json()),
    )
}

fn adopt(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("adopt"), true)?;
    if parsed.positionals.len() != 1 {
        return Err(Error::validation("Usage: rabi adopt PATH [options]"));
    }
    let context = command_context(&parsed)?;
    let mut input = metadata_input(&parsed)?;
    input.name = single_value(&parsed, "name")?.map(str::to_owned);
    if let Some(name) = &input.name {
        worktree::validate_worktree_name(name)?;
    }
    let path = Path::new(&parsed.positionals[0]);
    if let Some(base) = single_value(&parsed, "base")? {
        input.base = Some(base.to_owned());
    }
    let tree = crate::allocation::adopt(&context, path, input)?;
    if !crate::output::json() {
        outln!(
            "Adopted workspace '{}' at {}",
            visible_text(&tree.name),
            visible_text(&tree.path)
        );
    }
    emit_json(&serde_json::json!({"workspace":tree.to_json(None)}))
}

fn forget(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("forget"), true)?;
    if parsed.positionals.is_empty() {
        return Err(Error::validation("Usage: rabi forget SEL..."));
    }
    let context = command_context(&parsed)?;
    // Resolve every selector before dropping links that later selectors might need.
    let trees = parsed
        .positionals
        .iter()
        .map(|selector| {
            crate::selectors::resolve_including_orphaned(&context, selector, false, None)
        })
        .collect::<Result<Vec<_>>>()?;
    let (items, error) = forget_resolved(&context, trees, worktree::worktree_at_path);
    emit_json(&serde_json::json!({"items":items}))?;
    match error {
        Some(error) => fail_with(&error, error.exit_code()),
        None => Ok(0),
    }
}

fn forget_resolved(
    context: &crate::context::Context,
    trees: Vec<worktree::WorktreeInfo>,
    mut refresh: impl FnMut(&crate::context::Context, &Path) -> Result<worktree::WorktreeInfo>,
) -> (Vec<serde_json::Value>, Option<Error>) {
    let mut items = Vec::new();
    for tree in trees {
        if let Err(error) = metadata::remove(&context.git_root, Path::new(&tree.path)) {
            let partial = !items.is_empty();
            items.push(serde_json::json!({"workspace":tree.to_json(None),"ok":false,"error":error.to_json(error.exit_code()).error}));
            let error = if partial {
                Error::status(PARTIAL, error.to_string())
            } else {
                error
            };
            return (items, Some(error));
        }
        // Removal has completed. A failed read must not mark this item as unforgotten.
        let forgotten = if tree.orphaned {
            // Orphaned records have no Git checkout to resolve after removal.
            let mut forgotten = tree.clone();
            forgotten.managed = worktree::is_managed(context, Path::new(&tree.path), false);
            forgotten.metadata = metadata::WorktreeMetadata::default();
            forgotten.recorded = false;
            Ok(forgotten)
        } else {
            refresh(context, Path::new(&tree.path))
        };
        if !crate::output::json() {
            outln!("Forgot workspace '{}'", visible_text(&tree.name));
        }
        match forgotten {
            Ok(forgotten) => {
                items.push(serde_json::json!({"workspace":forgotten.to_json(None),"ok":true}));
            }
            Err(error) => {
                // Keep only known identity fields; intent was removed and Git state is unknown.
                items.push(serde_json::json!({
                    "workspace": {"path":tree.path,"kind":if tree.is_default {"primary"} else {"workspace"},"setup":null},
                    "ok":true,
                    "error":error.to_json(error.exit_code()).error,
                }));
                return (items, Some(Error::status(PARTIAL, error.to_string())));
            }
        }
    }
    (items, None)
}

fn list(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("list"), false)?;
    let json = crate::output::json();
    if (parsed.flag("names") && parsed.flag("paths"))
        || (json && (parsed.flag("names") || parsed.flag("paths")))
    {
        return Err(Error::validation(
            "Only one of --json, --names or --paths can be specified",
        ));
    }
    let kind = single_value(&parsed, "kind")?;
    if kind.is_some_and(|kind| !matches!(kind, "workspace" | "primary")) {
        return Err(Error::validation("--kind must be workspace or primary"));
    }
    let contexts = if parsed.flag("all-repos") {
        if parsed.value("repo").is_some() {
            return Err(Error::validation(
                "--all-repos cannot be combined with -R/--repo",
            ));
        }
        let catalog = projects::list_catalog(
            preferences::load_preferences()?
                .ghq_discovery
                .unwrap_or(true),
        )?;
        for warning in catalog.warnings {
            crate::output::warning("repo_discovery", warning);
        }
        catalog
            .projects
            .iter()
            .map(|repo| {
                let (ProjectListEntry::Registry { root_path, .. }
                | ProjectListEntry::Ghq { root_path, .. }) = repo;
                crate::context::create_at(Path::new(root_path))
            })
            .collect::<Result<Vec<_>>>()?
    } else {
        vec![command_context(&parsed)?]
    };
    let mut workspaces = Vec::new();
    let mut repos = Vec::new();
    for context in contexts {
        if let Some(warning) = metadata::read_problem(&context.git_root) {
            crate::output::warning("metadata_read", format!("Warning: {warning}"));
        }
        let repo = crate::selectors::repo_json(&context.git_root);
        if !json && !parsed.flag("names") && !parsed.flag("paths") {
            outln!(
                "{}  (primary: {})",
                visible_text(&repo.name),
                visible_text(&repo.root)
            );
        }
        repos.push(repo);
        let mut count = 0;
        for tree in worktree::list_worktrees(&context, false, false)? {
            if kind.is_some_and(|kind| (kind == "primary") != tree.is_default) {
                continue;
            }
            count += 1;
            let workspace = tree.to_json(None);
            if !json {
                if parsed.flag("names") {
                    outln!("{}", tree.name);
                } else if parsed.flag("paths") {
                    outln!("{}", tree.path);
                } else {
                    let upstream = workspace
                        .upstream
                        .as_ref()
                        .map(|u| {
                            if u.gone {
                                "upstream:gone".into()
                            } else {
                                format!(
                                    "↑{} ↓{}",
                                    u.ahead.map(|n| n.to_string()).unwrap_or_else(|| "?".into()),
                                    u.behind
                                        .map(|n| n.to_string())
                                        .unwrap_or_else(|| "?".into())
                                )
                            }
                        })
                        .unwrap_or_default();
                    let links = [
                        tree.metadata.links.pr.as_ref().map(|l| {
                            format!(
                                "#{}",
                                l.number
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| l.url.clone().unwrap_or_default())
                            )
                        }),
                        tree.metadata.links.issue.as_ref().map(|l| {
                            format!(
                                "issue #{}",
                                l.number
                                    .map(|n| n.to_string())
                                    .unwrap_or_else(|| l.url.clone().unwrap_or_default())
                            )
                        }),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                    outln!(
                        "{} {}  {}  {}  {}  {}  {}  {}{}",
                        if workspace.current { "*" } else { " " },
                        visible_text(&tree.name),
                        visible_text(workspace.branch.as_deref().unwrap_or("(detached)")),
                        upstream,
                        if tree.is_clean { "" } else { "dirty" },
                        visible_text(&links),
                        workspace
                            .metadata
                            .setup
                            .as_ref()
                            .filter(|s| s.state != crate::setup::State::Ok)
                            .map(|s| format!("setup:{}", s.state.as_str()))
                            .unwrap_or_default(),
                        visible_text(tree.metadata.purpose.as_deref().unwrap_or("")),
                        if tree.orphaned { " [orphaned]" } else { "" }
                    );
                }
            }
            workspaces.push(workspace);
        }
        if count == 0 && !json && !parsed.flag("names") && !parsed.flag("paths") {
            outln!("No linked workspaces found");
        }
    }
    if json {
        let mut data = serde_json::json!({"workspaces": workspaces});
        if parsed.flag("all-repos") {
            data["repos"] = serde_json::to_value(repos).map_err(|e| Error::msg(e.to_string()))?;
        }
        emit_json(&data)
    } else {
        Ok(SUCCESS)
    }
}

fn inspect(args: &[String], path_only: bool) -> Result<i32> {
    let parsed = args::parse(args, &command_options("show"), true)?;
    if parsed.positionals.len() > 1 {
        return Err(Error::validation("Usage: rabi show/path [SEL]"));
    }
    let context = command_context(&parsed)?;
    if let Some(warning) = metadata::read_problem(&context.git_root) {
        crate::output::warning("metadata_read", format!("Warning: {warning}"));
    }
    let tree = crate::selectors::resolve_including_orphaned(
        &context,
        parsed
            .positionals
            .first()
            .map(String::as_str)
            .unwrap_or("."),
        false,
        None,
    )?;
    if path_only && tree.orphaned {
        return Err(Error::msg(
            "Workspace is orphaned: its directory is gone. Use rabi show to inspect its record.",
        ));
    }
    if crate::output::json() {
        if path_only {
            emit_json(&serde_json::json!({"path": tree.path}))
        } else {
            emit_json(&worktree::WorktreeRefOutput {
                version: crate::error::JSON_VERSION,
                worktree: tree.to_json(None),
            })
        }
    } else {
        if path_only {
            outln!("{}", tree.path);
        } else {
            let object = tree.to_json(None);
            outln!("{} ({})", visible_text(&tree.name), object.kind);
            outln!("Path: {}", visible_text(&tree.path));
            outln!("Repository: {}", visible_text(&object.repo.name));
            outln!("Repository path: {}", visible_text(&object.repo.root));
            outln!(
                "ID: {}",
                visible_text(object.metadata.id.as_deref().unwrap_or("unknown"))
            );
            outln!(
                "HEAD: {}",
                visible_text(object.head.as_deref().unwrap_or("unknown"))
            );
            outln!("Managed: {}", object.managed);
            if let Some(setup) = &object.metadata.setup {
                outln!("Setup: {}", setup.state.as_str());
                if let Some(error) = &setup.error {
                    outln!("Setup error: {}", visible_text(error));
                }
            }
            outln!(
                "Detached: {}",
                object
                    .detached
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
            for (label, value) in [
                ("Created from", &object.metadata.created_from),
                ("Created at", &object.metadata.created_at),
                ("Updated at", &object.metadata.updated_at),
            ] {
                outln!(
                    "{label}: {}",
                    visible_text(value.as_deref().unwrap_or("unknown"))
                );
            }
            outln!(
                "Branch: {}",
                visible_text(object.branch.as_deref().unwrap_or("(detached)"))
            );
            outln!(
                "Base: {}",
                visible_text(object.base.as_deref().unwrap_or("unknown"))
            );
            outln!(
                "Purpose: {}",
                visible_text(tree.metadata.purpose.as_deref().unwrap_or(""))
            );
            outln!(
                "Dirty: {}",
                object
                    .dirty
                    .map(|dirty| dirty.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
            outln!("Current: {}", object.current);
            outln!("Orphaned: {}", object.orphaned);
            if let Some(upstream) = object.upstream {
                outln!(
                    "Upstream: {}{}",
                    visible_text(&upstream.r#ref),
                    if upstream.gone {
                        " (gone)".into()
                    } else {
                        format!(
                            " ↑{} ↓{}",
                            upstream
                                .ahead
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".into()),
                            upstream
                                .behind
                                .map(|n| n.to_string())
                                .unwrap_or_else(|| "?".into())
                        )
                    }
                );
            }
            for (kind, link) in [
                ("PR", tree.metadata.links.pr),
                ("Issue", tree.metadata.links.issue),
            ] {
                if let Some(link) = link {
                    outln!(
                        "{kind}: {}",
                        visible_text(&link.url.unwrap_or_else(|| {
                            link.number.map(|n| n.to_string()).unwrap_or_default()
                        }))
                    );
                }
            }
        }
        Ok(SUCCESS)
    }
}
fn run_root(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("run"), true)?;
    Err(Error::validation(format!(
        "Unknown command 'run {}'",
        parsed.positionals.join(" ")
    )))
}

fn run_start(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("run start"), true)?;
    let [task] = parsed.positionals.as_slice() else {
        return Err(Error::validation(
            "Usage: rabi run start TASK --name NAME --base REF [options]",
        ));
    };
    let name = parsed
        .value("name")
        .ok_or_else(|| Error::validation("--name is required"))?;
    let base = parsed
        .value("base")
        .ok_or_else(|| Error::validation("--base is required"))?;
    let task = std::fs::read_to_string(task)
        .map_err(|error| Error::msg(format!("Failed to read task: {error}")))?;
    let context = command_context(&parsed)?;
    let mut inspection = crate::workflow::find(
        &context.source_checkout,
        parsed.value("workflow").unwrap_or("default"),
    )?
    .inspect();
    let provider = parsed
        .value("provider")
        .map(|value| match value {
            "codex" => Ok(crate::workflow::Provider::Codex),
            "cursor" => Ok(crate::workflow::Provider::Cursor),
            "claude" => Ok(crate::workflow::Provider::Claude),
            _ => Err(Error::validation(
                "--provider must be codex, cursor, or claude",
            )),
        })
        .transpose()?;
    if provider.is_some() || parsed.value("model").is_some() {
        inspection.override_agents(provider, parsed.value("model"))?;
    }
    if !inspection.valid() {
        print_workflow_diagnostics(&inspection);
        return Err(Error::validation(format!(
            "Workflow '{}' is invalid: {}",
            inspection.entry.name,
            inspection
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    let checks = parsed.values("check");
    let record = crate::executor::execute(
        &context,
        crate::executor::Request {
            workflow: &inspection,
            task: &task,
            checks: &checks,
            base,
            name,
        },
    )?;
    render_run(&context.git_root, &record)?;
    if record.state == crate::run::State::Stopped {
        let message = record
            .stop_reason
            .as_ref()
            .map(|reason| reason.message.as_str())
            .unwrap_or("Run stopped");
        return Err(Error::status(PARTIAL, message));
    }
    Ok(SUCCESS)
}

fn run_show(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("run show"), true)?;
    let [id] = parsed.positionals.as_slice() else {
        return Err(Error::validation("Usage: rabi run show RUN_ID"));
    };
    let context = command_context(&parsed)?;
    let (record, _) = crate::run::load(&context.git_root, id)?;
    render_run(&context.git_root, &record)?;
    Ok(SUCCESS)
}

fn render_run(root: &Path, record: &crate::run::Record) -> Result<()> {
    let directory = crate::run::directory(root, &record.id)?;
    if crate::output::json() {
        let mut value =
            serde_json::to_value(record).map_err(|error| Error::msg(error.to_string()))?;
        value["directory"] = serde_json::json!(directory);
        crate::output::data(&serde_json::json!({"run":value}))?;
    } else {
        outln!("Run: {}", record.id);
        outln!(
            "State: {}",
            match record.state {
                crate::run::State::Running => "running",
                crate::run::State::Succeeded => "succeeded",
                crate::run::State::Stopped => "stopped",
            }
        );
        outln!(
            "Workflow: {} (version {})",
            visible_text(&record.workflow.name),
            record.workflow.version
        );
        outln!(
            "Step: {} ({} started)",
            visible_text(record.current_step.as_deref().unwrap_or("finished")),
            record.steps_started
        );
        for (name, counter) in &record.loops {
            outln!(
                "Loop {}: {}/{}",
                visible_text(name),
                counter.used,
                counter.max
            );
        }
        outln!(
            "Workspace: {} ({})",
            visible_text(&record.workspace.name),
            visible_text(&record.workspace.path)
        );
        if let Some(pr) = &record.pr {
            outln!(
                "PR: {}",
                visible_text(pr.url.as_deref().unwrap_or("unknown URL"))
            );
        }
        if let Some(reason) = &record.stop_reason {
            let mut lines = reason.message.split('\n');
            outln!(
                "Stopped ({}): {}",
                visible_text(&reason.kind),
                visible_text(lines.next().unwrap_or_default())
            );
            for line in lines {
                outln!("  {}", visible_text(line));
            }
        }
        outln!("Records: {}", visible_text(&directory.to_string_lossy()));
    }
    Ok(())
}

fn workflow_root(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("workflow"), true)?;
    Err(Error::validation(format!(
        "Unknown command 'workflow {}'",
        parsed.positionals.join(" ")
    )))
}

fn workflow_list(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("workflow list"), false)?;
    let context = command_context(&parsed)?;
    let workflows = crate::workflow::list(&context.source_checkout)?;
    if crate::output::json() {
        crate::output::data(&serde_json::json!({"workflows":workflows}))?;
    } else {
        for workflow in workflows {
            outln!(
                "{}\t{}\t{}",
                visible_text(&workflow.name),
                workflow.source.as_str(),
                workflow
                    .path
                    .as_deref()
                    .map(|path| visible_text(&path.to_string_lossy()))
                    .unwrap_or_default()
            );
        }
    }
    Ok(SUCCESS)
}

fn workflow_show(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("workflow show"), true)?;
    let [name] = parsed.positionals.as_slice() else {
        return Err(Error::validation(
            "Usage: rabi workflow show NAME [options]",
        ));
    };
    let context = command_context(&parsed)?;
    let inspection = crate::workflow::find(&context.source_checkout, name)?.inspect();
    if !inspection.valid() {
        if !crate::output::json() {
            print_workflow_diagnostics(&inspection);
        }
        let mut error = Error::validation(format!("Workflow '{}' is invalid", visible_text(name)))
            .to_json(VALIDATION)
            .error;
        error.details = Some(serde_json::json!({"diagnostics":inspection.diagnostics}));
        crate::output::failure(error);
        return Ok(VALIDATION);
    }
    let definition = inspection
        .definition
        .as_ref()
        .ok_or_else(|| Error::validation("Workflow definition is missing"))?;
    let agents = definition.agents();
    if crate::output::json() {
        crate::output::data(&serde_json::json!({"workflow":{
            "name":inspection.entry.name, "version":definition.version, "schemaVersion":definition.schema_version,
            "source":inspection.entry.source, "path":inspection.entry.path, "definition":definition, "agents":agents, "steps":definition.step_summaries()
        }}))?;
    } else {
        outln!(
            "Workflow: {} ({}), version {}",
            visible_text(&inspection.entry.name),
            inspection.entry.source.as_str(),
            definition.version
        );
        for line in inspection.yaml.lines() {
            outln!("{}", visible_text(line));
        }
        for (name, step) in &definition.steps {
            outln!(
                "Step {}: allow_dirty={}",
                visible_text(name),
                step.allow_dirty
            );
        }
        for (name, agent) in agents {
            outln!(
                "Agent {}: {}",
                visible_text(&name),
                serde_json::to_string(&agent).map_err(|error| Error::msg(error.to_string()))?
            );
        }
    }
    Ok(SUCCESS)
}

fn print_workflow_diagnostics(inspection: &crate::workflow::Inspection) {
    for diagnostic in &inspection.diagnostics {
        errln!(
            "{}{}: {}",
            visible_text(&inspection.entry.name),
            diagnostic
                .step
                .as_deref()
                .map(|step| format!(" ({})", visible_text(step)))
                .unwrap_or_default(),
            visible_text(&diagnostic.message)
        );
    }
}

fn workflow_check(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("workflow check"), true)?;
    if parsed.positionals.len() > 1 {
        return Err(Error::validation(
            "Usage: rabi workflow check [NAME] [options]",
        ));
    }
    let context = command_context(&parsed)?;
    let entries = match parsed.positionals.first() {
        Some(name) => vec![crate::workflow::find(&context.source_checkout, name)?],
        None => crate::workflow::list(&context.source_checkout)?,
    };
    let inspections: Vec<_> = entries.into_iter().map(|entry| entry.inspect()).collect();
    let valid = inspections.iter().all(crate::workflow::Inspection::valid);
    if crate::output::json() {
        let items: Vec<_> = inspections
            .iter()
            .map(crate::workflow::Inspection::check_json)
            .collect();
        crate::output::data(&serde_json::json!({"items":items}))?;
    } else {
        for inspection in &inspections {
            outln!(
                "{}: {}",
                visible_text(&inspection.entry.name),
                if inspection.valid() {
                    "valid"
                } else {
                    "invalid"
                }
            );
            print_workflow_diagnostics(inspection);
        }
    }
    if valid {
        Ok(SUCCESS)
    } else {
        Err(Error::validation("Workflow validation failed"))
    }
}

fn show(args: &[String]) -> Result<i32> {
    inspect(args, false)
}
fn path_cmd(args: &[String]) -> Result<i32> {
    inspect(args, true)
}

fn remove(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = args::parse(args, &command_options("remove"), true)?;
    if parsed.positionals.is_empty() {
        return Err(Error::validation(
            "Use `rabi remove SEL...`; use `rabi remove .` for the current workspace.",
        ));
    }
    let dry_run = parsed.flag("dry-run");
    // Even selector resolution lists status for other workspaces. Suppress Git
    // hooks before any inspection so a plan cannot run fsmonitor commands.
    let _hooks = dry_run.then(crate::process::suppress_git_hooks);
    let context = if let Some(source) = single_value(&parsed, "source-checkout")? {
        crate::context::open_for_hooks(selected_repository(&parsed)?.as_deref(), Path::new(source))?
    } else {
        command_context(&parsed)?
    };
    metadata::load_strict(&context.git_root)?;
    let options = worktree::RemoveOptions {
        discard_changes: parsed.flag("discard-changes"),
        keep_branch: parsed.flag("keep-branch"),
        delete_unpreserved_branch: parsed.flag("delete-unpreserved-branch"),
    };
    let mut items = Vec::new();
    let mut plans = Vec::new();
    let mut retry_targets = Vec::new();
    let mut first_error = None;
    let mut seen_paths = std::collections::HashSet::new();
    let mut seen_failures = std::collections::HashSet::new();
    // Resolve and check every selector before the first hook or mutation.
    for name in &parsed.positionals {
        match worktree::preflight_remove(&context, name, &options, None) {
            Ok(plan) => {
                if !seen_paths.insert(plan.worktree.path.clone()) {
                    continue;
                }
                items.push(serde_json::json!({
                    "workspace": plan.worktree.to_json(None), "removed": false,
                    "branchDeleted": false, "branchKeptReason": plan.branch_kept_reason,
                    "preservationReason": plan.preservation_reason,
                    "alreadyGone": plan.worktree.orphaned, "error": null,
                    "wouldRemove": true,
                    "wouldDeleteBranch": !plan.worktree.detached && !plan.worktree.orphaned && plan.branch_kept_reason.is_none(),
                }));
                let index = items.len() - 1;
                // Preserve the record's spelling before removal: a symlink
                // alias cannot be matched to its canonical path once gone.
                // Unrecorded targets still use their resolved absolute path.
                let retry_path =
                    metadata::matching_path(&context.git_root, Path::new(&plan.worktree.path))?
                        .unwrap_or_else(|| plan.worktree.path.clone());
                retry_targets.push((index, retry_path));
                plans.push((index, plan));
            }
            Err(mut failure) => {
                if matches!(failure.error, Error::WorktreeNotFound(_)) {
                    match crate::selectors::Selector::parse(name)?.unregistered_record(&context) {
                        Ok(Some(tree)) => {
                            if seen_paths.insert(tree.path.clone()) {
                                // A record proves existence, but supplies no
                                // observed branch or working-directory status.
                                let workspace = worktree::WorktreeJson {
                                    branch: None,
                                    dirty: None,
                                    detached: None,
                                    upstream: None,
                                    ..tree.to_json(None)
                                };
                                items.push(serde_json::json!({
                                    "workspace": workspace, "removed": false,
                                    "branchDeleted": false, "branchKeptReason": null,
                                    "alreadyGone": true, "error": null,
                                }));
                            }
                            continue;
                        }
                        Ok(None) => {}
                        Err(error) => failure.error = error,
                    }
                }
                if !seen_failures.insert(name.clone()) {
                    continue;
                }
                let missing = matches!(failure.error, Error::WorktreeNotFound(_));
                items.push(serde_json::json!({
                    "workspace": failure.worktree.as_ref().map(|tree| serde_json::to_value(tree.to_json(None))).transpose().map_err(|error| Error::msg(error.to_string()))?
                        .unwrap_or_else(|| serde_json::json!({"name": name})),
                    "removed": false, "branchDeleted": false, "branchKeptReason": null,
                    "alreadyGone": false, "error": failure.error.to_json(failure.error.exit_code()).error,
                }));
                if !json {
                    crate::output::failure(failure.error.to_json(failure.error.exit_code()).error);
                }
                // A typo must be visible even when another target also fails a
                // safety guard. All failures remain in the per-item results.
                if first_error.is_none() || missing {
                    first_error = Some(failure.error);
                }
            }
        }
    }
    if let Some(error) = first_error {
        emit_json(&serde_json::json!({"items": items, "plan": dry_run}))?;
        return fail_with(&error, error.exit_code());
    }
    if !json {
        for item in &items {
            if item["alreadyGone"] == true && item.get("wouldRemove").is_none() {
                outln!(
                    "Workspace '{}' is already gone",
                    visible_text(item["workspace"]["name"].as_str().unwrap_or("workspace"))
                );
            }
        }
    }
    if dry_run {
        if !json {
            for item in &items {
                if item["alreadyGone"] == true && item.get("wouldRemove").is_none() {
                    continue;
                }
                outln!(
                    "would remove {}{}{}",
                    visible_text(item["workspace"]["name"].as_str().unwrap_or("workspace")),
                    if item["wouldDeleteBranch"] == true {
                        " and its branch"
                    } else {
                        " (branch kept or no checkout)"
                    },
                    if item["preservationReason"] == "merged_pr" {
                        " (preserved by merged PR)"
                    } else {
                        ""
                    }
                );
            }
        }
        emit_json(&serde_json::json!({"items": items, "plan": true}))?;
        return Ok(SUCCESS);
    }
    let logger = logger_for(json);
    let mut changed = false;
    let mut first_error = None;
    let mut retry_metadata = std::collections::HashSet::new();
    if remove_uses_source_hooks(&context, &parsed) {
        // Keep the hook configuration available until every other target is
        // done, including across retries. Item reporting retains input order.
        plans.sort_by_key(|(_, plan)| {
            crate::paths::same_location(Path::new(&plan.worktree.path), &context.source_checkout)
        });
    }
    for (index, plan) in plans {
        match worktree::commit_remove(
            &context,
            plan,
            &options,
            &logger,
            hook_control(&parsed, json, true),
        ) {
            Ok(outcome) => {
                changed = true;
                if outcome.metadata_error.is_some() {
                    retry_metadata.insert(index);
                }
                // A failed branch deletion leaves a usable branch after the
                // workspace is retired. Report retention and continue; only
                // unfinished checkout/record work belongs in a remove retry.
                let branch_kept_reason = if let Some(detail) = &outcome.branch_error {
                    crate::output::warning(
                        "branch_kept_delete_failed",
                        format!(
                            "Warning: kept branch '{}' because deletion failed: {}",
                            visible_text(&outcome.worktree.branch),
                            visible_text(detail)
                        ),
                    );
                    Some("delete_failed".to_string())
                } else {
                    outcome.branch_kept_reason.clone()
                };
                let error = outcome.metadata_error.map(Error::msg);
                if outcome.branch_kept_reason.as_deref() == Some("unpreserved") {
                    crate::output::warning(
                        "branch_kept_unpreserved",
                        format!(
                            "Warning: kept branch '{}' because its commits are unpreserved.",
                            visible_text(&outcome.worktree.branch)
                        ),
                    );
                }
                items[index] = serde_json::json!({
                    "workspace": outcome.worktree.to_json(None), "removed": !outcome.worktree.orphaned,
                    "branchDeleted": outcome.branch_deleted, "branchKeptReason": branch_kept_reason,
                    "preservationReason": outcome.preservation_reason,
                    "detail": outcome.branch_error,
                    "alreadyGone": outcome.worktree.orphaned,
                    "error": error.as_ref().map(|error| error.to_json(error.exit_code()).error),
                });
                if !json {
                    outln!("{}", outcome.message);
                }
                if let Some(error) = error {
                    first_error = Some(error);
                    break;
                }
            }
            Err(failure) => {
                changed |= failure.changed;
                items[index]["error"] =
                    serde_json::json!(failure.error.to_json(failure.error.exit_code()).error);
                first_error = Some(failure.error);
                break;
            }
        }
    }
    if first_error.is_some() {
        for item in &mut items {
            if item["wouldRemove"] == true && item["error"].is_null() {
                item["error"] = serde_json::json!({"code": "not_attempted", "message": "Removal stopped after an earlier failure; retry only the unfinished targets."});
            }
        }
    }
    emit_json(&serde_json::json!({"items": items}))?;
    if let Some(error) = first_error {
        let code = if changed { PARTIAL } else { error.exit_code() };
        let mut body = error.aggregate_json(code);
        if code == PARTIAL {
            let targets = retry_targets
                .into_iter()
                .filter_map(|(index, path)| {
                    (!items[index]["error"].is_null()
                        && (items[index]["removed"] != true || retry_metadata.contains(&index)))
                    .then_some(path)
                })
                .collect::<Vec<_>>();
            let retry = remove_retry_command(&context, &parsed, json, &targets);
            body.message = format!(
                "{}. Retry with `{retry}`.",
                crate::output::diagnostic_value(&body.message)
            );
            let details = body.details.get_or_insert_with(|| serde_json::json!({}));
            details["retryCommand"] = serde_json::json!(retry);
        }
        crate::output::failure(body);
        return Ok(code);
    }
    Ok(SUCCESS)
}

fn remove_uses_source_hooks(context: &crate::context::Context, parsed: &Parsed) -> bool {
    !parsed.flag("no-hooks")
        && context.source_checkout != context.git_root
        && !crate::config::pre_remove_commands(context.config.as_ref()).is_empty()
}

fn remove_retry_command(
    context: &crate::context::Context,
    parsed: &Parsed,
    json: bool,
    targets: &[String],
) -> String {
    let mut command = format!(
        "cd {} && rabi -R {} remove",
        crate::allocation::shell_quote(&context.git_root),
        crate::allocation::shell_quote(&context.git_root)
    );
    let source_exists = context.source_checkout.is_dir();
    if source_exists && remove_uses_source_hooks(context, parsed) {
        command.push_str(&format!(
            " --source-checkout {}",
            crate::allocation::shell_quote(&context.source_checkout)
        ));
    }
    for flag in [
        "discard-changes",
        "delete-unpreserved-branch",
        "keep-branch",
    ] {
        if parsed.flag(flag) {
            command.push_str(&format!(" --{flag}"));
        }
    }
    // Selecting primary reloads its configuration. Preserve hook non-execution.
    // A retired hook source was removed last, leaving only metadata cleanup.
    if !source_exists
        || parsed.flag("no-hooks")
        || crate::config::pre_remove_commands(context.config.as_ref()).is_empty()
    {
        command.push_str(" --no-hooks");
    }
    if json {
        command.push_str(" --json");
    } else {
        command.push_str(" --output text");
    }
    if crate::runtime::non_interactive() {
        command.push_str(" --no-input");
    }
    command.push_str(" --");
    for target in targets {
        command.push(' ');
        command.push_str(&crate::allocation::shell_quote(target));
    }
    command
}

fn exec(args: &[String]) -> Result<i32> {
    let Some(separator) = args.iter().position(|arg| arg == "--") else {
        // Validate rabi options before the workspace selector.
        args::parse_with(args, &command_options("exec"), true, Some(1))?;
        return Err(Error::validation(
            "Usage: rabi exec [SEL] -- COMMAND [ARG...]",
        ));
    };
    let parsed = args::parse_with(&args[..separator], &command_options("exec"), true, Some(1))?;
    let command_args = &args[separator + 1..];
    if parsed.positionals.len() > 1 || command_args.is_empty() {
        return Err(Error::validation(
            "Usage: rabi exec [SEL] -- COMMAND [ARG...]",
        ));
    }
    let context = command_context(&parsed)?;
    let name = parsed
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or(".");
    let rest = command_args[1..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    match worktree::exec_in_worktree(&context, name, &command_args[0], &rest, &[], false) {
        Ok(0) => Ok(SUCCESS),
        Ok(code) => fail(
            &Error::CommandFailed {
                command: command_args[0].clone(),
                code,
            },
            code,
        ),
        Err(error) => fail(&error, error.child_exit_code()),
    }
}

fn config_root(args: &[String]) -> Result<i32> {
    Err(Error::validation(format!(
        "Unknown config subcommand: {}",
        args.join(" ")
    )))
}

fn config_get(args: &[String]) -> Result<i32> {
    config_read(args, false)
}

fn config_list(args: &[String]) -> Result<i32> {
    config_read(args, true)
}

fn config_read(args: &[String], list: bool) -> Result<i32> {
    let key = if list { "config list" } else { "config get" };
    let parsed = args::parse(args, &command_options(key), true)?;
    selected_repository(&parsed)?;
    if parsed.positionals.len() != usize::from(!list) {
        return Err(Error::validation(if list {
            "Usage: rabi config list [--sources]"
        } else {
            "Usage: rabi config get KEY [--sources]"
        }));
    }
    let keys = if list {
        preferences::SUPPORTED_KEYS.to_vec()
    } else {
        preferences::require_key(&parsed.positionals[0])?;
        vec![parsed.positionals[0].as_str()]
    };
    let loaded = preferences::load_preferences()?;
    let mut values = serde_json::Map::new();
    for key in keys {
        let effective = preferences::effective_value(&loaded, key)?;
        if crate::output::json() {
            values.insert(
                key.into(),
                if parsed.flag("sources") {
                    serde_json::to_value(effective)
                        .map_err(|error| Error::msg(error.to_string()))?
                } else {
                    effective.value
                },
            );
        } else {
            let value = match &effective.value {
                serde_json::Value::String(value) => value.clone(),
                value => value.to_string(),
            };
            let label = if list {
                format!("{key} = ")
            } else {
                String::new()
            };
            if parsed.flag("sources") {
                outln!(
                    "{label}{}\t{} ({})",
                    visible_text(&value),
                    effective.source,
                    visible_text(&effective.detail)
                );
            } else {
                outln!("{label}{}", visible_text(&value));
            }
        }
    }
    if crate::output::json() {
        emit_json(&serde_json::json!({"config": values}))
    } else {
        Ok(SUCCESS)
    }
}

fn config_set(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("config set"), true)?;
    selected_repository(&parsed)?;
    if parsed.positionals.len() != 2 {
        return Err(Error::validation("Usage: rabi config set KEY VALUE"));
    }
    let key = &parsed.positionals[0];
    let value = &parsed.positionals[1];
    preferences::require_key(key)?;
    if value.is_empty() {
        return Err(Error::validation(format!(
            "Config key '{key}' requires a value"
        )));
    }
    if let Some(message) = preferences::validate_preference_value(key, value) {
        return Err(Error::validation(message));
    }
    let path = preferences::set_preference(key, value)?;
    if crate::output::json() {
        let value = if matches!(key.as_str(), "sync.rewrite-published" | "discover-ghq") {
            serde_json::json!(value == "true")
        } else {
            serde_json::json!(value)
        };
        emit_json(&serde_json::json!({"key": key, "value": value}))
    } else {
        outln!(
            "Set {key} to '{}' in {}",
            visible_text(value),
            visible_text(&path.to_string_lossy())
        );
        Ok(SUCCESS)
    }
}

fn config_unset(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("config unset"), true)?;
    selected_repository(&parsed)?;
    if parsed.positionals.len() != 1 {
        return Err(Error::validation("Usage: rabi config unset KEY"));
    }
    let key = &parsed.positionals[0];
    preferences::require_key(key)?;
    let path = preferences::remove_preference(key)?;
    if crate::output::json() {
        emit_json(&serde_json::json!({"key": key, "unset": true}))
    } else {
        outln!("Unset {key} in {}", visible_text(&path.to_string_lossy()));
        Ok(SUCCESS)
    }
}

fn version(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("--version"), false)?;
    selected_repository(&parsed)?;
    if crate::output::json() {
        // `version` is application data here, distinct from the envelope schema.
        crate::output::app_version(crate::version_line())
    } else {
        outln!("{}", crate::version_line());
        Ok(SUCCESS)
    }
}

fn help_cmd(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("help"), true)?;
    selected_repository(&parsed)?;
    let key = parsed.positionals.join(" ");
    if !key.is_empty() && help::command_help(&key)?.is_none() {
        return Err(Error::validation(format!("Unknown command '{key}'")));
    }
    if crate::output::json() {
        emit_json(&help::json_help(&key)?)
    } else {
        outln!(
            "{}",
            if key.is_empty() {
                help::main_help()?
            } else {
                help::command_help(&key)?.unwrap_or_default()
            }
        );
        Ok(SUCCESS)
    }
}

fn clone_cmd(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("clone"), true) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if parsed.positionals.len() != 1 {
        return fail_with(&Error::validation(
                "Usage: rabi clone <url|host/owner/repo|owner/repo> [--branch <branch>] [--depth <n>] [--update] [--json]",
            ), VALIDATION);
    }
    selected_repository(&parsed)?;
    match repository::clone_repository(&repository::CloneRequest {
        source: &parsed.positionals[0],
        branch: parsed.value("branch"),
        depth: parsed.value("depth"),
        update: parsed.flag("update"),
    }) {
        Ok(outcome) => {
            let result = outcome.result;
            if json {
                emit_json(&serde_json::json!({
                    "repo": {"root": result.path, "name": format!("{}/{}/{}", result.host, result.owner, result.repo), "remoteUrl": result.remote_url},
                    "cloned": result.cloned, "updated": result.updated,
                }))?;
            } else {
                outln!("{}", repository::human_clone_message(&result));
            }
            if let Some(error) = outcome.registration_error {
                let status = if result.cloned || result.updated {
                    PARTIAL
                } else {
                    GENERAL
                };
                return fail_with(&Error::status(status, error.to_string()), status);
            }
            Ok(SUCCESS)
        }
        Err(error) => fail_with(&error, error.exit_code()),
    }
}

fn project_root(args: &[String]) -> Result<i32> {
    if args.is_empty() {
        if let Some(text) = help::command_help("repo")? {
            outln!("{text}");
        }
        return Ok(SUCCESS);
    }
    Err(Error::validation(format!(
        "Unknown repo subcommand: {}",
        args[0]
    )))
}

fn project_add(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("repo add"), true) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, VALIDATION),
    };
    if parsed.positionals.len() > 1 {
        return fail_with(
            &Error::msg("Usage: rabi repo add [path] [--json]"),
            VALIDATION,
        );
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => return bubble(Error::msg(error.to_string()), json),
    };
    let selected = selected_repository(&parsed)?;
    let input = parsed
        .positionals
        .first()
        .map(Path::new)
        .unwrap_or(selected.as_deref().unwrap_or(cwd.as_path()));
    let root = match projects::resolve_project_root(input) {
        Ok(root) => root,
        Err(error) => return bubble(error, json),
    };
    let store = match ProjectRegistryStore::default_store() {
        Ok(store) => store,
        Err(error) => return bubble(error, json),
    };
    let (project, added) = match store.add(&root) {
        Ok(added) => added,
        Err(error) => return bubble(error, json),
    };
    if json {
        return emit_json(&project_add_json(added, &project));
    } else if added {
        outln!(
            "Added repo '{}' ({})",
            visible_text(&project.name),
            visible_text(&project.root_path)
        );
    } else {
        outln!(
            "Repository '{}' is already registered ({})",
            visible_text(&project.name),
            visible_text(&project.root_path)
        );
    }
    Ok(SUCCESS)
}

fn project_list(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("repo list"), false) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, VALIDATION),
    };
    let modes = [json, parsed.flag("names"), parsed.flag("paths")]
        .iter()
        .filter(|enabled| **enabled)
        .count();
    if modes > 1 {
        return fail_with(
            &Error::msg("Only one of --json, --names, or --paths can be specified"),
            VALIDATION,
        );
    }
    let loaded = match preferences::load_preferences() {
        Ok(preferences) => preferences,
        Err(error) => return fail_with(&error, GENERAL),
    };
    if parsed.flag("registered") && parsed.flag("discovered") {
        return Err(Error::validation(
            "Use only one of --registered or --discovered",
        ));
    }
    selected_repository(&parsed)?;
    let mut catalog = match projects::list_catalog(
        !parsed.flag("registered") && loaded.ghq_discovery != Some(false),
    ) {
        Ok(catalog) => catalog,
        Err(error) => return bubble(error, json),
    };
    if parsed.flag("discovered") {
        catalog
            .projects
            .retain(|entry| matches!(entry, ProjectListEntry::Ghq { .. }));
    }
    for warning in &catalog.warnings {
        crate::output::warning("repo_discovery", format!("Warning: {warning}"));
    }
    if json {
        return emit_json(&catalog_json(&catalog.projects, catalog.version));
    }
    if catalog.projects.is_empty() {
        if !parsed.flag("names") && !parsed.flag("paths") {
            outln!("No repos found.");
        }
        return Ok(SUCCESS);
    }
    for project in &catalog.projects {
        match project {
            ProjectListEntry::Registry {
                id,
                name,
                root_path,
                ..
            } if parsed.flag("names") => outln!("{}", visible_text(name)),
            ProjectListEntry::Ghq { name, .. } if parsed.flag("names") => {
                outln!("{}", visible_text(name))
            }
            ProjectListEntry::Registry { root_path, .. }
            | ProjectListEntry::Ghq { root_path, .. }
                if parsed.flag("paths") =>
            {
                outln!("{}", visible_text(root_path));
            }
            ProjectListEntry::Ghq {
                name, root_path, ..
            } => {
                outln!("{} ({}) [ghq]", visible_text(name), visible_text(root_path))
            }
            ProjectListEntry::Registry {
                id,
                name,
                root_path,
                ..
            } => outln!(
                "{} ({}) [{}]",
                visible_text(name),
                visible_text(root_path),
                visible_text(id)
            ),
        }
    }
    Ok(SUCCESS)
}

fn project_remove(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("repo forget"), true) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if parsed.positionals.len() != 1 {
        return fail_with(
            &Error::msg("Usage: rabi repo remove <id|name|path> [--json]"),
            VALIDATION,
        );
    }
    selected_repository(&parsed)?;
    let identifier = &parsed.positionals[0];
    let store = match ProjectRegistryStore::default_store() {
        Ok(store) => store,
        Err(error) => return bubble(error, json),
    };
    let projects = match store.list() {
        Ok(projects) => projects,
        Err(error) => return bubble(error, json),
    };
    let project = match find_project(&projects, identifier) {
        Ok(Some(project)) => project,
        Ok(None) => {
            return fail_with(
                &Error::msg(format!("Repository '{identifier}' not found")),
                NOT_FOUND,
            )
        }
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let removed = match store.remove(&project.id) {
        Ok(Some(removed)) => removed,
        Ok(None) => {
            return fail_with(
                &Error::msg(format!("Repository '{identifier}' not found")),
                NOT_FOUND,
            )
        }
        Err(error) => return bubble(error, json),
    };
    if json {
        return emit_json(&serde_json::json!({
            "status": "removed",
            "repo": removed,
        }));
    } else {
        outln!(
            "Removed repo '{}' ({})",
            visible_text(&removed.name),
            visible_text(&removed.root_path)
        );
    }
    Ok(SUCCESS)
}

fn repo_show(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("repo show"), true)?;
    if parsed.positionals.len() > 1 {
        return Err(Error::validation("Usage: rabi repo show [REF]"));
    }
    let selected = selected_repository(&parsed)?;
    let root = match parsed.positionals.first() {
        Some(reference) => projects::resolve_project_ref(reference)?,
        None => crate::context::open(selected.as_deref())?.git_root,
    };
    let context = crate::context::create_at(&root)?;
    let name = repository::canonical_name_from_origin(&root).unwrap_or_else(|| {
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let default_branch = crate::git::default_branch(&root)?;
    let trusted = trust::trust_all() || trust::is_trusted_root(&root)?;
    let value = serde_json::json!({"repo": {
        "root": root, "name": name, "defaultBranch": default_branch,
        "configPresent": root.join(".rabi.toml").is_file(), "trusted": trusted,
        "workspaceRoot": context.worktrees_directory,
    }});
    if crate::output::json() {
        emit_json(&value)
    } else {
        outln!("{}\nPrimary checkout: {}\nDefault branch: {}\n.rabi.toml: {}\nTrusted: {trusted}\nWorkspace root: {}", visible_text(&name), visible_text(&root.to_string_lossy()), visible_text(default_branch.as_deref().unwrap_or("unknown")), root.join(".rabi.toml").is_file(), visible_text(&context.worktrees_directory.to_string_lossy()));
        Ok(SUCCESS)
    }
}

fn find_project(projects: &[ProjectRecord], identifier: &str) -> Result<Option<ProjectRecord>> {
    if let Some(project) = projects.iter().find(|project| project.id == identifier) {
        return Ok(Some(project.clone()));
    }
    if let Some(project) = projects
        .iter()
        .find(|project| project.root_path == identifier)
    {
        return Ok(Some(project.clone()));
    }
    let name_matches = projects
        .iter()
        .filter(|project| project.name == identifier)
        .collect::<Vec<_>>();
    if name_matches.len() == 1 {
        return Ok(Some(name_matches[0].clone()));
    }
    if name_matches.len() > 1 {
        return Err(Error::Status {
            code: "ambiguous",
            message: format!(
                "Repository '{}' is ambiguous; use its id or path",
                crate::output::diagnostic_value(identifier)
            ),
        });
    }
    match projects::resolve_project_ref(identifier) {
        Ok(root) => {
            let root = root.to_string_lossy().to_string();
            Ok(projects
                .iter()
                .find(|project| project.root_path == root)
                .cloned())
        }
        Err(Error::ProjectNotFound(_) | Error::PathNotFound(_) | Error::NotARepository) => Ok(None),
        Err(error) => Err(error),
    }
}

fn project_add_json(added: bool, project: &ProjectRecord) -> serde_json::Value {
    serde_json::json!({
        "status": if added { "added" } else { "existing" },
        "repo": project,
    })
}

fn catalog_json(projects: &[ProjectListEntry], version: u32) -> serde_json::Value {
    serde_json::json!({
        "version": version,
        "repos": projects,
    })
}

fn set(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("set"), true) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if parsed.positionals.len() > 1 {
        return fail_with(&Error::msg("Usage: rabi set [SEL] [options]"), VALIDATION);
    }
    let mut input = match metadata_input(&parsed) {
        Ok(input) => input,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if parsed.value("base").is_none()
        && input.purpose.is_none()
        && input.pr.is_none()
        && input.issue.is_none()
        && parsed.values("unset").is_empty()
    {
        return fail_with(
            &Error::msg("Specify at least one of --purpose, --pr, --issue, --base or --unset"),
            VALIDATION,
        );
    }
    let unset = parsed.values("unset");
    for field in &unset {
        if !matches!(field.as_str(), "purpose" | "base" | "pr" | "issue") {
            return Err(Error::validation(format!(
                "Invalid --unset field '{field}'; use purpose, base, pr or issue"
            )));
        }
        if parsed.value(field).is_some() {
            return Err(Error::validation(format!(
                "Cannot set and unset '{field}' together"
            )));
        }
    }
    let context = match command_context(&parsed) {
        Ok(context) => context,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let base = match single_value(&parsed, "base") {
        Ok(base) => base,
        Err(error) => return fail_with(&error, VALIDATION),
    };
    if let Some(base) = base {
        input.base = match crate::sync::normalize_base(&context.git_root, base) {
            Ok(base) => Some(base),
            Err(error) => return fail_with(&error, VALIDATION),
        };
    }
    let name = parsed
        .positionals
        .first()
        .map(String::as_str)
        .unwrap_or(".");
    let worktree = match crate::selectors::resolve(&context, name, false, None) {
        Ok(worktree) => worktree,
        Err(error @ Error::WorktreeNotFound(_)) => return fail_with(&error, NOT_FOUND),
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let input = match worktree::outside_name_to_store(&context, Path::new(&worktree.path)) {
        Ok(name) => metadata::MetadataInput { name, ..input },
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if let Err(error) =
        metadata::patch(&context.git_root, Path::new(&worktree.path), &input, &unset)
    {
        return fail_with(&error, GENERAL);
    }
    let worktree = match worktree::worktree_at_path(&context, Path::new(&worktree.path)) {
        Ok(worktree) => worktree,
        Err(error) => return fail_with(&error, GENERAL),
    };
    if json {
        return emit_json(&worktree::WorktreeRefOutput {
            version: crate::error::JSON_VERSION,
            worktree: worktree.to_json(None),
        });
    }
    outln!("Updated metadata for '{}'", worktree.name);
    Ok(SUCCESS)
}

/// Shared by common-option extraction and command validation.
fn command_options(key: &str) -> Vec<Opt> {
    let mut options = match key {
        "run start" => vec![
            repo_flag(),
            value_flag("name", None),
            value_flag("base", None),
            value_flag("workflow", None),
            value_flag("provider", None),
            value_flag("model", None),
            Opt {
                name: "check",
                short: None,
                value: true,
                multiple: true,
                remainder: false,
            },
        ],
        "sync" => vec![
            repo_flag(),
            flag("all", None),
            flag("main", None),
            flag("auto", None),
            flag("dry-run", None),
            flag("rebase", None),
            flag("merge", None),
            flag("rewrite-published", None),
            flag("keep-conflicts", None),
        ],
        "create" => {
            let mut options = vec![
                repo_flag(),
                value_flag("branch", None),
                flag("detach", None),
                flag("reuse", None),
                Opt {
                    name: "copy-file",
                    short: None,
                    value: true,
                    multiple: true,
                    remainder: false,
                },
                value_flag("base", None),
                flag("no-setup", None),
            ];
            options.extend(metadata_options());
            options
        }
        "setup" => vec![
            repo_flag(),
            value_flag("source-checkout", None),
            Opt {
                name: "copy-file",
                short: None,
                value: true,
                multiple: true,
                remainder: false,
            },
            flag("no-hooks", None),
        ],
        "adopt" => {
            let mut options = vec![
                repo_flag(),
                value_flag("name", None),
                value_flag("base", None),
            ];
            options.extend(metadata_options());
            options
        }
        "forget" => vec![repo_flag()],
        "list" => vec![
            repo_flag(),
            value_flag("kind", None),
            flag("names", None),
            flag("paths", None),
            flag("all-repos", None),
        ],
        "show" | "path" => vec![repo_flag()],
        "remove" => vec![
            repo_flag(),
            value_flag("source-checkout", None),
            flag("discard-changes", None),
            flag("delete-unpreserved-branch", None),
            flag("keep-branch", None),
            flag("no-hooks", None),
            flag("dry-run", None),
        ],
        "exec" => vec![repo_flag()],
        "clone" => vec![
            value_flag("branch", None),
            value_flag("depth", None),
            flag("update", None),
        ],
        "repo add" => vec![],
        "repo list" => vec![
            flag("names", None),
            flag("paths", None),
            flag("registered", None),
            flag("discovered", None),
        ],
        "config get" | "config list" => vec![flag("sources", None)],
        "help" => vec![flag("help", Some('h'))],
        "repo forget" => vec![],
        "set" => {
            let mut options = vec![
                repo_flag(),
                value_flag("base", None),
                Opt {
                    name: "unset",
                    short: None,
                    value: true,
                    multiple: true,
                    remainder: false,
                },
            ];
            options.extend(metadata_options());
            options
        }
        "gc" => vec![
            repo_flag(),
            flag("apply", None),
            flag("merged", None),
            flag("closed-pr", None),
            flag("orphaned", None),
            value_flag("older-than", None),
            flag("keep-branch", None),
            flag("no-hooks", None),
        ],

        _ => vec![],
    };
    if !options.iter().any(|option| option.name == "repo") {
        options.push(repo_flag());
    }
    options
}

fn repo_flag() -> Opt {
    value_flag("repo", Some('R'))
}

fn metadata_options() -> [Opt; 3] {
    [
        value_flag("purpose", None),
        value_flag("pr", None),
        value_flag("issue", None),
    ]
}

fn metadata_input(parsed: &Parsed) -> Result<MetadataInput> {
    Ok(MetadataInput {
        purpose: single_value(parsed, "purpose")?
            .map(metadata::parse_purpose)
            .transpose()?,
        pr: single_value(parsed, "pr")?
            .map(metadata::parse_link)
            .transpose()?,
        issue: single_value(parsed, "issue")?
            .map(metadata::parse_link)
            .transpose()?,
        ..MetadataInput::default()
    })
}

fn single_value<'a>(parsed: &'a Parsed, name: &str) -> Result<Option<&'a str>> {
    if parsed.occurrences(name) > 1 {
        return Err(Error::validation(format!(
            "The --{name} option can only be specified once"
        )));
    }
    match parsed.value(name) {
        Some(value) if value.starts_with('-') => Err(Error::validation(format!(
            "Option '--{name} <value>' received '{value}', which looks like an option"
        ))),
        value => Ok(value),
    }
}

fn selected_repository(parsed: &Parsed) -> Result<Option<std::path::PathBuf>> {
    if parsed.occurrences("repo") > 1 {
        return Err(Error::validation(
            "The --repo option can only be specified once".to_string(),
        ));
    }
    match parsed.value("repo") {
        Some(reference) => projects::resolve_project_ref(reference).map(Some),
        None => Ok(None),
    }
}

fn gc(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("gc"), false) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let older_than = match parsed.value("older-than") {
        Some(value) => match crate::gc::parse_duration(value) {
            Ok(duration) => Some(duration),
            Err(error) => return fail_with(&error, error.exit_code()),
        },
        None => None,
    };
    let context = match command_context(&parsed) {
        Ok(context) => context,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let request = crate::gc::Request {
        policy: crate::gc::Policy {
            selection: crate::gc::selection_from_flags(
                parsed.flag("merged"),
                parsed.flag("closed-pr"),
                parsed.flag("orphaned"),
                older_than,
            ),
            now: std::time::SystemTime::now(),
        },
        apply: parsed.flag("apply"),
        keep_branch: parsed.flag("keep-branch"),
        no_hooks: parsed.flag("no-hooks"),
    };
    let report = match crate::gc::run(&context, &request, &logger_for(json)) {
        Ok(report) => report,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if json {
        emit_json(&report)?;
        return Ok(report.exit_code());
    }
    for line in &report.lines {
        outln!("{line}");
    }
    Ok(report.exit_code())
}

fn command_context(parsed: &Parsed) -> Result<crate::context::Context> {
    let repository = selected_repository(parsed)?;
    crate::context::open(repository.as_deref())
}

fn hook_control(parsed: &Parsed, json: bool, prompt: bool) -> HookControl {
    HookControl {
        no_hooks: parsed.flag("no-hooks"),
        json,
        prompt,
    }
}

fn trust_cmd(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("trust add"), true) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if parsed.positionals.len() > 1 {
        return fail_with(
            &Error::validation("Usage: rabi trust add [REPO]"),
            VALIDATION,
        );
    }
    let start = match trust_start(&parsed) {
        Ok(start) => start,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let project = match trust::load_hooks(&start) {
        Ok(project) => project,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    errln!(
        "{}\n{}",
        visible_text(&project.project),
        trust::format_hooks(&project.setup, &project.pre_remove)
    );
    // Bare repositories have no checkout; preserve their existing trust path
    // without trying to discover checkout-local workflow definitions.
    if let Ok(checkout) = crate::git::top_level(&start) {
        let workflows = crate::workflow::list(&checkout)?;
        errln!("workflows:");
        for workflow in workflows
            .iter()
            .filter(|entry| entry.source == crate::workflow::Source::Repository)
        {
            if let Some(path) = &workflow.path {
                errln!(
                    "  {}  {}",
                    visible_text(&workflow.name),
                    visible_text(&path.to_string_lossy())
                );
            }
        }
    }
    errln!("Workflow agent steps and check commands, including built-in workflows, execute under this repository trust.");
    match trust::trust_root(Path::new(&project.git_root)) {
        Ok(record) => {
            if json {
                emit_json(&serde_json::json!({
                    "version": crate::error::JSON_VERSION,
                    "path": record.path,
                    "trustedAt": record.trusted_at,
                }))
            } else {
                outln!("Trusted {}", visible_text(&record.path));
                Ok(SUCCESS)
            }
        }
        Err(error) => fail_with(&error, error.exit_code()),
    }
}

fn untrust_cmd(args: &[String]) -> Result<i32> {
    let json = crate::output::json();
    let parsed = match args::parse(args, &command_options("trust remove"), true) {
        Ok(parsed) => parsed,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if parsed.positionals.len() > 1 {
        return fail_with(
            &Error::validation("Usage: rabi trust remove [REPO]"),
            VALIDATION,
        );
    }
    let start = match trust_start(&parsed) {
        Ok(start) => start,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let git_root = match crate::git::git_root(&start) {
        Ok(git_root) => git_root,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    let key = match trust::common_dir_key(&git_root) {
        Ok(key) => key.to_string_lossy().to_string(),
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    match trust::untrust_root(&git_root) {
        Ok(removed) => {
            if json {
                emit_json(&serde_json::json!({
                    "version": crate::error::JSON_VERSION,
                    "path": key,
                    "removed": removed,
                }))
            } else if removed {
                outln!("Removed trust for {}", visible_text(&key));
                Ok(SUCCESS)
            } else {
                outln!("Not trusted: {}", visible_text(&key));
                Ok(SUCCESS)
            }
        }
        Err(error) => fail_with(&error, error.exit_code()),
    }
}

fn print_trust_list(json: bool) -> Result<i32> {
    let projects = match trust::list_trusted() {
        Ok(projects) => projects,
        Err(error) => return fail_with(&error, error.exit_code()),
    };
    if json {
        let projects = projects
            .iter()
            .map(|project| {
                serde_json::json!({
                    "path": project.path,
                    "trustedAt": project.trusted_at,
                })
            })
            .collect::<Vec<_>>();
        return emit_json(&serde_json::json!({
            "version": crate::error::JSON_VERSION,
            "repos": projects,
        }));
    }
    if projects.is_empty() {
        outln!("No trusted repos");
        return Ok(SUCCESS);
    }
    for project in projects {
        outln!(
            "{}\t{}",
            visible_text(&project.path),
            visible_text(&project.trusted_at)
        );
    }
    Ok(SUCCESS)
}

fn trust_root_cmd(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("trust"), true)?;
    if parsed.positionals.is_empty() {
        return Err(Error::validation(
            "Missing trust subcommand. Use add, remove, or list.",
        ));
    }
    Err(Error::validation(format!(
        "Unknown command 'trust {}'",
        parsed.positionals.join(" ")
    )))
}

fn trust_list_cmd(args: &[String]) -> Result<i32> {
    let parsed = args::parse(args, &command_options("trust list"), false)?;
    selected_repository(&parsed)?;
    print_trust_list(crate::output::json())
}

fn trust_start(parsed: &Parsed) -> Result<std::path::PathBuf> {
    let selected = selected_repository(parsed)?;
    match parsed.positionals.first() {
        Some(reference) => projects::resolve_project_ref(reference),
        None => match selected {
            Some(root) => Ok(root),
            None => std::env::current_dir().map_err(|error| Error::msg(error.to_string())),
        },
    }
}

fn flag(name: &'static str, short: Option<char>) -> Opt {
    Opt {
        name,
        short,
        value: false,
        multiple: false,
        remainder: false,
    }
}

fn value_flag(name: &'static str, short: Option<char>) -> Opt {
    Opt {
        name,
        short,
        value: true,
        multiple: false,
        remainder: false,
    }
}

fn fail(error: &Error, code: i32) -> Result<i32> {
    fail_with(error, code)
}

fn fail_with(error: &Error, requested: i32) -> Result<i32> {
    let json = crate::output::json();
    let code = if matches!(error, Error::CommandFailed { .. } | Error::Signaled(_)) && !json {
        // Child status transparency belongs only to standalone exec.
        if crate::output::command_is_exec() {
            error.child_exit_code()
        } else {
            error.exit_code()
        }
    } else {
        crate::error::exit_for_code(error.json_code(requested))
    };
    crate::output::failure(error.to_json(requested).error);
    Ok(code)
}

fn emit_json(value: &impl serde::Serialize) -> Result<i32> {
    crate::output::data(value)
}

fn bubble(error: Error, json: bool) -> Result<i32> {
    if json {
        let code = error.exit_code();
        fail_with(&error, code)
    } else {
        Err(error)
    }
}

fn logger_for(json: bool) -> Logger {
    if json {
        Logger::json()
    } else {
        Logger::on()
    }
}

static PREFERENCE_COMMANDS: &[Command] = &[
    Command {
        name: "list",
        help_key: "config list",
        run: config_list,
        children: &[],
    },
    Command {
        name: "get",
        help_key: "config get",
        run: config_get,
        children: &[],
    },
    Command {
        name: "set",
        help_key: "config set",
        run: config_set,
        children: &[],
    },
    Command {
        name: "unset",
        help_key: "config unset",
        run: config_unset,
        children: &[],
    },
];

static PROJECT_COMMANDS: &[Command] = &[
    Command {
        name: "show",
        help_key: "repo show",
        run: repo_show,
        children: &[],
    },
    Command {
        name: "add",
        help_key: "repo add",
        run: project_add,
        children: &[],
    },
    Command {
        name: "list",
        help_key: "repo list",
        run: project_list,
        children: &[],
    },
    Command {
        name: "forget",
        help_key: "repo forget",
        run: project_remove,
        children: &[],
    },
];

static TRUST_COMMANDS: &[Command] = &[
    Command {
        name: "add",
        help_key: "trust add",
        run: trust_cmd,
        children: &[],
    },
    Command {
        name: "remove",
        help_key: "trust remove",
        run: untrust_cmd,
        children: &[],
    },
    Command {
        name: "list",
        help_key: "trust list",
        run: trust_list_cmd,
        children: &[],
    },
];

static WORKFLOW_COMMANDS: &[Command] = &[
    Command {
        name: "list",
        help_key: "workflow list",
        run: workflow_list,
        children: &[],
    },
    Command {
        name: "show",
        help_key: "workflow show",
        run: workflow_show,
        children: &[],
    },
    Command {
        name: "check",
        help_key: "workflow check",
        run: workflow_check,
        children: &[],
    },
];

static RUN_COMMANDS: &[Command] = &[
    Command {
        name: "start",
        help_key: "run start",
        run: run_start,
        children: &[],
    },
    Command {
        name: "show",
        help_key: "run show",
        run: run_show,
        children: &[],
    },
];

static COMMANDS: &[Command] = &[
    Command {
        name: "run",
        help_key: "run",
        run: run_root,
        children: RUN_COMMANDS,
    },
    Command {
        name: "workflow",
        help_key: "workflow",
        run: workflow_root,
        children: WORKFLOW_COMMANDS,
    },
    Command {
        name: "completions",
        help_key: "completions",
        run: completions,
        children: &[],
    },
    Command {
        name: "shell-init",
        help_key: "shell-init",
        run: shell_init,
        children: &[],
    },
    Command {
        name: "-v",
        help_key: "--version",
        run: version,
        children: &[],
    },
    Command {
        name: "help",
        help_key: "help",
        run: help_cmd,
        children: &[],
    },
    Command {
        name: "sync",
        help_key: "sync",
        run: sync,
        children: &[],
    },
    Command {
        name: "create",
        help_key: "create",
        run: create,
        children: &[],
    },
    Command {
        name: "setup",
        help_key: "setup",
        run: setup,
        children: &[],
    },
    Command {
        name: "adopt",
        help_key: "adopt",
        run: adopt,
        children: &[],
    },
    Command {
        name: "forget",
        help_key: "forget",
        run: forget,
        children: &[],
    },
    Command {
        name: "list",
        help_key: "list",
        run: list,
        children: &[],
    },
    Command {
        name: "show",
        help_key: "show",
        run: show,
        children: &[],
    },
    Command {
        name: "path",
        help_key: "path",
        run: path_cmd,
        children: &[],
    },
    Command {
        name: "remove",
        help_key: "remove",
        run: remove,
        children: &[],
    },
    Command {
        name: "gc",
        help_key: "gc",
        run: gc,
        children: &[],
    },
    Command {
        name: "trust",
        help_key: "trust",
        run: trust_root_cmd,
        children: TRUST_COMMANDS,
    },
    Command {
        name: "exec",
        help_key: "exec",
        run: exec,
        children: &[],
    },
    Command {
        name: "set",
        help_key: "set",
        run: set,
        children: &[],
    },
    Command {
        name: "config",
        help_key: "config",
        run: config_root,
        children: PREFERENCE_COMMANDS,
    },
    Command {
        name: "--version",
        help_key: "--version",
        run: version,
        children: &[],
    },
    Command {
        name: "clone",
        help_key: "clone",
        run: clone_cmd,
        children: &[],
    },
    Command {
        name: "repo",
        help_key: "repo",
        run: project_root,
        children: PROJECT_COMMANDS,
    },
];

#[cfg(test)]
mod contract_tests {
    use super::*;
    use std::collections::BTreeSet;

    fn collect_commands(commands: &[Command], keys: &mut BTreeSet<String>) {
        for command in commands {
            keys.insert(command.help_key.into());
            collect_commands(command.children, keys);
        }
    }

    #[test]
    fn help_contract_covers_every_command_and_exact_parser_flags() {
        let contract: serde_json::Value =
            serde_json::from_str(include_str!("../help-data.json")).unwrap();
        let mut commands = BTreeSet::new();
        collect_commands(COMMANDS, &mut commands);
        for key in commands {
            let entry = &contract["helps"][&key];
            assert!(entry.is_object(), "missing command {key}");
            let actual = command_options(&key)
                .iter()
                .map(|opt| (opt.name.to_string(), opt.short, opt.value))
                .collect::<BTreeSet<_>>();
            let documented = entry["options"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|opt| {
                    let name = opt["name"].as_str().unwrap();
                    (!matches!(name, "json" | "output" | "no-input" | "help")).then(|| {
                        (
                            name.to_string(),
                            opt["short"].as_str().and_then(|short| short.chars().next()),
                            opt["type"] == "string",
                        )
                    })
                })
                .collect::<BTreeSet<_>>();
            let actual = actual
                .into_iter()
                .filter(|(name, _, _)| name != "help")
                .collect::<BTreeSet<_>>();
            assert_eq!(documented, actual, "flags for {key}");
        }
        for key in contract["helps"].as_object().unwrap().keys() {
            assert!(
                find(
                    &key.split_whitespace()
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                    COMMANDS
                )
                .is_some(),
                "unknown documented command {key}"
            );
        }
    }
}

#[cfg(test)]
mod forget_tests {
    use super::*;
    use crate::context::Context;
    use crate::git;
    use std::fs;
    use std::path::PathBuf;

    struct Fixture {
        root: PathBuf,
        context: Context,
        trees: Vec<worktree::WorktreeInfo>,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("rabi-forget-{}", uuid::Uuid::new_v4()));
            let repo = root.join("repo");
            fs::create_dir_all(&repo).unwrap();
            git::run(&["init", "--template=", "-b", "main"], Some(&repo)).unwrap();
            crate::test_git::configure_repo(&repo);
            git::run(
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "initial",
                ],
                Some(&repo),
            )
            .unwrap();
            let context = Context {
                retirement_queries: Default::default(),
                git_root: repo.clone(),
                source_checkout: repo.clone(),
                worktrees_directory: root.join("workspaces"),
                project_common_dir: None,
                config: None,
                preferences: preferences::Preferences::default(),
            };
            let mut trees = Vec::new();
            for name in ["first", "second"] {
                let path = root.join(name);
                git::run(
                    &["worktree", "add", "-b", name, path.to_str().unwrap()],
                    Some(&repo),
                )
                .unwrap();
                metadata::replace(
                    &repo,
                    &path,
                    &MetadataInput {
                        name: Some(format!("alias-{name}")),
                        purpose: Some("old purpose".into()),
                        pr: Some(metadata::WorktreeLink {
                            number: Some(7),
                            url: None,
                        }),
                        ..MetadataInput::default()
                    },
                    metadata::BranchOrigin::Adopted,
                )
                .unwrap();
                trees.push(worktree::worktree_at_path(&context, &path).unwrap());
            }
            Self {
                root,
                context,
                trees,
            }
        }

        fn record_path(&self, index: usize) -> PathBuf {
            self.context
                .git_root
                .join(".git/rabi/workspaces")
                .join(format!(
                    "{}.json",
                    self.trees[index].metadata.id.as_ref().unwrap()
                ))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn assert_refresh_failure(fail_at: usize) {
        let fixture = Fixture::new();
        let mut calls = 0;
        let (items, error) =
            forget_resolved(&fixture.context, fixture.trees.clone(), |context, path| {
                // The fault occurs after the real persistent removal, including its PR link.
                assert!(!metadata::records_path(&context.git_root, path).unwrap());
                let call = calls;
                calls += 1;
                if call == fail_at {
                    Err(Error::msg("injected refresh failure"))
                } else {
                    worktree::worktree_at_path(context, path)
                }
            });
        assert_eq!(error.unwrap().exit_code(), PARTIAL);
        assert_eq!(items.len(), fail_at + 1);
        for (index, item) in items.iter().enumerate() {
            assert_eq!(item["ok"], true);
            assert_eq!(item["workspace"]["path"], fixture.trees[index].path);
            assert!(item["workspace"]["id"].is_null());
            assert!(item["workspace"]["purpose"].is_null());
            assert!(item["workspace"]["links"]["pr"].is_null());
            assert!(!fixture.record_path(index).exists());
            assert!(Path::new(&fixture.trees[index].path).join(".git").is_file());
        }
        assert_eq!(items[fail_at]["error"]["code"], "failed");
        assert_eq!(
            items[fail_at]["error"]["message"],
            "injected refresh failure"
        );
        if fail_at == 0 {
            assert!(fixture.record_path(1).exists());
        }
    }

    #[test]
    fn review6_forget_first_refresh_failure_keeps_deleted_item_ok_and_returns_partial() {
        assert_refresh_failure(0);
    }

    #[test]
    fn review6_forget_later_refresh_failure_keeps_deleted_items_ok_and_returns_partial() {
        assert_refresh_failure(1);
    }

    #[test]
    fn review6_forget_removal_failure_after_an_earlier_removal_is_partial() {
        let fixture = Fixture::new();
        let (items, error) =
            forget_resolved(&fixture.context, fixture.trees.clone(), |context, path| {
                let refreshed = worktree::worktree_at_path(context, path)?;
                // Make the next metadata read fail after the first removal completed.
                fs::write(fixture.record_path(1), "{").unwrap();
                Ok(refreshed)
            });
        assert_eq!(error.unwrap().exit_code(), PARTIAL);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["ok"], true);
        assert_eq!(items[1]["ok"], false);
        assert!(!fixture.record_path(0).exists());
        assert!(fixture.record_path(1).exists());
    }

    #[test]
    fn review6_forget_failure_before_removal_preserves_the_original_exit() {
        let fixture = Fixture::new();
        fs::write(fixture.record_path(0), "{").unwrap();
        let (items, error) = forget_resolved(&fixture.context, fixture.trees.clone(), |_, _| {
            panic!("refresh must not run when removal failed")
        });
        assert_eq!(error.unwrap().exit_code(), GENERAL);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["ok"], false);
        assert!(fixture.record_path(0).exists());
        assert!(fixture.record_path(1).exists());
    }
}

#[cfg(test)]
mod run_tests {
    use super::*;
    use crate::{allocation, context::Context, git, run};
    use std::{fs, path::PathBuf};

    struct Fixture {
        root: PathBuf,
        context: Context,
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("rabi-run-allocation-{}", uuid::Uuid::new_v4()));
            let repo = root.join("repo");
            fs::create_dir_all(&repo).unwrap();
            git::run(&["init", "--template=", "-b", "main"], Some(&repo)).unwrap();
            crate::test_git::configure_repo(&repo);
            git::run(
                &[
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.com",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "initial",
                ],
                Some(&repo),
            )
            .unwrap();
            let context = Context {
                retirement_queries: Default::default(),
                git_root: repo.clone(),
                source_checkout: repo,
                worktrees_directory: root.join("workspaces"),
                project_common_dir: None,
                config: None,
                preferences: preferences::Preferences::default(),
            };
            Self { root, context }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn allocation_success_and_partial_setup_can_both_be_saved_as_runs() {
        for partial in [false, true] {
            let fixture = Fixture::new();
            let context = &fixture.context;
            fs::write(context.git_root.join(".gitignore"), ".env\n").unwrap();
            fs::write(context.git_root.join(".env"), "EXAMPLE=1\n").unwrap();
            if partial {
                std::os::unix::fs::symlink("missing", context.git_root.join(".worktreeinclude"))
                    .unwrap();
            }
            crate::output::begin("create", true);
            let allocation = allocation::create(
                context,
                allocation::Request {
                    name: Some("topic"),
                    base: Some("main"),
                    branch: None,
                    pr: None,
                    issue: None,
                    detach: false,
                    reuse: false,
                    copy_files: &[".env".into()],
                    metadata: MetadataInput {
                        purpose: Some("run groundwork".into()),
                        ..MetadataInput::default()
                    },
                    hooks: HookControl {
                        no_hooks: true,
                        json: true,
                        prompt: false,
                    },
                    no_setup: false,
                },
                &Logger {
                    enabled: false,
                    json: true,
                },
            )
            .unwrap();
            assert!(
                crate::output::test_data().is_none(),
                "allocation must not emit create data"
            );
            assert_eq!(allocation.workspace.name, "topic");
            assert_eq!(allocation.workspace.branch, "topic");
            assert!(Path::new(&allocation.workspace.path).join(".git").exists());
            assert!(allocation.workspace.metadata.id.is_some());
            assert_eq!(
                allocation.workspace.metadata.purpose.as_deref(),
                Some("run groundwork")
            );
            assert_eq!(
                allocation.source,
                serde_json::json!({"kind":"new", "base":"refs/heads/main", "reused":false})
            );
            assert!(allocation.metadata_error.is_none());
            assert_eq!(allocation.setup.failed(), partial);
            assert_eq!(allocation.retry_command.is_some(), partial);
            if partial {
                assert!(allocation.setup.copy_error.is_some());
                assert_eq!(allocation.setup.status.state, crate::setup::State::Failed);
                assert_eq!(
                    allocation.retry_command.as_deref(),
                    Some(
                        format!(
                            "cd '{}' && rabi setup 'topic' --copy-file '.env'",
                            context.source_checkout.display()
                        )
                        .as_str()
                    )
                );
            } else {
                assert_eq!(allocation.setup.copied, [".env"]);
                assert_eq!(
                    fs::read_to_string(Path::new(&allocation.workspace.path).join(".env")).unwrap(),
                    "EXAMPLE=1\n"
                );
            }
            let rendered = render_create(
                &allocation,
                &Logger {
                    enabled: false,
                    json: true,
                },
            );
            assert_eq!(
                rendered.map_err(|error| error.exit_code()),
                if partial { Err(6) } else { Ok(0) }
            );
            let workflow = crate::workflow::builtin().inspect();
            let definition = workflow.definition.as_ref().unwrap();
            let mut record = run::Record::from_allocation(&allocation, &workflow.entry, definition);
            assert_eq!(record.workspace.id, allocation.workspace.metadata.id);
            assert_eq!(record.workspace.path, allocation.workspace.path);
            if partial {
                record.finish(Some(run::StopReason {
                    kind: "setup_failed".into(),
                    message: "Copying failed".into(),
                }));
            }
            run::create(&context.git_root, &record, "# Task", &workflow.yaml).unwrap();
            let file = run::directory(&context.git_root, &record.id)
                .unwrap()
                .join("run.json");
            let bytes = fs::read(&file).unwrap();
            assert_eq!(run::load(&context.git_root, &record.id).unwrap().0, record);
            assert_eq!(fs::read(&file).unwrap(), bytes);
            // Linked checkout paths resolve the very same run directory.
            assert_eq!(
                run::load(Path::new(&allocation.workspace.path), &record.id)
                    .unwrap()
                    .0,
                record
            );
        }
    }
}
