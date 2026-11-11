use std::path::{Path, PathBuf};

use crate::config::{self, RabiConfig};
use crate::error::{Error, Result};
use crate::git;
use crate::paths;
use crate::preferences::{self, Preferences};

#[derive(Debug, Clone)]
pub struct Context {
    pub(crate) retirement_queries: std::cell::RefCell<crate::github::RetirementQueries>,
    pub git_root: PathBuf,
    /// Checkout the command is running in. `.rabi.toml` is loaded from here.
    pub source_checkout: PathBuf,
    pub worktrees_directory: PathBuf,
    /// Canonical git common dir when `worktrees_directory` is the default
    /// project directory. Unset when `workspace-root` overrides it.
    pub project_common_dir: Option<PathBuf>,
    pub config: Option<RabiConfig>,
    pub preferences: Preferences,
}

pub fn create(git_root: &Path) -> Result<Context> {
    assemble(git_root, git::source_checkout(git_root))
}

/// Context for a repository selected with `--repo`.
///
/// Hooks are read from that repository's main checkout. Copies always use the primary.
pub fn create_at(git_root: &Path) -> Result<Context> {
    assemble(git_root, git_root.to_path_buf())
}

/// Open `repository` when `--repo` selected one, otherwise the current directory.
pub fn open(repository: Option<&Path>) -> Result<Context> {
    if let Some(root) = repository {
        return create_at(root);
    }
    let cwd = std::env::current_dir().map_err(|error| Error::msg(error.to_string()))?;
    let git_root = git::git_root(&cwd)?;
    create(&git_root)
}

/// Execution declarations always come from the selected primary checkout.
pub(crate) fn open_for_execution(repository: Option<&Path>) -> Result<Context> {
    let root = match repository {
        Some(root) => root.to_path_buf(),
        None => git::git_root(&std::env::current_dir().map_err(|e| Error::msg(e.to_string()))?)?,
    };
    create_at(&root)
}

/// Select hooks independently of the target checkout used by dot.
/// The source must belong to the selected repository; copies still use primary.
pub(crate) fn open_for_hooks(repository: Option<&Path>, source: &Path) -> Result<Context> {
    let cwd = std::env::current_dir().map_err(|error| Error::msg(error.to_string()))?;
    let git_root = match repository {
        Some(root) => root.to_path_buf(),
        None => git::git_root(&cwd)?,
    };
    let source_checkout = git::top_level(source)?;
    if !paths::same_location(&git::git_root(&source_checkout)?, &git_root) {
        return Err(Error::validation(
            "--source-checkout must belong to the selected repository",
        ));
    }
    assemble(&git_root, source_checkout)
}

fn assemble(git_root: &Path, source_checkout: PathBuf) -> Result<Context> {
    let config = config::load_config_from(&source_checkout, git_root)?;
    let preferences = preferences::load_preferences()?;
    let common_dir = git::canonical_common_dir(git_root)?;
    let location = match preferences
        .worktrees_directory
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        Some(directory) => {
            let expanded = if directory == "~" || directory.starts_with("~/") {
                let home = std::env::var_os("HOME")
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| Error::msg("HOME is not set"))?;
                PathBuf::from(home).join(directory.strip_prefix("~/").unwrap_or(""))
            } else {
                PathBuf::from(directory)
            };
            paths::configured_worktrees_directory(git_root, &expanded.to_string_lossy())
        }
        None => {
            let data_home = paths::data_home_from_env()?;
            paths::default_worktrees_directory(git_root, &common_dir, &data_home)
        }
    };
    Ok(Context {
        retirement_queries: Default::default(),
        git_root: git_root.to_path_buf(),
        source_checkout,
        worktrees_directory: location.path,
        project_common_dir: location.project_common_dir,
        config,
        preferences,
    })
}
