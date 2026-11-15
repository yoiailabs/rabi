//! Fetch and integrate bases, requiring permission to rewrite published history.
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

mod fetch;

use serde::Serialize;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::{git, metadata, worktree};

#[derive(Default)]
pub struct Request {
    pub names: Vec<String>,
    pub all: bool,
    pub main: bool,
    pub auto: bool,
    pub dry_run: bool,
    pub rebase: bool,
    pub merge: bool,
    pub rewrite_published: bool,
    pub keep_conflicts: bool,
}

impl Request {
    pub fn validate(&self) -> Result<()> {
        if (self.all && !self.names.is_empty())
            || [self.auto, self.rebase, self.merge]
                .into_iter()
                .filter(|flag| *flag)
                .count()
                > 1
        {
            return Err(Error::validation(
                "Conflicting sync selectors or strategies",
            ));
        }
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub workspace: Option<worktree::WorktreeJson>,
    pub strategy_source: String,
    pub rewrite_allowed: bool,
    pub rewrite_source: Option<String>,
    pub rewrote_published: bool,
    pub name: String,
    pub path: Option<String>,
    pub branch: Option<String>,
    pub base: Option<String>,
    pub status: String,
    pub strategy: Option<String>,
    pub before: Option<String>,
    pub after: Option<String>,
    pub conflicts: Vec<String>,
    pub detail: Option<String>,
    pub code: Option<String>,
}

impl Item {
    fn status(&mut self, status: &str, detail: Option<String>) {
        self.status = status.to_string();
        self.detail = detail.map(|value| short_detail(&value));
    }
}

#[derive(Serialize)]
pub struct Report {
    pub plan: bool,
    #[serde(rename = "main")]
    pub default: Option<Item>,
    #[serde(rename = "items")]
    pub worktrees: Vec<Item>,
    #[serde(skip)]
    pub exit: i32,
}

// git::output accepts a nonzero exit when stderr is empty. Sync must fail closed
// for both safety queries and mutations, including rollback commands.
fn checked_output(args: &[&str], root: &Path, trim: bool) -> Result<String> {
    let result = git::raw_git(root, args)?;
    if result.code != 0 {
        return Err(Error::msg(if result.stderr.trim().is_empty() {
            format!("Git {} failed (exit {})", args.join(" "), result.code)
        } else {
            result.stderr.trim().to_string()
        }));
    }
    Ok(if trim {
        result.stdout.trim().to_string()
    } else {
        result.stdout
    })
}

pub fn normalize_base(root: &Path, reference: &str) -> Result<String> {
    if reference.starts_with('-') || reference.is_empty() {
        return Err(Error::validation("Base must be an existing ref"));
    }
    let resolve = |args: &[&str]| -> Result<String> {
        let result = git::raw_git(root, args)?;
        if result.code != 0 {
            return Err(Error::validation(format!(
                "Base '{reference}' is not a valid ref"
            )));
        }
        Ok(result.stdout.trim().to_string())
    };
    let full = resolve(&["rev-parse", "--symbolic-full-name", "--verify", reference])?;
    if full.is_empty() || full == "HEAD" {
        return resolve(&["rev-parse", "--verify", &format!("{reference}^{{commit}}")]);
    }
    if !full.starts_with("refs/") || full.lines().count() != 1 {
        return Err(Error::validation(format!(
            "Base '{reference}' must name an existing ref"
        )));
    }
    resolve(&["rev-parse", "--verify", &format!("{full}^{{commit}}")])?;
    Ok(full)
}

pub fn creation_base(root: &Path, explicit: Option<&str>) -> Result<String> {
    match explicit {
        Some(base) => normalize_base(root, base),
        None => fallback_base(root).or_else(|_| normalize_base(root, "HEAD")),
    }
}

pub fn fallback_base(root: &Path) -> Result<String> {
    let detected = git::default_branch(root)?;
    if let Some(default) = detected
        .as_deref()
        .filter(|name| name.starts_with("origin/"))
    {
        return normalize_base(root, &format!("refs/remotes/{default}"));
    }
    if let Some(default) = detected.as_deref() {
        if let Some(upstream) = default_upstream(root, default)? {
            return Ok(upstream);
        }
    }
    let specs = fetch_refspecs(root)?;
    let remotes = checked_output(&["remote"], root, true)?;
    // Prefer origin even when its HEAD points into a custom fetch namespace.
    let origin = remotes.lines().filter(|remote| *remote == "origin");
    let others = remotes.lines().filter(|remote| *remote != "origin");
    for remote in origin.chain(others) {
        let head = git::raw_git(
            root,
            &[
                "symbolic-ref",
                "--quiet",
                &format!("refs/remotes/{remote}/HEAD"),
            ],
        )?;
        if head.code != 0 && head.code != 1 {
            return Err(Error::msg(format!(
                "Cannot resolve {remote}/HEAD (exit {}): {}",
                head.code,
                head.stderr.trim()
            )));
        }
        if head.code == 0 && commit(root, head.stdout.trim())?.is_some() {
            return Ok(head.stdout.trim().to_string());
        }
    }
    let default = match detected {
        Some(default) => default,
        None => {
            for name in ["main", "master"] {
                if let Some(remote) = tracking_ref(root, &specs, "origin", name)? {
                    return Ok(remote);
                }
            }
            return Err(Error::msg("Cannot determine the default branch"));
        }
    };
    let configured = git::raw_git(
        root,
        &["config", "--get", &format!("branch.{default}.remote")],
    )?;
    if configured.code != 0 && configured.code != 1 {
        return Err(Error::msg(format!(
            "Cannot read branch.{default}.remote (exit {}): {}",
            configured.code,
            configured.stderr.trim()
        )));
    }
    let remote_name = if configured.code == 0 && configured.stdout.trim() != "." {
        configured.stdout.trim()
    } else {
        "origin"
    };
    if let Some(remote) = tracking_ref(root, &specs, remote_name, &default)? {
        Ok(remote)
    } else {
        normalize_base(root, &format!("refs/heads/{default}"))
    }
}

// Git resolves branch.merge through the configured remote's fetch refspecs.
fn default_upstream(root: &Path, branch: &str) -> Result<Option<String>> {
    let reference = format!("refs/heads/{branch}");
    let Ok(resolved) = git::raw_git(
        root,
        &[
            "for-each-ref",
            "--format=%(refname) %(upstream)",
            &reference,
        ],
    ) else {
        return Ok(None);
    };
    if resolved.code != 0 {
        return Ok(None);
    }
    let upstream = resolved.stdout.lines().find_map(|line| {
        let (name, upstream) = line.split_once(' ')?;
        (name == reference).then_some(upstream)
    });
    Ok(upstream
        .filter(|upstream| upstream.starts_with("refs/remotes/"))
        .filter(|upstream| commit(root, upstream).ok().flatten().is_some())
        .map(str::to_string))
}

fn commit(root: &Path, reference: &str) -> Result<Option<String>> {
    let result = git::raw_git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{reference}^{{commit}}"),
        ],
    )?;
    match result.code {
        0 => Ok(Some(result.stdout.trim().to_string())),
        1 => Ok(None),
        _ => Err(Error::msg(result.stderr)),
    }
}

fn ancestor(root: &Path, a: &str, b: &str) -> Result<bool> {
    let result = git::raw_git(root, &["merge-base", "--is-ancestor", a, b])?;
    match result.code {
        0 => Ok(true),
        1 => Ok(false),
        _ => Err(Error::msg(result.stderr)),
    }
}

fn common_base(root: &Path, tip: &str, base: &str) -> Result<String> {
    let result = git::raw_git(root, &["merge-base", tip, base])?;
    match result.code {
        0 if !result.stdout.trim().is_empty() => Ok(result.stdout.trim().to_string()),
        1 => Err(Error::msg(format!("no common history with {base}"))),
        _ => Err(Error::msg(format!(
            "Cannot determine common history with {base} (exit {}): {}",
            result.code,
            result.stderr.trim()
        ))),
    }
}

fn dirty(path: &Path) -> Result<bool> {
    Ok(!checked_output(
        &["status", "--porcelain=v2", "--untracked-files=all"],
        path,
        false,
    )?
    .is_empty())
}

// Checkout during rebase may overwrite ignored files. Preserve those contents too.
fn ignored_would_be_overwritten(path: &Path, base: &str, rebase: bool) -> Result<bool> {
    let fork = common_base(path, "HEAD", base)?;
    let ignored = checked_output(
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
        ],
        path,
        false,
    )?;
    if ignored.is_empty() {
        return Ok(false);
    }
    // Disable rename detection so both ends of renames and copies are included.
    // -m also includes paths changed relative to each parent of merge commits.
    let changed = checked_output(
        &[
            "log",
            "--name-only",
            "--format=",
            "-z",
            "--no-renames",
            "-m",
            &format!("{fork}..{base}"),
            "--",
        ],
        path,
        false,
    )?;
    let checkout = if rebase {
        checked_output(
            &[
                "log",
                "--name-only",
                "--format=",
                "-z",
                "--no-renames",
                "-m",
                &format!("{fork}..HEAD"),
                "--",
            ],
            path,
            false,
        )?
    } else {
        checked_output(
            &[
                "diff",
                "--name-only",
                "--no-renames",
                "-z",
                "HEAD",
                base,
                "--",
            ],
            path,
            false,
        )?
    };
    let config = git::raw_git(path, &["config", "--bool", "--get", "core.ignorecase"])?;
    let ignorecase = match config.code {
        0 => config.stdout.trim() == "true",
        1 => false, // Unset, as on a case-sensitive filesystem.
        _ => return Err(Error::msg(config.stderr)),
    };
    let comparison_path = |value: &str| {
        if ignorecase {
            value.to_lowercase()
        } else {
            value.to_string()
        }
    };
    let targets: HashSet<String> = changed
        .split('\0')
        .chain(checkout.split('\0'))
        .filter(|p| !p.is_empty())
        .map(comparison_path)
        .collect();
    let mut prefixes = HashSet::new();
    for target in &targets {
        let mut parent = target.as_str();
        while let Some((prefix, _)) = parent.rsplit_once('/') {
            prefixes.insert(prefix);
            parent = prefix;
        }
    }
    for entry in ignored.split('\0').filter(|p| !p.is_empty()) {
        let normalized = comparison_path(entry.trim_end_matches('/'));
        let mut entry = normalized.as_str();
        if prefixes.contains(entry) || targets.contains(entry) {
            return Ok(true);
        }
        while let Some((parent, _)) = entry.rsplit_once('/') {
            if targets.contains(parent) {
                return Ok(true);
            }
            entry = parent;
        }
    }
    Ok(false)
}

struct FetchRefspec {
    remote: String,
    source: String,
    destination: String,
}

// Fetch refspecs can store a remote's branches in any tracking namespace.
fn fetch_refspecs(root: &Path) -> Result<Vec<FetchRefspec>> {
    let mut specs = Vec::new();
    for remote in checked_output(&["remote"], root, true)?.lines() {
        let configured = git::raw_git(
            root,
            &["config", "--get-all", &format!("remote.{remote}.fetch")],
        )?;
        if configured.code != 0 && configured.code != 1 {
            return Err(Error::msg(configured.stderr));
        }
        for spec in configured.stdout.lines() {
            let spec = spec.strip_prefix('+').unwrap_or(spec);
            if let Some((source, destination)) = spec.split_once(':') {
                if !source.starts_with('^') && !destination.is_empty() {
                    specs.push(FetchRefspec {
                        remote: remote.to_string(),
                        source: source.to_string(),
                        destination: destination.to_string(),
                    });
                }
            }
        }
    }
    Ok(specs)
}

fn map_ref(reference: &str, from: &str, to: &str) -> Option<String> {
    if let Some((prefix, suffix)) = from.split_once('*') {
        let captured = reference.strip_prefix(prefix)?.strip_suffix(suffix)?;
        let (prefix, suffix) = to.split_once('*')?;
        Some(format!("{prefix}{captured}{suffix}"))
    } else {
        (reference == from).then(|| to.to_string())
    }
}

fn tracking_ref(
    root: &Path,
    specs: &[FetchRefspec],
    remote: &str,
    branch: &str,
) -> Result<Option<String>> {
    for spec in specs.iter().filter(|spec| spec.remote == remote) {
        if let Some(reference) = map_ref(
            &format!("refs/heads/{branch}"),
            &spec.source,
            &spec.destination,
        ) {
            if commit(root, &reference)?.is_some() {
                return Ok(Some(reference));
            }
        }
    }
    Ok(None)
}

fn source_for(base: &str, specs: &[FetchRefspec]) -> Option<(String, String)> {
    // Prefer a specific mapping over a broader overlapping namespace, including
    // remotes named team and team/origin. Exact refspecs precede patterns.
    specs
        .iter()
        .filter_map(|spec| {
            map_ref(base, &spec.destination, &spec.source).map(|source| (spec, source))
        })
        .max_by_key(|(spec, _)| (!spec.destination.contains('*'), spec.destination.len()))
        .map(|(spec, source)| (spec.remote.clone(), source))
}

fn remote_for(base: &str, specs: &[FetchRefspec]) -> Option<String> {
    source_for(base, specs).map(|(remote, _)| remote)
}

// Flattening a merge drops its resolution and any changes made only in that
// commit. Compare trees without checking out either parent or touching the index.
fn unsafe_merge(path: &Path, base: &str) -> Result<Option<String>> {
    let merges = checked_output(
        &[
            "log",
            "--no-show-signature",
            "--merges",
            "--format=%H %T %P",
            &format!("{base}..HEAD"),
            "--",
        ],
        path,
        true,
    )?;
    for merge in merges.lines() {
        let fields: Vec<_> = merge.split_whitespace().collect();
        let [sha, tree, p1, p2] = fields.as_slice() else {
            return Ok(Some(format!(
                "Cannot verify merge commit {}; use --merge",
                fields.first().copied().unwrap_or("unknown")
            )));
        };
        let automatic = git::raw_git(path, &["merge-tree", "--write-tree", p1, p2])?;
        if automatic.code != 0 && automatic.code != 1 {
            return Err(Error::msg(format!(
                "Cannot verify merge commit {sha}; use --merge (requires Git 2.38 or later)"
            )));
        }
        if automatic.code == 1 || automatic.stdout.lines().next() != Some(*tree) {
            let short = checked_output(&["rev-parse", "--short", sha], path, true)?;
            return Ok(Some(format!(
                "merge commit {short} has changes of its own; use --merge"
            )));
        }
    }
    Ok(None)
}

pub fn run(context: &Context, request: &Request) -> Result<Report> {
    let _hooks = crate::process::suppress_git_hooks();
    request.validate()?;
    metadata::load_strict(&context.git_root)?;
    let listed = worktree::list_worktrees(context, false, true)?;
    let mut targets = Vec::new();
    let mut include_main = request.main;
    if request.all {
        targets = listed
            .iter()
            .filter(|tree| !tree.is_default)
            .cloned()
            .collect();
    } else if !request.names.is_empty() {
        for name in &request.names {
            let tree = crate::selectors::resolve(context, name, false, None)?;
            // Primary selections use only the default branch's ff-only path,
            // just like an implicit sync from the primary checkout.
            if tree.is_default {
                include_main = true;
                continue;
            }
            if !targets
                .iter()
                .any(|item: &worktree::WorktreeInfo| item.path == tree.path)
            {
                targets.push(tree);
            }
        }
    } else if !request.main {
        let tree = crate::selectors::resolve_optional_current(context, None, false)?;
        if tree.is_default {
            include_main = true;
        } else {
            targets.push(tree);
        }
    }
    // Only workspace integration can leave conflicts; preserve the existing
    // single-workspace rule even when primary is explicitly included (ff-only).
    if request.keep_conflicts && targets.len() != 1 {
        return Err(Error::validation(
            "--keep-conflicts requires exactly one workspace target",
        ));
    }
    // Resolve the default identity independently of a worktree's recorded base.
    let (fallback, mut fallback_error) = match fallback_base(&context.git_root) {
        Ok(base) => (base, None),
        Err(error) => (String::new(), Some(error.to_string())),
    };
    let remotes = fetch_refspecs(&context.git_root)?;
    let default_name = if fallback_error.is_none() {
        match default_local_name(&context.git_root, &fallback, &remotes) {
            Ok(name) => Some(name),
            Err(error) => {
                fallback_error = Some(error.to_string());
                None
            }
        }
    } else {
        None
    };
    // §3.4: --main only operates on primary while it is on the default branch.
    let primary = listed.iter().find(|tree| tree.is_default);
    let primary_on_default = primary.is_some_and(|tree| {
        !tree.detached && Some(tree.branch.as_str()) == default_name.as_deref()
    });
    let cleared_default = primary.filter(|tree| primary_on_default && tree.metadata.base_cleared);
    let bases: Vec<String> = targets
        .iter()
        .map(|tree| {
            if tree.metadata.base_cleared {
                String::new()
            } else if !tree.detached && Some(tree.branch.as_str()) == default_name.as_deref() {
                fallback.clone()
            } else {
                tree.metadata
                    .base
                    .clone()
                    .unwrap_or_else(|| fallback.clone())
            }
        })
        .collect();
    let mut involved = BTreeSet::new();
    for base in targets
        .iter()
        .zip(&bases)
        .filter(|(tree, _)| !tree.metadata.base_cleared)
        .map(|(_, base)| base)
        .chain(
            (include_main && primary_on_default && cleared_default.is_none()).then_some(&fallback),
        )
    {
        if let Some(remote) = remote_for(base, &remotes) {
            involved.insert(remote);
        }
    }
    let mut fetch_errors = BTreeMap::new();
    match fetch::check(&context.git_root, &involved) {
        Err(error) if error.exit_code() == crate::error::REFUSED => return Err(error),
        Err(error) => {
            // Preserve per-item operational failures, without fetching after
            // an incomplete safety query.
            for remote in involved {
                fetch_errors.insert(remote, error.to_string());
            }
        }
        Ok(()) => {
            for remote in involved {
                if let Err(error) = fetch::fetch(&context.git_root, &remote) {
                    fetch_errors.insert(remote, error.to_string());
                }
            }
        }
    }
    let mut report = Report {
        plan: request.dry_run,
        default: None,
        worktrees: Vec::new(),
        exit: 0,
    };
    let fetch_error =
        |base: &str| remote_for(base, &remotes).and_then(|remote| fetch_errors.get(&remote));
    if include_main {
        let item = if fallback_error.is_none() && !primary_on_default {
            let mut item = match primary {
                Some(tree) => tree_item(tree, &fallback),
                None => failed_item("main", Error::msg("Primary checkout is unavailable")),
            };
            item.strategy = Some("ff-only".into());
            item.status(
                "not-default-branch",
                Some("Primary checkout is not on the default branch".into()),
            );
            item
        } else if let Some(tree) = cleared_default {
            base_unknown_item(context, tree)
        } else {
            let result = match (
                fallback_error.as_ref().or_else(|| fetch_error(&fallback)),
                default_name.as_deref(),
            ) {
                (Some(error), _) => Err(Error::msg(error.clone())),
                (None, Some(name)) => sync_default(context, &fallback, name, request.dry_run, true),
                (None, None) => Err(Error::msg("Cannot determine the default branch")),
            };
            match result {
                Ok(item) => item,
                Err(error) => failed_item("default", error),
            }
        };
        report.default = Some(item);
    }
    let mut planned_refs = BTreeMap::new();
    if request.dry_run {
        if let Some(item) = report
            .default
            .as_ref()
            .filter(|item| item.status == "updated")
        {
            if let (Some(reference), Some(after)) = (&item.branch, &item.after) {
                // Execution integrates primary first. Plan later workspaces
                // against that same tip without writing a temporary ref.
                planned_refs.insert(reference.clone(), Some(after.clone()));
            }
        }
    }
    for (tree, base) in targets.iter().zip(&bases) {
        if tree.metadata.base_cleared {
            report.worktrees.push(base_unknown_item(context, tree));
            continue;
        }
        // Only targets using the failed remote must refuse their stale base.
        let base_error = tree
            .metadata
            .base
            .is_none()
            .then_some(fallback_error.as_ref())
            .flatten();
        let result = match base_error.or_else(|| fetch_error(base)) {
            Some(error) => Err(Error::msg(error.clone())),
            None => sync_tree(
                context,
                tree,
                base,
                request,
                &fallback,
                default_name.as_deref(),
                &planned_refs,
            ),
        };
        let item = match result {
            Ok(item) => item,
            Err(error) => {
                let mut item = tree_item(tree, base);
                if base.is_empty() {
                    item.base = None;
                }
                item.status("failed", Some(error.to_string()));
                item
            }
        };
        if request.dry_run && item.status == "updated" {
            // tree.branch is always a short name, even when it starts with
            // refs/heads/. Match the full refs used by primary and base reads.
            let reference = format!("refs/heads/{}", tree.branch);
            // None records a planned update whose future tip is unknown.
            planned_refs.insert(reference, item.after.clone());
        }
        report.worktrees.push(item);
    }
    for item in report.default.iter_mut().chain(&mut report.worktrees) {
        let main_strategy = item.strategy.as_deref() == Some("ff-only");
        item.strategy_source = if main_strategy {
            "default"
        } else {
            strategy_source(context, request)
        }
        .into();
        item.rewrite_allowed = request.rewrite_published
            || context.preferences.sync_rewrite_published.unwrap_or(false);
        item.rewrite_source = if request.rewrite_published {
            Some("flag".into())
        } else {
            context
                .preferences
                .sync_rewrite_published
                .map(|_| "config".into())
        };
        if item.workspace.is_none() {
            let tree = if item.name == "default" {
                primary
            } else {
                listed
                    .iter()
                    .find(|tree| item.path.as_deref() == Some(tree.path.as_str()))
            };
            item.workspace = tree.map(|tree| tree.to_json(item.base.clone()));
        }
    }
    let ordinary_failure = !fetch_errors.is_empty()
        || report
            .default
            .iter()
            .chain(&report.worktrees)
            .any(|item| item.status == "failed" && item.code.as_deref() != Some("published"))
        || (targets.is_empty()
            && report
                .default
                .as_ref()
                .is_some_and(|item| matches!(item.status.as_str(), "dirty" | "diverged")));
    let refused = report
        .worktrees
        .iter()
        .any(|item| item.code.as_deref() == Some("published"));
    let items = || report.default.iter().chain(&report.worktrees);
    // Leaving a conflict in progress can move HEAD during rebase. It still
    // returns exit 5 when no other item changed.
    let changed = !request.dry_run
        && items().any(|item| item.status != "conflict" && item.before != item.after);
    let conflict = items().any(|item| item.status == "conflict");
    report.exit = if changed && (ordinary_failure || conflict || refused) {
        crate::error::PARTIAL
    } else if ordinary_failure {
        crate::error::GENERAL
    } else if conflict {
        crate::error::CONFLICT
    } else if refused {
        crate::error::REFUSED
    } else {
        crate::error::SUCCESS
    };
    Ok(report)
}

fn short_detail(value: &str) -> String {
    value
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

fn failed_item(name: &str, error: Error) -> Item {
    Item {
        workspace: None,
        strategy_source: "default".into(),
        rewrite_allowed: false,
        rewrite_source: None,
        rewrote_published: false,
        name: name.to_string(),
        path: None,
        branch: None,
        base: None,
        status: "failed".into(),
        strategy: None,
        before: None,
        after: None,
        conflicts: Vec::new(),
        detail: Some(short_detail(&error.to_string())),
        code: None,
    }
}

fn tree_item(tree: &worktree::WorktreeInfo, base: &str) -> Item {
    Item {
        workspace: Some(tree.to_json((!base.is_empty()).then(|| base.to_string()))),
        strategy_source: "default".into(),
        rewrite_allowed: false,
        rewrite_source: None,
        rewrote_published: false,
        name: tree.name.clone(),
        path: Some(tree.path.clone()),
        branch: (!tree.detached).then(|| tree.branch.clone()),
        base: Some(base.to_string()),
        status: "up-to-date".into(),
        strategy: None,
        before: tree.head.clone(),
        after: tree.head.clone(),
        conflicts: Vec::new(),
        detail: None,
        code: None,
    }
}

fn base_unknown_item(context: &Context, tree: &worktree::WorktreeInfo) -> Item {
    let mut item = tree_item(tree, "");
    item.base = None;
    item.status(
        "base-unknown",
        Some(format!(
            "Base was explicitly cleared; set a base: {}",
            set_base_hint(context, tree, "REF")
        )),
    );
    item
}

fn set_base_hint(context: &Context, tree: &worktree::WorktreeInfo, reference: &str) -> String {
    if tree.name.starts_with('-') && !Path::new(&tree.path).is_dir() {
        format!(
            "rabi -R {} set --base {reference} -- {}",
            crate::allocation::shell_quote(&context.git_root),
            crate::allocation::shell_quote(&tree.name)
        )
    } else if tree.name.starts_with('-') {
        format!(
            "(cd {} && rabi set . --base {reference})",
            crate::allocation::shell_quote(&tree.path)
        )
    } else {
        format!("rabi set {} --base {reference}", tree.name)
    }
}

// Read private state directly, including registrations whose checkout is missing.
fn operation_state(path: &Path, state: &str) -> Result<Option<String>> {
    match std::fs::read_to_string(path.join(state)) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(Error::msg(format!(
            "Cannot inspect Git state {state}: {error}"
        ))),
    }
}

fn active_operation(path: &Path) -> Result<Option<&'static str>> {
    let git_dir = checked_output(&["rev-parse", "--absolute-git-dir"], path, true)?;
    for (state, operation) in [
        ("rebase-merge", "rebase"),
        ("rebase-apply", "rebase"),
        ("MERGE_HEAD", "merge"),
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
        ("BISECT_START", "bisect"),
        ("sequencer", "sequencer"),
    ] {
        if Path::new(&git_dir)
            .join(state)
            .try_exists()
            .map_err(|error| Error::msg(error.to_string()))?
        {
            return Ok(Some(operation));
        }
    }
    Ok(None)
}

fn branch_reservation(context: &Context, local: &str) -> Result<Option<(String, &'static str)>> {
    let common = git::git_common_dir(&context.git_root)?;
    let mut dirs = vec![common.clone()];
    match std::fs::read_dir(common.join("worktrees")) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| Error::msg(error.to_string()))?;
                if entry
                    .file_type()
                    .map_err(|error| Error::msg(error.to_string()))?
                    .is_dir()
                {
                    dirs.push(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(Error::msg(error.to_string())),
    }
    for path in dirs {
        for (state, operation) in [
            ("rebase-merge/head-name", "rebased"),
            ("rebase-apply/head-name", "rebased"),
            ("rebase-merge/update-refs", "rebased"),
            ("BISECT_START", "bisected"),
        ] {
            if let Some(value) = operation_state(&path, state)? {
                if value.lines().any(|line| {
                    line == local || line == local.strip_prefix("refs/heads/").unwrap_or(local)
                }) {
                    let checkout = if path == common {
                        context.git_root.clone()
                    } else {
                        operation_state(&path, "gitdir")?
                            .and_then(|value| {
                                Path::new(value.trim()).parent().map(Path::to_path_buf)
                            })
                            .unwrap_or_else(|| path.clone())
                    };
                    return Ok(Some((checkout.display().to_string(), operation)));
                }
            }
        }
    }
    Ok(None)
}

pub(crate) fn default_local_branch(root: &Path, base: &str) -> Result<String> {
    let base = normalize_base(root, base)?;
    default_local_name(root, &base, &fetch_refspecs(root)?)
}

fn default_local_name(root: &Path, base: &str, remotes: &[FetchRefspec]) -> Result<String> {
    if let Some(default) = git::default_branch(root)? {
        if !default.starts_with("origin/")
            && default_upstream(root, &default)?.as_deref() == Some(base)
        {
            return Ok(default);
        }
    }
    let source = source_for(base, remotes).map(|(_, source)| source);
    let local_name = if let Some(source) = source.as_deref() {
        source
            .strip_prefix("refs/heads/")
            .ok_or_else(|| Error::msg("Default branch must name a branch"))?
    } else if base.starts_with("refs/remotes/") {
        return Err(Error::msg(format!(
            "No configured remote matches default base '{base}'"
        )));
    } else {
        base.strip_prefix("refs/heads/")
            .ok_or_else(|| Error::msg("Default branch must name a branch"))?
    };
    Ok(local_name.to_string())
}

fn sync_default(
    context: &Context,
    base: &str,
    local_name: &str,
    dry_run: bool,
    primary_only: bool,
) -> Result<Item> {
    // Fetch may create a remote HEAD. Keep the branch identity resolved during
    // planning so integration and the preflight guards use the same target.
    let local = format!("refs/heads/{local_name}");
    let root = &context.git_root;
    let before = commit(root, &local)?;
    let mut item = Item {
        workspace: None,
        strategy_source: "default".into(),
        rewrite_allowed: false,
        rewrite_source: None,
        rewrote_published: false,
        name: local_name.to_string(),
        path: None,
        branch: Some(local.clone()),
        base: Some(base.to_string()),
        status: "up-to-date".into(),
        strategy: Some("ff-only".into()),
        before: before.clone(),
        after: before.clone(),
        conflicts: Vec::new(),
        detail: None,
        code: None,
    };
    let trees = worktree::list_worktrees(context, false, true)?;
    // Fetch can take time; enforce --main at the mutation boundary too.
    if primary_only {
        let primary = trees
            .iter()
            .find(|tree| tree.is_default)
            .ok_or_else(|| Error::msg("Primary checkout is unavailable"))?;
        if primary.detached || primary.branch != local_name {
            let mut item = tree_item(primary, base);
            item.strategy = Some("ff-only".into());
            item.status(
                "not-default-branch",
                Some("Primary checkout is not on the default branch".into()),
            );
            return Ok(item);
        }
    }
    if let Some((path, operation)) = branch_reservation(context, &local)? {
        item.path = Some(path.clone());
        item.status(
            "dirty",
            Some(format!("{local_name} is being {operation} in {path}")),
        );
        return Ok(item);
    }
    let checked = trees
        .into_iter()
        .find(|tree| !tree.detached && tree.branch == local_name);
    if let Some(tree) = &checked {
        if tree.metadata.base_cleared {
            return Ok(base_unknown_item(context, tree));
        }
        item.path = Some(tree.path.clone());
        item.workspace = Some(tree.to_json(Some(base.into())));
    }
    let target = commit(root, base)?.ok_or_else(|| Error::msg("Default base no longer exists"))?;
    if let Some(tree) = &checked {
        if !Path::new(&tree.path).is_dir() {
            item.status("dirty", Some("Workspace directory is missing".into()));
            return Ok(item);
        }
        if let Some(operation) = active_operation(Path::new(&tree.path))? {
            item.status(
                "dirty",
                Some(format!("{operation} in progress in {}", tree.path)),
            );
            return Ok(item);
        }
    }
    if before.as_deref() == Some(&target) {
        return Ok(item);
    }
    if let Some(before) = &before {
        common_base(root, before, base)?;
        if !ancestor(root, before, &target)? {
            item.status("diverged", None);
            return Ok(item);
        }
    }
    if let Some(tree) = checked {
        let path = Path::new(&tree.path);
        if ignored_would_be_overwritten(path, base, false)? {
            item.status("dirty", Some("Ignored files would be overwritten".into()));
            return Ok(item);
        }
        if dirty(path)? {
            item.status("dirty", None);
            return Ok(item);
        }
        if dry_run {
            item.status(
                "updated",
                Some(format!("Would fast-forward checkout {}", tree.path)),
            );
            item.after = Some(target);
            return Ok(item);
        }
        let result = git::raw_git(
            path,
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "merge.autoStash=false",
                "merge",
                "--no-autostash",
                "--no-squash",
                "--commit",
                "--no-verify-signatures",
                "--ff-only",
                "--no-edit",
                base,
            ],
        )?;
        if result.code != 0 {
            item.status(
                if dirty(path)? { "dirty" } else { "failed" },
                Some(result.stderr.trim().into()),
            );
            return Ok(item);
        }
    } else if !dry_run {
        checked_output(
            &[
                "-c",
                "core.hooksPath=/dev/null",
                "update-ref",
                &local,
                &target,
                before.as_deref().unwrap_or(""),
            ],
            root,
            true,
        )?;
    }
    item.after = Some(target);
    item.status("updated", None);
    Ok(item)
}

fn sync_tree(
    context: &Context,
    tree: &worktree::WorktreeInfo,
    base_ref: &str,
    request: &Request,
    default_base: &str,
    default_name: Option<&str>,
    planned_refs: &BTreeMap<String, Option<String>>,
) -> Result<Item> {
    // Keep this guard at the integration boundary, regardless of selector/strategy.
    if !tree.detached && default_name == Some(tree.branch.as_str()) {
        let mut item = sync_default(context, default_base, &tree.branch, request.dry_run, false)?;
        item.name = tree.name.clone();
        return Ok(item);
    }
    let mut item = tree_item(tree, base_ref);
    let path = Path::new(&tree.path);
    if !path.is_dir() || tree.orphaned {
        item.status(
            "dirty",
            Some("Workspace directory is missing or registration is orphaned".into()),
        );
        return Ok(item);
    }
    let operation = active_operation(path)?;
    if tree.detached {
        item.status("detached", operation.map(|op| format!("{op} in progress")));
        return Ok(item);
    }
    if let Some(operation) = operation {
        item.status("dirty", Some(format!("{operation} in progress")));
        return Ok(item);
    }
    if let Some((reserved_path, operation)) =
        branch_reservation(context, &format!("refs/heads/{}", tree.branch))?
    {
        item.status(
            "dirty",
            Some(format!(
                "{} is being {operation} in {reserved_path}",
                tree.branch
            )),
        );
        return Ok(item);
    }
    if dirty(path)? {
        item.status("dirty", None);
        return Ok(item);
    }
    let base = match planned_refs.get(base_ref) {
        Some(Some(tip)) => tip.as_str(),
        Some(None) => {
            // The earlier integration has no commit ID yet. Do not decide
            // reachability, strategy or publication against its obsolete tip.
            item.status(
                "updated",
                Some(format!("Integration depends on planned update of {base_ref}; resulting tip and strategy are unknown")),
            );
            item.after = None;
            return Ok(item);
        }
        None => base_ref,
    };
    let base_exists = commit(path, base)?.is_some();
    if !base_exists {
        item.status(
            "base-gone",
            Some(format!(
                "Set a new base: {}",
                set_base_hint(context, tree, "<ref>")
            )),
        );
        return Ok(item);
    }
    let reachability = git::tip_reachability(
        path,
        Some(&tree.branch),
        tree.head.as_deref(),
        &Some(base.to_string()),
    );
    let moved = !crate::gc::still_at_creation_point(
        path,
        &tree.branch,
        tree.metadata.created_from.as_deref(),
        base,
        !metadata::records_path(&context.git_root, path)?,
    );
    if moved
        && (ancestor(path, "HEAD", base)?
            || reachability.landed
            || git::changes_landed(path, "HEAD", base))
    {
        item.status(
            "integrated",
            Some("Use rabi gc to remove landed workspaces".into()),
        );
        return Ok(item);
    }
    common_base(path, "HEAD", base)?;
    let own = checked_output(&["rev-list", &format!("{base}..HEAD"), "--"], path, true)?;
    let specs = fetch_refspecs(path)?;
    let refs = checked_output(
        &["for-each-ref", "--format=%(refname)%09%(upstream)"],
        path,
        false,
    )?;
    let current_ref = format!("refs/heads/{}", tree.branch);
    let observed: Vec<_> = refs
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    let upstream = observed.iter().find_map(|(reference, upstream)| {
        (*reference == current_ref && !upstream.is_empty()).then_some(*upstream)
    });
    let mut unpublished_args = vec!["rev-list", "HEAD", "--not", base, "--remotes"];
    // A configured destination or upstream is publication evidence even in
    // refs/heads/. Only the current branch itself cannot prove publication.
    unpublished_args.extend(observed.iter().filter_map(|(reference, _)| {
        (*reference != current_ref
            && (source_for(reference, &specs).is_some() || Some(*reference) == upstream))
            .then_some(*reference)
    }));
    unpublished_args.push("--");
    let unpublished = checked_output(&unpublished_args, path, true)?;
    let unpublished: BTreeSet<&str> = unpublished.lines().collect();
    let pushed = own.lines().any(|sha| !unpublished.contains(sha));
    let policy = if request.merge {
        "merge"
    } else if request.rebase {
        "rebase"
    } else if request.auto {
        "auto"
    } else {
        context
            .preferences
            .sync_strategy
            .as_deref()
            .unwrap_or("auto")
    };
    let strategy = if policy == "auto" {
        if pushed {
            "merge"
        } else {
            "rebase"
        }
    } else {
        policy
    };
    item.strategy = Some(strategy.into());
    if ancestor(path, base, "HEAD")? {
        return Ok(item);
    }
    if strategy == "rebase"
        && pushed
        && !request.rewrite_published
        && !context.preferences.sync_rewrite_published.unwrap_or(false)
    {
        item.status(
            "refused",
            Some("Published commits require --rewrite-published with rebase".into()),
        );
        item.code = Some("published".into());
        return Ok(item);
    }
    if strategy == "rebase" {
        if let Some(detail) = unsafe_merge(path, base)? {
            item.status("failed", Some(detail));
            return Ok(item);
        }
    }
    if ignored_would_be_overwritten(path, base, strategy == "rebase")? {
        item.status("dirty", Some("Ignored files would be overwritten".into()));
        return Ok(item);
    }
    if request.dry_run {
        item.status("updated", Some(format!("Would {strategy} onto {base_ref}")));
        // Rebased and merged commit IDs do not exist yet. Never invent them.
        item.after = if own.is_empty() {
            commit(path, base)?
        } else {
            None
        };
        return Ok(item);
    }
    let short_base = base
        .strip_prefix("refs/remotes/")
        .or_else(|| base.strip_prefix("refs/heads/"))
        .unwrap_or(base);
    let message = format!("Merge {short_base} into {}", tree.branch);
    let args = if own.is_empty() {
        vec![
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "merge.autoStash=false",
            "merge",
            "--no-squash",
            "--commit",
            "--ff-only",
            "--no-gpg-sign",
            "--no-verify-signatures",
            base,
        ]
    } else if strategy == "merge" {
        vec![
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "merge.autoStash=false",
            "-c",
            "commit.gpgSign=false",
            "merge",
            "--no-squash",
            "--commit",
            "--ff",
            "--no-gpg-sign",
            "--no-verify-signatures",
            "--no-edit",
            "--no-verify",
            "-m",
            &message,
            base,
        ]
    } else {
        vec![
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "rebase.autoStash=false",
            "-c",
            "rebase.autoSquash=false",
            "-c",
            "rebase.rebaseMerges=false",
            "-c",
            "rebase.updateRefs=false",
            "-c",
            "commit.gpgSign=false",
            "rebase",
            "--merge",
            "--reapply-cherry-picks",
            "--keep-empty",
            "--empty=keep",
            "--no-autostash",
            base,
        ]
    };
    let action = format!("rabi sync: fast-forward to {base}");
    let result = if own.is_empty() {
        git::raw_git_with_env(path, &args, &[("GIT_REFLOG_ACTION", &action)])?
    } else {
        git::raw_git(path, &args)?
    };
    // A successful exit alone does not prove integration finished (e.g. --no-commit).
    let mut pending = None;
    for state in ["MERGE_HEAD", "rebase-merge", "rebase-apply"] {
        let state_path = checked_output(&["rev-parse", "--git-path", state], path, true)?;
        if path.join(state_path).exists() {
            pending = Some(if state == "MERGE_HEAD" {
                "merge"
            } else {
                "rebase"
            });
            break;
        }
    }
    let incomplete = result.code == 0 && (pending.is_some() || dirty(path)?);
    if result.code != 0 || incomplete {
        let files = checked_output(
            &["diff", "--name-only", "--diff-filter=U", "-z"],
            path,
            false,
        )?;
        item.conflicts = files
            .split('\0')
            .filter(|file| !file.is_empty())
            .map(str::to_string)
            .collect();
        if let Some(operation) = pending.filter(|_| incomplete || !request.keep_conflicts) {
            let abort = git::raw_git(
                path,
                &["-c", "core.hooksPath=/dev/null", operation, "--abort"],
            )?;
            if abort.code != 0 {
                return Err(Error::msg(format!(
                    "Cannot abort {operation}: {}",
                    abort.stderr
                )));
            }
        }
        if incomplete {
            // Squash can leave an index without operation state to abort. The
            // preflight required a clean tree; reset --merge preserves ignored files.
            if let Some(before) = item.before.as_deref() {
                checked_output(
                    &["-c", "core.hooksPath=/dev/null", "reset", "--merge", before],
                    path,
                    true,
                )?;
            }
            item.after = commit(path, "HEAD")?;
            item.status(
                "failed",
                Some(format!("{strategy} incomplete; workspace restored")),
            );
            return Ok(item);
        }
        item.after = commit(path, "HEAD")?;
        item.status(
            if item.conflicts.is_empty() {
                "failed"
            } else {
                "conflict"
            },
            Some(if item.conflicts.is_empty() {
                short_detail(&result.stderr)
            } else if request.keep_conflicts {
                format!("{strategy} left in progress")
            } else {
                format!("{strategy} aborted; workspace unchanged")
            }),
        );
        return Ok(item);
    }
    item.after = commit(path, "HEAD")?;
    item.rewrote_published = strategy == "rebase" && pushed && item.after != item.before;
    if item.rewrote_published {
        crate::output::workspace_warning("published_rewritten",
            "Published history was rewritten; updating the existing remote branch may require a force push",
            &tree.name);
    }
    if own.is_empty() {
        if let Some(tip) = item.after.as_deref() {
            if let Err(error) = metadata::record_sync_tip(&context.git_root, path, tip) {
                let rollback = match item.before.as_deref() {
                    Some(before) => checked_output(
                        &["-c", "core.hooksPath=/dev/null", "reset", "--keep", before],
                        path,
                        true,
                    )
                    .map(|_| ()),
                    None => Err(Error::msg("Original HEAD is unavailable")),
                };
                item.after = commit(path, "HEAD")?;
                let detail = match rollback {
                    Ok(()) => format!("metadata update failed: {error}; workspace restored"),
                    Err(rollback) => {
                        format!("metadata update failed: {error}; rollback failed: {rollback}")
                    }
                };
                item.status("failed", Some(detail));
                return Ok(item);
            }
        }
    }
    item.status(
        if item.after == item.before {
            "up-to-date"
        } else {
            "updated"
        },
        None,
    );
    Ok(item)
}

fn strategy_source(context: &Context, request: &Request) -> &'static str {
    if request.auto || request.rebase || request.merge {
        "flag"
    } else if context.preferences.sync_strategy.is_some() {
        "config"
    } else {
        "default"
    }
}
