use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Serialize;

use crate::config;
use crate::context::Context;
use crate::error::{Error, Result};
use crate::git::{self, GitWorktree};
use crate::metadata;
use crate::output::{errln, outln};
use crate::paths;
use crate::process;
use crate::trust::{self, HookControl};

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub repo_root: PathBuf,
    pub managed: bool,
    /// A record exists, independently of whether this directory is managed.
    pub recorded: bool,
    pub name: String,
    pub path: String,
    /// Original Git registration path, before resolving the checkout itself.
    pub(crate) registered_path: PathBuf,
    pub path_to_display: String,
    pub branch: String,
    pub is_clean: bool,
    pub head: Option<String>,
    pub is_default: bool,
    pub detached: bool,
    pub orphaned: bool,
    pub metadata: metadata::WorktreeMetadata,
}

pub struct Logger {
    pub enabled: bool,
    /// When set, progress lines go to stderr so stdout can stay a JSON document.
    pub json: bool,
}

impl Logger {
    pub fn on() -> Self {
        Self {
            enabled: true,
            json: false,
        }
    }

    pub fn json() -> Self {
        Self {
            enabled: true,
            json: true,
        }
    }

    pub(crate) fn log(&self, message: &str) {
        if self.enabled {
            if self.json {
                errln!("{message}");
            } else {
                outln!("{message}");
            }
        }
    }

    pub(crate) fn warn(&self, code: &str, message: &str) {
        if self.enabled {
            crate::output::warning(code, message);
        }
    }
}

pub fn validate_worktree_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(Error::Validation(
            "Workspace name cannot be empty".to_string(),
        ));
    }
    if name == "." {
        return Err(Error::validation(
            "Workspace name cannot be the reserved selector '.'",
        ));
    }
    if Path::new(name).is_absolute() {
        return Err(Error::validation("Workspace name must be a relative name"));
    }
    if !name
        .chars()
        .all(|char| char.is_ascii_alphanumeric() || matches!(char, '-' | '_' | '.' | '/'))
    {
        return Err(Error::Validation(
            "Workspace name can only contain letters, numbers, hyphens, underscores, dots, and slashes"
                .to_string(),
        ));
    }
    if name.contains("..") {
        return Err(Error::Validation(
            "Workspace name cannot contain consecutive dots".to_string(),
        ));
    }
    Ok(())
}

pub fn list_worktrees(
    context: &Context,
    exclude_default: bool,
    include_prunable: bool,
) -> Result<Vec<WorktreeInfo>> {
    list_worktrees_with_status(context, exclude_default, include_prunable, true)
}

/// Resolve committed-work inspection targets without reading checkout state.
pub(crate) fn list_worktrees_without_status(context: &Context) -> Result<Vec<WorktreeInfo>> {
    list_worktrees_with_status(context, false, true, false)
}

fn list_worktrees_with_status(
    context: &Context,
    exclude_default: bool,
    include_prunable: bool,
    inspect_status: bool,
) -> Result<Vec<WorktreeInfo>> {
    let git_root_string = context.git_root.to_string_lossy().to_string();
    let stored = metadata::load(&context.git_root).unwrap_or_default();
    let mut infos = Vec::new();
    let common_dir = git::git_common_dir(&context.git_root)?;
    let canonical_common = git::canonical_common_dir(&context.git_root)?;
    let listed = git::list_worktrees(&context.git_root)
        .map_err(|error| Error::msg(format!("Failed to list workspaces: {error}")))?;
    for mut worktree in listed.iter().cloned() {
        worktree.path = main_checkout_path(&worktree.path, &context.git_root, &common_dir);
        if !include_prunable && worktree.prunable {
            continue;
        }
        if exclude_default && worktree.path == git_root_string {
            continue;
        }
        let found = metadata::find_in(&stored, Path::new(&worktree.path));
        let recorded_path = found.map(|(key, _)| PathBuf::from(key.as_str()));
        let record = found.map(|(_, record)| record.clone()).unwrap_or_default();
        infos.push(info_from_git(
            &worktree,
            context,
            record,
            recorded_path.as_deref(),
            &canonical_common,
            inspect_status,
        ));
    }
    for (key, record) in &stored {
        let path = Path::new(key);
        if !crate::gc::orphaned_record(path, &listed)
            || paths::same_location(path, &context.git_root)
        {
            continue;
        }
        infos.push(orphaned_info(context, path, record, &canonical_common));
    }

    Ok(infos)
}

pub(crate) fn orphaned_info(
    context: &Context,
    path: &Path,
    record: &metadata::WorktreeMetadata,
    canonical_common: &Path,
) -> WorktreeInfo {
    WorktreeInfo {
        repo_root: context.git_root.clone(),
        managed: true,
        recorded: true,
        name: resolve_worktree_name(
            path,
            &context.worktrees_directory,
            Some(path),
            record.name.as_deref(),
            canonical_common,
            None,
            "HEAD",
        ),
        path: path.to_string_lossy().into_owned(),
        registered_path: path.to_path_buf(),
        path_to_display: paths::list_path(path),
        branch: "(detached HEAD)".to_string(),
        is_clean: true,
        head: None,
        is_default: false,
        detached: false,
        orphaned: true,
        metadata: record.clone(),
    }
}

/// `git worktree list` reports the git dir as the main worktree for a separate
/// git dir and for a submodule. The main checkout is `git_root`.
fn main_checkout_path(path: &str, git_root: &Path, common_dir: &Path) -> String {
    let listed = paths::normalize(Path::new(path));
    let root = paths::normalize(git_root);
    if listed == paths::normalize(common_dir) && listed != root {
        root.to_string_lossy().into_owned()
    } else {
        path.to_string()
    }
}

fn info_from_git(
    worktree: &GitWorktree,
    context: &Context,
    metadata: metadata::WorktreeMetadata,
    recorded_path: Option<&Path>,
    common_dir: &Path,
    inspect_status: bool,
) -> WorktreeInfo {
    let short_head = worktree
        .head
        .as_deref()
        .map(|head| head.chars().take(7).collect::<String>())
        .unwrap_or_else(|| "HEAD".to_string());
    let branch = match worktree.branch.as_deref() {
        Some(branch) if branch != "(detached HEAD)" => branch.to_string(),
        _ => "(detached HEAD)".to_string(),
    };
    let detached = branch == "(detached HEAD)";
    let original = PathBuf::from(&worktree.path);
    let path = paths::canonical_location(&original);
    let name = resolve_worktree_name(
        &original,
        &context.worktrees_directory,
        recorded_path,
        metadata.name.as_deref(),
        common_dir,
        worktree.branch.as_deref(),
        &short_head,
    );
    WorktreeInfo {
        repo_root: context.git_root.clone(),
        managed: is_managed(context, &path, recorded_path.is_some()),
        recorded: recorded_path.is_some(),
        name,
        path: path.to_string_lossy().into_owned(),
        registered_path: original.clone(),
        path_to_display: paths::list_path(&path),
        branch,
        // Status-free callers use only identity/metadata, never this field.
        is_clean: inspect_status && path.is_dir() && git::is_clean(&path),
        head: recorded_head(worktree.head.as_deref()),
        is_default: paths::same_location(&original, &context.git_root),
        detached,
        orphaned: false,
        metadata,
    }
}

pub(crate) fn is_managed(context: &Context, path: &Path, recorded: bool) -> bool {
    !paths::same_location(path, &context.git_root)
        && (recorded || managed_directory(context, path).is_some())
}

/// The configured directory is the only layout managed without a record.
pub(crate) fn managed_directory(context: &Context, path: &Path) -> Option<PathBuf> {
    paths::is_strictly_inside(path, &context.worktrees_directory)
        .then(|| context.worktrees_directory.clone())
}

/// Git reports an unborn HEAD as an all-zero SHA. That is not a commit.
fn recorded_head(head: Option<&str>) -> Option<String> {
    head.filter(|sha| !sha.is_empty() && sha.bytes().any(|byte| byte != b'0'))
        .map(ToOwned::to_owned)
}

pub(crate) fn resolve_worktree_name(
    path: &Path,
    worktrees_directory: &Path,
    recorded_path: Option<&Path>,
    stored_name: Option<&str>,
    common_dir: &Path,
    branch: Option<&str>,
    short_head: &str,
) -> String {
    if let Some(name) = stored_name.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_string();
    }
    if let Some(name) = directory_name(path, worktrees_directory) {
        return name;
    }
    if let Some(recorded) = recorded_path {
        return name_from_recorded_path(recorded, common_dir);
    }
    workspace_layout_name(path, common_dir, true)
        .or_else(|| branch_or_short_hash(branch, short_head))
        .unwrap_or_else(|| short_head.to_string())
}

fn branch_or_short_hash(branch: Option<&str>, short_head: &str) -> Option<String> {
    match branch {
        Some(name) if !name.is_empty() && name != "(detached HEAD)" => Some(name.to_string()),
        _ if !short_head.is_empty() && short_head != "HEAD" => Some(short_head.to_string()),
        _ => None,
    }
}

pub(crate) fn outside_record_name(
    branch: Option<&str>,
    short_head: &str,
    basename: &str,
    name_taken: impl Fn(&str) -> bool,
) -> Result<String> {
    let preferred =
        branch_or_short_hash(branch, short_head).unwrap_or_else(|| basename.to_string());
    if !name_taken(&preferred) {
        return Ok(preferred);
    }
    if preferred != basename && name_taken(basename) {
        return Err(Error::msg(format!(
            "Workspace name '{preferred}' is already used, and the directory name '{basename}' is also already used. Refusing to record this outside workspace."
        )));
    }
    Err(Error::msg(format!(
        "Workspace name '{preferred}' is already used. Refusing to record this outside workspace."
    )))
}

/// Name recorded under the current workspace directory layout.
///
/// The default directory is `rabi/workspaces/<checkout>-<hash>/<name>`.
/// A configured `rabi/workspaces/<name>` directory has no hash segment.
fn name_from_recorded_path(path: &Path, common_dir: &Path) -> String {
    workspace_layout_name(path, common_dir, false).unwrap_or_else(|| basename_of(path))
}

fn workspace_layout_name(path: &Path, common_dir: &Path, project_only: bool) -> Option<String> {
    let normalized = paths::normalize(path);
    let components = normalized
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect::<Vec<_>>();
    let index = components
        .windows(2)
        .rposition(|pair| pair[0] == "rabi" && pair[1] == "workspaces")?;
    let rest = &components[index + 2..];
    let (project, name) = rest.split_first()?;
    if !name.is_empty() && paths::is_project_directory(project, common_dir) {
        return Some(join_components(name));
    }
    if project_only || rest.is_empty() {
        None
    } else {
        Some(join_components(rest))
    }
}

fn basename_of(path: &Path) -> String {
    let normalized = paths::normalize(path);
    normalized
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| normalized.display().to_string())
}

/// Name to save on a first record outside the configured workspace directory.
///
/// `Ok(None)` leaves the name unset. A layout path keeps its folder name.
/// `Err` refuses the write.
pub(crate) fn outside_name_to_store(context: &Context, path: &Path) -> Result<Option<String>> {
    if paths::is_strictly_inside(path, &context.worktrees_directory) {
        return Ok(None);
    }
    if metadata::records_path(&context.git_root, path)? {
        return Ok(None);
    }
    let common_dir = git::canonical_common_dir(&context.git_root)?;
    let listed = list_worktrees(context, false, true)?;
    let Some(worktree) = listed
        .iter()
        .find(|item| paths::same_location(Path::new(&item.path), path))
    else {
        return Ok(None);
    };
    let short_head = worktree
        .head
        .as_deref()
        .map(|head| head.chars().take(7).collect::<String>())
        .unwrap_or_else(|| "HEAD".to_string());
    let branch = (!worktree.detached).then_some(worktree.branch.as_str());
    let taken = |name: &str| {
        listed.iter().any(|other| {
            !other.orphaned
                && other.name == name
                && !paths::same_location(Path::new(&other.path), path)
        })
    };
    if let Some(layout) = workspace_layout_name(path, &common_dir, false) {
        if taken(&layout) {
            return Err(Error::msg(format!(
                "Workspace name '{layout}' is already used. Refusing to record this outside workspace."
            )));
        }
        return Ok(None);
    }
    let basename = basename_of(path);
    let chosen = outside_record_name(branch, &short_head, &basename, taken)?;
    Ok(branch_or_short_hash(branch, &short_head).map(|_| chosen))
}

fn join_components(components: &[std::ffi::OsString]) -> String {
    components
        .iter()
        .map(|component| component.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn directory_name(path: &Path, worktrees_directory: &Path) -> Option<String> {
    if !paths::is_strictly_inside(path, worktrees_directory) {
        return None;
    }
    let relative = paths::relative(
        &paths::canonical_location(worktrees_directory),
        &paths::canonical_location(path),
    );
    if relative.is_empty() {
        None
    } else {
        Some(relative)
    }
}

fn same_worktree(worktree: &WorktreeInfo, name: &str) -> bool {
    worktree.name == name || (worktree.branch != "(detached HEAD)" && worktree.branch == name)
}

pub(crate) fn select_worktree(
    mut worktrees: Vec<WorktreeInfo>,
    name: &str,
) -> Result<WorktreeInfo> {
    if worktrees
        .iter()
        .any(|worktree| !worktree.orphaned && same_worktree(worktree, name))
    {
        worktrees.retain(|worktree| !worktree.orphaned);
    }
    let directories = worktrees
        .iter()
        .filter(|worktree| worktree.name == name)
        .cloned()
        .collect::<Vec<_>>();
    match directories.as_slice() {
        [found] => return Ok(found.clone()),
        [] => {}
        _ => {
            return Err(Error::Status {
                code: "ambiguous",
                message: format!("More than one workspace is named '{name}'."),
            });
        }
    }
    let branches = worktrees
        .into_iter()
        .filter(|worktree| worktree.branch != "(detached HEAD)" && worktree.branch == name)
        .collect::<Vec<_>>();
    match branches.as_slice() {
        [found] => Ok(found.clone()),
        [] => Err(Error::WorktreeNotFound(name.to_string())),
        _ => Err(Error::Status {
            code: "ambiguous",
            message: format!(
            "Branch '{name}' matches more than one workspace. Use the workspace directory name."
        ),
        }),
    }
}

fn invalid_checkout(tree: &WorktreeInfo, action: &str, reason: &str) -> Error {
    Error::msg(format!(
        "Cannot {action} workspace '{}': {reason}. Check its .git entry and repair or remove the stale Git worktree registration before retrying.",
        crate::output::visible_text(&tree.name)
    ))
}

/// Parent directory symlinks are valid; the registered checkout itself must not be a symlink.
pub(crate) fn validate_registered_path(tree: &WorktreeInfo, action: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(&tree.registered_path).map_err(|error| {
        invalid_checkout(
            tree,
            action,
            &format!("failed to inspect checkout ({error})"),
        )
    })?;
    if metadata.file_type().is_symlink() {
        return Err(invalid_checkout(
            tree,
            action,
            "registered checkout path is a symlink",
        ));
    }
    Ok(())
}

/// Validate the checkout against Git's independent registration before mutations.
pub(crate) fn validate_checkout(
    context: &Context,
    tree: &WorktreeInfo,
    action: &str,
) -> Result<()> {
    validate_registered_path(tree, action)?;
    let invalid = |reason: &str| invalid_checkout(tree, action, reason);
    let path = &tree.registered_path;
    let observed = git::top_level(path)
        .map_err(|error| invalid(&format!("failed to inspect checkout ({error})")))?;
    if !paths::same_location(path, &observed) {
        return Err(invalid(
            "checkout top-level does not match its registered path",
        ));
    }
    let common_dir = git::canonical_common_dir(path)
        .map_err(|error| invalid(&format!("failed to inspect checkout ({error})")))?;
    if common_dir != git::canonical_common_dir(&context.git_root)? {
        return Err(invalid("checkout does not belong to this repository"));
    }
    if !git::matches_registered_git_dir(&context.git_root, path)
        .map_err(|error| invalid(&format!("failed to inspect checkout ({error})")))?
    {
        return Err(invalid(
            "checkout Git directory does not match its registered Git directory",
        ));
    }
    Ok(())
}

/// The worktree Git has registered at `path`, independent of its branch or directory name.
pub fn worktree_at_path(context: &Context, path: &Path) -> Result<WorktreeInfo> {
    list_worktrees(context, false, true)?
        .into_iter()
        .find(|worktree| {
            !worktree.orphaned && paths::same_location(Path::new(&worktree.path), path)
        })
        .ok_or_else(|| {
            Error::msg(format!(
                "Workspace at {} is no longer registered",
                path.display()
            ))
        })
}

/// Look up an observed name or Git branch literally, without parsing user selectors.
pub fn find_worktree(
    context: &Context,
    name: &str,
    exclude_default: bool,
    expected_path: Option<&Path>,
) -> Result<WorktreeInfo> {
    let found = find_including_orphaned(context, name, exclude_default, expected_path)?;
    require_live(found, name)
}

pub(crate) fn require_live(found: WorktreeInfo, name: &str) -> Result<WorktreeInfo> {
    if found.orphaned {
        return Err(Error::msg(format!(
            "Workspace '{name}' is orphaned: its directory is gone. Use rabi remove to remove the stale record."
        )));
    }
    Ok(found)
}

/// Literal internal lookup, including stale records; no input validation.
/// Only workspace names match; branch aliases are excluded.
pub(crate) fn find_named_including_orphaned(context: &Context, name: &str) -> Result<WorktreeInfo> {
    select_worktree(
        list_worktrees(context, false, true)?
            .into_iter()
            .filter(|tree| tree.name == name)
            .collect(),
        name,
    )
}

/// Literal internal lookup, including stale records; no input validation.
pub fn find_including_orphaned(
    context: &Context,
    name: &str,
    exclude_default: bool,
    expected_path: Option<&Path>,
) -> Result<WorktreeInfo> {
    let listed = list_worktrees(context, false, true)?;
    let live_match = listed
        .iter()
        .any(|worktree| !worktree.orphaned && same_worktree(worktree, name));
    let worktrees = listed
        .into_iter()
        .filter(|worktree| !(exclude_default && worktree.is_default))
        .filter(|worktree| !live_match || !worktree.orphaned)
        .filter(|worktree| {
            expected_path.is_none_or(|path| paths::same_location(Path::new(&worktree.path), path))
        })
        .collect();
    select_worktree(worktrees, name)
}

pub fn ensure_absent(context: &Context, name: &str) -> Result<()> {
    if list_worktrees(context, false, true)?
        .iter()
        .any(|worktree| !worktree.orphaned && same_worktree(worktree, name))
    {
        Err(Error::WorktreeAlreadyExists(name.to_string()))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct HookFailure {
    pub command: String,
    pub code: i32,
    pub signal: Option<String>,
    pub error: Option<String>,
}

impl HookFailure {
    pub fn warning(&self, path: &Path) -> String {
        let reason = if let Some(signal) = &self.signal {
            format!("terminated by signal {signal} (exit code {})", self.code)
        } else if let Some(error) = &self.error {
            format!("failed to start: {error}")
        } else {
            format!("failed with exit code {}", self.code)
        };
        format!(
            "Warning: Setup command {reason}: {}\nWorkspace path: {}",
            crate::output::visible_text(&self.command),
            crate::output::visible_text(&paths::canonical_location(path).to_string_lossy())
        )
    }
}

pub struct RemoveOptions {
    pub discard_changes: bool,
    pub keep_branch: bool,
    pub delete_unpreserved_branch: bool,
}

pub struct RemoveOutcome {
    pub message: String,
    pub worktree: WorktreeInfo,
    pub branch_deleted: bool,
    pub branch_error: Option<String>,
    pub branch_kept_reason: Option<String>,
    pub preservation_reason: Option<String>,
    /// Set when Git already removed the worktree and the metadata record could not.
    pub metadata_error: Option<String>,
}

pub struct RemoveError {
    pub error: Error,
    pub worktree: Option<Box<WorktreeInfo>>,
    pub changed: bool,
}

impl RemoveError {
    fn bare(error: Error) -> Self {
        Self {
            error,
            worktree: None,
            changed: false,
        }
    }

    fn kept(error: Error, worktree: &WorktreeInfo) -> Self {
        Self {
            error,
            worktree: Some(Box::new(worktree.clone())),
            changed: false,
        }
    }
}

pub struct RemovePlan {
    name: String,
    selector: crate::selectors::Selector,
    pub worktree: WorktreeInfo,
    path: PathBuf,
    inside: bool,
    changed: usize,
    pub branch_kept_reason: Option<String>,
    pub preservation_reason: Option<String>,
}

pub(crate) fn preserved(context: &Context, worktree: &WorktreeInfo) -> bool {
    preservation_reason(context, worktree).is_some()
}

fn preservation_reason(context: &Context, worktree: &WorktreeInfo) -> Option<String> {
    let fallback = crate::sync::fallback_base(&context.git_root).ok();
    if git::retirement_preserved(
        &context.git_root,
        (!worktree.detached).then_some(worktree.branch.as_str()),
        worktree.head.as_deref(),
        worktree.metadata.base.as_deref().or(fallback.as_deref()),
    ) {
        return Some("local".to_string());
    }
    (!worktree.detached
        && context.retirement_queries.borrow_mut().merged_contains(
            &context.git_root,
            &worktree.branch,
            worktree.metadata.links.pr.as_ref(),
        ))
    .then(|| "merged_pr".to_string())
}

pub fn preflight_remove(
    context: &Context,
    name: &str,
    options: &RemoveOptions,
    expected_path: Option<&Path>,
) -> std::result::Result<RemovePlan, RemoveError> {
    let selector = crate::selectors::Selector::parse(name).map_err(RemoveError::bare)?;
    preflight_remove_selected(context, &selector, options, expected_path)
}

/// Garbage collection already obtained this name from Git or metadata.
pub(crate) fn preflight_remove_named(
    context: &Context,
    name: &str,
    options: &RemoveOptions,
    expected_path: Option<&Path>,
) -> std::result::Result<RemovePlan, RemoveError> {
    preflight_remove_selected(
        context,
        &crate::selectors::Selector::Name(name.to_owned()),
        options,
        expected_path,
    )
}

fn preflight_remove_selected(
    context: &Context,
    selector: &crate::selectors::Selector,
    options: &RemoveOptions,
    expected_path: Option<&Path>,
) -> std::result::Result<RemovePlan, RemoveError> {
    let name = selector.as_str();
    let force = options.discard_changes;
    let mut worktree = selector
        .resolve_including_orphaned(context, false, expected_path)
        .map_err(RemoveError::bare)?;
    if worktree.is_default {
        return Err(RemoveError::kept(
            Error::Status {
                code: "primary",
                message: "The primary checkout cannot be removed".into(),
            },
            &worktree,
        ));
    }
    if std::fs::symlink_metadata(&worktree.path)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    {
        worktree.orphaned = true;
    }
    let label = if name == "." {
        worktree.name.as_str()
    } else {
        name
    };
    let path = PathBuf::from(&worktree.path);
    // Freeze location selectors while cwd still exists. Rechecks must not
    // reinterpret dot or relative paths after another target removes cwd.
    let selector = match selector {
        crate::selectors::Selector::Path(_) => {
            crate::selectors::Selector::Path(worktree.path.clone())
        }
        crate::selectors::Selector::Special(input) if input == "." => {
            crate::selectors::Selector::Path(worktree.path.clone())
        }
        _ => selector.clone(),
    };
    let inside = managed_directory(context, &path).is_some();
    let recorded = metadata::records_path(&context.git_root, &path)
        .map_err(|error| RemoveError::kept(error, &worktree))?;
    if !inside && !recorded {
        return Err(RemoveError::kept(
            Error::msg(format!(
                "Workspace '{label}' was not created by rabi (for example, it was made by plain git or an older tool). Remove it with `git worktree remove {}`.",
                path.display()
            )),
            &worktree,
        ));
    }
    if worktree.orphaned {
        check_detached_preserved(context, &worktree, options, label)?;
        return Ok(RemovePlan {
            name: name.to_string(),
            selector: selector.clone(),
            worktree,
            path,
            inside,
            changed: 0,
            branch_kept_reason: None,
            preservation_reason: None,
        });
    }
    if !force
        && git::has_removal_submodules(&path)
            .map_err(|error| RemoveError::kept(error, &worktree))?
    {
        return Err(RemoveError::kept(
            Error::Status {
                code: "dirty",
                message: format!("Workspace '{label}': {}", git::SUBMODULE_REMOVAL_DETAIL),
            },
            &worktree,
        ));
    }
    let changed = git::status_count(&path).map_err(|error| RemoveError::kept(error, &worktree))?;
    if changed > 0 && !force {
        return Err(RemoveError::kept(
            Error::Uncommitted {
                name: label.to_string(),
                files: changed,
            },
            &worktree,
        ));
    }
    let preservation_reason = preservation_reason(context, &worktree);
    let preserved = preservation_reason.is_some();
    check_detached_preserved(context, &worktree, options, label)?;
    let branch_kept_reason = if worktree.detached {
        None
    } else if options.keep_branch {
        Some("keep-branch".to_string())
    } else if !preserved && !options.delete_unpreserved_branch {
        Some("unpreserved".to_string())
    } else {
        None
    };
    Ok(RemovePlan {
        name: name.to_string(),
        selector: selector.clone(),
        worktree,
        path,
        inside,
        changed,
        branch_kept_reason,
        preservation_reason,
    })
}

fn check_detached_preserved(
    context: &Context,
    worktree: &WorktreeInfo,
    options: &RemoveOptions,
    label: &str,
) -> std::result::Result<(), RemoveError> {
    if worktree.detached && !options.delete_unpreserved_branch && !preserved(context, worktree) {
        return Err(RemoveError::kept(
            Error::Status { code: "unpreserved", message: format!(
                "Workspace '{label}' has unpreserved detached commits. Use --delete-unpreserved-branch to remove it."
            ) }, worktree));
    }
    Ok(())
}

pub fn commit_remove(
    context: &Context,
    plan: RemovePlan,
    options: &RemoveOptions,
    logger: &Logger,
    hooks: HookControl,
) -> std::result::Result<RemoveOutcome, RemoveError> {
    commit_remove_checked(context, plan, options, logger, hooks, || Ok(()))
}

pub(crate) fn commit_remove_checked(
    context: &Context,
    plan: RemovePlan,
    options: &RemoveOptions,
    logger: &Logger,
    hooks: HookControl,
    check: impl FnOnce() -> Result<()>,
) -> std::result::Result<RemoveOutcome, RemoveError> {
    let RemovePlan {
        name,
        selector,
        worktree,
        path,
        ..
    } = plan;
    let force = options.discard_changes;
    let commands = if worktree.orphaned {
        Vec::new()
    } else {
        trust::authorize(
            &context.git_root,
            config::setup_commands(context.config.as_ref()),
            config::pre_remove_commands(context.config.as_ref()),
            trust::HookKind::PreRemove,
            hooks,
        )
        .map_err(|error| RemoveError::kept(error, &worktree))?
    };
    metadata::prepare(&context.git_root).map_err(|error| RemoveError::kept(error, &worktree))?;
    if !commands.is_empty() {
        logger.log("\nRunning pre-remove commands...");
        if let Err(error) = run_hook_commands(context, &worktree, &commands, false, logger) {
            return Err(RemoveError::kept(error, &worktree));
        }
    }
    check().map_err(|error| RemoveError::kept(error, &worktree))?;
    if worktree.orphaned {
        let listed = git::list_worktrees(&context.git_root)
            .map_err(|error| RemoveError::kept(error, &worktree))?;
        let registered = listed
            .iter()
            .find(|tree| paths::same_location(Path::new(&tree.path), &path));
        let missing = std::fs::symlink_metadata(&path)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
        let changed_registration = registered.is_some_and(|tree| {
            tree.head != worktree.head || tree.branch.as_deref() != Some(worktree.branch.as_str())
        });
        if !missing || changed_registration {
            return Err(RemoveError::kept(
                Error::msg(format!(
                    "Workspace '{name}' is no longer orphaned; refusing to remove its record."
                )),
                &worktree,
            ));
        }
        let recorded_path = metadata::matching_path(&context.git_root, &path)
            .map_err(|error| RemoveError::kept(error, &worktree))?;
        if registered.is_some() {
            // Losing a directory does not preserve the registered detached HEAD.
            // Recheck immediately before removing its last Git registration.
            check_detached_preserved(context, &worktree, options, &name)?;
            git::remove_worktree(&context.git_root, &path, true)
                .map_err(|error| RemoveError::kept(error, &worktree))?;
        }
        if let Some(recorded_path) = recorded_path {
            metadata::remove(&context.git_root, Path::new(&recorded_path)).map_err(|error| {
                let mut failure = RemoveError::kept(error, &worktree);
                failure.changed = registered.is_some();
                failure
            })?;
        }
        return Ok(RemoveOutcome {
            message: format!("Workspace '{name}' was already gone; removed only the stale record."),
            worktree,
            branch_deleted: false,
            branch_error: None,
            branch_kept_reason: None,
            preservation_reason: None,
            metadata_error: None,
        });
    }
    let fresh = preflight_remove_selected(context, &selector, options, Some(&path))?;
    if fresh.worktree.orphaned {
        // A hook may delete or move the checkout. The orphan plan does not
        // authorize branch deletion; leave its registration and record for a
        // later orphan cleanup instead of continuing with normal removal.
        return Err(RemoveError::kept(
            Error::msg("Workspace directory disappeared while running pre-remove commands; refusing further removal"),
            &fresh.worktree,
        ));
    }
    if fresh.worktree.head != worktree.head || fresh.worktree.branch != worktree.branch {
        return Err(RemoveError::kept(
            Error::msg("Workspace changed while running pre-remove commands"),
            &fresh.worktree,
        ));
    }
    let RemovePlan {
        worktree,
        path,
        inside,
        changed,
        branch_kept_reason,
        preservation_reason,
        ..
    } = fresh;
    let recorded_path = metadata::matching_path(&context.git_root, &path)
        .map_err(|error| RemoveError::kept(error, &worktree))?;
    if let Err(error) = git::remove_worktree(&context.git_root, &path, force) {
        return Err(RemoveError::kept(error, &worktree));
    }
    // An unrecorded target has no metadata cleanup to retry once Git removes
    // it. Reading an unrelated broken store here would invent unfinished work.
    let metadata_error = recorded_path
        .as_deref()
        .map(|path| metadata::remove(&context.git_root, Path::new(path)))
        .transpose()
        .err()
        .map(|error| error.to_string());
    let cleanup_warning = if inside {
        managed_directory(context, &path).and_then(|root| remove_empty_parents(&path, &root))
    } else {
        None
    };
    let name = if name == "." {
        worktree.name.as_str()
    } else {
        &name
    };
    let (message, branch_deleted, branch_error) = if worktree.branch == "(detached HEAD)" {
        (format!("Deleted workspace '{name}'"), false, None)
    } else if branch_kept_reason.is_some() {
        (
            format!(
                "Deleted workspace '{name}' and kept its branch '{}'",
                worktree.branch
            ),
            false,
            None,
        )
    } else {
        match git::delete_branch(&context.git_root, &worktree.branch) {
            Ok(()) => (
                format!(
                    "Deleted workspace '{name}' and its branch '{}'",
                    worktree.branch
                ),
                true,
                None,
            ),
            Err(error) => {
                let branch_error = error.to_string();
                (
                    format!(
                        "Deleted workspace '{name}' and kept its branch '{}'",
                        worktree.branch
                    ),
                    false,
                    Some(branch_error),
                )
            }
        }
    };
    let message = if preservation_reason.as_deref() == Some("merged_pr") {
        format!("{message} (preserved by merged PR)")
    } else {
        message
    };
    let message = if changed > 0 {
        format!("Warning: Workspace '{name}' had uncommitted changes ({changed} files)\n{message}")
    } else {
        message
    };
    let message = if let Some(warning) = cleanup_warning {
        format!("{message}\nWarning: {warning}")
    } else {
        message
    };
    Ok(RemoveOutcome {
        message,
        worktree,
        branch_deleted,
        branch_error,
        branch_kept_reason,
        preservation_reason,
        metadata_error,
    })
}

pub fn current_worktree_name(context: &Context) -> Result<Option<String>> {
    let cwd = std::env::current_dir().map_err(|error| Error::msg(error.to_string()))?;
    let top = match git::top_level(&cwd) {
        Ok(top) => top,
        Err(_) => return Ok(None),
    };
    if paths::same_location(&top, &context.git_root) {
        return Ok(None);
    }
    Ok(list_worktrees(context, true, true)?
        .into_iter()
        .find_map(|worktree| {
            paths::same_location(Path::new(&worktree.path), &top).then_some(worktree.name)
        }))
}

pub fn exec_in_worktree(
    context: &Context,
    name: &str,
    command: &str,
    args: &[&str],
    no_env: bool,
) -> Result<i32> {
    if let Some(warning) = metadata::read_problem(&context.git_root) {
        crate::output::warning("metadata_read", format!("Warning: {warning}"));
    }
    let worktree = crate::selectors::resolve(context, name, false, None)?;
    let environment = if no_env {
        Default::default()
    } else {
        let declaration = crate::execution_env::load(&context.git_root)?;
        if declaration.is_empty() {
            Default::default()
        } else if crate::trust::trust_all() || crate::trust::is_trusted_root(&context.git_root)? {
            declaration.for_workspace(&context.git_root, Path::new(&worktree.path))?
        } else {
            crate::output::warning("env_untrusted", "Repository env was skipped because the repository is untrusted. Run `rabi trust add` to enable it or use `--no-env` to silence this warning.");
            Default::default()
        }
    };
    let path = Path::new(&worktree.path);
    let owned = workspace_env(context, &worktree.name, path);
    let pairs = environment
        .values
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_os_str()))
        .chain(
            owned
                .iter()
                .map(|(key, value)| (key.as_str(), std::ffi::OsStr::new(value))),
        )
        .collect::<Vec<_>>();
    process::inherit_os(command, args, Some(path), &pairs, false)
}

fn exec_in_resolved_worktree(
    context: &Context,
    worktree: &WorktreeInfo,
    command: &str,
    args: &[&str],
    env: &[(&str, &str)],
    redirect_stdout_to_stderr: bool,
) -> Result<i32> {
    let path = PathBuf::from(&worktree.path);
    let owned = workspace_env(context, &worktree.name, &path);
    let mut pairs = owned
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    for (key, value) in env {
        if let Some(existing) = pairs.iter_mut().find(|(name, _)| *name == *key) {
            existing.1 = value;
        } else {
            pairs.push((key, value));
        }
    }
    process::inherit(
        command,
        args,
        Some(&path),
        &pairs,
        redirect_stdout_to_stderr,
    )
}

fn workspace_env(context: &Context, name: &str, path: &Path) -> Vec<(String, String)> {
    let stored = metadata::load(&context.git_root).unwrap_or_default();
    let id = metadata::find_in(&stored, path).and_then(|(_, record)| record.id.as_deref());
    rabi_env(name, path, id, &context.git_root)
}

pub fn rabi_env(name: &str, path: &Path, id: Option<&str>, repo: &Path) -> Vec<(String, String)> {
    vec![
        ("RABI".to_string(), "1".to_string()),
        ("RABI_WORKSPACE".to_string(), name.to_string()),
        (
            "RABI_WORKSPACE_ID".to_string(),
            id.unwrap_or_default().to_string(),
        ),
        (
            "RABI_WORKSPACE_PATH".to_string(),
            path.display().to_string(),
        ),
        ("RABI_REPO_PATH".to_string(), repo.display().to_string()),
    ]
}

fn hook_failure_from_start(command: &str, error: &Error) -> HookFailure {
    HookFailure {
        command: command.to_string(),
        code: error.child_exit_code(),
        signal: match error {
            Error::Signaled(signal) => Some(signal.clone()),
            _ => None,
        },
        error: match error {
            Error::Spawn { .. } => Some(error.to_string()),
            _ => None,
        },
    }
}

pub(crate) fn run_hook_commands(
    context: &Context,
    worktree: &WorktreeInfo,
    commands: &[String],
    post: bool,
    logger: &Logger,
) -> Result<Option<HookFailure>> {
    let _hook_scope = crate::runtime::hook_scope();
    let path = Path::new(&worktree.path);
    let original_dir = fs::metadata(path).map_err(|error| Error::msg(error.to_string()))?;
    let mut expected_id = worktree.metadata.id.clone();
    let mut expected_recorded = worktree.recorded;
    for command in commands {
        if post {
            logger.log(&format!("Executing: {command}"));
        } else {
            logger.log(&format!("Executing pre-remove command: {command}"));
        }
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        let mut run = || {
            // A link may move during an earlier hook. Keep the preflight target,
            // and refuse a removed or replaced checkout before running another command.
            let current = worktree_at_path(context, path)?;
            let dir = fs::metadata(path).map_err(|error| Error::msg(error.to_string()))?;
            if (expected_recorded && current.metadata.id != expected_id)
                || dir.dev() != original_dir.dev()
                || dir.ino() != original_dir.ino()
                || !paths::same_location(
                    &git::git_common_dir(path)?,
                    &git::git_common_dir(&context.git_root)?,
                )
            {
                return Err(Error::msg("Workspace changed before running hook command"));
            }
            // An unrecorded checkout can be recorded by a hook. Pin its new ID
            // so later hooks still reject replacement of that record.
            expected_id = current.metadata.id;
            expected_recorded = current.recorded;
            exec_in_resolved_worktree(
                context,
                worktree,
                &shell,
                &["-c", command],
                &[],
                logger.json,
            )
        };
        let code = match run() {
            Ok(code) => code,
            Err(error) => {
                if post {
                    return Ok(Some(hook_failure_from_start(command, &error)));
                }
                return Err(error);
            }
        };
        if code != 0 {
            if post {
                return Ok(Some(HookFailure {
                    command: command.clone(),
                    code,
                    signal: None,
                    error: None,
                }));
            }
            return Err(Error::msg(format!(
                "Pre-remove command failed with exit code {code}: {command}"
            )));
        }
    }
    Ok(None)
}

// Keep the project directory and its marker: another creator may have claimed
// it and still be waiting to run git worktree add.
fn remove_empty_parents(path: &Path, root: &Path) -> Option<String> {
    let mut current = path.parent().map(Path::to_path_buf);
    while let Some(dir) = current {
        if !paths::is_strictly_inside(&dir, root) {
            break;
        }
        match fs::remove_dir(&dir) {
            Ok(()) => current = dir.parent().map(Path::to_path_buf),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => {
                return Some(format!(
                    "Failed to remove empty directory {}: {error}",
                    dir.display()
                ));
            }
        }
    }
    None
}

/// Copy gitignored files matched by `.worktreeinclude` and extra gitignore patterns.
///
/// Existing files are left in place. Symlinks are skipped and are not followed.
#[cfg(test)]
fn copy_included_files(source: &Path, target: &Path, extra: &[String]) -> Result<()> {
    copy_files_recorded(source, target, extra, &mut Vec::new())
}

pub(crate) fn copy_files_recorded(
    source: &Path,
    target: &Path,
    extra: &[String],
    copied: &mut Vec<String>,
) -> Result<()> {
    let Some(matcher) = include_matcher(source, extra)? else {
        return Ok(());
    };
    for listed in gitignored_paths(source)? {
        let Some(relative) = normal_relative(&listed) else {
            continue;
        };
        if !matcher
            .matched_path_or_any_parents(relative, false)
            .is_ignore()
        {
            continue;
        }
        let existed = target.join(relative).exists();
        copy_one(source, target, relative)?;
        if !existed && target.join(relative).is_file() {
            copied.push(listed);
        }
    }
    Ok(())
}

fn include_matcher(source: &Path, extra: &[String]) -> Result<Option<Gitignore>> {
    let include_path = source.join(".worktreeinclude");
    let has_file = match fs::symlink_metadata(&include_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(Error::msg(
                "Failed to read .worktreeinclude: refusing to follow a symlink",
            ));
        }
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => {
            return Err(Error::msg(
                "Failed to read .worktreeinclude: expected a file",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(Error::msg(format!(
                "Failed to read .worktreeinclude: {error}"
            )));
        }
    };
    if !has_file && extra.is_empty() {
        return Ok(None);
    }
    let mut builder = GitignoreBuilder::new(source);
    if has_file {
        if let Some(error) = builder.add(&include_path) {
            return Err(Error::msg(format!(
                "Failed to read .worktreeinclude: {error}"
            )));
        }
    }
    for pattern in extra {
        let pattern = pattern.replace('\\', "/");
        builder
            .add_line(Some(include_path.clone()), &pattern)
            .map_err(|error| Error::msg(format!("Invalid copy pattern '{pattern}': {error}")))?;
    }
    builder.build().map(Some).map_err(|error| {
        if has_file {
            Error::msg(format!("Failed to read .worktreeinclude: {error}"))
        } else {
            Error::msg(format!("Invalid copy pattern: {error}"))
        }
    })
}

fn gitignored_paths(source: &Path) -> Result<Vec<String>> {
    let stdout = git::output(
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
        Some(source),
        false,
    )
    .map_err(|error| Error::msg(format!("Failed to list gitignored files: {error}")))?;
    Ok(stdout
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn normal_relative(path: &str) -> Option<&Path> {
    let path = Path::new(path);
    if path.as_os_str().is_empty() || path.is_absolute() {
        return None;
    }
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        .then_some(path)
}

fn copy_one(source_root: &Path, target_root: &Path, relative: &Path) -> Result<()> {
    let source = source_root.join(relative);
    let metadata = match fs::symlink_metadata(&source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(copy_error(relative, error)),
    };
    // Skip symlinks instead of following them out of the repository.
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(());
    }
    let target = target_root.join(relative);
    match fs::symlink_metadata(&target) {
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(copy_error(relative, error)),
    }
    ensure_real_parents(target_root, relative)?;
    fs::copy(&source, &target).map_err(|error| copy_error(relative, error))?;
    Ok(())
}

fn ensure_real_parents(root: &Path, relative: &Path) -> Result<()> {
    let Some(parent) = relative
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    let mut current = root.to_path_buf();
    for component in parent.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(Error::msg(format!(
                    "Failed to copy {}: refusing to follow a symlink",
                    relative.display()
                )));
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(Error::msg(format!(
                    "Failed to copy {}: parent is not a directory",
                    relative.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if let Err(error) = fs::create_dir(&current) {
                    if error.kind() != std::io::ErrorKind::AlreadyExists {
                        return Err(copy_error(relative, error));
                    }
                }
            }
            Err(error) => return Err(copy_error(relative, error)),
        }
    }
    Ok(())
}

fn copy_error(relative: &Path, error: impl std::fmt::Display) -> Error {
    Error::msg(format!("Failed to copy {}: {error}", relative.display()))
}

/// Machine-readable workspace. Field names are camelCase, matching `list --json`.
///
/// `base` is set when the command that produced the object knows the starting
/// commit-ish (create, and GitHub issue/PR checkout), otherwise it is the recorded
/// base. `purpose`, `createdAt`, `updatedAt`, `createdFrom`, and `links`
/// come from the repository metadata store.
/// `createdFrom` is the full creation commit, or null when the record has
/// none. Cleanup fields such as
/// `prunable` and `locked` can be added later without a version bump. Parsers
/// must ignore unknown fields.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeJson {
    pub current: bool,
    pub repo: crate::selectors::RepoJson,
    pub managed: bool,
    pub upstream: Option<crate::selectors::UpstreamJson>,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub base: Option<String>,
    pub dirty: Option<bool>,
    pub kind: &'static str,
    pub detached: Option<bool>,
    pub orphaned: bool,
    #[serde(flatten)]
    pub metadata: metadata::WorktreeMetadata,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetadataSaveOutcome {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyFilesOutcome {
    pub ok: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HookOutcome {
    pub ran: bool,
    pub ok: bool,
    pub command: Option<String>,
    pub exit_code: Option<i32>,
    pub signal: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeListOutput {
    pub version: u32,
    #[serde(rename = "workspaces")]
    pub worktrees: Vec<WorktreeJson>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeRefOutput {
    pub version: u32,
    #[serde(rename = "workspace")]
    pub worktree: WorktreeJson,
}

impl WorktreeInfo {
    pub fn to_json(&self, base: Option<String>) -> WorktreeJson {
        WorktreeJson {
            current: crate::selectors::is_current(Path::new(&self.path)),
            repo: crate::selectors::repo_json(&self.repo_root),
            managed: self.managed,
            upstream: crate::selectors::upstream(self),
            name: self.name.clone(),
            path: absolute_path(&self.path),
            branch: if self.detached || self.orphaned {
                None
            } else {
                Some(self.branch.clone())
            },
            head: self.head.clone(),
            base: base.or_else(|| self.metadata.base.clone()),
            dirty: (!self.orphaned).then_some(!self.is_clean),
            kind: if self.is_default {
                "primary"
            } else {
                "workspace"
            },
            detached: (!self.orphaned).then_some(self.detached),
            orphaned: self.orphaned,
            metadata: metadata::WorktreeMetadata {
                base: None,
                base_cleared: false,
                setup: crate::setup::display(self.recorded, self.metadata.setup.as_ref()),
                ..self.metadata.clone()
            },
        }
    }
}

pub fn metadata_save_outcome(error: Option<&str>) -> MetadataSaveOutcome {
    MetadataSaveOutcome {
        ok: error.is_none(),
        error: error.map(ToOwned::to_owned),
    }
}

pub fn copy_files_outcome(error: Option<&str>) -> CopyFilesOutcome {
    CopyFilesOutcome {
        ok: error.is_none(),
        error: error.map(ToOwned::to_owned),
    }
}

pub fn hook_outcome(ran: bool, failure: Option<&HookFailure>) -> HookOutcome {
    if let Some(failure) = failure {
        HookOutcome {
            ran: true,
            ok: false,
            command: Some(failure.command.clone()),
            exit_code: Some(failure.code),
            signal: failure.signal.clone(),
            error: failure.error.clone(),
        }
    } else {
        HookOutcome {
            ran,
            ok: true,
            command: None,
            exit_code: ran.then_some(0),
            signal: None,
            error: None,
        }
    }
}

fn absolute_path(path: &str) -> String {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = std::env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    paths::canonical_location(&absolute)
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str, branch: &str) -> WorktreeInfo {
        WorktreeInfo {
            repo_root: PathBuf::from("/repo"),
            managed: false,
            recorded: false,
            name: name.to_string(),
            path: format!("/worktrees/{name}"),
            registered_path: PathBuf::from(format!("/worktrees/{name}")),
            path_to_display: name.to_string(),
            branch: branch.to_string(),
            is_clean: true,
            head: Some("abc1234".to_string()),
            is_default: false,
            detached: branch == "(detached HEAD)",
            orphaned: false,
            metadata: metadata::WorktreeMetadata::default(),
        }
    }

    #[test]
    fn unborn_head_is_not_a_commit_sha() {
        assert_eq!(recorded_head(None), None);
        assert_eq!(recorded_head(Some("")), None);
        assert_eq!(
            recorded_head(Some("0000000000000000000000000000000000000000")),
            None
        );
        assert_eq!(
            recorded_head(Some(
                "0000000000000000000000000000000000000000000000000000000000000000"
            )),
            None
        );
        assert_eq!(
            recorded_head(Some("0123456789abcdef0123456789abcdef01234567")).as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
    }

    #[test]
    fn worktree_json_is_camel_case_and_nulls_a_detached_branch() {
        let mut info = sample("next", "(detached HEAD)");
        info.detached = true;
        info.is_clean = false;
        info.head = Some("0123456789abcdef".to_string());
        let value = serde_json::to_value(info.to_json(None)).expect("json");
        assert_eq!(value["name"], "next");
        assert_eq!(value["path"], "/worktrees/next");
        assert!(value["branch"].is_null());
        assert_eq!(value["head"], "0123456789abcdef");
        assert!(value["base"].is_null());
        assert_eq!(value["dirty"], true);
        assert_eq!(value["kind"], "workspace");
        assert!(value.get("default").is_none());
        assert_eq!(value["detached"], true);
        assert!(value.get("isDefault").is_none());
        assert!(value.get("is_clean").is_none());

        let branched = sample("feature", "feature");
        let value = serde_json::to_value(branched.to_json(Some("HEAD".to_string()))).expect("json");
        assert_eq!(value["branch"], "feature");
        assert_eq!(value["base"], "HEAD");
        assert_eq!(value["dirty"], false);
        assert_eq!(value["detached"], false);
        assert!(value.get("owner").is_none());
        assert!(value["purpose"].is_null());
        assert!(value["createdAt"].is_null());
        assert!(value.get("lastUsedAt").is_none());
        assert!(value["links"]["pr"].is_null());
        assert!(value["links"]["issue"].is_null());
        assert!(value.get("metadata").is_none());
    }

    #[test]
    fn directory_name_beats_a_branch_and_ambiguous_branches_error() {
        let preferred = select_worktree(
            vec![sample("a", "topic"), sample("topic", "other")],
            "topic",
        )
        .expect("directory");
        assert_eq!(preferred.name, "topic");
        assert_eq!(preferred.branch, "other");

        let fallback = select_worktree(vec![sample("a", "topic")], "topic").expect("branch");
        assert_eq!(fallback.name, "a");

        let error = select_worktree(vec![sample("a", "topic"), sample("b", "topic")], "topic")
            .expect_err("ambiguous");
        assert!(error.to_string().contains("more than one"));
        assert!(matches!(
            select_worktree(vec![sample("a", "other")], "topic"),
            Err(Error::WorktreeNotFound(_))
        ));
    }

    #[test]
    fn recorded_path_keeps_slash_names_and_skips_the_project_directory() {
        let common = Path::new("/repo/.git");
        assert_eq!(
            name_from_recorded_path(Path::new("/repo/.git/rabi/workspaces/next"), common),
            "next"
        );
        assert_eq!(
            name_from_recorded_path(
                Path::new("/repo/.git/rabi/workspaces/feature/topic"),
                common
            ),
            "feature/topic"
        );
        let project = paths::project_directory_name("repo", common);
        let nested = PathBuf::from(format!("/data/rabi/workspaces/{project}/feature/topic"));
        assert_eq!(name_from_recorded_path(&nested, common), "feature/topic");
        let single = PathBuf::from(format!("/data/rabi/workspaces/{project}/inside"));
        assert_eq!(name_from_recorded_path(&single, common), "inside");
        assert_eq!(
            name_from_recorded_path(Path::new("/tmp/custom-wt/alias"), common),
            "alias"
        );
        let directory = Path::new("/data/wt");
        assert_eq!(
            resolve_worktree_name(
                Path::new("/data/wt/next"),
                directory,
                Some(Path::new("/repo/.git/rabi/workspaces/other")),
                None,
                common,
                None,
                "abcdef0",
            ),
            "next"
        );
        assert_eq!(
            resolve_worktree_name(
                Path::new("/tmp/stray"),
                directory,
                None,
                None,
                common,
                Some("legacy"),
                "abcdef0",
            ),
            "legacy"
        );
        assert_eq!(
            resolve_worktree_name(
                Path::new("/tmp/stray"),
                directory,
                None,
                None,
                common,
                None,
                "abcdef0",
            ),
            "abcdef0"
        );
        assert_eq!(
            resolve_worktree_name(
                Path::new("/tmp/stray"),
                directory,
                Some(Path::new("/tmp/stray")),
                None,
                common,
                Some("legacy"),
                "abcdef0",
            ),
            "stray"
        );
        assert_eq!(
            resolve_worktree_name(
                Path::new("/tmp/stray"),
                directory,
                Some(Path::new("/tmp/stray")),
                Some("kept"),
                common,
                None,
                "fffffff",
            ),
            "kept"
        );
        assert_eq!(
            resolve_worktree_name(
                Path::new("/repo/custom/workspaces/next"),
                directory,
                Some(Path::new("/repo/custom/workspaces/next")),
                None,
                common,
                None,
                "4158e88",
            ),
            "next"
        );
        let taken = |name: &str| name == "dup" || name == "zz";
        let both = outside_record_name(Some("zz"), "abcdef0", "dup", taken).expect_err("both");
        assert!(
            both.to_string().contains("zz") && both.to_string().contains("dup"),
            "{both}"
        );
        let branch_taken = |name: &str| name == "zz";
        let branch =
            outside_record_name(Some("zz"), "abcdef0", "side", branch_taken).expect_err("branch");
        assert!(branch.to_string().contains("zz"), "{branch}");
        let basename_taken = |name: &str| name == "dup";
        assert_eq!(
            outside_record_name(Some("zz"), "abcdef0", "dup", basename_taken).expect("branch"),
            "zz"
        );
        let hash_taken = |name: &str| name == "abcdef0";
        let detached = outside_record_name(None, "abcdef0", "loose", hash_taken).expect_err("hash");
        assert!(detached.to_string().contains("abcdef0"), "{detached}");
        let free = |_name: &str| false;
        assert_eq!(
            outside_record_name(None, "abcdef0", "loose", free).expect("free"),
            "abcdef0"
        );
    }

    #[test]
    fn rejects_invalid_names() {
        assert_eq!(
            validate_worktree_name("").expect_err("empty").to_string(),
            "Workspace name cannot be empty"
        );
        assert!(validate_worktree_name("feature/test").is_ok());
        assert_eq!(
            validate_worktree_name("has space")
                .expect_err("space")
                .to_string(),
            "Workspace name can only contain letters, numbers, hyphens, underscores, dots, and slashes"
        );
        assert_eq!(
            validate_worktree_name("bad..name")
                .expect_err("dots")
                .to_string(),
            "Workspace name cannot contain consecutive dots"
        );
    }

    #[test]
    fn rabi_env_exports_rabi_variables() {
        for id in [Some("ws_test"), None] {
            let env = rabi_env(
                "feature/env",
                Path::new("/workspaces/feature/env"),
                id,
                Path::new("/repo"),
            );
            assert_eq!(
                env,
                vec![
                    ("RABI".to_string(), "1".to_string()),
                    ("RABI_WORKSPACE".to_string(), "feature/env".to_string()),
                    (
                        "RABI_WORKSPACE_ID".to_string(),
                        id.unwrap_or_default().to_string()
                    ),
                    (
                        "RABI_WORKSPACE_PATH".to_string(),
                        "/workspaces/feature/env".to_string()
                    ),
                    ("RABI_REPO_PATH".to_string(), "/repo".to_string()),
                ]
            );
        }
    }

    #[test]
    fn copies_only_matching_gitignored_files() {
        let repo = CopyRepo::new();
        fs::write(repo.root.join("tracked.env"), "committed\n").expect("tracked");
        repo.git(&["add", "tracked.env"]);
        repo.git(&["commit", "-m", "track env"]);
        fs::write(repo.root.join(".gitignore"), "*.env\n*.log\nvendor/\n").expect("gitignore");
        fs::write(
            repo.root.join(".worktreeinclude"),
            "*.env\nnested/**\n!nested/skip.env\nvendor/\n!vendor/skip.json\n",
        )
        .expect("include");
        fs::create_dir_all(repo.root.join("nested/deep")).expect("nested");
        fs::create_dir_all(repo.root.join("vendor")).expect("vendor");
        fs::write(repo.root.join("secret.env"), "secret\n").expect("secret");
        fs::write(repo.root.join("nested/deep/inner.env"), "inner\n").expect("inner");
        fs::write(repo.root.join("nested/skip.env"), "skip\n").expect("skip");
        fs::write(repo.root.join("nested/deep/note.txt"), "note\n").expect("note");
        fs::write(repo.root.join("notes.log"), "log\n").expect("log");
        fs::write(repo.root.join("vendor/config.json"), "config\n").expect("config");
        fs::write(repo.root.join("vendor/skip.json"), "skipped\n").expect("vendor skip");
        fs::write(repo.root.join("tracked.env"), "dirty\n").expect("dirty");

        let target = repo.root.join("target-copy");
        fs::create_dir_all(target.join("vendor")).expect("target vendor");
        fs::write(target.join("secret.env"), "already\n").expect("existing");
        copy_included_files(&repo.root, &target, &["notes.log".to_string()]).expect("copy");

        assert_eq!(read(&target.join("secret.env")), "already\n");
        assert_eq!(read(&target.join("nested/deep/inner.env")), "inner\n");
        assert_eq!(read(&target.join("vendor/config.json")), "config\n");
        assert_eq!(read(&target.join("notes.log")), "log\n");
        assert!(!target.join("nested/skip.env").exists());
        assert!(!target.join("nested/deep/note.txt").exists());
        assert!(!target.join("vendor/skip.json").exists());
        assert!(!target.join("tracked.env").exists());
        assert!(!target.join("README.md").exists());
    }

    #[test]
    fn skips_symlinks_instead_of_following_them() {
        let repo = CopyRepo::new();
        let outside = repo.root.join("outside-tree");
        fs::create_dir_all(&outside).expect("outside");
        fs::write(outside.join("secret.env"), "nope\n").expect("outside secret");
        fs::write(repo.root.join("real.env"), "real\n").expect("real");
        std::os::unix::fs::symlink(repo.root.join("real.env"), repo.root.join("link.env"))
            .expect("file link");
        std::os::unix::fs::symlink(&outside, repo.root.join("outside")).expect("dir link");
        fs::write(repo.root.join(".gitignore"), "*.env\noutside\n").expect("gitignore");
        fs::write(repo.root.join(".worktreeinclude"), "*\n").expect("include");

        let target = repo.root.join("target-copy");
        fs::create_dir_all(&target).expect("target");
        copy_included_files(&repo.root, &target, &[]).expect("copy");

        assert_eq!(read(&target.join("real.env")), "real\n");
        assert!(!target.join("link.env").exists());
        assert!(!target.join("outside").exists());
        assert!(!target.join("secret.env").exists());
        assert!(!target.join("outside/secret.env").exists());
    }

    #[test]
    fn symlinked_worktreeinclude_is_not_followed() {
        let repo = CopyRepo::new();
        let elsewhere = repo.root.join("elsewhere");
        fs::create_dir_all(&elsewhere).expect("elsewhere");
        fs::write(elsewhere.join("include"), "*.env\n").expect("include");
        std::os::unix::fs::symlink(
            elsewhere.join("include"),
            repo.root.join(".worktreeinclude"),
        )
        .expect("symlink include");
        fs::write(repo.root.join(".gitignore"), "*.env\n").expect("gitignore");
        fs::write(repo.root.join("secret.env"), "secret\n").expect("secret");
        let target = repo.root.join("target-copy");
        fs::create_dir_all(&target).expect("target");

        let error = copy_included_files(&repo.root, &target, &[]).expect_err("symlink include");
        assert!(error.to_string().contains("symlink"), "{error}");
        assert!(!target.join("secret.env").exists());
    }

    struct CopyRepo {
        root: PathBuf,
    }

    impl CopyRepo {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "phantom-include-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ));
            fs::create_dir_all(root.join(".home")).expect("home");
            crate::test_git::write_config(&root.join(".home"));
            fs::create_dir_all(root.join(".template")).expect("template");
            let repo = Self { root };
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "user.email", "phantom@example.com"]);
            repo.git(&["config", "user.name", "Phantom"]);
            fs::write(repo.root.join(".git-exclude"), "").expect("exclude");
            repo.git(&[
                "config",
                "core.excludesFile",
                repo.root
                    .join(".git-exclude")
                    .to_str()
                    .expect("exclude path"),
            ]);
            fs::write(repo.root.join("README.md"), "readme\n").expect("readme");
            repo.git(&["add", "README.md"]);
            repo.git(&["commit", "-m", "init"]);
            repo
        }

        fn git(&self, args: &[&str]) {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(&self.root)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", self.root.join(".home"))
                .env("GIT_TEMPLATE_DIR", self.root.join(".template"))
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    impl Drop for CopyRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
    }
}
