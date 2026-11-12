//! Local committed-change inspection. Policy/classification have no Git I/O.
mod classify;
mod diff;
mod policy;

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

// Reuse the existing byte executor without extending shared Git/process APIs.
// Byte output is essential: paths must never be decoded lossily for matching.
fn capture(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut options = vec!["--no-lazy-fetch"];
    options.extend_from_slice(args);
    crate::process::raw_bytes("git", &options, Some(root), &[("LC_ALL", "C")], None)
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
    let oid_bytes = match base_sha.len() {
        40 => 20,
        64 => 32,
        _ => return Err(Error::msg("Unsupported Git object ID length")),
    };
    let Some((mode, directory)) =
        tree_entry(root, &format!("{base_sha}^{{tree}}"), b".rabi", oid_bytes)?
    else {
        return Ok(None);
    };
    if mode != 0o40000 {
        return Ok(None);
    }
    let Some((mode, blob)) = tree_entry(root, &directory, b"policy.toml", oid_bytes)? else {
        return Ok(None);
    };
    if !matches!(mode, 0o100644 | 0o100755) {
        return Err(Error::msg(
            "Invalid risk policy .rabi/policy.toml: must be a regular file",
        ));
    }
    read(root, &["cat-file", "blob", &blob]).map(Some)
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
        Policy::parse(contents)?
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
