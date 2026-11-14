//! GET-only external intake. Permission checks precede every task-store mutation.
use std::{collections::HashSet, path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::store::{Store, View};
use crate::{
    error::{Error, Result},
    github::RepoIdentity,
};

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct IssueSource {
    pub number: u64,
    pub author: String,
    pub url: String,
}
impl IssueSource {
    pub(super) fn valid(&self) -> bool {
        self.number > 0
            && login(&self.author)
            && self.url.starts_with("https://")
            && !self.url.chars().any(char::is_control)
    }
}

fn login(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
}

struct Client<'a> {
    root: &'a Path,
    repo: RepoIdentity,
}
impl<'a> Client<'a> {
    fn new(root: &'a Path) -> Result<Self> {
        let repo = crate::github::repo_info(root)
            .map_err(|_| Error::msg("Cannot resolve intake repository with gh"))?;
        for component in [&repo.owner, &repo.name] {
            if component.is_empty()
                || matches!(component.as_str(), "." | "..")
                || !component
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            {
                return Err(Error::msg("Invalid intake repository identity"));
            }
        }
        Ok(Self { root, repo })
    }

    fn get(&self, path: &str) -> Result<Value> {
        let response = crate::process::raw_with_timeout(
            "gh",
            &[
                "api",
                path,
                "--hostname",
                &self.repo.host,
                "--method",
                "GET",
            ],
            Some(self.root),
            &[("GH_PROMPT_DISABLED", "1")],
            Duration::from_secs(30),
        )
        .map_err(|_| Error::msg("Cannot read GitHub intake evidence"))?;
        if response.code != 0 {
            return Err(Error::msg("Cannot read GitHub intake evidence"));
        }
        serde_json::from_str(&response.stdout)
            .map_err(|_| Error::msg("Invalid GitHub intake response"))
    }

    fn path(&self, suffix: &str) -> String {
        format!("repos/{}/{}/{suffix}", self.repo.owner, self.repo.name)
    }

    fn permitted(&self, actor: &str) -> Result<bool> {
        if !login(actor) {
            return Ok(false);
        }
        let value = self.get(&self.path(&format!("collaborators/{actor}/permission")))?;
        let permission = value["permission"]
            .as_str()
            .ok_or_else(|| Error::msg("Invalid collaborator permission response"))?;
        Ok(matches!(permission, "write" | "maintain" | "admin"))
    }

    fn issue(&self, number: u64) -> Result<Accepted> {
        let value = self.get(&self.path(&format!("issues/{number}")))?;
        if value["number"].as_u64() != Some(number) || value.get("pull_request").is_some() {
            return Err(Error::msg(
                "Intake requires a matching issue, not a pull request",
            ));
        }
        let author = value
            .pointer("/user/login")
            .and_then(Value::as_str)
            .filter(|author| login(author))
            .ok_or_else(|| Error::msg("Issue author unavailable"))?;
        if !self.permitted(author)? {
            return Err(not_permitted());
        }
        let title = value["title"]
            .as_str()
            .ok_or_else(|| Error::msg("Issue title unavailable"))?;
        let body = match &value["body"] {
            Value::Null => "",
            Value::String(body) => body,
            _ => return Err(Error::msg("Invalid issue body")),
        };
        // Shared sanitization strips terminal/control sequences and redacts secrets
        // and local paths before imposing a UTF-8-safe 4096-byte cap per field.
        let title = super::publish::sanitized_with_marker(title.as_bytes(), self.root);
        let body = super::publish::sanitized_with_marker(body.as_bytes(), self.root);
        let source = IssueSource {
            number,
            author: author.into(),
            url: format!(
                "https://{}/{}/{}/issues/{number}",
                self.repo.host, self.repo.owner, self.repo.name
            ),
        };
        Ok(Accepted {
            intent: format!("{title}\n\n{body}\n\nIssue: {}", source.url),
            source,
        })
    }

    fn label_actor(&self, number: u64, label: &str) -> Result<Option<String>> {
        let mut actor = None;
        for page in 1..=MAX_PAGES {
            let events = self.get(&self.path(&format!(
                "issues/{number}/events?per_page={PAGE_SIZE}&page={page}"
            )))?;
            let events = events
                .as_array()
                .ok_or_else(|| Error::msg("Invalid issue events"))?;
            for event in events {
                let kind = event["event"]
                    .as_str()
                    .ok_or_else(|| Error::msg("Invalid issue event type"))?;
                if !matches!(kind, "labeled" | "unlabeled") {
                    continue;
                }
                let event_label = event
                    .pointer("/label/name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::msg("Issue label event has no label name"))?;
                if event_label != label {
                    continue;
                }
                // The events API enumerates oldest first. A missing actor on the
                // latest addition invalidates older authorized additions too.
                match kind {
                    "labeled" => {
                        actor = event
                            .pointer("/actor/login")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    }
                    "unlabeled" => actor = None,
                    _ => {}
                }
            }
            if events.len() < PAGE_SIZE {
                return Ok(actor);
            }
        }
        Err(Error::msg(
            "Issue event pagination exceeded the safety limit",
        ))
    }
}

struct Accepted {
    intent: String,
    source: IssueSource,
}
fn not_permitted() -> Error {
    Error::Status {
        code: "intake_not_permitted",
        message: "Issue intake requires write, maintain or admin repository permission".into(),
    }
}

pub(crate) fn new(
    root: &Path,
    number: u64,
    branch: Option<&str>,
    base: Option<&str>,
) -> Result<View> {
    let _git = super::boundary::git_read_scope();
    let accepted = Client::new(root)?.issue(number)?;
    Store::locate(root)?
        .create_issue(accepted.intent, branch, base, accepted.source)?
        .ok_or_else(|| Error::Status {
            code: "exists",
            message: "Issue is already linked to a task".into(),
        })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    pub plan: bool,
    pub items: Vec<Item>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Item {
    pub issue_number: u64,
    pub action: &'static str,
    pub task_id: Option<String>,
    pub reason: Option<&'static str>,
}

// Encode query values as bytes so labels containing &, #, + or Unicode cannot
// inject additional filters or pagination parameters.
fn query_value(value: &str) -> String {
    value
        .bytes()
        .map(|c| {
            if c.is_ascii_alphanumeric() || b"-_.~".contains(&c) {
                char::from(c).to_string()
            } else {
                format!("%{c:02X}")
            }
        })
        .collect()
}

pub(crate) fn intake(root: &Path, label: &str, limit: usize, dry_run: bool) -> Result<Report> {
    let _git = super::boundary::git_read_scope();
    let client = Client::new(root)?;
    let store = Store::locate(root)?;
    let mut report = Report {
        plan: dry_run,
        items: Vec::new(),
    };
    let mut accepted = Vec::new();
    let mut seen = HashSet::new();
    let mut examined = 0;
    for page in 1..=MAX_PAGES {
        let issues = client.get(&client.path(&format!(
            "issues?state=open&labels={}&per_page={PAGE_SIZE}&page={page}",
            query_value(label)
        )))?;
        let issues = issues
            .as_array()
            .ok_or_else(|| Error::msg("Invalid issue list"))?;
        for issue in issues {
            if issue.get("pull_request").is_some() {
                continue;
            }
            let number = issue["number"]
                .as_u64()
                .filter(|n| *n > 0)
                .ok_or_else(|| Error::msg("Invalid issue number"))?;
            if !seen.insert(number) {
                continue;
            }
            examined += 1;
            let mut item = Item {
                issue_number: number,
                action: "skipped",
                task_id: None,
                reason: None,
            };
            if store.linked_issue(number)? {
                item.reason = Some("already_linked");
            } else if let Some(actor) = client.label_actor(number, label)? {
                if client.permitted(&actor)? {
                    match client.issue(number) {
                        Ok(value) => accepted.push((report.items.len(), value)),
                        Err(Error::Status {
                            code: "intake_not_permitted",
                            ..
                        }) => item.reason = Some("intake_not_permitted"),
                        Err(error) => return Err(error),
                    }
                } else {
                    item.reason = Some("intake_not_permitted");
                }
            } else {
                item.reason = Some("intake_not_permitted");
            }
            report.items.push(item);
            if examined >= limit {
                break;
            }
        }
        if examined >= limit || issues.len() < PAGE_SIZE {
            break;
        }
        if page == MAX_PAGES {
            return Err(Error::msg("Issue pagination exceeded the safety limit"));
        }
    }
    // Finish all remote evidence reads before writing. Partial local store errors
    // are safe to retry because issue reservations include terminal tasks.
    for (index, value) in accepted {
        let item = &mut report.items[index];
        if dry_run {
            item.action = "would_create";
        } else if let Some(view) = store.create_issue(value.intent, None, None, value.source)? {
            item.action = "created";
            item.task_id = Some(view.task.id);
        } else {
            item.reason = Some("already_linked");
        }
    }
    Ok(report)
}
