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
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Hooks {
    pub setup: Vec<String>,
    pub pre_remove: Vec<String>,
}

/// Load a primary checkout's configuration. Callers selecting a different
/// configuration source must supply the actual primary to `load_config_from`.
pub fn load_config(primary: &Path) -> Result<Option<RabiConfig>> {
    load_config_from(primary, primary)
}

pub(crate) fn load_config_from(source: &Path, primary: &Path) -> Result<Option<RabiConfig>> {
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
    config.env.validate(source, primary)?;
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

fn parse_config(content: &str) -> Result<RabiConfig> {
    toml_edit::de::from_str(content)
        .map_err(|error| Error::msg(format!("Invalid {FILE_NAME}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

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
            ("[env]\npath-prepend = ['relative']", "path-prepend[0]"),
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
    fn config_source_is_distinct_from_primary_expansion_and_diagnostics() {
        let root = std::env::temp_dir().join(format!("rabi-env-source-{}", uuid::Uuid::new_v4()));
        let source = root.join("source:checkout");
        let primary = root.join("primary");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join(FILE_NAME),
            "[env]\npath-prepend = ['${RABI_REPO_PATH}/bin']\n",
        )
        .unwrap();
        assert!(load_config_from(&source, &primary).is_ok());

        let invalid_primary = root.join("primary:checkout");
        let message = load_config_from(&source, &invalid_primary)
            .unwrap_err()
            .to_string();
        assert!(
            message.starts_with(&source.join(FILE_NAME).display().to_string()),
            "{message}"
        );
        assert!(message.contains("env.path-prepend[0]"), "{message}");
        assert!(
            !message.contains(&invalid_primary.display().to_string()),
            "{message}"
        );

        fs::write(source.join(FILE_NAME), "[env.set]\nVALUE = '$UNKNOWN'\n").unwrap();
        let message = load_config_from(&source, &primary).unwrap_err().to_string();
        assert!(
            message.starts_with(&source.join(FILE_NAME).display().to_string()),
            "{message}"
        );
        assert!(message.contains("env.set.VALUE"), "{message}");
        assert!(
            !message.contains(&primary.join(FILE_NAME).display().to_string()),
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
}
