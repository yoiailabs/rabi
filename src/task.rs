//! Task bookkeeping and inspection. Policy/classification have no Git I/O.
pub(crate) mod advance;
pub(crate) mod assessment;
pub(crate) mod boundary;
mod classifier_provider;
mod classify;
pub(crate) mod cleanup;
mod diff;
pub(crate) mod implement;
pub(crate) mod intake;
mod land;
mod policy;
mod preflight;
pub(crate) mod publish;
pub(crate) mod review;
mod reviewer_provider;
pub(crate) mod store;

use std::path::Path;

use crate::error::{Error, Result};
use classify::Classification;
use policy::Policy;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Report {
    base: Base,
    merge_base: String,
    head: String,
    policy: PolicySource,
    #[serde(flatten)]
    classification: Classification,
    #[serde(skip)]
    entries: Vec<classify::ChangedEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<assessment::Record>,
}

#[derive(serde::Serialize)]
struct Base {
    r#ref: String,
    sha: String,
}

#[derive(serde::Serialize)]
struct PolicySource {
    present: bool,
    source: String,
}

// Shared task reads use the boundary environment and hardened Git executor.
// Byte output is essential: paths must never be decoded lossily for matching.
fn capture(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    let _git = boundary::git_read_scope();
    let mut options = vec!["--no-lazy-fetch"];
    options.extend_from_slice(args);
    // GIT_NO_REPLACE_OBJECTS does not disable legacy info/grafts. A path
    // beneath the null device cannot exist, and overrides inherited graft files.
    crate::process::raw_bytes(
        "git",
        &options,
        Some(root),
        &[
            ("LC_ALL", "C"),
            ("GIT_NO_REPLACE_OBJECTS", "1"),
            ("GIT_GRAFT_FILE", "/dev/null/rabi-grafts"),
        ],
        None,
    )
}

/// Inspect task worktrees without invoking attributes filters or opening nested
/// repositories. Cached tree differences still report staged gitlink changes.
fn dirty(root: &Path) -> Result<bool> {
    // hash-object reads empty stdin, never worktree files or filters, and selects
    // the repository's object format without writing an object.
    let empty_tree = text(root, &["hash-object", "-t", "tree", "--stdin"])?;
    let attributes = format!("--attr-source={empty_tree}");
    let status = read(
        root,
        &[
            &attributes,
            "status",
            "--porcelain",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?;
    let staged = read(
        root,
        &[
            &attributes,
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=none",
            "--submodule=short",
            "HEAD",
            "--",
        ],
    )?;
    Ok(!status.is_empty() || !staged.is_empty())
}

fn git_error(args: &[&str], stderr: &[u8]) -> Error {
    let stderr = String::from_utf8_lossy(stderr);
    let compatibility = if (stderr.contains("unknown option")
        || stderr.contains("unrecognized option"))
        && (stderr.contains("--no-lazy-fetch") || stderr.contains("--attr-source"))
    {
        "task risk requires Git 2.44 or later: "
    } else {
        ""
    };
    Error::Status {
        code: "git_failed",
        message: format!(
            "{compatibility}Git {} failed: {}",
            args.iter()
                .find(|arg| !arg.starts_with('-'))
                .copied()
                .unwrap_or("inspection"),
            stderr.trim()
        ),
    }
}

fn read(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let result = capture(root, args)?;
    if !result.status.success() {
        return Err(git_error(args, &result.stderr));
    }
    Ok(result.stdout)
}

fn text(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(read(root, args)?)
        .map(|value| value.trim().to_owned())
        .map_err(|_| Error::msg("Git returned non-UTF-8 commit information"))
}

/// Reject unsupported Git before shared repository/selector discovery begins.
/// `git version` works outside a repository, so discovery keeps its own
/// `not_repository` classification.
pub(crate) fn check_git(root: &Path) -> Result<()> {
    text(root, &["version"]).map(|_| ())
}

fn default_branch(root: &Path) -> Result<String> {
    // Match the shared default-branch lookup, but protect each object query
    // with --no-lazy-fetch rather than an environment variable older Git ignores.
    let symbolic = capture(
        root,
        &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"],
    )?;
    if symbolic.status.success() {
        let reference = std::str::from_utf8(&symbolic.stdout)
            .map_err(|_| Error::msg("Git returned a non-UTF-8 default branch"))?
            .trim();
        if !reference.is_empty() && resolves_commit(root, reference)? {
            return Ok(reference
                .strip_prefix("refs/remotes/")
                .unwrap_or(reference)
                .to_owned());
        }
    } else if symbolic.status.code() != Some(1) {
        return Err(git_error(&["symbolic-ref"], &symbolic.stderr));
    }
    for name in ["main", "master"] {
        if resolves_commit(root, &format!("refs/heads/{name}"))? {
            return Ok(format!("origin/{name}"));
        }
    }
    Err(Error::msg(
        "Cannot determine the default branch; specify --base REF",
    ))
}

fn resolves_commit(root: &Path, reference: &str) -> Result<bool> {
    let spec = format!("{reference}^{{commit}}");
    let args = [
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &spec,
    ];
    let result = capture(root, &args)?;
    if result.status.success() {
        Ok(true)
    } else if result.status.code() == Some(1) {
        Ok(false)
    } else {
        Err(git_error(&args, &result.stderr))
    }
}

/// Read tree objects directly. A failed <commit>:<path> lookup can stat the
/// checkout for diagnostics; no path-expression failure is used here.
fn policy_blob(root: &Path, base_sha: &str) -> Result<Option<Vec<u8>>> {
    let Some((mode, blob)) = committed_entry(root, base_sha, ".rabi/policy.toml")? else {
        return Ok(None);
    };
    if !matches!(mode, 0o100644 | 0o100755) {
        return Err(Error::msg(
            "Invalid risk policy .rabi/policy.toml: must be a regular file",
        ));
    }
    read(root, &["cat-file", "blob", &blob]).map(Some)
}

/// Resolve only literal, root-relative tree components; never consult checkout files.
fn committed_entry(root: &Path, sha: &str, path: &str) -> Result<Option<(u32, String)>> {
    let oid_bytes = match sha.len() {
        40 => 20,
        64 => 32,
        _ => return Err(Error::msg("Unsupported Git object ID length")),
    };
    let parts: Vec<_> = path.split('/').collect();
    if parts
        .iter()
        .any(|part| part.is_empty() || matches!(*part, "." | "..") || part.contains('\0'))
    {
        return Ok(None);
    }
    let mut tree = format!("{sha}^{{tree}}");
    for (index, part) in parts.iter().enumerate() {
        let Some((mode, oid)) = tree_entry(root, &tree, part.as_bytes(), oid_bytes)? else {
            return Ok(None);
        };
        if index + 1 == parts.len() {
            return Ok(Some((mode, oid)));
        }
        if mode != 0o40000 {
            return Ok(None);
        }
        tree = oid;
    }
    Ok(None)
}

fn tree_entry(
    root: &Path,
    tree: &str,
    name: &[u8],
    oid_bytes: usize,
) -> Result<Option<(u32, String)>> {
    let bytes = read(root, &["cat-file", "tree", tree])?;
    let mut remaining = bytes.as_slice();
    let malformed = || Error::Status {
        code: "git_failed",
        message: "Git returned a malformed tree object".into(),
    };
    while !remaining.is_empty() {
        let space = remaining
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(malformed)?;
        let nul = remaining
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(malformed)?;
        if space >= nul {
            return Err(malformed());
        }
        let mode = std::str::from_utf8(&remaining[..space])
            .ok()
            .and_then(|value| u32::from_str_radix(value, 8).ok())
            .ok_or_else(malformed)?;
        let oid = remaining
            .get(nul + 1..nul + 1 + oid_bytes)
            .ok_or_else(malformed)?;
        if &remaining[space + 1..nul] == name {
            let oid = oid.iter().map(|byte| format!("{byte:02x}")).collect();
            return Ok(Some((mode, oid)));
        }
        remaining = &remaining[nul + 1 + oid_bytes..];
    }
    Ok(None)
}

fn commit(root: &Path, reference: &str) -> Result<String> {
    text(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
    )
    .map_err(|error| Error::Status {
        code: "git_failed",
        message: format!("Cannot resolve commit '{reference}': {error}"),
    })
}

pub(crate) fn inspect(root: &Path, base: Option<&str>) -> Result<Report> {
    inspect_inner(root, base, false)
}

fn inspect_for_classifier(root: &Path, base: Option<&str>) -> Result<Report> {
    inspect_inner(root, base, true)
}

fn inspect_inner(root: &Path, base: Option<&str>, classifier: bool) -> Result<Report> {
    let base_ref = match base {
        Some(reference) => reference.to_owned(),
        None => default_branch(root)?,
    };
    let base_sha = commit(root, &base_ref)?;
    let head = commit(root, "HEAD")?;
    let merge_base = text(root, &["merge-base", &base_sha, &head])?;
    let source = format!("{base_sha}:.rabi/policy.toml");
    let blob = policy_blob(root, &base_sha)?;
    let present = blob.is_some();
    let policy = if let Some(bytes) = blob {
        let contents = std::str::from_utf8(&bytes)
            .map_err(|_| Error::msg("Invalid .rabi/policy.toml: policy must be UTF-8"))?;
        if classifier && contents.trim().is_empty() {
            Policy::default()
        } else {
            Policy::parse(contents)?
        }
    } else {
        Policy::default()
    };
    // Neither the head branch nor the worktree/index may turn binary changes
    // into text (or alter line limits): the base decides, like the policy.
    let attributes = format!("--attr-source={base_sha}");
    let mut raw_args = vec![attributes.as_str()];
    raw_args.extend(diff_args("--raw", &merge_base, &head));
    let raw = read(root, &raw_args)?;
    let mut numstat_args = vec![attributes.as_str()];
    numstat_args.extend(diff_args("--numstat", &merge_base, &head));
    let numstat = read(root, &numstat_args)?;
    let entries = diff::parse(&raw, &numstat);
    Ok(Report {
        base: Base {
            r#ref: base_ref,
            sha: base_sha,
        },
        merge_base,
        head,
        policy: PolicySource {
            present,
            source: if present {
                source
            } else {
                "built-in defaults (no policy file)".into()
            },
        },
        classification: classify::classify(&policy, &entries),
        entries,
        record: None,
    })
}

fn diff_args<'a>(format: &'a str, base: &'a str, head: &'a str) -> Vec<&'a str> {
    vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-relative",
        "--ignore-submodules=none",
        format,
        "-z",
        "-M",
        "--no-abbrev",
        base,
        head,
        "--",
    ]
}

impl Report {
    pub(crate) fn print(&self) {
        use crate::output::{outln, visible_text};
        outln!(
            "Base: {} ({})",
            visible_text(&self.base.r#ref),
            self.base.sha
        );
        outln!("Merge-base: {}", self.merge_base);
        outln!("Head: {}", self.head);
        outln!("Policy: {}", self.policy.source);
        outln!(
            "Tier: {}{}",
            self.classification.tier,
            if self.classification.floor {
                " FLOOR"
            } else {
                ""
            }
        );
        outln!(
            "Reasons: {}",
            visible_text(&self.classification.reasons.join(", "))
        );
        if let Some(record) = &self.record {
            outln!(
                "Policy blob: {}",
                record.policy_blob_sha.as_deref().unwrap_or("none")
            );
            outln!(
                "Document blob: {}",
                record.doc_blob_sha.as_deref().unwrap_or("none")
            );
            outln!(
                "Classifier provider: {}",
                match record.provider {
                    Some(classifier_provider::Provider::Claude) => "claude",
                    None => "none",
                }
            );
            outln!("Uncertainty: {}", visible_text(&record.uncertainty));
            outln!("Recorded: {}", record.created_at);
        }
        for file in &self.classification.files {
            let path = match &file.old_path {
                Some(old) => format!("{} → {}", visible_text(old), visible_text(&file.path)),
                None => visible_text(&file.path),
            };
            outln!(
                "{} {} {} [{}]",
                file.tier,
                visible_text(&file.status),
                path,
                visible_text(&file.reasons.join(", "))
            );
        }
    }
}

pub(crate) enum GithubAction<'a> {
    Status,
    DryRun,
    Merge { external_review: bool },
    Approve { sha: &'a str, note: Option<&'a str> },
}

/// GitHub inspection shares risk's status-free selectors and protected object reads.
pub(crate) fn github_report(
    context: &crate::context::Context,
    selector: &str,
    base: Option<&str>,
    action: GithubAction<'_>,
) -> Result<()> {
    let client = crate::github::facts::Client::new(&context.git_root)?;
    let (number, workspace) = if let Some(number) = selector.strip_prefix("pr:") {
        let number = number
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| Error::validation("Expected pr:N with a positive PR number"))?;
        (number, None)
    } else {
        let tree = crate::selectors::resolve_without_status(context, selector)?;
        crate::worktree::validate_checkout(context, &tree, "task")?;
        let number = match &tree.metadata.links.pr {
            Some(link) => client.recorded_number(link)?,
            None => client.branch_number(&tree.branch)?,
        };
        (number, Some(tree))
    };
    let facts = client.snapshot(number)?;
    if matches!(action, GithubAction::Status) {
        if crate::output::json() {
            crate::output::data(&facts)?;
        } else {
            facts.print();
        }
        return Ok(());
    }
    if let GithubAction::Approve { sha, .. } = action {
        if facts.pr.state != "open"
            || facts.inconsistent
            || sha != facts.pr.head_sha.to_ascii_lowercase()
        {
            return Err(Error::validation(
                "Approval requires an open PR and --sha equal to its current head SHA",
            ));
        }
    }
    let root = &context.git_root;
    let base_local = resolves_commit(root, &facts.pr.base_sha)?;
    let blob = if base_local {
        policy_blob(root, &facts.pr.base_sha)?
    } else {
        client.policy(&facts.pr.base_sha)?
    };
    let policy = match &blob {
        Some(bytes) => Policy::parse(
            std::str::from_utf8(bytes)
                .map_err(|_| Error::msg("Invalid .rabi/policy.toml: policy must be UTF-8"))?,
        )?,
        None => Policy::default(),
    };
    let (merge_base, kinds) = pr_kinds(root, &facts)?;
    if let GithubAction::Approve { sha, note } = action {
        let risk =
            preflight::classify_files(&facts, &policy, kinds.as_deref()).map_err(Error::msg)?;
        let description: String = format!(
            "rabi approval: {}{}",
            risk.tier,
            note.map_or(String::new(), |text| format!(": {text}"))
        )
        .chars()
        .take(140)
        .collect();
        client.approve(sha, &description)?;
        if crate::output::json() {
            crate::output::data(&serde_json::json!({
                "repository": facts.repository, "number": number, "sha": sha,
                "state": "success", "context": "rabi/approval", "description": description,
                "tier": risk.tier,
            }))?;
        } else {
            crate::output::outln!(
                "Approved {}: rabi/approval success ({})",
                sha,
                crate::output::visible_text(&description)
            );
        }
        return Ok(());
    }
    let head = workspace
        .as_ref()
        .map(|tree| commit(Path::new(&tree.path), "HEAD"))
        .transpose()?;
    let review_root = workspace
        .as_ref()
        .map_or(root.as_path(), |tree| Path::new(&tree.path));
    let report = merge_evaluation(
        review_root,
        &client,
        &facts,
        &policy,
        blob.as_deref(),
        head,
        kinds,
        merge_base.as_deref(),
        base,
    )?;
    if crate::output::json() {
        crate::output::data(&report)?;
    } else {
        report.print();
    }
    if let GithubAction::Merge { external_review } = action {
        if !report.permits_merge(external_review) {
            return Err(Error::status(crate::error::REFUSED,
                "Merge preflight blocked or incomplete; --external-review covers only review_record and classification_record"));
        }
        let result = client.merge(number, policy.merge.method.flag(), &facts.pr.head_sha);
        // Re-read even after a failed gh invocation: it may have merged remotely
        // before reporting an error. Never retry the write automatically.
        let updated = match client.snapshot(number) {
            Ok(updated) => updated,
            Err(reread) => {
                let invocation = match &result {
                    Ok(()) => "ok".to_string(),
                    Err(error) => format!("failed: {error}"),
                };
                return Err(Error::msg(format!(
                    "Merge outcome unknown: gh pr merge {invocation}; re-reading the PR failed: {reread}"
                )));
            }
        };
        let merged = updated.pr.merged;
        let merged_head = merged.then_some(updated.pr.head_sha.as_str());
        if crate::output::json() {
            crate::output::data(&serde_json::json!({
                "preflight": report, "merged": merged,
                "mergeCommitSha": updated.pr.merge_commit_sha,
                "mergedHeadSha": merged_head, "facts": updated,
            }))?;
        } else {
            updated.print();
            crate::output::outln!(
                "Merged: {} Merge commit: {} Merged head: {}",
                merged,
                updated.pr.merge_commit_sha.as_deref().unwrap_or("unknown"),
                merged_head.unwrap_or("none")
            );
        }
        result?;
        if !merged {
            return Err(Error::msg(
                "GitHub did not report the PR as merged after gh pr merge",
            ));
        }
    }
    Ok(())
}

/// Shared complete slice-2 preflight, including current-base policy freshness.
#[allow(clippy::too_many_arguments)]
fn merge_evaluation(
    root: &Path,
    client: &crate::github::facts::Client<'_>,
    facts: &crate::github::facts::Snapshot,
    policy: &Policy,
    blob: Option<&[u8]>,
    head: Option<String>,
    kinds: Option<Vec<classify::ChangedEntry>>,
    merge_base: Option<&str>,
    base: Option<&str>,
) -> Result<preflight::Evaluation> {
    let policy_matches_base_tip = facts.base_tip_sha.as_deref().and_then(|tip| {
        if tip == facts.pr.base_sha {
            return Some(true);
        }
        let tip_blob = resolves_commit(root, tip).and_then(|local| {
            if local {
                policy_blob(root, tip)
            } else {
                client.policy(tip)
            }
        });
        tip_blob.ok().map(|tip_blob| tip_blob.as_deref() == blob)
    });
    let classification_record = merge_base.and_then(|merge_base| {
        assessment::matching(
            root,
            &facts.pr.head_sha,
            &facts.pr.base_sha,
            merge_base,
            policy,
        )
    });
    let mut report = preflight::evaluate(
        facts,
        policy,
        &preflight::Local {
            head,
            kinds,
            policy_matches_base_tip,
            classification_record,
        },
        base,
    );
    if let Some(record) = merge_base.and_then(|merge_base| {
        let store = store::Store::locate(root).ok()?;
        let task = store.implementation_task(None).ok()?;
        if task.pr_number != Some(facts.pr.number) || task.branch != facts.pr.head_ref {
            return None;
        }
        review::earned_pass(root, &store, &task, &facts.pr.head_sha)
            .filter(|record| record.merge_base_sha == merge_base)
    }) {
        report.review_pass(&record);
    }
    Ok(report)
}

fn pr_kinds(
    root: &Path,
    facts: &crate::github::facts::Snapshot,
) -> Result<(Option<String>, Option<Vec<classify::ChangedEntry>>)> {
    if !resolves_commit(root, &facts.pr.base_sha)? || !resolves_commit(root, &facts.pr.head_sha)? {
        return Ok((None, None));
    }
    let Ok(merge_base) = text(
        root,
        &["merge-base", &facts.pr.base_sha, &facts.pr.head_sha],
    ) else {
        return Ok((None, None));
    };
    let attributes = format!("--attr-source={}", facts.pr.base_sha);
    let mut raw_args = vec![attributes.as_str()];
    raw_args.extend(diff_args("--raw", &merge_base, &facts.pr.head_sha));
    let mut numstat_args = vec![attributes.as_str()];
    numstat_args.extend(diff_args("--numstat", &merge_base, &facts.pr.head_sha));
    let kinds = diff::parse(&read(root, &raw_args)?, &read(root, &numstat_args)?);
    Ok((Some(merge_base), Some(kinds)))
}
