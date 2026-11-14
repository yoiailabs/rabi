use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result, JSON_VERSION};
use crate::preferences;
use crate::process;

/// Clone target parsed from a URL or `host/owner/repo` shorthand.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RepoSpec {
    host: String,
    owner: String,
    repo: String,
    /// Passed to `git clone`, including any userinfo.
    clone_url: String,
    /// Shown in JSON and errors. `user:password` userinfo is removed.
    remote_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloneResult {
    pub version: u32,
    pub path: String,
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub remote_url: String,
    pub cloned: bool,
    pub updated: bool,
}

pub struct CloneRequest<'a> {
    pub source: &'a str,
    pub branch: Option<&'a str>,
    pub depth: Option<&'a str>,
    pub update: bool,
}

pub fn clone_repository(request: &CloneRequest<'_>) -> Result<CloneResult> {
    let spec = parse_clone_source(request.source)?;
    let branch = normalize_branch(request.branch)?;
    let depth = parse_depth(request.depth)?;
    let root = resolve_repository_root()?;
    let destination = root.join(&spec.host).join(&spec.owner).join(&spec.repo);
    let (cloned, updated) = place_clone(&destination, &spec, branch, depth, request.update)?;
    let path = fs::canonicalize(&destination).unwrap_or(destination);
    Ok(CloneResult {
        version: JSON_VERSION,
        path: path.to_string_lossy().into_owned(),
        host: spec.host,
        owner: spec.owner,
        repo: spec.repo,
        remote_url: spec.remote_url,
        cloned,
        updated,
    })
}

pub fn human_clone_message(result: &CloneResult) -> String {
    if result.cloned {
        format!("Cloned into {}", result.path)
    } else if result.updated {
        format!("Updated {}", result.path)
    } else {
        format!("Already cloned at {}", result.path)
    }
}

/// Root used for `<root>/<host>/<owner>/<repo>`.
///
/// Order: `root` in the user config, the first `git config ghq.root` value,
/// `$GHQ_ROOT`, then `~/ghq`. Repository shorthand selects this same location.
pub fn resolve_repository_root() -> Result<PathBuf> {
    let (root, warnings) = resolve_repository_root_quiet()?;
    for warning in warnings {
        crate::output::warning("clone_root", format!("Warning: {warning}"));
    }
    Ok(root)
}

/// Same root as [`resolve_repository_root`], without printing warnings.
pub(crate) fn resolve_repository_root_quiet() -> Result<(PathBuf, Vec<String>)> {
    let home = home_dir()?;
    let config_root = preferences::load_preferences()?.root;
    let ghq_root = std::env::var("GHQ_ROOT").ok();
    let mut warnings = Vec::new();
    let (root, _, _) = resolve_root_with_source(
        config_root.as_deref(),
        read_git_ghq_roots,
        ghq_root.as_deref(),
        &home,
        &mut warnings,
    )?;
    Ok((root, warnings))
}

pub(crate) fn clone_root_with_source(
    config_root: Option<&str>,
) -> Result<(PathBuf, &'static str, &'static str)> {
    let home = home_dir()?;
    let env_root = std::env::var("GHQ_ROOT").ok();
    let mut warnings = Vec::new();
    let value = resolve_root_with_source(
        config_root,
        read_git_ghq_roots,
        env_root.as_deref(),
        &home,
        &mut warnings,
    )?;
    for warning in warnings {
        crate::output::warning("clone_root", format!("Warning: {warning}"));
    }
    Ok(value)
}

/// `host/owner/repo` for an `origin` URL this command understands.
///
/// `git remote get-url` applies `insteadOf`.
pub(crate) fn canonical_name_from_origin(root: &Path) -> Option<String> {
    if let Some(name) = expanded_origin_url(root)
        .ok()
        .flatten()
        .as_deref()
        .and_then(canonical_remote_name)
    {
        return Some(name);
    }
    let url = origin_url(root).ok().flatten()?;
    canonical_remote_name(&url)
}

pub(crate) fn canonical_remote_name(url: &str) -> Option<String> {
    let url = url.trim();
    if !is_hosted_remote(url) {
        return None;
    }
    let spec = parse_clone_source(url).ok()?;
    Some(format!("{}/{}/{}", spec.host, spec.owner, spec.repo))
}

/// Select a checkout by filesystem path or its exact clone-layout location.
/// Only explicit path prefixes select filesystem lookup.
pub fn resolve_repository_ref(reference: &str) -> Result<PathBuf> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err(Error::validation(
            "Option '--repo <value>' requires a repository path or host/owner/repo name",
        ));
    }
    if reference.starts_with('-') {
        return Err(Error::validation(format!(
            "Option '--repo <value>' received '{reference}', which looks like an option"
        )));
    }
    let path = if reference == "~" {
        home_dir()?
    } else if let Some(rest) = reference.strip_prefix("~/") {
        home_dir()?.join(rest)
    } else if reference.starts_with('~') {
        return Err(Error::validation("Repository paths support only ~ or ~/"));
    } else {
        PathBuf::from(reference)
    };
    if reference.starts_with('/')
        || reference.starts_with('~')
        || reference.starts_with("./")
        || reference.starts_with("../")
        || matches!(reference, "." | "..")
    {
        return resolve_directory_reference(&path, reference);
    }
    let Some(segments) = path_segments(reference) else {
        return Err(Error::RepositoryNotFound(reference.to_owned()));
    };
    if !matches!(segments.len(), 2 | 3) {
        return Err(Error::RepositoryNotFound(reference.to_owned()));
    }
    if reference.starts_with('/')
        || reference.ends_with('/')
        || reference.contains('@')
        || reference.contains("://")
        || reference.chars().any(char::is_whitespace)
    {
        return Err(Error::validation(
            "Repository names must be host/owner/repo or owner/repo",
        ));
    }
    let spec = parse_shorthand(reference)?;
    let root = resolve_repository_root()?;
    resolve_named_repository(&root, &spec, reference)
}

fn resolve_named_repository(root: &Path, spec: &RepoSpec, reference: &str) -> Result<PathBuf> {
    let destination = root.join(&spec.host).join(&spec.owner).join(&spec.repo);
    if !destination_exists(&destination)? {
        return Err(Error::RepositoryNotFound(reference.to_owned()));
    }
    ensure_matching_repository(&destination, spec)?;
    let root = crate::git::git_root(&destination)?;
    fs::canonicalize(root).map_err(|error| Error::msg(error.to_string()))
}

fn resolve_directory_reference(path: &Path, reference: &str) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| Error::msg(error.to_string()))?
            .join(path)
    };
    let path = absolute.as_path();
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::PathNotFound(reference.to_owned()));
        }
        Err(error) => return Err(Error::msg(error.to_string())),
    };
    if !metadata.is_dir() {
        if path.file_name().is_some_and(|name| name == ".git") {
            if let Some(checkout) = path.parent() {
                if let Ok(root) = filesystem_checkout_root(checkout) {
                    return Err(not_checkout_root(reference, &root));
                }
            }
        }
        return Err(Error::validation(format!(
            "Repository path '{reference}' is not a directory"
        )));
    }
    resolve_checkout_path(path, reference)
}

fn resolve_checkout_path(path: &Path, reference: &str) -> Result<PathBuf> {
    let checkout = match filesystem_checkout_root(path) {
        Ok(checkout) => checkout,
        Err(error) => {
            // Git's admin directory is outside the work tree. Still give a
            // checkout-root hint without accepting that directory.
            for ancestor in path.ancestors() {
                if ancestor.file_name().is_some_and(|name| name == ".git") {
                    if let Some(checkout) = ancestor.parent() {
                        if let Ok(root) = filesystem_checkout_root(checkout) {
                            return Err(not_checkout_root(reference, &root));
                        }
                    }
                }
            }
            return Err(error);
        }
    };
    let canonical = fs::canonicalize(path).map_err(|error| Error::msg(error.to_string()))?;
    let checkout = fs::canonicalize(checkout).map_err(|error| Error::msg(error.to_string()))?;
    if canonical != checkout {
        return Err(not_checkout_root(reference, &checkout));
    }
    let root = crate::git::git_root(path)?;
    fs::canonicalize(root).map_err(|error| Error::msg(error.to_string()))
}

fn filesystem_checkout_root(path: &Path) -> Result<PathBuf> {
    let inside = crate::git::output(&["rev-parse", "--is-inside-work-tree"], Some(path), true)?;
    if inside != "true" {
        return Err(Error::validation(
            "Repository path is not inside a checkout",
        ));
    }
    // Git returns only ../ components here. Keep the filesystem path itself out
    // of text output so trailing whitespace and non-UTF-8 bytes survive.
    let relative = crate::git::output(&["rev-parse", "--show-cdup"], Some(path), true)?;
    fs::canonicalize(path.join(relative)).map_err(|error| Error::msg(error.to_string()))
}

fn not_checkout_root(reference: &str, checkout: &Path) -> Error {
    Error::validation(format!(
        "Repository path '{reference}' is not a repository root. Use the checkout root {} instead.",
        checkout.display()
    ))
}

/// Display identity for the selected repository, independent of registrations.
pub(crate) fn canonical_name(root: &Path) -> String {
    if let Some(name) = canonical_name_from_origin(root) {
        return name;
    }
    if let Ok((clone_root, _)) = resolve_repository_root_quiet() {
        let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let clone_root = fs::canonicalize(&clone_root).unwrap_or(clone_root);
        if let Ok(relative) = canonical.strip_prefix(&clone_root) {
            let segments = relative
                .components()
                .map(|component| match component {
                    Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>();
            if let Some(segments) = segments.filter(|segments| segments.len() == 3) {
                return format!(
                    "{}/{}/{}",
                    segments[0].to_ascii_lowercase(),
                    segments[1],
                    segments[2]
                );
            }
        }
    }
    root.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// A remote git can talk to by scheme or scp-style `host:path`.
fn is_hosted_remote(url: &str) -> bool {
    if url.is_empty() || url.starts_with('/') || url.starts_with('.') || url.starts_with('~') {
        return false;
    }
    url.contains("://") || url.contains(':')
}

#[cfg(test)]
fn resolve_repository_root_from(
    config_root: Option<&str>,
    git_roots: impl FnOnce() -> Result<Vec<String>>,
    ghq_root: Option<&str>,
    home: &Path,
    warnings: &mut Vec<String>,
) -> Result<PathBuf> {
    Ok(resolve_root_with_source(config_root, git_roots, ghq_root, home, warnings)?.0)
}

fn resolve_root_with_source(
    config_root: Option<&str>,
    git_roots: impl FnOnce() -> Result<Vec<String>>,
    ghq_root: Option<&str>,
    home: &Path,
    warnings: &mut Vec<String>,
) -> Result<(PathBuf, &'static str, &'static str)> {
    if let Some(value) = nonempty(config_root) {
        return Ok((
            expand_absolute_path(value, home, "clone-root")?,
            "config",
            "clone-root",
        ));
    }
    match git_roots() {
        Ok(roots) => {
            if let Some(value) = roots
                .iter()
                .find_map(|value| nonempty(Some(value.as_str())))
            {
                return Ok((
                    expand_absolute_path(value, home, "git config ghq.root")?,
                    "default",
                    "ghq.root",
                ));
            }
        }
        Err(error) => warnings.push(format!("{error} Continuing with $GHQ_ROOT or ~/ghq.")),
    }
    if let Some(value) = nonempty(ghq_root) {
        return Ok((
            expand_absolute_path(value, home, "GHQ_ROOT")?,
            "env",
            "GHQ_ROOT",
        ));
    }
    Ok((
        crate::paths::normalize(&home.join("ghq")),
        "default",
        "~/ghq",
    ))
}

fn read_git_ghq_roots() -> Result<Vec<String>> {
    match process::raw("git", &["config", "--get-all", "ghq.root"], None) {
        Ok(captured) if captured.code == 0 => Ok(nonempty_lines(&captured.stdout)),
        Ok(captured) if captured.stderr.trim().is_empty() => Ok(Vec::new()),
        Ok(captured) => Err(Error::msg(format!(
            "Failed to read git config ghq.root: {}",
            captured.stderr.trim()
        ))),
        Err(Error::Spawn { .. }) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

fn place_clone(
    destination: &Path,
    spec: &RepoSpec,
    branch: Option<&str>,
    depth: Option<u32>,
    update: bool,
) -> Result<(bool, bool)> {
    if destination_exists(destination)? {
        reap_stale_staging(destination.parent(), destination);
        return adopt_existing(destination, spec, update);
    }
    let created_parents = create_missing_parents(destination)?;
    let Some(parent) = destination.parent() else {
        remove_created_parents(&created_parents);
        return Err(Error::validation(format!(
            "Cannot create clone destination {}",
            destination.display()
        )));
    };
    reap_stale_staging(Some(parent), destination);
    // The staging directory is the checkout. Renaming it onto the destination
    // stays on one filesystem. POSIX rename fails when the destination is a
    // non-empty directory, which is the racing clone we must not delete.
    let staging = parent.join(format!(
        ".rabi-clone-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    if let Err(error) = fs::create_dir(&staging) {
        remove_created_parents(&created_parents);
        return Err(io_failure(
            &error,
            format!("Failed to create {}: {error}", staging.display()),
        ));
    }
    if let Err(error) = clone_with_git(&spec.clone_url, &staging, branch, depth) {
        remove_staging(&staging);
        remove_created_parents(&created_parents);
        return Err(error);
    }
    match publish_clone(&staging, destination, spec, update) {
        Ok(result) => Ok(result),
        Err(error) => {
            remove_created_parents(&created_parents);
            Err(error)
        }
    }
}

fn publish_clone(
    staging: &Path,
    destination: &Path,
    spec: &RepoSpec,
    update: bool,
) -> Result<(bool, bool)> {
    match fs::rename(staging, destination) {
        Ok(()) => Ok((true, false)),
        Err(error) if destination_appeared(&error) => {
            remove_staging(staging);
            adopt_existing(destination, spec, update)
        }
        Err(error) => {
            remove_staging(staging);
            Err(io_failure(
                &error,
                format!(
                    "Failed to move clone into {}: {error}",
                    destination.display()
                ),
            ))
        }
    }
}

/// Directories between the clone root and the destination that did not exist.
/// Listed deepest first so cleanup removes the leaf first.
fn create_missing_parents(destination: &Path) -> Result<Vec<PathBuf>> {
    let mut missing = Vec::new();
    let mut cursor = destination.parent().map(Path::to_path_buf);
    while let Some(dir) = cursor {
        if destination_exists(&dir)? {
            break;
        }
        cursor = dir.parent().map(Path::to_path_buf);
        missing.push(dir);
    }
    let mut created = Vec::new();
    for dir in missing.iter().rev() {
        match fs::create_dir(dir) {
            Ok(()) => created.push(dir.clone()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                // `created` is shallowest-first while directories are being made.
                // Cleanup matches the returned list and removes the leaf first.
                created.reverse();
                remove_created_parents(&created);
                return Err(io_failure(
                    &error,
                    format!("Failed to create {}: {error}", dir.display()),
                ));
            }
        }
    }
    created.reverse();
    Ok(created)
}

fn remove_created_parents(created_deepest_first: &[PathBuf]) {
    for dir in created_deepest_first {
        if fs::remove_dir(dir).is_err() {
            break;
        }
    }
}

fn adopt_existing(destination: &Path, spec: &RepoSpec, update: bool) -> Result<(bool, bool)> {
    ensure_matching_repository(destination, spec)?;
    if update {
        fetch_origin(destination)?;
        Ok((false, true))
    } else {
        Ok((false, false))
    }
}

fn remove_staging(staging: &Path) {
    let _ = fs::remove_dir_all(staging);
}

fn ensure_matching_repository(path: &Path, spec: &RepoSpec) -> Result<()> {
    if !is_directory(path) {
        return Err(not_a_repository(path));
    }
    let Some(toplevel) = repository_toplevel(path)? else {
        return Err(not_a_repository(path));
    };
    let canonical = fs::canonicalize(path)
        .map_err(|error| Error::msg(format!("Failed to resolve {}: {error}", path.display())))?;
    if toplevel != canonical {
        return Err(Error::validation(format!(
            "{} is inside the Git repository at {}, not a repository root.",
            path.display(),
            toplevel.display()
        )));
    }
    let Some(actual) = origin_url(path)? else {
        return Err(Error::validation(format!(
            "{} is a Git repository without an origin remote. Expected '{}'.",
            path.display(),
            spec.remote_url
        )));
    };
    if !same_repository(spec, &actual) {
        return Err(Error::validation(format!(
            "{} is a Git repository with origin '{}', which does not match '{}' ({}/{}/{}).",
            path.display(),
            redact_userinfo(&actual),
            spec.remote_url,
            spec.host,
            spec.owner,
            spec.repo
        )));
    }
    Ok(())
}

fn not_a_repository(path: &Path) -> Error {
    Error::validation(format!(
        "{} exists but is not a Git repository.",
        path.display()
    ))
}

fn clone_with_git(
    url: &str,
    destination: &Path,
    branch: Option<&str>,
    depth: Option<u32>,
) -> Result<()> {
    let destination = path_arg(destination)?;
    let mut args = vec![
        "clone".to_string(),
        "--origin".to_string(),
        "origin".to_string(),
    ];
    if let Some(branch) = branch {
        args.push("--branch".to_string());
        args.push(branch.to_string());
    }
    if let Some(depth) = depth {
        args.push("--depth".to_string());
        args.push(depth.to_string());
    }
    args.push("--".to_string());
    args.push(url.to_string());
    args.push(destination);
    run_git_visible(&args, "git clone")
}

fn fetch_origin(destination: &Path) -> Result<()> {
    let destination = path_arg(destination)?;
    let args = [
        "-C".to_string(),
        destination,
        "fetch".to_string(),
        "origin".to_string(),
    ];
    run_git_visible(&args, "git fetch")
}

fn run_git_visible(args: &[String], command: &str) -> Result<()> {
    let refs = args.iter().map(String::as_str).collect::<Vec<_>>();
    let code = process::inherit("git", &refs, None, &[], true)?;
    if code == 0 {
        Ok(())
    } else {
        Err(Error::CommandFailed {
            command: command.to_string(),
            code,
        })
    }
}

fn repository_toplevel(path: &Path) -> Result<Option<PathBuf>> {
    let arg = path_arg(path)?;
    match process::raw_with_env(
        "git",
        &["-C", &arg, "rev-parse", "--show-toplevel"],
        None,
        &[("LC_ALL", "C")],
    ) {
        Ok(captured) if captured.code == 0 => {
            let toplevel = PathBuf::from(captured.stdout.trim());
            Ok(Some(fs::canonicalize(&toplevel).unwrap_or(toplevel)))
        }
        Ok(captured) if git_missing_repository(&captured.stderr) => Ok(None),
        Ok(captured) => Err(git_inspect_error(path, &captured.stderr, captured.code)),
        Err(error) => Err(error),
    }
}

fn origin_url(path: &Path) -> Result<Option<String>> {
    // `remote get-url` expands insteadOf. Compare the URL Git stored.
    read_origin(path, false)
}

fn expanded_origin_url(path: &Path) -> Result<Option<String>> {
    read_origin(path, true)
}

fn read_origin(path: &Path, expand_instead_of: bool) -> Result<Option<String>> {
    let arg = path_arg(path)?;
    let mut args = vec!["-C", arg.as_str()];
    if expand_instead_of {
        args.extend(["remote", "get-url", "origin"]);
    } else {
        args.extend(["config", "--get", "remote.origin.url"]);
    }
    match process::raw_with_env("git", &args, None, &[("LC_ALL", "C")]) {
        Ok(captured) if captured.code == 0 => {
            let url = captured.stdout.trim();
            if url.is_empty() {
                Ok(None)
            } else {
                Ok(Some(url.to_string()))
            }
        }
        Ok(captured) if captured.stderr.trim().is_empty() => Ok(None),
        Ok(captured) => Err(git_inspect_error(path, &captured.stderr, captured.code)),
        Err(error) => Err(error),
    }
}

fn destination_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_failure(
            &error,
            format!("Failed to inspect {}: {error}", path.display()),
        )),
    }
}

fn parse_clone_source(input: &str) -> Result<RepoSpec> {
    let input = input.trim();
    if input.is_empty() || input.chars().any(char::is_whitespace) {
        return Err(invalid_source(input));
    }
    if let Some((scheme, rest)) = input.split_once("://") {
        return parse_scheme_url(input, scheme, rest);
    }
    if let Some((left, path)) = input.split_once(':') {
        return parse_scp(input, left, path);
    }
    parse_shorthand(input)
}

fn parse_scheme_url(input: &str, scheme: &str, rest: &str) -> Result<RepoSpec> {
    if !matches!(scheme, "https" | "http" | "ssh") {
        return Err(Error::validation(format!(
            "Unsupported URL scheme '{scheme}' in '{}'. Expected https, http, or ssh.",
            redact_userinfo(input)
        )));
    }
    if rest.contains('?') || rest.contains('#') {
        return Err(invalid_source(input));
    }
    let Some((authority, path)) = rest.split_once('/') else {
        return Err(invalid_path(input));
    };
    if authority.is_empty() {
        return Err(invalid_source(input));
    }
    let host = host_from_authority(input, authority)?;
    let (owner, repo) = owner_repo_from_path(input, path)?;
    Ok(spec_from_urls(
        host,
        owner,
        repo,
        input.trim_end_matches('/'),
    ))
}

fn parse_scp(input: &str, left: &str, path: &str) -> Result<RepoSpec> {
    if path.starts_with('/') {
        return Err(Error::validation(format!(
            "Invalid repository '{}'. An scp-style path must be owner/repo, not an absolute path.",
            redact_userinfo(input)
        )));
    }
    if left.is_empty() || path.is_empty() || left.contains('/') {
        return Err(invalid_source(input));
    }
    let host = match left.rsplit_once('@') {
        Some((_, host)) if !host.is_empty() => host,
        Some(_) => return Err(invalid_source(input)),
        None => left,
    };
    let host = host.to_ascii_lowercase();
    validate_host(input, &host)?;
    let (owner, repo) = owner_repo_from_path(input, path)?;
    Ok(spec_from_urls(
        host,
        owner,
        repo,
        input.trim_end_matches('/'),
    ))
}

fn parse_shorthand(input: &str) -> Result<RepoSpec> {
    let (host, owner, repo) = match path_segments(input) {
        Some(segments) if segments.len() == 2 => ("github.com", segments[0], segments[1]),
        Some(segments) if segments.len() == 3 => (segments[0], segments[1], segments[2]),
        _ => return Err(invalid_path(input)),
    };
    let host = host.to_ascii_lowercase();
    validate_host(input, &host)?;
    let owner = validate_component(input, owner, "owner")?;
    let repo = validate_repo_name(input, repo)?;
    let remote_url = format!("https://{host}/{owner}/{repo}.git");
    Ok(RepoSpec {
        clone_url: remote_url.clone(),
        remote_url,
        host,
        owner,
        repo,
    })
}

fn host_from_authority(input: &str, authority: &str) -> Result<String> {
    let hostport = match authority.rsplit_once('@') {
        Some((_, hostport)) if !hostport.is_empty() => hostport,
        Some(_) => return Err(invalid_source(input)),
        None => authority,
    };
    let bracketed = hostport.starts_with('[');
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err(invalid_source(input));
        };
        if end == 0 {
            return Err(invalid_source(input));
        }
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            None
        } else if let Some(port) = after.strip_prefix(':').filter(|port| is_port_number(port)) {
            Some(port)
        } else {
            return Err(invalid_source(input));
        };
        (&rest[..end], port)
    } else if let Some((host, port)) = hostport.rsplit_once(':') {
        if host.is_empty() || !is_port_number(port) {
            return Err(invalid_source(input));
        }
        (host, Some(port))
    } else {
        (hostport, None)
    };
    validate_host(input, host)?;
    let host = host.to_ascii_lowercase();
    let rendered = if bracketed { format!("[{host}]") } else { host };
    Ok(match port {
        Some(port) => format!("{rendered}:{port}"),
        None => rendered,
    })
}

fn is_port_number(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|char| char.is_ascii_digit())
}

fn owner_repo_from_path(input: &str, path: &str) -> Result<(String, String)> {
    let Some(segments) = path_segments(path) else {
        return Err(invalid_path(input));
    };
    if segments.len() != 2 {
        return Err(invalid_path(input));
    }
    let owner = validate_component(input, segments[0], "owner")?;
    let repo = validate_repo_name(input, segments[1])?;
    Ok((owner, repo))
}

fn path_segments(path: &str) -> Option<Vec<&str>> {
    let path = path.trim_matches('/');
    if path.is_empty() {
        return None;
    }
    let segments = path.split('/').collect::<Vec<_>>();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    Some(segments)
}

fn validate_repo_name(input: &str, repo: &str) -> Result<String> {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    let repo = validate_component(input, repo, "repository")?;
    if staging_owner_pid(std::ffi::OsStr::new(&repo)).is_some() {
        return Err(Error::validation(format!(
            "Invalid repository '{repo}' in '{}'. That name is reserved for an in-progress clone.",
            redact_userinfo(input)
        )));
    }
    Ok(repo)
}

fn validate_host(input: &str, host: &str) -> Result<()> {
    if host.is_empty()
        || host == "."
        || host == ".."
        || host.contains('/')
        || host.contains('\\')
        || host.chars().any(char::is_control)
    {
        return Err(invalid_source(input));
    }
    Ok(())
}

fn validate_component(input: &str, value: &str, label: &str) -> Result<String> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.chars().any(|char| {
            char.is_control() || matches!(char, '/' | '\\' | ':' | '@' | '?' | '#' | ' ')
        })
    {
        return Err(Error::validation(format!(
            "Invalid {label} '{value}' in repository '{}'.",
            redact_userinfo(input)
        )));
    }
    Ok(value.to_string())
}

fn same_repository(expected: &RepoSpec, actual_url: &str) -> bool {
    if !is_hosted_remote(actual_url) {
        return false;
    }
    match parse_clone_source(actual_url) {
        Ok(actual) => {
            actual.host == expected.host
                && actual.owner == expected.owner
                && actual.repo == expected.repo
        }
        Err(_) => false,
    }
}

fn spec_from_urls(host: String, owner: String, repo: String, clone_url: &str) -> RepoSpec {
    let clone_url = with_canonical_host(clone_url, &host);
    RepoSpec {
        host,
        owner,
        repo,
        remote_url: redact_userinfo(&clone_url),
        clone_url,
    }
}

fn with_canonical_host(url: &str, host: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, String::new()),
        };
        let authority = match authority.rsplit_once('@') {
            Some((userinfo, _)) => format!("{userinfo}@{host}"),
            None => host.to_string(),
        };
        return format!("{scheme}://{authority}{path}");
    }
    if let Some((left, path)) = url.split_once(':') {
        return match left.rsplit_once('@') {
            Some((userinfo, _)) => format!("{userinfo}@{host}:{path}"),
            None => format!("{host}:{path}"),
        };
    }
    url.to_string()
}

/// Hide credentials in text we print.
///
/// `http` and `https` lose all userinfo, including a token used as the
/// username. `ssh` and scp-style URLs keep the login name and drop a password.
fn redact_userinfo(value: &str) -> String {
    if let Some((scheme, rest)) = value.split_once("://") {
        let (authority, path) = match rest.split_once('/') {
            Some((authority, path)) => (authority, format!("/{path}")),
            None => (rest, String::new()),
        };
        if matches!(scheme, "http" | "https") {
            if let Some((_, host)) = authority.rsplit_once('@') {
                return format!("{scheme}://{host}{path}");
            }
            return value.to_string();
        }
        if let Some((userinfo, host)) = authority.rsplit_once('@') {
            let username = userinfo
                .split_once(':')
                .map(|(user, _)| user)
                .unwrap_or(userinfo);
            if username.is_empty() {
                return format!("{scheme}://{host}{path}");
            }
            return format!("{scheme}://{username}@{host}{path}");
        }
        return value.to_string();
    }
    if let Some((userinfo, rest)) = value.rsplit_once('@') {
        if !rest.is_empty() {
            let username = userinfo
                .split_once(':')
                .map(|(user, _)| user)
                .unwrap_or(userinfo);
            if username.is_empty() {
                return rest.to_string();
            }
            return format!("{username}@{rest}");
        }
    }
    value.to_string()
}

fn normalize_branch(branch: Option<&str>) -> Result<Option<&str>> {
    let Some(value) = branch else {
        return Ok(None);
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(Error::validation(
            "Option '--branch <value>' requires a branch name".to_string(),
        ));
    }
    if trimmed.starts_with('-') {
        return Err(Error::validation(
            "Branch name cannot start with '-'".to_string(),
        ));
    }
    Ok(Some(trimmed))
}

fn parse_depth(depth: Option<&str>) -> Result<Option<u32>> {
    let Some(value) = depth else {
        return Ok(None);
    };
    match value.parse::<u32>() {
        Ok(number) if number > 0 => Ok(Some(number)),
        _ => Err(Error::validation(format!(
            "Option '--depth <value>' requires a positive integer, got '{value}'"
        ))),
    }
}

fn expand_absolute_path(value: &str, home: &Path, label: &str) -> Result<PathBuf> {
    let value = value.trim();
    let path = if value == "~" {
        home.to_path_buf()
    } else if let Some(rest) = value.strip_prefix("~/") {
        home.join(rest)
    } else if value.starts_with('~') {
        return Err(Error::validation(format!(
            "{label} '{value}' must be an absolute path or start with ~/."
        )));
    } else {
        PathBuf::from(value)
    };
    if !path.is_absolute() {
        return Err(Error::validation(format!(
            "{label} '{value}' must be an absolute path"
        )));
    }
    Ok(crate::paths::normalize(&path))
}

fn home_dir() -> Result<PathBuf> {
    match std::env::var_os("HOME") {
        Some(value) if !value.is_empty() => Ok(PathBuf::from(value)),
        _ => Err(Error::msg("HOME is not set")),
    }
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn nonempty_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

fn path_arg(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| Error::msg(format!("Path is not valid Unicode: {}", path.display())))
}

fn is_directory(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

#[cfg(test)]
fn is_staging_directory(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.as_encoded_bytes().starts_with(b".rabi-clone-"))
}

fn reap_stale_staging(parent: Option<&Path>, keep: &Path) {
    let Some(parent) = parent else {
        return;
    };
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let Some(pid) = staging_owner_pid(&entry.file_name()) else {
            continue;
        };
        if crate::process::process_is_alive(pid) {
            // A live pid may be a different process that reused it.
            // Leave the directory so this clone does not delete that checkout.
            continue;
        }
        let _ = fs::remove_dir_all(path);
    }
}

fn staging_owner_pid(name: &std::ffi::OsStr) -> Option<u32> {
    let name = name.to_str()?;
    let rest = name.strip_prefix(".rabi-clone-")?;
    let (pid, id) = rest.split_once('-')?;
    let pid = pid.parse::<u32>().ok()?;
    if pid == 0 {
        return None;
    }
    uuid::Uuid::parse_str(id).ok()?;
    Some(pid)
}

fn git_missing_repository(stderr: &str) -> bool {
    stderr.lines().any(|line| {
        let line = line.trim().to_ascii_lowercase();
        let line = line.strip_prefix("fatal:").map(str::trim).unwrap_or(&line);
        line.starts_with("not a git repository")
    })
}

fn git_inspect_error(path: &Path, stderr: &str, code: i32) -> Error {
    let detail = stderr.trim();
    let detail = if detail.is_empty() {
        format!("git exited with status {code}")
    } else {
        detail.to_string()
    };
    Error::msg(format!("Failed to inspect {}: {detail}", path.display()))
}

fn io_failure(error: &std::io::Error, message: String) -> Error {
    if error.kind() == std::io::ErrorKind::InvalidFilename {
        Error::validation(message)
    } else {
        Error::msg(message)
    }
}

fn invalid_source(input: &str) -> Error {
    let input = redact_userinfo(input);
    Error::validation(format!(
        "Invalid repository '{input}'. Expected an http(s) URL, an ssh:// URL, an scp-style git@host:owner/repo path, host/owner/repo, or owner/repo."
    ))
}

fn invalid_path(input: &str) -> Error {
    let input = redact_userinfo(input);
    Error::validation(format!(
        "Invalid repository '{input}'. Expected a path of <owner>/<repo> or <host>/<owner>/<repo>."
    ))
}

fn destination_appeared(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(input: &str) -> RepoSpec {
        parse_clone_source(input).unwrap_or_else(|error| panic!("{input}: {error}"))
    }

    #[test]
    fn canonical_name_applies_insteadof_without_taking_userinfo() {
        let root = temp_dir("insteadof-name");
        let repo = root.join("repo");
        fs::create_dir_all(&repo).expect("repo");
        git(&["init", "-b", "main"], Some(&repo));
        git(&["remote", "add", "origin", "gh:zed/aliased"], Some(&repo));
        git(
            &["config", "url.https://github.com/.insteadOf", "gh:"],
            Some(&repo),
        );
        assert_eq!(
            canonical_name_from_origin(&repo).as_deref(),
            Some("github.com/zed/aliased")
        );

        git(
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/example/widget.git",
            ],
            Some(&repo),
        );
        git(
            &["config", "--unset-all", "url.https://github.com/.insteadOf"],
            Some(&repo),
        );
        let mirror = format!("file://{}", root.join("bare").display());
        git(
            &[
                "config",
                &format!("url.{mirror}.insteadOf"),
                "https://github.com/example/widget.git",
            ],
            Some(&repo),
        );
        assert_eq!(
            canonical_name_from_origin(&repo).as_deref(),
            Some("github.com/example/widget"),
            "a file:// transport rewrite keeps the stored hosted name"
        );

        git(
            &["config", "--unset-all", &format!("url.{mirror}.insteadOf")],
            Some(&repo),
        );
        git(
            &[
                "config",
                "url.https://user:token@github.com/.insteadOf",
                "https://github.com/",
            ],
            Some(&repo),
        );
        let named = canonical_name_from_origin(&repo).expect("credential rewrite");
        assert_eq!(named, "github.com/example/widget");
        assert!(!named.contains("token"));

        git(
            &[
                "remote",
                "set-url",
                "origin",
                "https://user:secret@github.com/example/widget.git",
            ],
            Some(&repo),
        );
        let secret_mirror = format!("file://{}", root.join("mirror").display());
        git(
            &[
                "config",
                &format!("url.{secret_mirror}.insteadOf"),
                "https://user:secret@github.com/example/widget.git",
            ],
            Some(&repo),
        );
        ensure_matching_repository(&repo, &spec("https://github.com/example/widget.git"))
            .expect("stored origin still matches");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn canonical_remote_name_uses_host_owner_repo() {
        assert_eq!(
            canonical_remote_name("https://github.com/aku11i/phantom.git").as_deref(),
            Some("github.com/aku11i/phantom")
        );
        assert_eq!(
            canonical_remote_name("git@github.com:aku11i/phantom.git").as_deref(),
            Some("github.com/aku11i/phantom")
        );
        assert_eq!(
            canonical_remote_name("ssh://git@github.com/aku11i/phantom.git").as_deref(),
            Some("github.com/aku11i/phantom")
        );
        assert_eq!(canonical_remote_name("/tmp/repo.git"), None);
        assert_eq!(canonical_remote_name("file:///tmp/repo.git"), None);
        assert_eq!(canonical_remote_name("./repo"), None);
        assert_eq!(canonical_remote_name("backups/widget.git"), None);
        assert_eq!(canonical_remote_name("github.com/acme/widget"), None);
        assert_eq!(canonical_remote_name("acme/widget"), None);
        assert_eq!(
            canonical_remote_name("ssh://git@ghe.corp.example:2222/acme/tool.git").as_deref(),
            Some("ghe.corp.example:2222/acme/tool")
        );
    }

    #[test]
    fn parses_url_forms() {
        let cases = [
            (
                "https://github.com/aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
            (
                "https://github.com/aku11i/phantom",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom",
            ),
            (
                "https://github.com/aku11i/phantom/",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom",
            ),
            (
                "https://user:token@github.com/aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
            (
                "http://gitlab.example.com:8443/group/repo.git",
                "gitlab.example.com:8443",
                "group",
                "repo",
                "http://gitlab.example.com:8443/group/repo.git",
            ),
            (
                "ssh://git@github.com/aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "ssh://git@github.com/aku11i/phantom.git",
            ),
            (
                "ssh://git@github.com:22/aku11i/phantom.git",
                "github.com:22",
                "aku11i",
                "phantom",
                "ssh://git@github.com:22/aku11i/phantom.git",
            ),
            (
                "https://GitHub.com/aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
            (
                "git@GitHub.com:aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "git@github.com:aku11i/phantom.git",
            ),
            (
                "ssh://git@GitHub.com:22/aku11i/phantom.git",
                "github.com:22",
                "aku11i",
                "phantom",
                "ssh://git@github.com:22/aku11i/phantom.git",
            ),
            (
                "git@github.com:aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "git@github.com:aku11i/phantom.git",
            ),
            (
                "git@github.com:aku11i/phantom",
                "github.com",
                "aku11i",
                "phantom",
                "git@github.com:aku11i/phantom",
            ),
            (
                "github.com:aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "github.com:aku11i/phantom.git",
            ),
            (
                "github.com/aku11i/phantom",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
            (
                "github.com/aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
            (
                "aku11i/phantom",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
            (
                "aku11i/phantom.git",
                "github.com",
                "aku11i",
                "phantom",
                "https://github.com/aku11i/phantom.git",
            ),
        ];
        for (input, host, owner, repo, remote) in cases {
            let parsed = spec(input);
            assert_eq!(parsed.host, host, "{input}");
            assert_eq!(parsed.owner, owner, "{input}");
            assert_eq!(parsed.repo, repo, "{input}");
            assert_eq!(parsed.remote_url, remote, "{input}");
        }
    }

    #[test]
    fn rejects_invalid_repository_sources() {
        for input in [
            "",
            "aku11i",
            "https://github.com/aku11i",
            "https://github.com/a/b/c",
            "git@github.com:onlyrepo",
            "https://github.com/a/b?ref=1",
            "ftp://github.com/a/b",
            "file:///tmp/repo.git",
            "owner/../repo",
            "https://github.com/owner/../repo.git",
            "../etc/passwd",
            "not a repo",
        ] {
            let error = parse_clone_source(input).expect_err(input);
            assert!(matches!(error, Error::Validation(_)), "{input}: {error}");
            assert!(error.to_string().contains(input.trim()), "{error}");
        }
    }

    #[test]
    fn matches_equivalent_remote_urls() {
        let expected = spec("aku11i/phantom");
        assert!(same_repository(
            &expected,
            "https://github.com/aku11i/phantom.git"
        ));
        assert!(same_repository(&expected, "git@github.com:aku11i/phantom"));
        assert!(same_repository(
            &expected,
            "ssh://git@GitHub.com/aku11i/phantom.git"
        ));
        assert!(!same_repository(
            &expected,
            "https://gitlab.com/aku11i/phantom.git"
        ));
        assert!(!same_repository(
            &expected,
            "https://github.com/aku11i/other.git"
        ));
        assert!(!same_repository(&expected, "/tmp/local.git"));
        let port = spec("https://example.net:9443/acme/widget.git");
        assert_eq!(port.host, "example.net:9443");
        assert_eq!(
            Path::new("/ghq")
                .join(&port.host)
                .join(&port.owner)
                .join(&port.repo),
            PathBuf::from("/ghq/example.net:9443/acme/widget")
        );
        assert!(same_repository(
            &port,
            "https://Example.NET:9443/acme/widget"
        ));
        assert!(!same_repository(
            &port,
            "https://example.net:8443/acme/widget.git"
        ));
        assert!(!same_repository(
            &port,
            "https://example.net/acme/widget.git"
        ));
        let mixed = spec("https://GitHub.com/Aku11i/Phantom.git");
        assert!(same_repository(&mixed, "git@github.com:Aku11i/Phantom.git"));
        assert!(!same_repository(
            &mixed,
            "git@github.com:aku11i/phantom.git"
        ));
    }

    #[test]
    fn local_relative_origins_do_not_prove_hosted_repository_identity() {
        let expected = spec("acme/widget");
        for origin in [
            "acme/widget",
            "acme/widget.git",
            "github.com/acme/widget",
            "github.com/acme/widget.git",
            "./acme/widget",
            "../acme/widget",
        ] {
            assert!(!same_repository(&expected, origin), "{origin}");
        }
    }

    fn git_roots(values: &'static [&str]) -> impl FnOnce() -> Result<Vec<String>> {
        move || Ok(values.iter().map(|value| (*value).to_string()).collect())
    }

    #[test]
    fn root_resolution_follows_config_then_git_then_env_then_home() {
        let home = Path::new("/home/phantom");
        let mut warnings = Vec::new();
        let config = resolve_repository_root_from(
            Some("~/from-config"),
            git_roots(&["/from-git", "/second"]),
            Some("/from-env"),
            home,
            &mut warnings,
        )
        .expect("config");
        assert_eq!(config, PathBuf::from("/home/phantom/from-config"));
        assert!(warnings.is_empty());

        let git = resolve_repository_root_from(
            Some("  "),
            git_roots(&["~/from-git", "/second"]),
            Some("/from-env"),
            home,
            &mut warnings,
        )
        .expect("git");
        assert_eq!(git, PathBuf::from("/home/phantom/from-git"));

        let env = resolve_repository_root_from(
            None,
            git_roots(&[]),
            Some("~/from-env"),
            home,
            &mut warnings,
        )
        .expect("env");
        assert_eq!(env, PathBuf::from("/home/phantom/from-env"));

        let default =
            resolve_repository_root_from(None, git_roots(&[]), Some("   "), home, &mut warnings)
                .expect("default");
        assert_eq!(default, PathBuf::from("/home/phantom/ghq"));

        let relative = resolve_repository_root_from(
            Some("relative/root"),
            git_roots(&[]),
            None,
            home,
            &mut warnings,
        )
        .expect_err("relative");
        assert!(relative.to_string().contains("absolute"), "{relative}");
        let user = resolve_repository_root_from(
            None,
            git_roots(&["~other/src"]),
            None,
            home,
            &mut warnings,
        )
        .expect_err("user");
        assert!(user.to_string().contains("~/"), "{user}");
    }

    #[test]
    fn configured_root_skips_git_config_and_a_read_failure_falls_through() {
        let home = Path::new("/home/phantom");
        let mut warnings = Vec::new();
        let mut calls = 0;
        let configured = resolve_repository_root_from(
            Some("/from-config"),
            || {
                calls += 1;
                Err(Error::msg("broken gitconfig"))
            },
            Some("/from-env"),
            home,
            &mut warnings,
        )
        .expect("configured");
        assert_eq!(calls, 0);
        assert!(warnings.is_empty());
        assert_eq!(configured, PathBuf::from("/from-config"));

        let fallback = resolve_repository_root_from(
            None,
            || Err(Error::msg("Failed to read git config ghq.root: broken")),
            Some("~/from-env"),
            home,
            &mut warnings,
        )
        .expect("fallback");
        assert_eq!(fallback, PathBuf::from("/home/phantom/from-env"));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("broken"), "{warnings:?}");
        assert!(warnings[0].contains("~/ghq") || warnings[0].contains("GHQ_ROOT"));
    }

    #[test]
    fn depth_and_branch_options_are_validated() {
        let error = parse_depth(Some("0")).expect_err("zero");
        assert!(error.to_string().contains("positive integer"), "{error}");
        assert!(parse_depth(Some("-1")).is_err());
        assert!(parse_depth(Some("nope")).is_err());
        assert_eq!(parse_depth(Some("2")).expect("depth"), Some(2));
        assert_eq!(parse_depth(None).expect("none"), None);
        assert!(normalize_branch(Some("  ")).is_err());
        assert!(normalize_branch(Some("--branch")).is_err());
        assert_eq!(
            normalize_branch(Some("main")).expect("branch"),
            Some("main")
        );
    }

    #[test]
    fn exact_clone_location_requires_matching_repository_identity() {
        let root = temp_dir("repository-ref");
        let expected = spec("example.com/acme/widget");
        let reference = "example.com/acme/widget";
        assert!(matches!(
            resolve_named_repository(&root, &expected, reference),
            Err(Error::RepositoryNotFound(_))
        ));
        let checkout = root.join(reference);
        fs::create_dir_all(&checkout).expect("checkout");
        git(&["init", "-b", "main"], Some(&checkout));
        assert!(resolve_named_repository(&root, &expected, reference).is_err());
        git(
            &[
                "remote",
                "add",
                "origin",
                "https://example.com/acme/other.git",
            ],
            Some(&checkout),
        );
        assert!(resolve_named_repository(&root, &expected, reference).is_err());
        git(
            &[
                "remote",
                "set-url",
                "origin",
                "https://example.com/acme/widget.git",
            ],
            Some(&checkout),
        );
        assert_eq!(
            resolve_named_repository(&root, &expected, reference).expect("matching identity"),
            fs::canonicalize(&checkout).expect("canonical")
        );
        let relocated = root.join("elsewhere");
        fs::rename(&checkout, &relocated).expect("relocate");
        assert!(matches!(
            resolve_named_repository(&root, &expected, reference),
            Err(Error::RepositoryNotFound(_))
        ));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn filesystem_references_preserve_linked_checkout_resolution() {
        let root = temp_dir("filesystem-ref");
        let checkout = root.join("checkout");
        fs::create_dir_all(&checkout).expect("checkout");
        git(&["init", "-b", "main"], Some(&checkout));
        git(
            &["commit", "--allow-empty", "-m", "initial"],
            Some(&checkout),
        );
        let linked = root.join("linked");
        git(
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                linked.to_str().expect("path"),
            ],
            Some(&checkout),
        );
        let nested = linked.join("nested");
        fs::create_dir(&nested).expect("nested");
        let primary = fs::canonicalize(&checkout).expect("primary");
        for path in [checkout.clone(), linked.clone()] {
            assert_eq!(
                resolve_directory_reference(&path, "checkout").expect("filesystem path"),
                primary
            );
        }
        for path in [checkout.join(".git/objects"), linked.join(".git"), nested] {
            assert!(resolve_directory_reference(&path, "checkout").is_err());
        }
        let file = linked.join("file");
        fs::write(&file, "content").expect("file");
        assert!(matches!(
            resolve_directory_reference(&file, "file"),
            Err(Error::Validation(_))
        ));
        let missing = root.join("missing");
        assert!(matches!(
            resolve_directory_reference(&missing, "missing"),
            Err(Error::PathNotFound(_))
        ));
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn remote_url_redacts_userinfo_and_clone_url_keeps_it() {
        let parsed = spec("https://user:secret@github.com/aku11i/phantom.git");
        assert_eq!(parsed.remote_url, "https://github.com/aku11i/phantom.git");
        assert_eq!(
            parsed.clone_url,
            "https://user:secret@github.com/aku11i/phantom.git"
        );
        assert!(!parsed.remote_url.contains("secret"));
        let token = spec("https://ghp_SECRET123@example.com/acme/widget");
        assert_eq!(token.remote_url, "https://example.com/acme/widget");
        assert_eq!(
            token.clone_url,
            "https://ghp_SECRET123@example.com/acme/widget"
        );
        assert!(!token.remote_url.contains("ghp_SECRET123"));
        let ssh = spec("git@github.com:aku11i/phantom.git");
        assert_eq!(ssh.remote_url, "git@github.com:aku11i/phantom.git");
        assert_eq!(ssh.clone_url, ssh.remote_url);
        let ssh_password = spec("ssh://git:secret@github.com/aku11i/phantom.git");
        assert_eq!(
            ssh_password.remote_url,
            "ssh://git@github.com/aku11i/phantom.git"
        );
        assert!(ssh_password.clone_url.contains("secret"));
        assert!(!ssh_password.remote_url.contains("secret"));
        let mixed_host = spec("https://user:secret@GitHub.com/aku11i/phantom.git");
        assert_eq!(
            mixed_host.remote_url,
            "https://github.com/aku11i/phantom.git"
        );
        assert_eq!(
            mixed_host.clone_url,
            "https://user:secret@github.com/aku11i/phantom.git"
        );
        let empty_user = spec("https://@GitHub.com/aku11i/phantom.git");
        assert_eq!(empty_user.host, "github.com");
        assert_eq!(
            empty_user.clone_url,
            "https://@github.com/aku11i/phantom.git"
        );
        let empty_scp = spec("@GitHub.com:aku11i/phantom.git");
        assert_eq!(empty_scp.host, "github.com");
        assert_eq!(empty_scp.clone_url, "@github.com:aku11i/phantom.git");

        let error = parse_clone_source("https://ghp_SECRET123@example.com/only").expect_err("path");
        let message = error.to_string();
        assert!(!message.contains("ghp_SECRET123"), "{message}");
        assert!(message.contains("example.com"), "{message}");
        let file_url = parse_clone_source("file:///tmp/repo.git").expect_err("file");
        assert!(file_url.to_string().contains("file"), "{file_url}");
        let absolute =
            parse_clone_source("git@example.com:/acme/widget.git").expect_err("absolute");
        let absolute_message = absolute.to_string();
        assert!(absolute_message.contains("absolute"), "{absolute_message}");
        assert!(!same_repository(
            &spec("git@example.com:acme/widget.git"),
            "git@example.com:/acme/widget.git"
        ));
    }

    #[test]
    fn rename_replaces_an_empty_directory_and_keeps_a_non_empty_one() {
        let root = temp_dir("rename");
        let source = root.join("source");
        fs::create_dir(&source).expect("source");
        fs::write(source.join("file.txt"), "moved").expect("file");
        let empty = root.join("empty");
        fs::create_dir(&empty).expect("empty");
        fs::rename(&source, &empty).expect("replace empty");
        assert!(!source.exists());
        assert_eq!(
            fs::read_to_string(empty.join("file.txt")).expect("moved"),
            "moved"
        );

        let again = root.join("again");
        fs::create_dir(&again).expect("again");
        fs::write(again.join("new.txt"), "temp").expect("new");
        let error = fs::rename(&again, &empty).expect_err("non-empty");
        assert!(destination_appeared(&error), "{error}");
        assert!(again.exists());
        assert_eq!(
            fs::read_to_string(empty.join("file.txt")).expect("kept"),
            "moved"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn subdirectory_is_not_treated_as_the_repository() {
        let root = temp_dir("nested");
        let parent = root.join("github.com");
        fs::create_dir_all(&parent).expect("parent");
        git(
            &["init", "-b", "main", parent.to_str().expect("path")],
            None,
        );
        let nested = parent.join("example").join("widget");
        fs::create_dir_all(&nested).expect("nested");
        fs::write(nested.join("keep.txt"), "safe").expect("keep");
        let error =
            ensure_matching_repository(&nested, &spec("example/widget")).expect_err("nested");
        let message = error.to_string();
        assert!(message.contains("not a repository root"), "{message}");
        assert_eq!(
            fs::read_to_string(nested.join("keep.txt")).expect("kept"),
            "safe"
        );
        assert!(parent.join(".git").is_dir());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn lost_clone_race_keeps_the_existing_checkout() {
        let root = temp_dir("race");
        let destination = root.join("widget");
        fs::create_dir_all(&destination).expect("dest");
        git(
            &["init", "-b", "main", destination.to_str().expect("path")],
            None,
        );
        git(
            &[
                "-C",
                destination.to_str().expect("path"),
                "remote",
                "add",
                "origin",
                "https://user:secret@github.com/example/widget.git",
            ],
            None,
        );
        fs::write(destination.join("keep.txt"), "safe").expect("keep");
        let staging = root.join(".rabi-clone-test");
        fs::create_dir_all(&staging).expect("temp");
        fs::write(staging.join("new.txt"), "temp").expect("new");
        let spec = RepoSpec {
            host: "github.com".into(),
            owner: "example".into(),
            repo: "widget".into(),
            clone_url: "https://user:secret@github.com/example/widget.git".into(),
            remote_url: "https://github.com/example/widget.git".into(),
        };
        let (cloned, updated) =
            publish_clone(&staging, &destination, &spec, false).expect("publish");
        assert!(!cloned);
        assert!(!updated);
        assert_eq!(
            fs::read_to_string(destination.join("keep.txt")).expect("kept"),
            "safe"
        );
        assert!(!staging.exists());

        git(
            &[
                "-C",
                destination.to_str().expect("path"),
                "remote",
                "set-url",
                "origin",
                "https://user:secret@github.com/example/different.git",
            ],
            None,
        );
        let staging = root.join(".rabi-clone-mismatch");
        fs::create_dir_all(&staging).expect("temp");
        fs::write(staging.join("new.txt"), "temp").expect("new");
        let error = publish_clone(&staging, &destination, &spec, false).expect_err("mismatch");
        let message = error.to_string();
        assert!(message.contains("does not match"), "{message}");
        assert!(!message.contains("secret"), "{message}");
        assert_eq!(
            fs::read_to_string(destination.join("keep.txt")).expect("kept"),
            "safe"
        );
        assert!(!staging.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unsupported_staging_prefix_is_never_reaped() {
        let root = temp_dir("unsupported-staging");
        let owner = root.join("example.com/acme");
        let name = format!(".mabo-clone-{}-{}", unused_pid(), uuid::Uuid::new_v4());
        let checkout = owner.join(&name);
        fs::create_dir_all(checkout.join(".git")).unwrap();
        fs::write(checkout.join("payload"), "preserve\n").unwrap();
        assert!(!is_staging_directory(&checkout));
        assert_eq!(staging_owner_pid(std::ffi::OsStr::new(&name)), None);
        reap_stale_staging(Some(&owner), &owner.join("destination"));
        assert_eq!(
            fs::read_to_string(checkout.join("payload")).unwrap(),
            "preserve\n"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_clone_removes_only_its_staging_directory() {
        let root = temp_dir("fail-clone");
        let owner = root.join("github.com").join("example");
        let destination = owner.join("widget");
        let sibling = owner.join("keep");
        fs::create_dir_all(&sibling).expect("sibling");
        fs::write(sibling.join("keep.txt"), "safe").expect("keep");
        let spec = RepoSpec {
            host: "github.com".into(),
            owner: "example".into(),
            repo: "widget".into(),
            clone_url: root.join("missing.git").to_string_lossy().into_owned(),
            remote_url: "https://github.com/example/widget.git".into(),
        };
        let error = place_clone(&destination, &spec, None, None, false).expect_err("clone");
        assert!(error.to_string().contains("git clone"), "{error}");
        assert_eq!(
            fs::read_to_string(sibling.join("keep.txt")).expect("kept"),
            "safe"
        );
        assert!(!destination.exists());
        let leftovers = fs::read_dir(&owner)
            .expect("owner")
            .map(|entry| entry.expect("entry").file_name())
            .filter(|name| is_staging_directory(Path::new(name)))
            .collect::<Vec<_>>();
        assert!(leftovers.is_empty(), "{leftovers:?}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_clone_removes_empty_parents_it_created() {
        let root = temp_dir("parents");
        let existing = root.join("github.com").join("other");
        fs::create_dir_all(&existing).expect("existing");
        fs::write(existing.join("keep.txt"), "safe").expect("keep");
        let destination = root.join("example.net:8443").join("acme").join("widget");
        let spec = RepoSpec {
            host: "example.net:8443".into(),
            owner: "acme".into(),
            repo: "widget".into(),
            clone_url: root.join("missing.git").to_string_lossy().into_owned(),
            remote_url: "https://example.net:8443/acme/widget.git".into(),
        };
        let error = place_clone(&destination, &spec, None, None, false).expect_err("clone");
        assert!(error.to_string().contains("git clone"), "{error}");
        assert!(!root.join("example.net:8443").exists());
        assert_eq!(
            fs::read_to_string(existing.join("keep.txt")).expect("kept"),
            "safe"
        );
        assert!(root.join("github.com").is_dir());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_parent_creation_removes_dirs_deepest_first() {
        let root = temp_dir("mkdir-fail");
        let too_long = "x".repeat(256);
        let destination = root
            .join("example.com")
            .join("acme")
            .join(too_long)
            .join("widget");
        let error = create_missing_parents(&destination).expect_err("mkdir");
        assert!(matches!(error, Error::Validation(_)), "{error}");
        assert_eq!(error.exit_code(), crate::error::VALIDATION, "{error}");
        assert!(error.to_string().contains("example.com"), "{error}");
        assert!(
            !root.join("example.com").exists(),
            "shallow parent was left behind"
        );
        assert!(root.is_dir());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn place_clone_removes_dead_staging_dirs_and_keeps_live_ones() {
        let root = temp_dir("stale");
        let owner = root.join("github.com").join("example");
        fs::create_dir_all(&owner).expect("owner");
        let sibling = owner.join("keep");
        fs::create_dir_all(&sibling).expect("sibling");
        fs::write(sibling.join("keep.txt"), "safe").expect("keep");
        let dead = owner.join(format!(
            ".rabi-clone-{}-{}",
            unused_pid(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(dead.join(".git")).expect("dead");
        fs::write(dead.join("keep.txt"), "stale").expect("stale");
        let new_dead = owner.join(format!(
            ".rabi-clone-{}-{}",
            unused_pid(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(new_dead.join(".git")).expect("current staging");
        let live = owner.join(format!(
            ".rabi-clone-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&live).expect("live");
        fs::write(live.join("keep.txt"), "live").expect("live");
        let malformed = owner.join(".rabi-clone-not-a-pid");
        fs::create_dir_all(&malformed).expect("malformed");
        let destination = owner.join("widget");
        let spec = RepoSpec {
            host: "github.com".into(),
            owner: "example".into(),
            repo: "widget".into(),
            clone_url: root.join("missing.git").to_string_lossy().into_owned(),
            remote_url: "https://github.com/example/widget.git".into(),
        };
        let error = place_clone(&destination, &spec, None, None, false).expect_err("clone");
        assert!(error.to_string().contains("git clone"), "{error}");
        assert!(!dead.exists(), "dead staging directory was left behind");
        assert!(
            !new_dead.exists(),
            "current staging directory was left behind"
        );
        assert_eq!(
            fs::read_to_string(live.join("keep.txt")).expect("live kept"),
            "live"
        );
        assert!(malformed.is_dir(), "unparsed staging name was removed");
        assert_eq!(
            fs::read_to_string(sibling.join("keep.txt")).expect("sibling"),
            "safe"
        );

        git(
            &["init", "-b", "main", destination.to_str().expect("path")],
            None,
        );
        git(
            &[
                "-C",
                destination.to_str().expect("path"),
                "remote",
                "add",
                "origin",
                "https://github.com/example/widget.git",
            ],
            None,
        );
        let dead_again = owner.join(format!(
            ".rabi-clone-{}-{}",
            unused_pid(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dead_again).expect("dead again");
        fs::write(dead_again.join("keep.txt"), "stale").expect("stale again");
        let adopted = place_clone(&destination, &spec, None, None, false).expect("adopt");
        assert_eq!(adopted, (false, false));
        assert!(!dead_again.exists(), "adopt left a dead staging directory");
        assert!(live.is_dir());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn reserved_staging_name_is_rejected_and_a_plain_dot_name_is_kept() {
        let reserved = format!("example.com/acme/.rabi-clone-9-{}", uuid::Uuid::nil());
        let error = parse_clone_source(&reserved).expect_err("reserved");
        assert!(matches!(error, Error::Validation(_)), "{error}");
        assert_eq!(error.exit_code(), crate::error::VALIDATION, "{error}");
        assert!(error.to_string().contains("reserved"), "{error}");
        let kept = spec("example.com/acme/.rabi-clone-notes");
        assert_eq!(kept.repo, ".rabi-clone-notes");
    }

    #[test]
    fn git_config_failure_is_not_classified_as_a_missing_repository() {
        assert!(git_missing_repository(
            "fatal: not a git repository (or any of the parent directories): .git"
        ));
        assert!(git_missing_repository(
            "fatal: not a git repository: '/tmp/repo'"
        ));
        assert!(!git_missing_repository(
            "fatal: bad config line 1 in file /tmp/not a git repository/config"
        ));
    }

    #[test]
    fn reap_keeps_the_destination_and_removes_a_dead_sibling() {
        let root = temp_dir("keep-dest");
        let dead_pid = unused_pid();
        let destination = root.join(format!(".rabi-clone-{dead_pid}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(destination.join(".git")).expect("dest");
        fs::write(destination.join("keep.txt"), "safe").expect("keep");
        let sibling = root.join(format!(".rabi-clone-{dead_pid}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&sibling).expect("sibling");
        reap_stale_staging(Some(&root), &destination);
        assert_eq!(
            fs::read_to_string(destination.join("keep.txt")).expect("kept"),
            "safe"
        );
        assert!(!sibling.exists(), "dead sibling remained");
        let _ = fs::remove_dir_all(root);
    }

    fn unused_pid() -> u32 {
        for pid in 1_000_000..1_001_000 {
            let alive = std::process::Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|status| status.success())
                .unwrap_or(true);
            if !alive {
                return pid;
            }
        }
        panic!("no unused pid");
    }

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "rabi-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&path).expect("temp");
        path
    }

    fn git(args: &[&str], cwd: Option<&Path>) {
        let mut command = crate::test_git::command("git");
        command.args(args);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let output = command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Phantom")
            .env("GIT_AUTHOR_EMAIL", "phantom@example.com")
            .env("GIT_COMMITTER_NAME", "Phantom")
            .env("GIT_COMMITTER_EMAIL", "phantom@example.com")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "{args:?} {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
