//! Read-only diagnostics: collect a snapshot, then evaluate it without I/O.
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

// Reasons are fixed strings. Never propagate subprocess output: a remote URL,
// gh error, API body or response header may contain credentials.
type Probe<T> = std::result::Result<T, &'static str>;

struct Facts {
    repository: Option<String>,
    default_branch: Option<String>,
    unavailable: Vec<(&'static str, &'static str)>,
    rules: Probe<Vec<Value>>,
    rulesets: Probe<Vec<Probe<Value>>>,
    scopes: Probe<Option<Vec<String>>>,
    workflow_permissions: Probe<String>,
    push_ssh: Probe<bool>,
    workflows: Probe<Vec<String>>,
}

#[derive(Debug, Serialize)]
struct Finding {
    code: &'static str,
    level: &'static str,
    message: String,
    detail: String,
}

impl Finding {
    fn warning(code: &'static str, message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code,
            level: "warning",
            message: message.into(),
            detail: detail.into(),
        }
    }
    fn info(code: &'static str, message: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            code,
            level: "info",
            message: message.into(),
            detail: detail.into(),
        }
    }
    fn unavailable(check: &str, reason: &str) -> Self {
        Self::info(
            "could_not_check",
            format!("Could not check {check}: {reason}"),
            check,
        )
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    repository: Option<String>,
    default_branch: Option<String>,
    findings: Vec<Finding>,
    counts: Counts,
}

#[derive(Serialize)]
struct Counts {
    warning: usize,
    info: usize,
}

impl Report {
    pub(crate) fn print(&self) {
        use crate::output::{outln, visible_text};
        for finding in &self.findings {
            outln!(
                "{} {}: {}",
                if finding.level == "warning" {
                    "WARN"
                } else {
                    "INFO"
                },
                finding.code,
                visible_text(&finding.message)
            );
        }
        if self.findings.is_empty() {
            outln!("No findings");
        }
        outln!(
            "{} warning(s), {} info finding(s)",
            self.counts.warning,
            self.counts.info
        );
    }
}

fn evaluate(facts: Facts) -> Report {
    let mut findings = Vec::new();
    for (check, reason) in facts.unavailable {
        findings.push(Finding::unavailable(check, reason));
    }
    let rules = [
        (
            "required_status_checks",
            "required_checks_not_enforced",
            false,
            "No required-status-checks ruleset rule protects the default branch; rabi's preflight remains a gate",
        ),
        (
            "non_fast_forward",
            "force_push_allowed",
            false,
            "No non-fast-forward ruleset rule protects the default branch",
        ),
        (
            "deletion",
            "deletion_allowed",
            false,
            "No deletion ruleset rule protects the default branch",
        ),
        (
            "merge_queue",
            "merge_queue",
            true,
            "A merge queue is enabled; rabi's merge flow does not support it yet",
        ),
    ];
    for (kind, code, present, message) in rules {
        match &facts.rules {
            Ok(rules) if rules.iter().any(|rule| rule["type"] == kind) == present => {
                findings.push(Finding::warning(
                    code,
                    message,
                    "Default-branch rulesets only; classic branch protection is not inspected",
                ))
            }
            Err(reason) => findings.push(Finding::unavailable(code, reason)),
            _ => {}
        }
    }
    match facts.rulesets {
        Ok(rulesets) => {
            for ruleset in rulesets {
                match ruleset {
                    Ok(rule)
                        if applies(&rule, facts.default_branch.as_deref())
                            && rule["current_user_can_bypass"] != "never" =>
                    {
                        let name = rule["name"].as_str().unwrap_or("unnamed ruleset");
                        findings.push(Finding::warning(
                            "ruleset_bypass",
                            format!("The current user may bypass ruleset {name}"),
                            "An active ruleset targets the default branch",
                        ));
                    }
                    Err(reason) => findings.push(Finding::unavailable("ruleset_bypass", reason)),
                    _ => {}
                }
            }
        }
        Err(reason) => findings.push(Finding::unavailable("ruleset_bypass", reason)),
    }
    match facts.scopes {
        Ok(Some(scopes)) if scopes.iter().any(|scope| scope == "workflow") => {
            findings.push(Finding::warning(
                "token_workflow_scope",
                "Anything that can push with this token can rewrite CI definitions",
                "The workflow OAuth scope is present",
            ))
        }
        Ok(None) => findings.push(Finding::info(
            "token_workflow_scope",
            "Could not determine whether the token can push workflow changes",
            "No X-OAuth-Scopes header; fine-grained and app token permissions cannot be inferred",
        )),
        Err(reason) => findings.push(Finding::unavailable("token_workflow_scope", reason)),
        _ => {}
    }
    match facts.workflow_permissions {
        Ok(permission) if permission == "write" => findings.push(Finding::warning(
            "default_workflow_permissions",
            "Default workflow token permissions are write",
            "Workflows without explicit permissions inherit this default",
        )),
        Err(reason) => findings.push(Finding::unavailable("default_workflow_permissions", reason)),
        _ => {}
    }
    match facts.push_ssh {
        Ok(true) => findings.push(Finding::info("remote_push_ssh", "SSH keys cannot be limited to exclude workflow changes; prefer HTTPS with a token without workflow permission", "The default branch remote has an SSH push URL")),
        Err(reason) => findings.push(Finding::unavailable("remote_push_ssh", reason)),
        _ => {}
    }
    match facts.workflows {
        Ok(workflows) => {
            for path in workflows {
                findings.push(Finding::info(
                    "workflow_review_manually",
                    format!("{path}: review its triggers and permissions manually; rabi doctor does not parse workflow YAML"),
                    "Committed workflow file on the locally available default-branch tip",
                ));
            }
        }
        Err(reason) => findings.push(Finding::unavailable("workflow_review_manually", reason)),
    }
    let warning = findings
        .iter()
        .filter(|finding| finding.level == "warning")
        .count();
    let info = findings.len() - warning;
    Report {
        repository: facts.repository,
        default_branch: facts.default_branch,
        findings,
        counts: Counts { warning, info },
    }
}

// Ruleset ref_name conditions use fnmatch patterns. Match common '*'/'?'
// patterns exactly; unsupported classes/escapes conservatively include a rule,
// and cannot establish an exclusion. A false positive is preferable to silence.
fn pattern(pattern: &str, branch: &str) -> Option<bool> {
    if matches!(pattern, "~ALL" | "~DEFAULT_BRANCH") {
        return Some(true);
    }
    if pattern.contains(['[', ']', '\\']) {
        return None;
    }
    if pattern.contains("**") {
        return None;
    }
    let text = branch.as_bytes();
    let mut previous = vec![false; text.len() + 1];
    previous[0] = true;
    for byte in pattern.bytes() {
        let mut next = vec![false; text.len() + 1];
        if byte == b'*' {
            next[0] = previous[0];
        }
        for i in 1..=text.len() {
            next[i] = match byte {
                b'*' => previous[i] || (text[i - 1] != b'/' && next[i - 1]),
                b'?' => text[i - 1] != b'/' && previous[i - 1],
                _ => byte == text[i - 1] && previous[i - 1],
            };
        }
        previous = next;
    }
    Some(previous[text.len()])
}

fn applies(rule: &Value, branch: Option<&str>) -> bool {
    if rule["enforcement"] != "active"
        || rule["target"]
            .as_str()
            .is_some_and(|target| target != "branch")
    {
        return false;
    }
    let Some(branch) = branch else {
        return false;
    };
    let branch = format!("refs/heads/{branch}");
    let conditions = &rule["conditions"]["ref_name"];
    let included = conditions["include"].as_array().is_none_or(|patterns| {
        patterns.iter().any(|value| {
            value
                .as_str()
                .is_none_or(|value| pattern(value, &branch).unwrap_or(true))
        })
    });
    let excluded = conditions["exclude"].as_array().is_some_and(|patterns| {
        patterns.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|value| pattern(value, &branch) == Some(true))
        })
    });
    included && !excluded
}

fn api(root: &Path, repo: &crate::github::RepoIdentity, path: &str) -> Probe<Value> {
    let output =
        crate::github::gh_api(root, &repo.host, path).map_err(|_| "GitHub CLI unavailable")?;
    if output.code != 0 {
        return Err(api_failure(&output));
    }
    serde_json::from_str(&output.stdout).map_err(|_| "GitHub returned invalid JSON")
}

fn api_failure(output: &crate::process::Captured) -> &'static str {
    if output.stderr.contains("403") || output.stdout.contains("403") {
        "GitHub returned 403 (not permitted)"
    } else if output.stderr.contains("404") || output.stdout.contains("404") {
        "GitHub returned 404 (missing or inaccessible)"
    } else {
        "GitHub request failed or authentication is unavailable"
    }
}

fn array(value: Value) -> Probe<Vec<Value>> {
    value
        .as_array()
        .cloned()
        .ok_or("GitHub returned an unexpected response shape")
}

fn scopes(root: &Path, repo: &crate::github::RepoIdentity) -> Probe<Option<Vec<String>>> {
    let output = crate::github::gh_api_headers(root, &repo.host, "user")
        .map_err(|_| "GitHub CLI unavailable")?;
    if output.code != 0 {
        return Err(api_failure(&output));
    }
    // Stop at the header/body boundary; never inspect, retain or print user data.
    for line in output.stdout.lines().take_while(|line| !line.is_empty()) {
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("x-oauth-scopes") {
                return Ok(Some(
                    value
                        .split(',')
                        .map(|scope| scope.trim().to_owned())
                        .collect(),
                ));
            }
        }
    }
    Ok(None)
}

fn rulesets(
    root: &Path,
    repo: &crate::github::RepoIdentity,
    prefix: &str,
) -> Probe<Vec<Probe<Value>>> {
    let list = api_list(root, repo, &format!("{prefix}/rulesets"))?;
    let mut result = Vec::new();
    for item in list {
        if item["enforcement"] == "disabled" || item["enforcement"] == "evaluate" {
            continue;
        }
        let detail = if let Some(id) = item["id"].as_u64() {
            api(root, repo, &format!("{prefix}/rulesets/{id}")).and_then(|detail| {
                if detail["enforcement"]
                    .as_str()
                    .is_some_and(|value| matches!(value, "active" | "disabled" | "evaluate"))
                {
                    Ok(detail)
                } else {
                    Err("GitHub returned no ruleset enforcement state")
                }
            })
        } else {
            Err("Ruleset has no numeric ID")
        };
        result.push(detail);
    }
    Ok(result)
}

fn api_list(root: &Path, repo: &crate::github::RepoIdentity, path: &str) -> Probe<Vec<Value>> {
    let mut result = Vec::new();
    for page in 1..=100 {
        let items = array(api(
            root,
            repo,
            &format!("{path}?per_page=100&page={page}"),
        )?)?;
        let last = items.len() < 100;
        result.extend(items);
        if last {
            return Ok(result);
        }
    }
    Err("GitHub pagination exceeded the inspection limit")
}

// Escape each URL path segment, including branch slashes and percent signs.
fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

pub(crate) fn inspect(root: &Path) -> Report {
    let unknown = "Repository identity or default branch is unavailable";
    let mut facts = Facts {
        repository: None,
        default_branch: None,
        unavailable: Vec::new(),
        rules: Err(unknown),
        rulesets: Err(unknown),
        scopes: Err(unknown),
        workflow_permissions: Err(unknown),
        push_ssh: Err(unknown),
        workflows: Err(unknown),
    };
    match crate::github::repo_info(root) {
        Ok(repo) => {
            facts.repository = Some(format!("{}/{}/{}", repo.host, repo.owner, repo.name));
            let prefix = format!("repos/{}/{}", segment(&repo.owner), segment(&repo.name));
            match api(root, &repo, &prefix) {
                Ok(value) => match value["default_branch"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                {
                    Some(branch) => facts.default_branch = Some(branch.to_owned()),
                    None => facts
                        .unavailable
                        .push(("default_branch", "Repository has no default branch")),
                },
                Err(reason) => facts.unavailable.push(("default_branch", reason)),
            }
            if let Some(branch) = &facts.default_branch {
                facts.rules = api_list(
                    root,
                    &repo,
                    &format!("{prefix}/rules/branches/{}", segment(branch)),
                );
                facts.rulesets = rulesets(root, &repo, &prefix);
            }
            facts.scopes = scopes(root, &repo);
            facts.workflow_permissions = api(
                root,
                &repo,
                &format!("{prefix}/actions/permissions/workflow"),
            )
            .and_then(|value| {
                value["default_workflow_permissions"]
                    .as_str()
                    .filter(|value| matches!(*value, "read" | "write"))
                    .map(str::to_owned)
                    .ok_or("GitHub returned no workflow permission default")
            });
        }
        Err(_) => facts.unavailable.push((
            "repository",
            "GitHub repository identity or authentication is unavailable",
        )),
    }
    // API failures do not suppress locally available diagnostics. No fetch.
    if facts.default_branch.is_none() {
        facts.default_branch = local_default(root).ok();
    }
    if let Some(branch) = &facts.default_branch {
        let remote = default_remote(root, branch);
        facts.push_ssh = push_remote(root, branch).and_then(|remote| {
            let urls = git_text(root, &["remote", "get-url", "--push", "--all", &remote])?;
            Ok(urls.lines().any(is_ssh))
        });
        facts.workflows = workflows(root, branch, remote.as_deref().ok());
    }
    evaluate(facts)
}

fn git_read(root: &Path, args: &[&str]) -> Probe<Vec<u8>> {
    let mut guarded = vec!["--no-lazy-fetch"];
    guarded.extend_from_slice(args);
    let output = crate::process::raw_bytes(
        "git",
        &guarded,
        Some(root),
        &[("LC_ALL", "C"), ("GIT_OPTIONAL_LOCKS", "0")],
        None,
    )
    .map_err(|_| "Git inspection could not run")?;
    if !output.status.success() {
        return Err("Git data is unavailable locally (Git 2.44 or later is required; no fetch was attempted)");
    }
    Ok(output.stdout)
}

fn git_text(root: &Path, args: &[&str]) -> Probe<String> {
    String::from_utf8(git_read(root, args)?)
        .map(|text| text.trim().to_owned())
        .map_err(|_| "Git data is not UTF-8")
}

fn local_default(root: &Path) -> Probe<String> {
    if let Ok(reference) = git_text(
        root,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(branch) = reference.strip_prefix("refs/remotes/origin/") {
            return Ok(branch.to_owned());
        }
    }
    for branch in ["main", "master"] {
        if tip(root, &format!("refs/heads/{branch}")).is_ok() {
            return Ok(branch.to_owned());
        }
    }
    Err("No locally known default branch")
}

fn default_remote(root: &Path, branch: &str) -> Probe<String> {
    if let Ok(remote) = git_text(
        root,
        &["config", "--get", &format!("branch.{branch}.remote")],
    ) {
        if !remote.is_empty() && remote != "." {
            return Ok(remote);
        }
    }
    let remotes = git_text(root, &["remote"])?;
    for remote in remotes.lines() {
        if git_text(
            root,
            &[
                "symbolic-ref",
                "--quiet",
                &format!("refs/remotes/{remote}/HEAD"),
            ],
        )
        .is_ok_and(|reference| reference == format!("refs/remotes/{remote}/{branch}"))
        {
            return Ok(remote.to_owned());
        }
    }
    if remotes.lines().any(|remote| remote == "origin") {
        return Ok("origin".into());
    }
    let names = remotes.lines().collect::<Vec<_>>();
    if names.len() == 1 {
        return Ok(names[0].to_owned());
    }
    Err("Default branch remote could not be determined")
}

fn push_remote(root: &Path, branch: &str) -> Probe<String> {
    let reason = "push remote could not be determined";
    let mut selected = "origin".to_owned();
    for key in [
        format!("branch.{branch}.pushRemote"),
        "remote.pushDefault".into(),
        format!("branch.{branch}.remote"),
    ] {
        if let Ok(remote) = git_text(root, &["config", "--get", &key]) {
            if !remote.is_empty() {
                selected = remote;
                break;
            }
        }
    }
    let remotes = git_text(root, &["remote"]).map_err(|_| reason)?;
    if selected != "." && remotes.lines().any(|remote| remote == selected) {
        Ok(selected)
    } else {
        Err(reason)
    }
}

fn is_ssh(url: &str) -> bool {
    url.starts_with("ssh://")
        || url.starts_with("git+ssh://")
        || url.starts_with("ssh+git://")
        || (!url.contains("://")
            && !url.starts_with('/')
            && url
                .split_once(':')
                .is_some_and(|(host, _)| !host.contains('/')))
}

fn tip(root: &Path, reference: &str) -> Probe<String> {
    git_text(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
}

// One non-recursive tree listing as (type, oid, name) entries.
fn tree_entries(root: &Path, tree: &str) -> Probe<Vec<(String, String, Vec<u8>)>> {
    let listing = git_read(root, &["ls-tree", "-z", "--full-tree", tree])?;
    let mut entries = Vec::new();
    for entry in listing
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let Some(tab) = entry.iter().position(|byte| *byte == b'\t') else {
            return Err("Git returned a malformed tree entry");
        };
        let header = std::str::from_utf8(&entry[..tab])
            .map_err(|_| "Git returned a malformed tree entry")?;
        let parts = header.split_whitespace().collect::<Vec<_>>();
        let [_, kind, oid] = parts.as_slice() else {
            return Err("Git returned a malformed tree entry");
        };
        entries.push((
            (*kind).to_owned(),
            (*oid).to_owned(),
            entry[tab + 1..].to_vec(),
        ));
    }
    Ok(entries)
}

fn subtree(root: &Path, tree: &str, name: &[u8]) -> Probe<Option<String>> {
    Ok(tree_entries(root, tree)?
        .into_iter()
        .find(|(kind, _, entry)| kind == "tree" && entry == name)
        .map(|(_, oid, _)| oid))
}

fn workflows(root: &Path, branch: &str, remote: Option<&str>) -> Probe<Vec<String>> {
    let sha = remote
        .and_then(|remote| tip(root, &format!("refs/remotes/{remote}/{branch}")).ok())
        .or_else(|| tip(root, &format!("refs/heads/{branch}")).ok())
        .ok_or("The default branch tip is not available locally; no fetch was attempted")?;
    // Walk only .github/workflows, one tree at a time, without path
    // expressions, checkout reads, or reading any workflow contents.
    let Some(github) = subtree(root, &sha, b".github")? else {
        return Ok(Vec::new());
    };
    let Some(directory) = subtree(root, &github, b"workflows")? else {
        return Ok(Vec::new());
    };
    let mut result = Vec::new();
    for (kind, _, name) in tree_entries(root, &directory)? {
        // Nested directories are ignored. Symlinks and submodule entries
        // named like workflows are listed too, for manual review.
        if kind == "tree" || !(name.ends_with(b".yml") || name.ends_with(b".yaml")) {
            continue;
        }
        let name =
            std::str::from_utf8(&name).map_err(|_| "A committed workflow path is not UTF-8")?;
        result.push(format!(".github/workflows/{name}"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn safe() -> Facts {
        Facts {
            repository: Some("github.example/example/project".into()),
            default_branch: Some("main".into()),
            unavailable: vec![],
            rules: Ok(vec![
                json!({"type":"required_status_checks"}),
                json!({"type":"non_fast_forward"}),
                json!({"type":"deletion"}),
            ]),
            rulesets: Ok(vec![]),
            scopes: Ok(Some(vec!["repo".into()])),
            workflow_permissions: Ok("read".into()),
            push_ssh: Ok(false),
            workflows: Ok(vec![]),
        }
    }

    #[test]
    fn facts_table_covers_each_repository_finding_and_negative() {
        let cases = [
            "required_checks_not_enforced",
            "force_push_allowed",
            "deletion_allowed",
            "merge_queue",
            "ruleset_bypass",
            "token_workflow_scope",
            "default_workflow_permissions",
            "remote_push_ssh",
            "could_not_check",
        ];
        for code in cases {
            assert!(evaluate(safe()).findings.is_empty(), "negative {code}");
            let mut facts = safe();
            match code {
                "required_checks_not_enforced" => facts
                    .rules
                    .as_mut()
                    .unwrap()
                    .retain(|rule| rule["type"] != "required_status_checks"),
                "force_push_allowed" => facts
                    .rules
                    .as_mut()
                    .unwrap()
                    .retain(|rule| rule["type"] != "non_fast_forward"),
                "deletion_allowed" => facts
                    .rules
                    .as_mut()
                    .unwrap()
                    .retain(|rule| rule["type"] != "deletion"),
                "merge_queue" => facts
                    .rules
                    .as_mut()
                    .unwrap()
                    .push(json!({"type":"merge_queue"})),
                "ruleset_bypass" => {
                    facts.rulesets = Ok(vec![Ok(
                        json!({"name":"Protection", "target":"branch", "enforcement":"active", "current_user_can_bypass":"always", "conditions":{"ref_name":{"include":["~DEFAULT_BRANCH"], "exclude":[]}}}),
                    )])
                }
                "token_workflow_scope" => facts.scopes = Ok(Some(vec!["workflow".into()])),
                "default_workflow_permissions" => facts.workflow_permissions = Ok("write".into()),
                "remote_push_ssh" => facts.push_ssh = Ok(true),
                "could_not_check" => facts.rules = Err("GitHub returned 403 (not permitted)"),
                _ => unreachable!(),
            }
            let report = evaluate(facts);
            if matches!(
                code,
                "required_checks_not_enforced"
                    | "force_push_allowed"
                    | "deletion_allowed"
                    | "merge_queue"
            ) {
                assert!(report
                    .findings
                    .iter()
                    .filter(|finding| finding.code == code)
                    .all(|finding| finding
                        .detail
                        .contains("classic branch protection is not inspected")));
            }
            assert!(
                report.findings.iter().any(|finding| finding.code == code),
                "{code}"
            );
            assert_eq!(
                report.counts.warning + report.counts.info,
                report.findings.len()
            );
        }
    }

    #[test]
    fn ruleset_conditions_and_bypass_table() {
        for (enforcement, bypass, include, exclude, expected) in [
            ("active", "always", "~DEFAULT_BRANCH", "", true),
            ("active", "pull_requests_only", "~ALL", "", true),
            ("active", "never", "~ALL", "", false),
            ("disabled", "always", "~ALL", "", false),
            ("evaluate", "always", "~ALL", "", false),
            ("active", "always", "refs/heads/release/*", "", false),
            ("active", "always", "refs/heads/m*", "", true),
            ("active", "always", "~ALL", "refs/heads/main", false),
            ("active", "always", "~ALL", "~DEFAULT_BRANCH", false),
            ("active", "always", "refs/heads/[m]ain", "", true),
        ] {
            let mut facts = safe();
            facts.rulesets = Ok(vec![Ok(
                json!({"name":"Protection", "target":"branch", "enforcement":enforcement, "current_user_can_bypass":bypass, "conditions":{"ref_name":{"include":[include], "exclude":[exclude]}}}),
            )]);
            assert_eq!(
                evaluate(facts)
                    .findings
                    .iter()
                    .any(|finding| finding.code == "ruleset_bypass"),
                expected,
                "{enforcement}/{bypass}/{include}/{exclude}"
            );
        }
    }

    #[test]
    fn unavailable_observations_are_info_and_do_not_invent_warnings() {
        let mut facts = safe();
        facts.rulesets = Err("unavailable");
        facts.scopes = Err("unavailable");
        facts.workflow_permissions = Err("unavailable");
        facts.push_ssh = Err("unavailable");
        facts.workflows = Err("unavailable");
        let report = evaluate(facts);
        assert_eq!(report.counts.warning, 0);
        assert_eq!(report.counts.info, 5);
        assert!(report
            .findings
            .iter()
            .all(|finding| finding.code == "could_not_check"));
        let mut facts = safe();
        facts.scopes = Ok(None);
        let report = evaluate(facts);
        assert_eq!(report.findings[0].code, "token_workflow_scope");
        assert_eq!(report.findings[0].level, "info");
    }

    #[test]
    fn remote_protocol_table() {
        for (url, expected) in [
            ("git@example.invalid:example/project.git", true),
            ("ssh://git@example.invalid/project", true),
            ("git+ssh://example.invalid/project", true),
            ("https://example.invalid/project", false),
            ("https://credential@example.invalid/project", false),
            ("/some/repository", false),
            ("../repository", false),
        ] {
            assert_eq!(is_ssh(url), expected, "{url}");
        }
        assert_eq!(segment("release/next%"), "release%2Fnext%25");
    }
}
