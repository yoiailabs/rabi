//! Repository execution environment and the shared, deliberately limited expander.
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

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

#[derive(Debug, Default)]
pub(crate) struct Resolved {
    pub values: BTreeMap<String, OsString>,
    path_count: usize,
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
}

impl Environment {
    pub fn is_empty(&self) -> bool {
        self.path_prepend.is_empty() && self.set.is_empty()
    }

    pub(crate) fn validate(&self, source: &Path, repo: &Path) -> Result<()> {
        // The target workspace is unknown at config load. Use a clean absolute
        // placeholder to validate its expansion syntax without borrowing the
        // primary path's contents. Resolve checks the real target at launch;
        // repository expansions already use the actual primary path here.
        self.resolve(
            source,
            repo,
            Path::new("/rabi-workspace"),
            std::env::var_os("HOME"),
            None,
        )
        .map(|_| ())
    }

    pub(crate) fn resolve(
        &self,
        source: &Path,
        repo: &Path,
        workspace: &Path,
        home: Option<OsString>,
        inherited_path: Option<&OsStr>,
    ) -> Result<Resolved> {
        let expander = Expander {
            home,
            variable: |key: &str| match key {
                "RABI_WORKSPACE_PATH" => Ok(workspace.display().to_string()),
                "RABI_REPO_PATH" => Ok(repo.display().to_string()),
                _ => Err(Error::msg(format!("unsupported variable ${{{key}}}"))),
            },
        };
        let invalid = |key: &str, reason: &dyn std::fmt::Display| {
            Error::msg(format!(
                "{}: env.{key}: {reason}",
                source.join(".rabi.toml").display()
            ))
        };
        let mut values = BTreeMap::new();
        for (key, value) in &self.set {
            let field = format!("set.{key}");
            if !valid_key(key) {
                return Err(invalid(&field, &"invalid environment variable name"));
            }
            if reserved_key(key) {
                return Err(invalid(&field, &"reserved environment variable"));
            }
            let expanded = expander.expand(value).map_err(|e| invalid(&field, &e))?;
            if expanded.contains('\0') {
                return Err(invalid(&field, &"NUL is not allowed"));
            }
            values.insert(key.clone(), expanded.into());
        }
        let mut paths = Vec::new();
        for (index, value) in self.path_prepend.iter().enumerate() {
            let key = format!("path-prepend[{index}]");
            let expanded = expander.expand(value).map_err(|e| invalid(&key, &e))?;
            if expanded.is_empty()
                || expanded.contains(['\0', ':'])
                || !Path::new(&expanded).is_absolute()
            {
                return Err(invalid(&key, &"entry must be non-empty, absolute after expansion, and contain neither NUL nor ':'"));
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

    pub(crate) fn declaration(&self) -> String {
        let paths = self
            .path_prepend
            .iter()
            .map(|path| format!("  path-prepend: {}", crate::output::visible_text(path)));
        let keys = self.set.keys().map(|key| format!("  set: {key}"));
        std::iter::once("env:".to_owned())
            .chain(paths)
            .chain(keys)
            .collect::<Vec<_>>()
            .join("\n")
    }
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
    ) || key.starts_with("RABI")
        || key.starts_with("LD_")
        || key.starts_with("DYLD_")
}

/// Load only the primary checkout's declaration; callers decide the trust policy.
pub(crate) fn load(repo: &Path) -> Result<Environment> {
    Ok(crate::config::load_config_from(repo, repo)?
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
    fn config_validation_defers_workspace_contents_but_validates_the_actual_repo() {
        let env = Environment {
            path_prepend: vec!["${RABI_WORKSPACE_PATH}/bin".into()],
            ..Default::default()
        };
        let primary = Path::new("/primary:checkout");
        assert!(env.validate(primary, primary).is_ok());
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
        assert!(env.validate(primary, primary).is_err());
        assert!(env
            .validate(Path::new("/primary"), Path::new("/primary"))
            .is_ok());
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
}
