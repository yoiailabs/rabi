pub(crate) mod facts;

use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::git;
use crate::metadata::{self, WorktreeLink};
use crate::process;

/// GitHub source information, resolved before checkout mutation.
pub(crate) struct PullRequest {
    pub number: u64,
    pub fork: bool,
    pub head_ref: String,
    pub head_repo: String,
    pub base_ref: Option<String>,
}

pub(crate) struct Subject {
    pub link: WorktreeLink,
    pub title: Option<String>,
    pub pull: Option<PullRequest>,
}

pub(crate) fn resolve(directory: &Path, reference: &str, pr: bool) -> Result<Subject> {
    let link = metadata::parse_link(reference)?;
    let number = link
        .number
        .ok_or_else(|| Error::validation("Expected a PR or issue number or URL"))?;
    let repo = repo_info(directory)?;
    if let Some(url) = link.url.as_deref() {
        let expected = subject_link(&repo, number, if pr { "pull" } else { "issues" });
        if !expected
            .url
            .as_deref()
            .is_some_and(|expected| crate::selectors::source_urls_equal(url, expected))
        {
            return Err(Error::validation(
                "Source URL must identify the selected repository and source kind",
            ));
        }
    }
    let issue = fetch_issue(
        directory,
        &repo.host,
        &repo.owner,
        &repo.name,
        &number.to_string(),
    )?
    .ok_or_else(|| Error::Status {
        code: "not_found",
        message: format!("GitHub subject #{number} not found or inaccessible"),
    })?;
    if pr != issue.pull_request.is_some() {
        return Err(Error::validation(
            "Use --pr for a pull request or --issue for an issue",
        ));
    }
    Ok(Subject {
        link: subject_link(&repo, number, if pr { "pull" } else { "issues" }),
        title: issue.title,
        pull: issue.pull_request,
    })
}

struct Issue {
    title: Option<String>,
    pull_request: Option<PullRequest>,
}

/// `refs/pull/<n>/head` still exists after GitHub deletes the head branch.
fn pull_head_refspec(number: u64, local_branch: &str) -> String {
    format!("refs/pull/{number}/head:refs/heads/{local_branch}")
}

pub(crate) enum PullFetch {
    Branch,
    Fork,
    DeletedHead,
}

pub(crate) fn fetch_pull_head(
    git_root: &Path,
    pull: &PullRequest,
    local_branch: &str,
) -> Result<PullFetch> {
    if pull.fork {
        git::fetch(
            git_root,
            "origin",
            &pull_head_refspec(pull.number, local_branch),
        )?;
        git::clear_branch_upstream(git_root, local_branch);
        return Ok(PullFetch::Fork);
    }
    if git::remote_has_branch(git_root, "origin", &pull.head_ref)? {
        match fetch_head_branch(git_root, &pull.head_ref, local_branch) {
            Ok(_) => return Ok(PullFetch::Branch),
            Err(error) if missing_remote_ref(&error) => {}
            Err(error) => return Err(error),
        }
    }
    git::fetch(
        git_root,
        "origin",
        &pull_head_refspec(pull.number, local_branch),
    )?;
    git::clear_branch_upstream(git_root, local_branch);
    Ok(PullFetch::DeletedHead)
}

fn fetch_head_branch(git_root: &Path, head_ref: &str, local_branch: &str) -> Result<String> {
    let tracking = format!("+refs/heads/{head_ref}:refs/remotes/origin/{head_ref}");
    git::fetch(git_root, "origin", &tracking)?;
    let upstream = format!("origin/{head_ref}");
    git::ensure_branch(git_root, local_branch, &format!("refs/remotes/{upstream}"))?;
    git::set_branch_tracking(git_root, local_branch, "origin", head_ref)?;
    Ok(upstream)
}

fn missing_remote_ref(error: &Error) -> bool {
    error.to_string().contains("couldn't find remote ref")
}

fn subject_link(repo: &RepoIdentity, number: u64, kind: &str) -> WorktreeLink {
    WorktreeLink {
        number: Some(number),
        url: Some(format!(
            "https://{}/{}/{}/{kind}/{number}",
            repo.host, repo.owner, repo.name
        )),
    }
}

#[derive(Debug, Clone)]
struct RepoIdentity {
    owner: String,
    name: String,
    /// Hostname for `gh api --hostname`, from `gh repo view`'s web `url`.
    host: String,
}

fn repo_info(cwd: &Path) -> Result<RepoIdentity> {
    let captured = process::raw(
        "gh",
        &["repo", "view", "--json", "url,owner,name"],
        Some(cwd),
    )?;
    if captured.code != 0 {
        return Err(Error::msg(format!(
            "Failed to get repository info: {}",
            first_detail(&captured)
        )));
    }
    let value: Value = serde_json::from_str(&captured.stdout)
        .map_err(|error| Error::msg(format!("Failed to get repository info: {error}")))?;
    let owner = value
        .pointer("/owner/login")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::msg("Failed to get repository info: missing owner"))?;
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::msg("Failed to get repository info: missing name"))?;
    let url = value
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::msg("Failed to get repository info: missing url"))?;
    Ok(RepoIdentity {
        owner: owner.to_string(),
        name: name.to_string(),
        host: host_from_web_url(url)?,
    })
}

/// Host only, with no port, from the web URL `gh repo view` returns.
fn host_from_web_url(url: &str) -> Result<String> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(Error::msg(format!(
            "Failed to get repository info: url '{url}' has no host"
        )));
    };
    if !matches!(scheme, "http" | "https") {
        return Err(Error::msg(format!(
            "Failed to get repository info: url '{url}' is not an http(s) URL"
        )));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let hostport = match authority.rsplit_once('@') {
        Some((_, host)) if !host.is_empty() => host,
        Some(_) => {
            return Err(Error::msg(format!(
                "Failed to get repository info: url '{url}' has no host"
            )))
        }
        None => authority,
    };
    let host = if let Some(inside) = hostport.strip_prefix('[') {
        let end = inside.find(']').ok_or_else(|| {
            Error::msg(format!(
                "Failed to get repository info: url '{url}' has no host"
            ))
        })?;
        &inside[..end]
    } else if let Some((host, port)) = hostport.rsplit_once(':') {
        if host.is_empty() || port.is_empty() || !port.chars().all(|char| char.is_ascii_digit()) {
            return Err(Error::msg(format!(
                "Failed to get repository info: url '{url}' has no host"
            )));
        }
        host
    } else {
        hostport
    };
    if host.is_empty() {
        return Err(Error::msg(format!(
            "Failed to get repository info: url '{url}' has no host"
        )));
    }
    Ok(host.to_ascii_lowercase())
}

fn fetch_issue(
    directory: &Path,
    host: &str,
    owner: &str,
    repo: &str,
    number: &str,
) -> Result<Option<Issue>> {
    let parsed = number.parse::<u64>().ok().filter(|value| *value > 0);
    let Some(parsed) = parsed else {
        return Err(Error::msg(format!(
            "Invalid issue or pull request number: {number}"
        )));
    };
    let path = format!("repos/{owner}/{repo}/issues/{parsed}");
    let captured = gh_api(directory, host, &path)?;
    if captured.code != 0 {
        if is_not_found(&captured) {
            return Ok(None);
        }
        return Err(Error::msg(first_detail(&captured).to_string()));
    }
    let value: Value =
        serde_json::from_str(&captured.stdout).map_err(|error| Error::msg(error.to_string()))?;
    let pull_request = if value.get("pull_request").is_some() {
        Some(fetch_pull(directory, host, owner, repo, parsed)?)
    } else {
        None
    };
    Ok(Some(Issue {
        title: value
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
        pull_request,
    }))
}

fn fetch_pull(
    directory: &Path,
    host: &str,
    owner: &str,
    repo: &str,
    number: u64,
) -> Result<PullRequest> {
    let path = format!("repos/{owner}/{repo}/pulls/{number}");
    let captured = gh_api(directory, host, &path)?;
    if captured.code != 0 {
        return Err(Error::msg(first_detail(&captured).to_string()));
    }
    let value: Value =
        serde_json::from_str(&captured.stdout).map_err(|error| Error::msg(error.to_string()))?;
    let head_ref = value
        .pointer("/head/ref")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let head_repo = value
        .pointer("/head/repo/full_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base_repo = value
        .pointer("/base/repo/full_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Ok(PullRequest {
        number,
        fork: !head_repo.is_empty() && head_repo != base_repo,
        head_ref,
        head_repo,
        base_ref: value
            .pointer("/base/ref")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PullRequestStatus {
    Open,
    Closed,
    Merged,
    Unavailable,
}

// Preservation probes never prompt, fetch, or turn operational failures into proof.
const PRESERVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const PR_FIELDS: &str = "state,number,url,headRefOid,headRefName";

fn retirement_gh(cwd: &Path, args: &[&str]) -> Option<Value> {
    // Tests and callers may shorten the deadline, but cannot extend the bound.
    let timeout = std::env::var("RABI_GH_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(std::time::Duration::from_millis)
        .unwrap_or(PRESERVATION_TIMEOUT)
        .min(PRESERVATION_TIMEOUT);
    let captured = process::raw_with_timeout(
        "gh",
        args,
        Some(cwd),
        &[("GH_PROMPT_DISABLED", "1")],
        timeout,
    )
    .ok()?;
    if captured.code != 0 {
        return None;
    }
    serde_json::from_str(&captured.stdout).ok()
}

fn pr_status(value: Option<&Value>) -> PullRequestStatus {
    match value
        .and_then(|value| value.get("state"))
        .and_then(Value::as_str)
    {
        Some("OPEN") => PullRequestStatus::Open,
        Some("CLOSED") => PullRequestStatus::Closed,
        Some("MERGED") => PullRequestStatus::Merged,
        _ => PullRequestStatus::Unavailable,
    }
}

/// Per-command remote snapshots. Local tips and ancestry are always rechecked.
#[derive(Debug, Clone, Default)]
pub(crate) struct RetirementQueries {
    repo: Option<Option<RepoIdentity>>,
    pulls: std::collections::BTreeMap<String, Option<Value>>,
    heads: std::collections::BTreeMap<String, Option<Value>>,
}

impl RetirementQueries {
    fn repo(&mut self, cwd: &Path) -> Option<RepoIdentity> {
        self.repo
            .get_or_insert_with(|| {
                let value = retirement_gh(cwd, &["repo", "view", "--json", "url,owner,name"])?;
                let owner = value.pointer("/owner/login")?.as_str()?;
                let name = value.get("name")?.as_str()?;
                let url = value.get("url")?.as_str()?;
                let host = host_from_web_url(url).ok()?;
                // Reconstruct the URL to reject malformed identities and extra paths.
                let expected = format!("https://{host}/{owner}/{name}");
                if !url.eq_ignore_ascii_case(&expected) || owner.is_empty() || name.is_empty() {
                    return None;
                }
                Some(RepoIdentity {
                    owner: owner.to_owned(),
                    name: name.to_owned(),
                    host,
                })
            })
            .clone()
    }

    fn pull(&mut self, cwd: &Path, link: &WorktreeLink) -> Option<Value> {
        let target = if let Some(url) = &link.url {
            url.clone()
        } else {
            link.number.filter(|number| *number > 0)?.to_string()
        };
        // A known repository identity lets URL and number links share a snapshot.
        let key = self
            .repo
            .as_ref()
            .and_then(Option::as_ref)
            .and_then(|repo| {
                let number = link.number?;
                let expected = subject_link(repo, number, "pull").url?;
                (link
                    .url
                    .as_ref()
                    .is_none_or(|url| crate::selectors::source_urls_equal(url, &expected)))
                .then_some(expected)
            })
            .unwrap_or_else(|| target.clone());
        self.pulls
            .entry(key)
            .or_insert_with(|| retirement_gh(cwd, &["pr", "view", &target, "--json", PR_FIELDS]))
            .clone()
    }

    pub(crate) fn status(&mut self, cwd: &Path, link: &WorktreeLink) -> PullRequestStatus {
        let repo = self.repo(cwd);
        if link.url.is_none() && repo.is_none() {
            return PullRequestStatus::Unavailable;
        }
        pr_status(self.pull(cwd, link).as_ref())
    }

    pub(crate) fn merged_contains(
        &mut self,
        cwd: &Path,
        branch: &str,
        link: Option<&WorktreeLink>,
    ) -> bool {
        let Some(repo) = self.repo(cwd) else {
            return false;
        };
        if let Some(link) = link {
            let Some(number) = link.number.filter(|number| *number > 0) else {
                return false;
            };
            let Some(expected) = subject_link(&repo, number, "pull").url else {
                return false;
            };
            if link
                .url
                .as_ref()
                .is_some_and(|url| !crate::selectors::source_urls_equal(url, &expected))
            {
                return false;
            }
            let Some(value) = self.pull(cwd, link) else {
                return false;
            };
            if !valid_merged_pr(&value, &repo)
                || value.get("number").and_then(Value::as_u64) != Some(number)
            {
                return false;
            }
            value
                .get("headRefOid")
                .and_then(Value::as_str)
                .is_some_and(|head| git::retirement_pr_contains(cwd, branch, head, true))
        } else {
            let value = self.heads.entry(branch.to_owned()).or_insert_with(|| {
                retirement_gh(
                    cwd,
                    &[
                        "pr", "list", "--head", branch, "--state", "merged", "--limit", "2",
                        "--json", PR_FIELDS,
                    ],
                )
            });
            let Some(values) = value.as_ref().and_then(Value::as_array) else {
                return false;
            };
            if values.len() != 1 {
                return false;
            }
            let value = &values[0];
            valid_merged_pr(value, &repo)
                && value.get("headRefName").and_then(Value::as_str) == Some(branch)
                && value
                    .get("headRefOid")
                    .and_then(Value::as_str)
                    .is_some_and(|head| git::retirement_pr_contains(cwd, branch, head, false))
        }
    }
}

fn valid_merged_pr(value: &Value, repo: &RepoIdentity) -> bool {
    let Some(number) = value
        .get("number")
        .and_then(Value::as_u64)
        .filter(|number| *number > 0)
    else {
        return false;
    };
    let Some(url) = value.get("url").and_then(Value::as_str) else {
        return false;
    };
    pr_status(Some(value)) == PullRequestStatus::Merged
        && subject_link(repo, number, "pull")
            .url
            .as_deref()
            .is_some_and(|expected| crate::selectors::source_urls_equal(url, expected))
}

/// Query a PR state with the same bounded, non-prompting capture as retirement.
pub fn pull_request_state(cwd: &Path, target: &str) -> PullRequestStatus {
    pr_status(retirement_gh(cwd, &["pr", "view", target, "--json", "state"]).as_ref())
}

/// `owner/repo` from `gh repo view` when that command can name the checkout.
pub fn current_repo_name(cwd: &Path) -> Option<String> {
    let Ok(captured) = process::raw(
        "gh",
        &["repo", "view", "--json", "nameWithOwner"],
        Some(cwd),
    ) else {
        return None;
    };
    if captured.code != 0 {
        return None;
    }
    let Ok(value) = serde_json::from_str::<Value>(&captured.stdout) else {
        return None;
    };
    value
        .get("nameWithOwner")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
}

fn gh_api(directory: &Path, host: &str, path: &str) -> Result<process::Captured> {
    let args = ["api", path, "--hostname", host];
    process::raw("gh", &args, Some(directory))
}

fn is_not_found(captured: &process::Captured) -> bool {
    let detail = format!("{} {}", captured.stderr, captured.stdout).to_ascii_lowercase();
    detail.contains("404") || detail.contains("not found")
}

fn first_detail(captured: &process::Captured) -> String {
    let stderr = captured.stderr.trim();
    if !stderr.is_empty() {
        stderr.to_string()
    } else {
        captured.stdout.trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_url_host_drops_the_port() {
        assert_eq!(
            host_from_web_url("https://ghe.corp.example/acme/tool").expect("host"),
            "ghe.corp.example"
        );
        assert_eq!(
            host_from_web_url("https://ghe.corp.example:8443/acme/tool").expect("port"),
            "ghe.corp.example"
        );
        assert_eq!(
            host_from_web_url("https://github.com/acme/tool").expect("github"),
            "github.com"
        );
        assert_eq!(
            host_from_web_url("https://GitHub.com/acme/tool").expect("case"),
            "github.com"
        );
    }
}
