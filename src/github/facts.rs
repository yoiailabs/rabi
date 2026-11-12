//! Read-only GitHub REST facts. Every request uses the shared GET-only helper.
use std::{collections::BTreeMap, path::Path};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

use super::{gh_api, repo_info, subject_link, RepoIdentity};
use crate::{
    error::{Error, Result},
    metadata::WorktreeLink,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    pub repository: String,
    pub default_branch: String,
    pub pr: Pull,
    pub files: Vec<File>,
    pub files_truncated: bool,
    pub check_runs: Vec<CheckRun>,
    pub statuses: Vec<CommitStatus>,
    pub reviews: Vec<Review>,
    pub review_decision: Option<String>,
    pub inconsistent: bool,
    pub final_head_sha: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Pull {
    pub number: u64,
    pub state: String,
    pub draft: bool,
    pub base_ref: String,
    pub base_sha: String,
    pub head_ref: String,
    pub head_sha: String,
    pub head_repository: Option<String>,
    pub same_repository: bool,
    pub mergeable: Option<bool>,
    pub mergeable_state: String,
    pub changed_files: u64,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct File {
    pub filename: String,
    pub previous_filename: Option<String>,
    pub status: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CheckRun {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub app: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CommitStatus {
    pub context: String,
    pub state: String,
    pub creator: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub(crate) struct Review {
    pub reviewer: String,
    pub state: String,
    pub id: u64,
}

#[derive(Deserialize)]
struct RawPr {
    number: u64,
    state: String,
    draft: bool,
    base: RawRef,
    head: RawRef,
    mergeable: Option<bool>,
    mergeable_state: Option<String>,
    changed_files: u64,
    additions: u64,
    deletions: u64,
}
#[derive(Deserialize)]
struct RawRef {
    r#ref: String,
    sha: String,
    repo: Option<RawRepo>,
}
#[derive(Deserialize)]
struct RawRepo {
    full_name: String,
}

pub(crate) struct Client<'a> {
    root: &'a Path,
    repo: RepoIdentity,
    prefix: String,
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|e| Error::msg(format!("Invalid GitHub facts: {e}")))
}

impl<'a> Client<'a> {
    pub fn new(root: &'a Path) -> Result<Self> {
        let repo = repo_info(root)?;
        let prefix = format!("repos/{}/{}", repo.owner, repo.name);
        Ok(Self { root, repo, prefix })
    }

    fn get(&self, suffix: &str) -> Result<Value> {
        let response = gh_api(
            self.root,
            &self.repo.host,
            &format!("{}{suffix}", self.prefix),
        )?;
        if response.code != 0 {
            return Err(Error::Status {
                code: if super::is_not_found(&response) {
                    "not_found"
                } else {
                    "failed"
                },
                message: format!("GitHub GET failed: {}", super::first_detail(&response)),
            });
        }
        serde_json::from_str(&response.stdout)
            .map_err(|e| Error::msg(format!("Invalid GitHub facts JSON: {e}")))
    }

    fn pages(&self, suffix: &str, key: Option<&str>, cap: Option<usize>) -> Result<Vec<Value>> {
        let mut all = Vec::new();
        for page in 1.. {
            let separator = if suffix.contains('?') { '&' } else { '?' };
            let value = self.get(&format!("{suffix}{separator}per_page=100&page={page}"))?;
            let rows = key
                .map_or(Some(&value), |key| value.get(key))
                .and_then(Value::as_array)
                .ok_or_else(|| Error::msg("Invalid GitHub facts: expected a paginated array"))?;
            all.extend(rows.iter().cloned());
            if rows.len() < 100 || cap.is_some_and(|cap| all.len() >= cap) {
                break;
            }
        }
        Ok(all)
    }

    pub fn recorded_number(&self, link: &WorktreeLink) -> Result<u64> {
        let number = link
            .number
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::validation("Recorded PR has no valid number"))?;
        if let Some(url) = &link.url {
            let expected = subject_link(&self.repo, number, "pull");
            if !expected
                .url
                .as_deref()
                .is_some_and(|expected| crate::selectors::source_urls_equal(url, expected))
            {
                return Err(Error::validation(
                    "Recorded PR URL belongs to a different repository",
                ));
            }
        }
        Ok(number)
    }

    pub fn branch_number(&self, branch: &str) -> Result<u64> {
        if branch.is_empty() {
            return Err(Error::validation("Workspace has no branch or recorded PR"));
        }
        let head = encode(&format!("{}:{branch}", self.repo.owner));
        let rows = self.pages(&format!("/pulls?state=open&head={head}"), None, None)?;
        let repository = format!("{}/{}", self.repo.owner, self.repo.name);
        let matching: Vec<_> = rows
            .iter()
            .filter(|row| {
                row.pointer("/head/ref").and_then(Value::as_str) == Some(branch)
                    && row
                        .pointer("/head/repo/full_name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(&repository))
            })
            .collect();
        match matching.as_slice() {
            [] => Err(Error::Status {
                code: "not_found",
                message: "No open PR for the workspace branch in the selected repository".into(),
            }),
            [row] => row
                .get("number")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .ok_or_else(|| Error::msg("Invalid GitHub facts: missing PR number")),
            _ => Err(Error::Status {
                code: "ambiguous",
                message: "Several open PRs match the workspace branch in the selected repository"
                    .into(),
            }),
        }
    }

    pub fn snapshot(&self, number: u64) -> Result<Snapshot> {
        let pull = format!("/pulls/{number}");
        let raw: RawPr = decode(self.get(&pull)?)?;
        if raw.number != number || !valid_sha(&raw.base.sha) || !valid_sha(&raw.head.sha) {
            return Err(Error::msg("Invalid GitHub facts: PR number or commit SHA"));
        }
        let repository = format!("{}/{}", self.repo.owner, self.repo.name);
        let repo = self.get("")?;
        let default_branch = repo
            .get("default_branch")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| Error::msg("Invalid GitHub facts: missing default branch"))?
            .into();
        let files = self
            .pages(&format!("{pull}/files"), None, Some(3000))?
            .into_iter()
            .map(|value| {
                #[derive(Deserialize)]
                struct RawFile {
                    filename: String,
                    previous_filename: Option<String>,
                    status: String,
                    additions: u64,
                    deletions: u64,
                }
                let file: RawFile = decode(value)?;
                Ok(File {
                    filename: file.filename,
                    previous_filename: file.previous_filename,
                    status: file.status,
                    additions: file.additions,
                    deletions: file.deletions,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let checks = self.pages(
            &format!("/commits/{}/check-runs?filter=all", raw.head.sha),
            Some("check_runs"),
            None,
        )?;
        let check_runs = checks
            .into_iter()
            .map(|value| {
                #[derive(Deserialize)]
                struct RawCheck {
                    id: u64,
                    name: String,
                    status: String,
                    conclusion: Option<String>,
                    app: Option<App>,
                    started_at: Option<String>,
                    completed_at: Option<String>,
                }
                #[derive(Deserialize)]
                struct App {
                    slug: Option<String>,
                }
                let run: RawCheck = decode(value)?;
                Ok(CheckRun {
                    id: run.id,
                    name: run.name,
                    status: run.status,
                    conclusion: run.conclusion,
                    app: run.app.and_then(|app| app.slug),
                    started_at: run.started_at,
                    completed_at: run.completed_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let statuses = self
            .pages(&format!("/commits/{}/statuses", raw.head.sha), None, None)?
            .into_iter()
            .map(|value| {
                #[derive(Deserialize)]
                struct RawStatus {
                    context: String,
                    state: String,
                    creator: Option<User>,
                }
                let status: RawStatus = decode(value)?;
                Ok(CommitStatus {
                    context: status.context,
                    state: status.state,
                    creator: status.creator.map(|user| user.login),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut latest = BTreeMap::new();
        for value in self.pages(&format!("{pull}/reviews"), None, None)? {
            #[derive(Deserialize)]
            struct RawReview {
                id: u64,
                state: String,
                user: User,
            }
            let review: RawReview = decode(value)?;
            if review.state == "PENDING" {
                continue;
            }
            let key = review.user.login.to_ascii_lowercase();
            let entry = latest.entry(key).or_insert_with(|| Review {
                reviewer: review.user.login.clone(),
                state: review.state.clone(),
                id: review.id,
            });
            if review.id > entry.id {
                *entry = Review {
                    reviewer: review.user.login,
                    state: review.state,
                    id: review.id,
                };
            }
        }
        let final_pr: RawPr = decode(self.get(&pull)?)?;
        if !valid_sha(&final_pr.head.sha) {
            return Err(Error::msg("Invalid GitHub facts: final head SHA"));
        }
        let head_repository = raw.head.repo.map(|repo| repo.full_name);
        Ok(Snapshot {
            repository: repository.clone(),
            default_branch,
            files_truncated: files.len() as u64 != raw.changed_files || files.len() >= 3000,
            inconsistent: raw.head.sha != final_pr.head.sha,
            final_head_sha: final_pr.head.sha,
            pr: Pull {
                number,
                state: raw.state,
                draft: raw.draft,
                base_ref: raw.base.r#ref,
                base_sha: raw.base.sha,
                head_ref: raw.head.r#ref,
                head_sha: raw.head.sha,
                same_repository: head_repository
                    .as_ref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&repository)),
                head_repository,
                mergeable: raw.mergeable,
                mergeable_state: raw.mergeable_state.unwrap_or_else(|| "unknown".into()),
                changed_files: raw.changed_files,
                additions: raw.additions,
                deletions: raw.deletions,
            },
            files,
            check_runs,
            statuses,
            reviews: latest.into_values().collect(),
            review_decision: None,
        })
    }

    /// Resolve the policy by tree entries, preserving task risk's regular-file rules.
    /// Reading blobs over REST does not install objects into the local repository.
    pub fn policy(&self, base: &str) -> Result<Option<Vec<u8>>> {
        let tree = self.get(&format!("/git/trees/{base}"))?;
        let entry = tree_entry(&tree, ".rabi")?;
        let Some(entry) = entry else {
            return Ok(None);
        };
        if entry.get("mode").and_then(Value::as_str) != Some("040000") {
            return Ok(None);
        }
        let tree = self.get(&format!("/git/trees/{}", object_sha(entry)?))?;
        let Some(entry) = tree_entry(&tree, "policy.toml")? else {
            return Ok(None);
        };
        if !matches!(
            entry.get("mode").and_then(Value::as_str),
            Some("100644" | "100755")
        ) {
            return Err(Error::msg(
                "Invalid risk policy .rabi/policy.toml: must be a regular file",
            ));
        }
        let blob = self.get(&format!("/git/blobs/{}", object_sha(entry)?))?;
        if blob.get("encoding").and_then(Value::as_str) != Some("base64") {
            return Err(Error::msg("Invalid GitHub policy blob encoding"));
        }
        decode_base64(
            blob.get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::msg("Invalid GitHub policy blob"))?,
        )
        .map(Some)
    }
}

#[derive(Deserialize)]
struct User {
    login: String,
}
fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit())
}
fn object_sha(value: &Value) -> Result<&str> {
    value
        .get("sha")
        .and_then(Value::as_str)
        .filter(|sha| valid_sha(sha))
        .ok_or_else(|| Error::msg("Invalid GitHub tree SHA"))
}
fn tree_entry<'a>(tree: &'a Value, path: &str) -> Result<Option<&'a Value>> {
    if tree.get("truncated").and_then(Value::as_bool) != Some(false) {
        return Err(Error::msg("Incomplete GitHub policy tree"));
    }
    let entries = tree
        .get("tree")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::msg("Invalid GitHub policy tree"))?;
    Ok(entries
        .iter()
        .find(|entry| entry.get("path").and_then(Value::as_str) == Some(path)))
}
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn decode_base64(value: &str) -> Result<Vec<u8>> {
    let bytes: Vec<_> = value.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let invalid = || Error::msg("Invalid base64 GitHub policy blob");
    if bytes.len() % 4 != 0 {
        return Err(invalid());
    }
    let mut output = Vec::new();
    for (index, group) in bytes.as_chunks::<4>().0.iter().enumerate() {
        let digit = |b| match b {
            b'A'..=b'Z' => Some(b - b'A'),
            b'a'..=b'z' => Some(b - b'a' + 26),
            b'0'..=b'9' => Some(b - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        };
        let a = digit(group[0]).ok_or_else(invalid)?;
        let b = digit(group[1]).ok_or_else(invalid)?;
        let c = if group[2] == b'=' {
            0
        } else {
            digit(group[2]).ok_or_else(invalid)?
        };
        let d = if group[3] == b'=' {
            0
        } else {
            digit(group[3]).ok_or_else(invalid)?
        };
        if (group[2] == b'=' && (group[3] != b'=' || b & 15 != 0))
            || (group[3] == b'=' && c & 3 != 0)
            || (group.contains(&b'=') && index + 1 != bytes.len() / 4)
        {
            return Err(invalid());
        }
        output.push((a << 2) | (b >> 4));
        if group[2] != b'=' {
            output.push((b << 4) | (c >> 2));
        }
        if group[3] != b'=' {
            output.push((c << 6) | d);
        }
    }
    Ok(output)
}

impl Snapshot {
    pub fn print(&self) {
        use crate::output::{outln, visible_text};
        outln!(
            "PR #{}: {}{}",
            self.pr.number,
            visible_text(&self.pr.state),
            if self.pr.draft { " (draft)" } else { "" }
        );
        outln!(
            "Repository: {} Head repository: {} Same repository: {}",
            visible_text(&self.repository),
            visible_text(self.pr.head_repository.as_deref().unwrap_or("unknown")),
            self.pr.same_repository
        );
        outln!("Default branch: {}", visible_text(&self.default_branch));
        outln!(
            "Base: {} ({})",
            visible_text(&self.pr.base_ref),
            self.pr.base_sha
        );
        outln!(
            "Head: {} ({})",
            visible_text(&self.pr.head_ref),
            self.pr.head_sha
        );
        outln!(
            "Mergeable: {:?} ({})",
            self.pr.mergeable,
            visible_text(&self.pr.mergeable_state)
        );
        outln!(
            "Files: {}/{} Truncated: {} +{} -{}",
            self.files.len(),
            self.pr.changed_files,
            self.files_truncated,
            self.pr.additions,
            self.pr.deletions
        );
        for file in &self.files {
            outln!(
                "File: {} {} Previous: {} +{} -{}",
                visible_text(&file.status),
                visible_text(&file.filename),
                visible_text(file.previous_filename.as_deref().unwrap_or("none")),
                file.additions,
                file.deletions
            );
        }
        for run in &self.check_runs {
            outln!(
                "Check: {} [{}] {}:{} id={} started={} completed={}",
                visible_text(&run.name),
                visible_text(run.app.as_deref().unwrap_or("unknown")),
                visible_text(&run.status),
                visible_text(run.conclusion.as_deref().unwrap_or("none")),
                run.id,
                visible_text(run.started_at.as_deref().unwrap_or("none")),
                visible_text(run.completed_at.as_deref().unwrap_or("none"))
            );
        }
        for status in &self.statuses {
            outln!(
                "Status: {} {} Creator: {}",
                visible_text(&status.context),
                visible_text(&status.state),
                visible_text(status.creator.as_deref().unwrap_or("unknown"))
            );
        }
        for review in &self.reviews {
            outln!(
                "Review: {} {}",
                visible_text(&review.reviewer),
                visible_text(&review.state)
            );
        }
        outln!(
            "Review decision: {}",
            visible_text(self.review_decision.as_deref().unwrap_or("unavailable"))
        );
        outln!(
            "Head inconsistent: {} Final head: {}",
            self.inconsistent,
            self.final_head_sha
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn base64_table() {
        for (encoded, decoded) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v\n", "foo"),
            ("c2NoZW1hPTEK", "schema=1\n"),
        ] {
            assert_eq!(decode_base64(encoded).unwrap(), decoded.as_bytes());
        }
        for bad in [
            "Z", "====", "Z===", "Zg=a", "Zg==Zg==", "Zh==", "Zm9=", "!!!!",
        ] {
            assert!(decode_base64(bad).is_err(), "{bad}");
        }
    }
}
