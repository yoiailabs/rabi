use std::fs;
use std::path::Path;

use serde::Deserialize;

use crate::error::{Error, Result};

const FILE_NAME: &str = ".rabi.toml";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RabiConfig {
    pub hooks: Hooks,
    pub env: crate::execution_env::Environment,
    pub precheck: Option<Precheck>,
    pub factory: Factory,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Hooks {
    pub setup: Vec<String>,
    pub pre_remove: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Precheck {
    pub commands: Vec<String>,
    pub timeout: Option<String>,
}

/// Local orchestration limits; zero fixes explicitly disables automatic retries.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Factory {
    pub fix_budget: u32,
    pub ci_timeout: String,
}
impl Default for Factory {
    fn default() -> Self {
        Self {
            fix_budget: 3,
            ci_timeout: "60m".into(),
        }
    }
}

/// Load declarations and validate syntax independently of the execution environment.
pub fn load_config(source: &Path) -> Result<Option<RabiConfig>> {
    load_config_for_execution(source, false)
}

/// An explicit execution bypass still parses TOML and its schema, but does not
/// validate environment values that will never be applied.
pub(crate) fn load_config_for_execution(source: &Path, no_env: bool) -> Result<Option<RabiConfig>> {
    let path = source.join(FILE_NAME);
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(Error::msg(format!(
                "Failed to read {}: {error}",
                path.display()
            )));
        }
    };
    let config = parse_config(&content)
        .map_err(|error| Error::msg(format!("{}: {error}", path.display())))?;
    if !no_env {
        config.env.validate(source)?;
    }
    Ok(Some(config))
}

pub fn setup_commands(config: Option<&RabiConfig>) -> &[String] {
    match config {
        Some(config) => &config.hooks.setup,
        None => &[],
    }
}

pub fn pre_remove_commands(config: Option<&RabiConfig>) -> &[String] {
    match config {
        Some(config) => &config.hooks.pre_remove,
        None => &[],
    }
}

pub(crate) fn parse_config(content: &str) -> Result<RabiConfig> {
    let config: RabiConfig = toml_edit::de::from_str(content)
        .map_err(|error| Error::msg(format!("Invalid {FILE_NAME}: {error}")))?;
    if let Some(precheck) = &config.precheck {
        if precheck
            .commands
            .iter()
            .any(|command| command.trim().is_empty() || command.contains('\0'))
        {
            return Err(Error::msg(
                "Invalid precheck.commands: commands must be nonempty and contain no NUL",
            ));
        }
        if let Some(timeout) = &precheck.timeout {
            crate::run::timeout(timeout).map_err(|_| {
                Error::msg(
                    "Invalid precheck.timeout: expected a positive duration with s, m or h suffix",
                )
            })?;
        }
    }
    crate::run::timeout(&config.factory.ci_timeout).map_err(|_| {
        Error::msg("Invalid factory.ci-timeout: expected a positive duration with s, m or h suffix")
    })?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_defaults_and_limits_are_validated() {
        let config = parse_config("").unwrap();
        assert_eq!(config.factory.fix_budget, 3);
        assert_eq!(config.factory.ci_timeout, "60m");
        let config = parse_config("[factory]\nfix-budget=0\nci-timeout='2h'").unwrap();
        assert_eq!(config.factory.fix_budget, 0);
        for declaration in [
            "fix-budget=-1",
            "fix-budget='3'",
            "ci-timeout='0m'",
            "ci-timeout=60",
            "extra=true",
        ] {
            assert!(
                parse_config(&format!("[factory]\n{declaration}")).is_err(),
                "{declaration}"
            );
        }
    }

    #[test]
    fn precheck_schema_and_values_are_validated() {
        let config =
            parse_config("[precheck]\ncommands = ['cargo fmt --check']\ntimeout = '2m'").unwrap();
        assert_eq!(config.precheck.unwrap().commands, vec!["cargo fmt --check"]);
        for text in [
            "[precheck]\nunknown = []",
            "[precheck]\ncommands = 'true'",
            "[precheck]\ncommands = ['']",
            "[precheck]\ncommands = [\"\\u0000\"]",
            "[precheck]\ntimeout = '0s'",
            "[precheck]\ntimeout = '2'",
            "[precheck]\ntimeout = 2",
        ] {
            assert!(parse_config(text).is_err(), "{text}");
        }
    }

    #[test]
    fn parses_project_hooks() {
        let config = parse_config(
            r#"
# repository hooks
[hooks]
setup = ["pnpm install"]
pre-remove = ["./scripts/teardown.sh"]
"#,
        )
        .expect("config");
        assert_eq!(config.hooks.setup, vec!["pnpm install".to_string()]);
        assert_eq!(
            config.hooks.pre_remove,
            vec!["./scripts/teardown.sh".to_string()]
        );
    }

    #[test]
    fn missing_hooks_are_empty() {
        let config = parse_config("").expect("empty");
        assert!(config.hooks.setup.is_empty());
        assert!(config.hooks.pre_remove.is_empty());

        let config = parse_config("[hooks]\nsetup = [\"pnpm install\"]\n").expect("partial");
        assert_eq!(config.hooks.setup, vec!["pnpm install".to_string()]);
        assert!(config.hooks.pre_remove.is_empty());
        assert!(setup_commands(None).is_empty());
        assert!(pre_remove_commands(None).is_empty());
    }

    #[test]
    fn rejects_unknown_keys_and_wrong_types() {
        let error = parse_config("worktreesDirectory = \"../phantom-worktrees\"\n")
            .expect_err("project directory");
        let message = error.to_string();
        assert!(message.contains(FILE_NAME), "{message}");
        assert!(message.contains("worktreesDirectory"), "{message}");

        let error =
            parse_config("directoryNameSeparator = \"-\"\n").expect_err("project separator");
        assert!(
            error.to_string().contains("directoryNameSeparator"),
            "{error}"
        );

        let error = parse_config("[hooks]\ncopy-files = [\".env\"]\n").expect_err("copy files");
        let message = error.to_string();
        assert!(message.contains("copy-files"), "{message}");

        let error = parse_config("[hooks]\nsetup = \"pnpm install\"\n").expect_err("type");
        let message = error.to_string();
        assert!(
            message.contains("invalid type") || message.contains("setup"),
            "{message}"
        );
    }

    #[test]
    fn parses_optional_environment_and_rejects_unknown_fields_and_wrong_types() {
        assert!(parse_config("").unwrap().env.is_empty());
        assert!(parse_config("[env]\n").unwrap().env.is_empty());
        let config = parse_config(
            r#"
[env]
path-prepend = ["/opt/tools", "${RABI_WORKSPACE_PATH}/bin"]
[env.set]
VALUE = ""
"#,
        )
        .unwrap();
        assert_eq!(config.env.path_prepend.len(), 2);
        assert_eq!(config.env.set["VALUE"], "");
        for text in [
            "[env]\nunknown = []",
            "[env]\npath_prepend = []",
            "[env]\npath-prepend = '/bin'",
            "[env]\nset = 'value'",
            "[env.set]\nVALUE = 1",
        ] {
            assert!(parse_config(text).is_err(), "{text}");
        }
    }

    #[test]
    fn environment_validation_errors_are_reported_at_config_load() {
        let root = std::env::temp_dir().join(format!("rabi-env-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for (text, key) in [
            ("[env.set]\nPATH = '/bin'", "set.PATH"),
            ("[env.set]\nVALUE = '$UNKNOWN'", "set.VALUE"),
            ("[env]\npath-prepend = ['/bin:/other']", "path-prepend[0]"),
            ("[env.set]\nVALUE = \"\\u0000\"", "set.VALUE"),
        ] {
            fs::write(root.join(FILE_NAME), text).unwrap();
            let error = load_config(&root).unwrap_err();
            assert_eq!(error.exit_code(), 1);
            let message = error.to_string();
            assert!(
                message.contains(&root.join(FILE_NAME).display().to_string()),
                "{message}"
            );
            assert!(message.contains(key), "{message}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_loading_does_not_expand_paths_and_diagnostics_name_the_source() {
        let root = std::env::temp_dir().join(format!("rabi-env-source-{}", uuid::Uuid::new_v4()));
        let source = root.join("source:checkout");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join(FILE_NAME),
            "[env]\npath-prepend = ['${RABI_REPO_PATH}/bin']\n",
        )
        .unwrap();
        assert!(load_config(&source).is_ok());

        fs::write(source.join(FILE_NAME), "[env.set]\nVALUE = '$UNKNOWN'\n").unwrap();
        let message = load_config(&source).unwrap_err().to_string();
        assert!(
            message.starts_with(&source.join(FILE_NAME).display().to_string()),
            "{message}"
        );
        assert!(message.contains("env.set.VALUE"), "{message}");
        assert!(
            !message.contains(&root.join("primary/.rabi.toml").display().to_string()),
            "{message}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn load_config_reports_the_file_and_the_problem() {
        let root = std::env::temp_dir().join(format!(
            "phantom-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("root");
        assert!(load_config(&root).expect("missing").is_none());

        fs::write(root.join(FILE_NAME), "[\n").expect("toml");
        let error = load_config(&root).expect_err("invalid toml");
        let message = error.to_string();
        assert!(message.contains(FILE_NAME), "{message}");
        assert!(
            message.contains("parse")
                || message.contains("expected")
                || message.contains("invalid"),
            "{message}"
        );

        fs::write(
            root.join(FILE_NAME),
            "[hooks]\npostCreate = [\"pnpm install\"]\n",
        )
        .expect("schema");
        let error = load_config(&root).expect_err("unknown key");
        let message = error.to_string();
        assert!(message.contains(FILE_NAME), "{message}");
        assert!(message.contains("postCreate"), "{message}");

        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn no_env_skips_value_validation_but_still_parses_toml_and_schema() {
        let root =
            std::env::temp_dir().join(format!("rabi-no-env-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        for text in [
            "[env.set]\nPATH = '/bin'",
            "[env.set]\nVALUE = '$UNKNOWN'",
            "[env]\npath-prepend = ['/bin:/other']",
            "[env.set]\nVALUE = \"\\u0000\"",
        ] {
            fs::write(root.join(FILE_NAME), text).unwrap();
            assert!(load_config_for_execution(&root, true).is_ok(), "{text}");
            assert!(load_config(&root).is_err(), "{text}");
        }
        for text in ["[", "[env]\nunknown = true", "[env.set]\nVALUE = 1"] {
            fs::write(root.join(FILE_NAME), text).unwrap();
            assert!(load_config_for_execution(&root, true).is_err(), "{text}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}
