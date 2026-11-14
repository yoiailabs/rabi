#![cfg_attr(
    not(test),
    deny(
        clippy::disallowed_macros,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
    )
)]

mod allocation;
pub mod args;
pub(crate) mod atomic;
pub mod cli;
pub mod config;
pub mod context;
mod doctor;
pub mod error;
mod execution_env;
pub mod gc;
pub mod git;
pub mod github;
mod handoff;
mod hash;
pub mod help;
pub mod metadata;
pub mod names;
pub mod output;
pub mod paths;
pub mod preferences;
pub mod process;
mod provider;
pub mod repository;
mod repository_state;
mod run;
mod runtime;
mod selectors;
pub mod setup;
mod shell;
pub mod sync;
mod task;
mod tracking;
pub mod trust;
pub mod worktree;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn version_line() -> String {
    let sha = option_env!("RABI_GIT_SHA").filter(|sha| !sha.is_empty());
    format_version(VERSION, sha)
}

fn format_version(version: &str, sha: Option<&str>) -> String {
    match sha {
        Some(sha) => format!("rabi v{version}+rust ({sha})"),
        None => format!("rabi v{version}+rust"),
    }
}

#[cfg(test)]
#[path = "../tests/common/git.rs"]
mod test_git;

#[cfg(test)]
#[path = "../tests/common/scripts.rs"]
pub(crate) mod test_support;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_line_marks_the_rust_build() {
        assert_eq!(format_version("7.0.0", None), "rabi v7.0.0+rust");
        assert_eq!(
            format_version("7.0.0", Some("abc1234")),
            "rabi v7.0.0+rust (abc1234)"
        );
        assert_ne!(format_version("7.0.0", None), "rabi v7.0.0");
    }
}
