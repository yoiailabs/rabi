//! Shared selector and observed workspace state. All reads leave records untouched.
use std::path::Path;

use crate::context::Context;
use crate::error::{Error, Result};
use crate::{git, metadata, paths, worktree::WorktreeInfo};

/// Parsed user input can be resolved again without revalidating observed names.
/// Internal callers construct `Name` directly for names obtained from Git or metadata.
#[derive(Debug, Clone)]
pub(crate) enum Selector {
    Name(String),
    Special(String),
    Path(String),
}

impl Selector {
    pub(crate) fn parse(input: &str) -> Result<Self> {
        if Path::new(input).is_absolute() || input.starts_with("./") || input.starts_with("../") {
            Ok(Self::Path(input.to_owned()))
        } else if input == "."
            || input.starts_with("pr:")
            || input.starts_with("issue:")
            || input.starts_with("https://")
            || input.starts_with("http://")
        {
            Ok(Self::Special(input.to_owned()))
        } else {
            crate::worktree::validate_worktree_name(input)?;
            Ok(Self::Name(input.to_owned()))
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Name(value) | Self::Special(value) | Self::Path(value) => value,
        }
    }

    /// Remove historically acknowledges a recorded checkout whose Git
    /// registration is gone but whose files remain. It must leave those files
    /// and the record alone. Ordinary selectors and gc exclude this state.
    pub(crate) fn unregistered_record(&self, context: &Context) -> Result<Option<WorktreeInfo>> {
        let registered = git::list_worktrees(&context.git_root)?;
        let common = git::canonical_common_dir(&context.git_root)?;
        let records = metadata::load_strict(&context.git_root)?;
        let candidates = records
            .iter()
            .filter_map(|(path, record)| {
                let path = Path::new(path);
                if paths::same_location(path, &context.git_root)
                    || registered
                        .iter()
                        .any(|tree| paths::same_location(Path::new(&tree.path), path))
                {
                    return None;
                }
                let mut tree = crate::worktree::orphaned_info(context, path, record, &common);
                tree.orphaned = false;
                Some(tree)
            })
            .collect::<Vec<_>>();
        let found = match self {
            Self::Name(name) => candidates
                .into_iter()
                .filter(|tree| tree.name == *name)
                .collect(),
            Self::Path(path) => {
                let path = absolute_path(path)?;
                candidates
                    .into_iter()
                    .filter(|tree| paths::same_location(Path::new(&tree.path), &path))
                    .collect()
            }
            Self::Special(input) if input == "." => {
                // A stale gitfile cannot tell Git which checkout contains cwd.
                // The explicitly selected repository's records still can.
                let cwd = std::env::current_dir().map_err(|error| Error::msg(error.to_string()))?;
                candidates
                    .into_iter()
                    .filter(|tree| {
                        let path = Path::new(&tree.path);
                        paths::same_location(path, &cwd) || paths::is_strictly_inside(&cwd, path)
                    })
                    .collect()
            }
            Self::Special(input) => matches(context, &candidates, input)?,
        };
        match found.as_slice() {
            [] => Ok(None),
            [tree] => Ok(Some(tree.clone())),
            _ => Err(Error::Status {
                code: "ambiguous",
                message: format!(
                    "Selector '{}' matches multiple recorded workspaces",
                    self.as_str()
                ),
            }),
        }
    }

    pub(crate) fn resolve_including_orphaned(
        &self,
        context: &Context,
        exclude_default: bool,
        expected_path: Option<&Path>,
    ) -> Result<WorktreeInfo> {
        if let Self::Name(name) = self {
            return crate::worktree::find_including_orphaned(
                context,
                name,
                exclude_default,
                expected_path,
            );
        }
        let input = self.as_str();
        let listed = crate::worktree::list_worktrees(context, false, true)?;
        let mut found = if let Self::Path(path) = self {
            let path = absolute_path(path)?;
            listed
                .into_iter()
                .filter(|tree| paths::same_location(Path::new(&tree.path), &path))
                .collect()
        } else {
            matches(context, &listed, input)?
        };
        found.retain(|tree| {
            expected_path.is_none_or(|path| paths::same_location(Path::new(&tree.path), path))
        });
        match found.as_slice() {
            [tree] if exclude_default && tree.is_default => Err(Error::Status {
                code: "primary",
                message: "The primary checkout cannot be selected for this operation".into(),
            }),
            [tree] => Ok(tree.clone()),
            [] => Err(Error::WorktreeNotFound(input.into())),
            _ => Err(Error::Status {
                code: "ambiguous",
                message: format!(
                    "Selector '{input}' matches multiple workspaces; use a workspace name"
                ),
            }),
        }
    }
}

// Absolute selectors remain usable after an earlier removal deletes cwd.
fn absolute_path(path: &str) -> Result<std::path::PathBuf> {
    let path = Path::new(path);
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()
            .map_err(|error| Error::msg(error.to_string()))?
            .join(path))
    }
}

/// Interpret a user selector at the command boundary, then resolve it.
pub(crate) fn resolve_including_orphaned(
    context: &Context,
    input: &str,
    exclude_default: bool,
    expected_path: Option<&Path>,
) -> Result<WorktreeInfo> {
    Selector::parse(input)?.resolve_including_orphaned(context, exclude_default, expected_path)
}

pub(crate) fn resolve(
    context: &Context,
    input: &str,
    exclude_default: bool,
    expected_path: Option<&Path>,
) -> Result<WorktreeInfo> {
    let found = resolve_including_orphaned(context, input, exclude_default, expected_path)?;
    crate::worktree::require_live(found, input)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RepoJson {
    pub root: String,
    pub name: String,
}

pub(crate) fn repo_json(root: &Path) -> RepoJson {
    RepoJson {
        root: root.to_string_lossy().into_owned(),
        name: crate::projects::canonical_name(root),
    }
}

pub(crate) fn is_current(path: &Path) -> bool {
    std::env::current_dir()
        .ok()
        .and_then(|cwd| git::top_level(&cwd).ok())
        .is_some_and(|top| paths::same_location(&top, path))
}

pub(crate) fn matches(
    context: &Context,
    listed: &[WorktreeInfo],
    selector: &str,
) -> Result<Vec<WorktreeInfo>> {
    if selector == "." {
        let cwd = std::env::current_dir().map_err(|e| Error::msg(e.to_string()))?;
        let root = git::git_root(&cwd).map_err(|_| Error::WorktreeNotFound(".".into()))?;
        if !paths::same_location(&root, &context.git_root) {
            return Err(Error::validation(
                "Selector '.' cannot be combined with a different -R/--repo; use a workspace name",
            ));
        }
        let top = git::top_level(&cwd)?;
        return Ok(listed
            .iter()
            .filter(|tree| paths::same_location(&top, Path::new(&tree.path)))
            .cloned()
            .collect());
    }
    let (pr, link) = if let Some(number) = selector.strip_prefix("pr:") {
        (true, metadata::parse_link(number)?)
    } else if let Some(number) = selector.strip_prefix("issue:") {
        (false, metadata::parse_link(number)?)
    } else {
        let (kind, _) = metadata::link_target(selector).ok_or_else(|| {
            Error::validation("Selector URL must identify a pull request or issue")
        })?;
        (
            matches!(kind, "pull" | "pulls"),
            metadata::parse_link(selector)?,
        )
    };
    if link.number.is_none()
        || (selector.contains(':') && !selector.starts_with("http") && link.url.is_some())
    {
        return Err(Error::validation(
            "Selector requires a positive PR or issue number",
        ));
    }
    Ok(listed
        .iter()
        .filter(|tree| {
            let stored = if pr {
                &tree.metadata.links.pr
            } else {
                &tree.metadata.links.issue
            };
            stored.as_ref().is_some_and(|stored| {
                if link.url.is_some() {
                    return links_equal(context, stored, &link, pr);
                }
                stored.number == link.number
            })
        })
        .cloned()
        .collect())
}

/// Link identity shared by URL selectors and allocation retries. Bare numbers
/// belong to the selected repository; the terminal pair supplies kind/number.
pub(crate) fn links_equal(
    context: &Context,
    actual: &metadata::WorktreeLink,
    wanted: &metadata::WorktreeLink,
    pr: bool,
) -> bool {
    let actual = link_identity(context, actual, pr);
    actual.is_some() && actual == link_identity(context, wanted, pr)
}

fn link_identity(
    context: &Context,
    link: &metadata::WorktreeLink,
    pr: bool,
) -> Option<(String, bool, u64)> {
    match &link.url {
        Some(url) => url_link_identity(url),
        None => Some((repo_json(&context.git_root).name, pr, link.number?)),
    }
}

fn url_link_identity(url: &str) -> Option<(String, bool, u64)> {
    let parts = url_link_parts(url)?;
    Some((
        parts.repository.to_owned(),
        matches!(parts.kind, "pull" | "pulls"),
        parts.number,
    ))
}

/// Source validation keeps exact scheme/kind/number spelling while treating
/// the repository's host, owner, and name as case-insensitive identities.
pub(crate) fn source_urls_equal(actual: &str, expected: &str) -> bool {
    let (Some(actual), Some(expected)) = (url_link_parts(actual), url_link_parts(expected)) else {
        return false;
    };
    let (Some(actual_repo), Some(expected_repo)) = (
        repository_parts(actual.repository),
        repository_parts(expected.repository),
    ) else {
        return false;
    };
    actual.scheme == expected.scheme
        && actual.kind == expected.kind
        && actual.number_text == expected.number_text
        && actual_repo
            .iter()
            .zip(expected_repo)
            .all(|(actual, expected)| actual.eq_ignore_ascii_case(expected))
}

fn repository_parts(repository: &str) -> Option<[&str; 3]> {
    let (host, path) = repository.split_once('/')?;
    let (owner, repo) = path.split_once('/')?;
    Some([host, owner, repo])
}

struct UrlLinkParts<'a> {
    scheme: &'a str,
    repository: &'a str,
    kind: &'a str,
    number: u64,
    number_text: &'a str,
}

fn url_link_parts(url: &str) -> Option<UrlLinkParts<'_>> {
    let (kind, number) = metadata::link_target(url)?;
    let clean = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let (scheme, path) = clean.split_once("://")?;
    let (prefix, number_text) = path.rsplit_once('/')?;
    let (repository, _) = prefix.rsplit_once('/')?;
    Some(UrlLinkParts {
        scheme,
        repository,
        kind,
        number,
        number_text,
    })
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct UpstreamJson {
    pub r#ref: String,
    pub ahead: Option<u64>,
    pub behind: Option<u64>,
    pub gone: bool,
}

pub(crate) fn upstream(tree: &WorktreeInfo) -> Option<UpstreamJson> {
    let path = Path::new(&tree.path);
    if tree.detached || tree.orphaned || !path.is_dir() {
        return None;
    }
    let branch_ref = format!("refs/heads/{}", tree.branch);
    let refs = git::output(
        &[
            "for-each-ref",
            "--format=%(refname)%00%(upstream)",
            &branch_ref,
        ],
        Some(path),
        true,
    )
    .ok()?;
    let reference = refs
        .lines()
        .filter_map(|line| line.split_once('\0'))
        .find(|(name, _)| *name == branch_ref)?
        .1
        .to_owned();
    if reference.is_empty() {
        return None;
    }
    let gone = match git::raw_git(path, &["show-ref", "--verify", "--quiet", &reference])
        .ok()?
        .code
    {
        0 => false,
        1 => true,
        _ => return None,
    };
    let counts = if gone {
        None
    } else {
        git::output(
            &[
                "rev-list",
                "--left-right",
                "--count",
                &format!("HEAD...{reference}"),
            ],
            Some(path),
            true,
        )
        .ok()
    };
    let mut counts = counts.as_deref().unwrap_or("").split_whitespace();
    Some(UpstreamJson {
        r#ref: reference,
        ahead: counts.next().and_then(|s| s.parse().ok()),
        behind: counts.next().and_then(|s| s.parse().ok()),
        gone,
    })
}

#[cfg(test)]
mod tests {
    use super::source_urls_equal;

    #[test]
    fn source_url_matching_preserves_kind_and_number_validation() {
        let expected = "https://github.com/acme/demo/pull/7";
        assert!(source_urls_equal(
            "https://GitHub.com/Acme/Demo/pull/7/?plain=1#details",
            expected
        ));
        for actual in [
            "https://github.com/acme/demo/pull/8",
            "https://github.com/acme/demo/pull/07",
            "https://github.com/acme/demo/pull/+7",
            "https://github.com/acme/demo/pull/0",
            "https://github.com/acme/demo/pull/7/files",
            "https://github.com/acme/demo/pull/7/issues/7",
            "https://github.com/acme/demo/issues/7",
            "https://github.com/acme/demo/pulls/7",
            "https://github.com/acme/demo/Pull/7",
        ] {
            assert!(!source_urls_equal(actual, expected), "{actual}");
        }
    }
}
