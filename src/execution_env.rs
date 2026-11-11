//! Repository execution environment and the shared, deliberately limited expander.
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};

const WORKSPACE_VARIABLE: &str = "${RABI_WORKSPACE_PATH}";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Environment {
    pub path_prepend: Vec<String>,
    pub set: BTreeMap<String, String>,
}

/// Callers supply the variables available to their configuration domain. Expansion
/// is one pass: substituted values are literal, never recursively interpreted.
pub(crate) struct Expander<F> {
    pub home: Option<OsString>,
    pub variable: F,
}

impl<F> Expander<F>
where
    F: Fn(&str) -> Result<String>,
{
    pub fn expand(&self, text: &str) -> Result<String> {
        let mut output = String::new();
        let mut remaining = text;
        if let Some(suffix) = remaining.strip_prefix("~/") {
            output.push_str(&self.home()?);
            output.push('/');
            remaining = suffix;
        }
        while let Some(index) = remaining.find('$') {
            output.push_str(&remaining[..index]);
            let expression = &remaining[index..];
            let variable = expression
                .strip_prefix("${")
                .and_then(|tail| tail.split_once('}'))
                .ok_or_else(|| Error::msg("unsupported dollar expression"))?;
            if variable.0 == "HOME" {
                output.push_str(&self.home()?);
            } else {
                output.push_str(&(self.variable)(variable.0)?);
            }
            remaining = variable.1;
        }
        output.push_str(remaining);
        Ok(output)
    }

    fn home(&self) -> Result<String> {
        self.home
            .as_ref()
            .filter(|home| !home.is_empty())
            .and_then(|home| home.to_str())
            .map(str::to_owned)
            .ok_or_else(|| Error::msg("HOME must be set to a non-empty UTF-8 value for expansion"))
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Skipped {
    Path(usize),
    Set(usize, String),
}

#[derive(Debug, Default)]
pub(crate) struct Resolved {
    pub values: BTreeMap<String, OsString>,
    path_count: usize,
    skipped: Vec<Skipped>,
}

impl Resolved {
    pub fn summary(&self) -> String {
        let mut keys = Vec::new();
        if self.path_count > 0 {
            keys.push(format!("PATH(+{})", self.path_count));
        }
        keys.extend(self.values.keys().filter(|key| *key != "PATH").cloned());
        keys.join(", ")
    }

    pub fn warn_skipped(&self) {
        for skipped in &self.skipped {
            let entry = match skipped {
                Skipped::Path(index) => format!("env.path-prepend[{index}]"),
                Skipped::Set(index, key) => format!("env.set[{index}] (set.{key})"),
            };
            crate::output::warning("env_workspace_path_skipped", format!("Skipped {entry} for rabi run: the provider process must not take executables from the workspace."));
        }
    }
}

impl Environment {
    pub fn is_empty(&self) -> bool {
        self.path_prepend.is_empty() && self.set.is_empty()
    }

    /// Validate only declarations, without reading HOME or inspecting checkout paths.
    pub(crate) fn validate(&self, source: &Path) -> Result<()> {
        let syntax = Expander {
            home: Some(OsString::from("/home-placeholder")),
            variable: |key: &str| match key {
                "RABI_WORKSPACE_PATH" | "RABI_REPO_PATH" => Ok("/path-placeholder".into()),
                _ => Err(unsupported_variable(key)),
            },
        };
        for (key, value) in &self.set {
            let field = format!("set.{key}");
            if !valid_key(key) {
                return Err(invalid(
                    source,
                    &field,
                    &"invalid environment variable name",
                ));
            }
            if reserved_key(key) {
                return Err(invalid(source, &field, &"reserved environment variable"));
            }
            if value.contains('\0') {
                return Err(invalid(source, &field, &"NUL is not allowed"));
            }
            syntax
                .expand(value)
                .map_err(|e| invalid(source, &field, &e))?;
        }
        for (index, value) in self.path_prepend.iter().enumerate() {
            let field = format!("path-prepend[{index}]");
            if value.contains(['\0', ':']) {
                return Err(invalid(
                    source,
                    &field,
                    &"literal NUL and ':' are not allowed",
                ));
            }
            syntax
                .expand(value)
                .map_err(|e| invalid(source, &field, &e))?;
        }
        Ok(())
    }

    pub(crate) fn resolve(
        &self,
        source: &Path,
        repo: &Path,
        workspace: &Path,
        home: Option<OsString>,
        inherited_path: Option<&OsStr>,
    ) -> Result<Resolved> {
        self.resolve_for(source, repo, workspace, (home, inherited_path), false)
    }

    fn resolve_for(
        &self,
        source: &Path,
        repo: &Path,
        workspace: &Path,
        environment: (Option<OsString>, Option<&OsStr>),
        provider: bool,
    ) -> Result<Resolved> {
        let (home, inherited_path) = environment;
        self.validate(source)?;
        let expander = Expander {
            home,
            variable: |key: &str| match key {
                "RABI_WORKSPACE_PATH" => path_variable(workspace, key),
                "RABI_REPO_PATH" => path_variable(repo, key),
                _ => Err(unsupported_variable(key)),
            },
        };
        let mut values = BTreeMap::new();
        let mut skipped = Vec::new();
        for (index, (key, value)) in self.set.iter().enumerate() {
            if provider && value.contains(WORKSPACE_VARIABLE) {
                skipped.push(Skipped::Set(index, key.clone()));
                continue;
            }
            let field = format!("set.{key}");
            let expanded = expander
                .expand(value)
                .map_err(|e| invalid(source, &field, &e))?;
            if expanded.contains('\0') {
                return Err(invalid(source, &field, &"NUL is not allowed"));
            }
            values.insert(key.clone(), expanded.into());
        }
        let mut paths = Vec::new();
        for (index, value) in self.path_prepend.iter().enumerate() {
            if provider && value.contains(WORKSPACE_VARIABLE) {
                skipped.push(Skipped::Path(index));
                continue;
            }
            let field = format!("path-prepend[{index}]");
            let expanded = expander
                .expand(value)
                .map_err(|e| invalid(source, &field, &e))?;
            if provider
                && Path::new(&expanded).is_absolute()
                && inside_workspace(Path::new(&expanded), workspace)
                    .map_err(|e| invalid(source, &field, &e))?
            {
                skipped.push(Skipped::Path(index));
                continue;
            }
            if expanded.is_empty()
                || expanded.contains(['\0', ':'])
                || !Path::new(&expanded).is_absolute()
            {
                return Err(invalid(source, &field, &"entry must be non-empty, absolute after expansion, and contain neither NUL nor ':'"));
            }
            paths.push(expanded);
        }
        if !paths.is_empty() {
            let mut path = OsString::from(paths.join(":"));
            if let Some(inherited) = inherited_path.filter(|path| !path.is_empty()) {
                path.push(":");
                path.push(inherited);
            }
            values.insert("PATH".into(), path);
        }
        Ok(Resolved {
            values,
            path_count: paths.len(),
            skipped,
        })
    }

    pub(crate) fn for_workspace(&self, repo: &Path, workspace: &Path) -> Result<Resolved> {
        self.resolve(
            repo,
            repo,
            workspace,
            std::env::var_os("HOME"),
            std::env::var_os("PATH").as_deref(),
        )
    }

    pub(crate) fn for_provider(&self, repo: &Path, workspace: &Path) -> Result<Resolved> {
        self.resolve_for(
            repo,
            repo,
            workspace,
            (
                std::env::var_os("HOME"),
                std::env::var_os("PATH").as_deref(),
            ),
            true,
        )
    }

    pub(crate) fn declaration(&self) -> String {
        let paths = self
            .path_prepend
            .iter()
            .map(|path| format!("  path-prepend: {}", crate::output::visible_text(path)));
        let keys = self
            .set
            .iter()
            .map(|(key, value)| format!("  set: {key}={}", crate::output::visible_text(value)));
        std::iter::once("env:".to_owned())
            .chain(paths)
            .chain(keys)
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn unsupported_variable(key: &str) -> Error {
    Error::msg(format!(
        "unsupported variable ${{{}}}",
        crate::output::diagnostic_value(key)
    ))
}

fn invalid(source: &Path, key: &str, reason: &dyn std::fmt::Display) -> Error {
    Error::msg(format!(
        "{}: env.{}: {}",
        crate::output::diagnostic_value(&source.join(".rabi.toml").to_string_lossy()),
        crate::output::diagnostic_value(key),
        crate::output::diagnostic_value(&reason.to_string())
    ))
}

fn path_variable(path: &Path, key: &str) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| Error::msg(format!("{key} needs a UTF-8 checkout path for expansion")))
}

fn normalized(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

/// Resolve existing ancestors too, so a missing bin directory beneath a symlink
/// cannot later become a provider interpreter directory inside the workspace.
fn canonical_existing(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(canonical) => {
                let mut full = canonical;
                for component in suffix.iter().rev() {
                    full.push(component);
                }
                return Ok(normalized(&full));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                let Some(name) = ancestor.components().next_back() else {
                    return Err(Error::msg(format!(
                        "Cannot inspect execution PATH entry: {error}"
                    )));
                };
                suffix.push(name.as_os_str().to_os_string());
                let Some(parent) = ancestor.parent() else {
                    return Err(Error::msg(format!(
                        "Cannot inspect execution PATH entry: {error}"
                    )));
                };
                ancestor = parent;
            }
            Err(error) => {
                return Err(Error::msg(format!(
                    "Cannot inspect execution PATH entry: {error}"
                )))
            }
        }
    }
}

fn inside_workspace(entry: &Path, workspace: &Path) -> Result<bool> {
    let workspace = std::path::absolute(workspace).map_err(|e| Error::msg(e.to_string()))?;
    if normalized(entry).starts_with(normalized(&workspace)) {
        return Ok(true);
    }
    Ok(canonical_existing(entry)?.starts_with(canonical_existing(&workspace)?))
}

fn valid_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn reserved_key(key: &str) -> bool {
    matches!(
        key,
        "PATH"
            | "HOME"
            | "SHELL"
            | "CODEX_HOME"
            | "CLAUDE_CONFIG_DIR"
            | "NODE_OPTIONS"
            | "BASH_ENV"
            | "ENV"
            | "XDG_STATE_HOME"
            | "XDG_CONFIG_HOME"
            | "XDG_DATA_HOME"
            | "XDG_CACHE_HOME"
            | "XDG_RUNTIME_DIR"
            | "GIT_DIR"
            | "GIT_WORK_TREE"
            | "GIT_INDEX_FILE"
            | "GIT_COMMON_DIR"
            | "GIT_OBJECT_DIRECTORY"
            | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
            | "GIT_NAMESPACE"
            | "GIT_CEILING_DIRECTORIES"
            | "GIT_DISCOVERY_ACROSS_FILESYSTEM"
    ) || key.starts_with("RABI")
        || key.starts_with("LD_")
        || key.starts_with("DYLD_")
        || key.starts_with("GIT_CONFIG_")
}

/// Load only the primary checkout's declaration; callers decide the trust policy.
pub(crate) fn load(repo: &Path) -> Result<Environment> {
    Ok(crate::config::load_config(repo)?
        .map(|config| config.env)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        environment: &Environment,
        home: Option<&str>,
        path: Option<&str>,
    ) -> Result<Resolved> {
        environment.resolve(
            Path::new("/repo"),
            Path::new("/repo"),
            Path::new("/workspace"),
            home.map(OsString::from),
            path.map(OsStr::new),
        )
    }

    #[test]
    fn expands_only_supported_variables_without_recursion() {
        let env = Environment {
            path_prepend: vec![
                "~/.cargo/bin".into(),
                "${RABI_WORKSPACE_PATH}/node_modules/.bin".into(),
            ],
            set: BTreeMap::from([
                (
                    "VALUE".into(),
                    "${HOME}:${RABI_REPO_PATH}:literal ~ word".into(),
                ),
                ("EMPTY".into(), "".into()),
            ]),
        };
        let resolved = resolve(&env, Some("/home/example"), Some("/bin")).unwrap();
        assert_eq!(
            resolved.values["PATH"],
            "/home/example/.cargo/bin:/workspace/node_modules/.bin:/bin"
        );
        assert_eq!(
            resolved.values["VALUE"],
            "/home/example:/repo:literal ~ word"
        );
        assert_eq!(resolved.values["EMPTY"], "");
        assert_eq!(resolved.summary(), "PATH(+2), EMPTY, VALUE");
        let expander = Expander {
            home: None,
            variable: |_: &str| Ok("$literal".into()),
        };
        assert_eq!(expander.expand("${FUTURE}").unwrap(), "$literal");
    }

    #[test]
    fn rejects_unsupported_dollar_syntax_and_missing_home_with_file_and_key() {
        for text in ["$HOME", "${UNKNOWN}", "$", "$(pwd)", "$$", "${HOME", "${}"] {
            let env = Environment {
                set: BTreeMap::from([("VALUE".into(), text.into())]),
                ..Default::default()
            };
            let error = resolve(&env, Some("/home/example"), None)
                .unwrap_err()
                .to_string();
            assert!(
                error.contains("/repo/.rabi.toml: env.set.VALUE:"),
                "{error}"
            );
        }
        for home in [None, Some("")] {
            for text in ["~/bin", "${HOME}/bin"] {
                let env = Environment {
                    path_prepend: vec![text.into()],
                    ..Default::default()
                };
                assert!(resolve(&env, home, None)
                    .unwrap_err()
                    .to_string()
                    .contains("HOME"));
            }
        }
        assert!(resolve(&Environment::default(), None, None)
            .unwrap()
            .values
            .is_empty());
    }

    #[test]
    fn rejects_all_reserved_classes_and_invalid_keys() {
        for key in [
            "PATH",
            "RABI",
            "RABI_OTHER",
            "RABIES",
            "HOME",
            "SHELL",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "LD_PRELOAD",
            "LD_",
            "DYLD_LIBRARY_PATH",
            "DYLD_",
            "NODE_OPTIONS",
            "BASH_ENV",
            "ENV",
            "",
            "9A",
            "A-B",
            "A.B",
            "é",
        ] {
            let env = Environment {
                set: BTreeMap::from([(key.into(), "value".into())]),
                ..Default::default()
            };
            assert!(resolve(&env, None, None).is_err(), "{key}");
        }
        for key in ["_", "_a1", "aZ09", "NODE_PATH"] {
            let env = Environment {
                set: BTreeMap::from([(key.into(), "value".into())]),
                ..Default::default()
            };
            assert!(resolve(&env, None, None).is_ok(), "{key}");
        }
    }

    fn rejects_reserved_keys_at_load(keys: &[&str]) {
        let source =
            std::env::temp_dir().join(format!("rabi-env-reserved-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&source).unwrap();
        for key in keys {
            std::fs::write(
                source.join(".rabi.toml"),
                format!("[env.set]\n{key} = '/example'\n"),
            )
            .unwrap();
            let error = crate::config::load_config(&source).unwrap_err();
            assert_eq!(error.exit_code(), 1, "{key}");
            let message = error.to_string();
            assert!(message.contains(".rabi.toml"), "{message}");
            assert!(message.contains(&format!("env.set.{key}")), "{message}");
            assert!(
                message.contains("reserved environment variable"),
                "{message}"
            );
            assert!(
                crate::config::load_config_for_execution(&source, true).is_ok(),
                "{key}"
            );
        }
        std::fs::remove_dir_all(source).unwrap();
    }

    #[test]
    fn reserves_xdg_base_directory_variables_at_load() {
        rejects_reserved_keys_at_load(&[
            "XDG_STATE_HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
        ]);
    }

    #[test]
    fn reserves_git_repository_selection_variables_at_load() {
        rejects_reserved_keys_at_load(&[
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_NAMESPACE",
            "GIT_CEILING_DIRECTORIES",
            "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        ]);
    }

    #[test]
    fn reserves_git_config_prefix_at_load() {
        rejects_reserved_keys_at_load(&[
            "GIT_CONFIG_",
            "GIT_CONFIG_GLOBAL",
            "GIT_CONFIG_SYSTEM",
            "GIT_CONFIG_COUNT",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_CUSTOM",
        ]);
    }

    #[test]
    fn permits_nonreserved_xdg_and_git_neighbors() {
        for key in [
            "XDG_CONFIG_DIRS",
            "XDG_DATA_DIRS",
            "XDG_STATE_HOMES",
            "GIT_CONFIG",
            "GIT_CONFIGURED",
            "GIT_DIRECTORY",
            "GIT_WORK_TREE_SUFFIX",
            "GIT_INDEX_VERSION",
            "GIT_SSH_COMMAND",
            "GIT_TERMINAL_PROMPT",
        ] {
            let environment = Environment {
                set: BTreeMap::from([(key.into(), "value".into())]),
                ..Default::default()
            };
            assert!(environment.validate(Path::new("/repo")).is_ok(), "{key}");
        }
    }

    #[test]
    fn validates_paths_and_nul_after_expansion() {
        for text in ["", "relative", "~", "a:b", "/bin:/other", "/nul\0path"] {
            let env = Environment {
                path_prepend: vec![text.into()],
                ..Default::default()
            };
            assert!(resolve(&env, None, None).is_err(), "{text:?}");
        }
        let env = Environment {
            path_prepend: vec!["~/bin".into()],
            ..Default::default()
        };
        assert!(resolve(&env, Some("/home:bad"), None).is_err());
        let env = Environment {
            set: BTreeMap::from([("VALUE".into(), "\0".into())]),
            ..Default::default()
        };
        assert!(resolve(&env, None, None).is_err());
    }

    #[test]
    fn syntax_validation_defers_all_environment_and_path_contents() {
        let env = Environment {
            path_prepend: vec!["${RABI_WORKSPACE_PATH}/bin".into()],
            ..Default::default()
        };
        let primary = Path::new("/primary:checkout");
        assert!(env.validate(primary).is_ok());
        let resolved = env
            .resolve(primary, primary, Path::new("/workspace"), None, None)
            .unwrap();
        assert_eq!(resolved.values["PATH"], "/workspace/bin");
        assert!(env
            .resolve(
                primary,
                primary,
                Path::new("/workspace:checkout"),
                None,
                None
            )
            .is_err());

        let env = Environment {
            path_prepend: vec!["${RABI_REPO_PATH}/bin".into()],
            ..Default::default()
        };
        assert!(env.validate(primary).is_ok());
        assert!(env
            .resolve(primary, primary, Path::new("/workspace"), None, None)
            .is_err());
        assert!(env.validate(Path::new("/primary")).is_ok());
    }

    #[test]
    fn path_merge_and_command_precedence_preserve_inherited_values() {
        let env = Environment {
            path_prepend: vec!["/first".into(), "/second".into()],
            set: BTreeMap::from([("VALUE".into(), "declared".into())]),
        };
        for inherited in [None, Some("")] {
            assert_eq!(
                resolve(&env, None, inherited).unwrap().values["PATH"],
                "/first:/second"
            );
        }
        let resolved = resolve(&env, None, Some("/bin")).unwrap();
        let mut command = std::process::Command::new("/usr/bin/env");
        command
            .env_clear()
            .env("VALUE", "inherited")
            .env("OTHER", "kept")
            .envs(&resolved.values)
            .env("RABI", "1");
        let output = String::from_utf8(command.output().unwrap().stdout).unwrap();
        assert!(output.contains("VALUE=declared\n"));
        assert!(output.contains("OTHER=kept\n"));
        assert!(output.contains("RABI=1\n"));
        assert_eq!(
            resolve(&Environment::default(), None, Some("/bin"))
                .unwrap()
                .values
                .len(),
            0
        );
    }
    #[test]
    fn load_time_syntax_validation_needs_no_home_or_path_contents() {
        for value in [
            "",
            "relative/bin",
            "~",
            "~/bin",
            "${HOME}/bin",
            "${RABI_REPO_PATH}/bin",
            "${RABI_WORKSPACE_PATH}/bin",
        ] {
            let env = Environment {
                path_prepend: vec![value.into()],
                ..Default::default()
            };
            assert!(
                env.validate(Path::new("/source:checkout")).is_ok(),
                "{value}"
            );
        }
        for value in [
            "$HOME",
            "${UNSUPPORTED}",
            "${HOME",
            "/bin:/other",
            "/nul\0path",
        ] {
            let env = Environment {
                path_prepend: vec![value.into()],
                ..Default::default()
            };
            assert!(env.validate(Path::new("/source")).is_err(), "{value:?}");
        }
    }

    #[test]
    fn provider_skips_workspace_entries_before_expansion_and_counts_only_applied_paths() {
        let env = Environment {
            path_prepend: vec![
                "${HOME}/${RABI_WORKSPACE_PATH}/bin".into(),
                "/workspace/sub/../bin".into(),
                "/workspace".into(),
                "/trusted/tools".into(),
            ],
            set: BTreeMap::from([
                ("SKIPPED".into(), "${HOME}:${RABI_WORKSPACE_PATH}".into()),
                ("APPLIED".into(), "value".into()),
            ]),
        };
        let resolved = env
            .resolve_for(
                Path::new("/repo"),
                Path::new("/repo"),
                Path::new("/workspace"),
                (None, Some(OsStr::new("/bin"))),
                true,
            )
            .unwrap();
        assert_eq!(resolved.summary(), "PATH(+1), APPLIED");
        assert_eq!(resolved.values["PATH"], "/trusted/tools:/bin");
        assert!(!resolved.values.contains_key("SKIPPED"));
        assert_eq!(
            resolved.skipped,
            vec![
                Skipped::Set(1, "SKIPPED".into()),
                Skipped::Path(0),
                Skipped::Path(1),
                Skipped::Path(2)
            ]
        );
        let env = Environment {
            path_prepend: vec!["${HOME}/bin".into()],
            ..Default::default()
        };
        let resolved = env
            .resolve_for(
                Path::new("/repo"),
                Path::new("/repo"),
                Path::new("/workspace:checkout"),
                (Some(OsString::from("/workspace:checkout")), None),
                true,
            )
            .unwrap();
        assert!(resolved.values.is_empty());
        assert_eq!(resolved.skipped, vec![Skipped::Path(0)]);
    }

    #[test]
    fn provider_path_checks_follow_symlinks_and_allow_missing_outside_paths() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("rabi-env-paths-{}", uuid::Uuid::new_v4()));
        let workspace = root.join("workspace");
        std::fs::create_dir_all(workspace.join("bin")).unwrap();
        let alias = root.join("alias");
        symlink(&workspace, &alias).unwrap();
        assert!(inside_workspace(&alias.join("bin"), &workspace).unwrap());
        assert!(inside_workspace(&alias.join("missing/bin"), &workspace).unwrap());
        assert!(inside_workspace(&workspace.join("sub/../bin"), &workspace).unwrap());
        assert!(!inside_workspace(&root.join("missing/../tools"), &workspace).unwrap());
        std::fs::write(root.join("regular-file"), "content").unwrap();
        assert!(!inside_workspace(&root.join("regular-file/bin"), &workspace).unwrap());
        // Canonicalize before reducing '..' where an existing symlink affects it.
        std::fs::create_dir_all(root.join("outside")).unwrap();
        symlink(workspace.join("bin"), root.join("outside/link")).unwrap();
        assert!(inside_workspace(&root.join("outside/link/.."), &workspace).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn path_variables_reject_non_utf8_only_when_expansion_is_needed() {
        use std::os::unix::ffi::OsStrExt;
        let bad = Path::new(OsStr::from_bytes(b"/checkout-\xff"));
        let source = Path::new("/repo");
        assert!(Environment::default()
            .resolve(source, bad, bad, None, None)
            .is_ok());
        for variable in ["RABI_REPO_PATH", "RABI_WORKSPACE_PATH"] {
            let env = Environment {
                set: BTreeMap::from([("VALUE".into(), format!("${{{variable}}}"))]),
                ..Default::default()
            };
            assert!(env.validate(bad).is_ok());
            let message = env
                .resolve(source, bad, bad, None, None)
                .unwrap_err()
                .to_string();
            assert!(
                message.contains("UTF-8")
                    && message.contains(variable)
                    && message.contains("env.set.VALUE"),
                "{message}"
            );
            if variable == "RABI_WORKSPACE_PATH" {
                let resolved = env
                    .resolve_for(source, bad, bad, (None, None), true)
                    .unwrap();
                assert!(resolved.values.is_empty());
            }
        }
    }

    #[test]
    fn trust_declarations_show_values_with_terminal_controls_escaped() {
        let env = Environment {
            set: BTreeMap::from([
                ("COMMAND".into(), "first\nsecond\u{1b}[2J\u{202e}".into()),
                ("EMPTY".into(), "".into()),
            ]),
            ..Default::default()
        };
        let declaration = env.declaration();
        assert!(
            declaration.contains("set: COMMAND=first\\u{a}second\\u{1b}[2J\\u{202e}"),
            "{declaration}"
        );
        assert!(declaration.contains("set: EMPTY="));
        assert!(!declaration.contains('\u{1b}'));
    }
    #[test]
    fn environment_diagnostics_escape_sources_keys_and_unknown_variables() {
        let controls = "\u{1b}[2J\u{202e}";
        let source = PathBuf::from(format!("/source-{controls}"));
        let invalid_key = format!("INVALID{controls}");
        let env = Environment {
            set: BTreeMap::from([(invalid_key, "value".into())]),
            ..Default::default()
        };
        let message = env.validate(&source).unwrap_err().to_string();
        assert!(
            message.contains("source-\\u{1b}[2J\\u{202e}/.rabi.toml"),
            "{message}"
        );
        assert!(
            message.contains("env.set.INVALID\\u{1b}[2J\\u{202e}"),
            "{message}"
        );
        assert!(
            !message.contains('\u{1b}') && !message.contains('\u{202e}'),
            "{message}"
        );

        let env = Environment {
            set: BTreeMap::from([("VALUE".into(), format!("${{{controls}}}"))]),
            ..Default::default()
        };
        let message = env.validate(&source).unwrap_err().to_string();
        assert!(
            message.contains("unsupported variable ${\\u{1b}[2J\\u{202e}}"),
            "{message}"
        );
        assert!(
            !message.contains('\u{1b}') && !message.contains('\u{202e}'),
            "{message}"
        );
        // The application-time expander uses the same escaped variable labels.
        let message = unsupported_variable(controls).to_string();
        assert!(message.contains("${\\u{1b}[2J\\u{202e}}"), "{message}");
        assert!(
            !message.contains('\u{1b}') && !message.contains('\u{202e}'),
            "{message}"
        );
    }
}
