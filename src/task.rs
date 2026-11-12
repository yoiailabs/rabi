//! Local committed-change inspection. Policy/classification have no Git I/O.
mod classify;
mod diff;
mod policy;

use std::path::Path;

use crate::error::{Error, Result};
use crate::git;
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
fn capture(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<std::process::Output> {
    let mut env = env.to_vec();
    env.extend([("GIT_NO_LAZY_FETCH", "1"), ("LC_ALL", "C")]);
    crate::process::raw_bytes("git", args, Some(root), &env, None)
}

fn git_error(args: &[&str], stderr: &[u8]) -> Error {
    Error::Status {
        code: "git_failed",
        message: format!(
            "Git {} failed: {}",
            args.first().copied().unwrap_or("inspection"),
            String::from_utf8_lossy(stderr).trim()
        ),
    }
}

fn read_with_env(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Vec<u8>> {
    let result = capture(root, args, env)?;
    if !result.status.success() {
        return Err(git_error(args, &result.stderr));
    }
    Ok(result.stdout)
}

fn text(root: &Path, args: &[&str]) -> Result<String> {
    let result = git::raw_git_with_env(root, args, &[("GIT_NO_LAZY_FETCH", "1")])?;
    if result.code != 0 {
        return Err(git_error(args, result.stderr.as_bytes()));
    }
    Ok(result.stdout.trim().to_owned())
}

fn policy_present(root: &Path, base_sha: &str, source: &str) -> Result<bool> {
    let args = ["cat-file", "-e", source];
    let result = capture(root, &args, &[])?;
    if result.status.success() {
        return Ok(true);
    }
    // Only Git's explicit missing-path diagnosis permits defaults. Missing
    // promised blobs, unreadable trees, and all other failures remain errors.
    let stderr = String::from_utf8_lossy(&result.stderr);
    let missing = format!("fatal: path '.rabi/policy.toml' does not exist in '{base_sha}'");
    let only_on_disk =
        format!("fatal: path '.rabi/policy.toml' exists on disk, but not in '{base_sha}'");
    if stderr.trim() == missing || stderr.trim() == only_on_disk {
        Ok(false)
    } else {
        Err(git_error(&args, &result.stderr))
    }
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
        None => {
            let default = git::default_branch(root)?.ok_or_else(|| {
                Error::msg("Cannot determine the default branch; specify --base REF")
            })?;
            if default.starts_with("origin/") {
                default
            } else {
                format!("origin/{default}")
            }
        }
    };
    let base_sha = commit(root, &base_ref)?;
    let head = commit(root, "HEAD")?;
    let merge_base = text(root, &["merge-base", &base_sha, &head])?;
    let source = format!("{base_sha}:.rabi/policy.toml");
    let present = policy_present(root, &base_sha, &source)?;
    let policy = if present {
        let bytes = read_with_env(root, &["show", &source], &[])?;
        let contents = std::str::from_utf8(&bytes)
            .map_err(|_| Error::msg("Invalid .rabi/policy.toml: policy must be UTF-8"))?;
        Policy::parse(contents)?
    } else {
        Policy::default()
    };
    // Neither the head branch nor the worktree/index may turn binary changes
    // into text (or alter line limits): the base decides, like the policy.
    let attributes = [("GIT_ATTR_SOURCE", base_sha.as_str())];
    let raw = read_with_env(root, &diff_args("--raw", &merge_base, &head), &attributes)?;
    let numstat = read_with_env(
        root,
        &diff_args("--numstat", &merge_base, &head),
        &attributes,
    )?;
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
