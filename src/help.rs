use std::io::IsTerminal;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::{Error, Result};

#[derive(Debug, Deserialize)]
pub(crate) struct HelpFile {
    pub(crate) commands: Vec<CommandSummary>,
    #[serde(rename = "globalOptions")]
    pub(crate) global_options: Vec<CommandOption>,
    pub(crate) helps: std::collections::BTreeMap<String, CommandHelp>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommandSummary {
    pub(crate) name: String,
    pub(crate) description: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommandHelp {
    name: String,
    pub(crate) description: String,
    pub(crate) usage: String,
    #[serde(default)]
    pub(crate) options: Vec<CommandOption>,
    #[serde(default)]
    examples: Vec<CommandExample>,
    #[serde(default)]
    notes: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CommandOption {
    pub(crate) name: String,
    pub(crate) short: Option<String>,
    #[serde(default)]
    pub(crate) aliases: Vec<String>,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) description: String,
    #[serde(default)]
    multiple: bool,
    example: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CommandExample {
    description: String,
    command: String,
}

pub(crate) fn data() -> Result<&'static HelpFile> {
    static DATA: OnceLock<std::result::Result<HelpFile, String>> = OnceLock::new();
    match DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../help-data.json")).map_err(|error| error.to_string())
    }) {
        Ok(file) => Ok(file),
        Err(error) => Err(Error::msg(error.clone())),
    }
}

pub fn json_help(key: &str) -> Result<serde_json::Value> {
    let mut value: serde_json::Value = serde_json::from_str(include_str!("../help-data.json"))
        .map_err(|error| Error::msg(error.to_string()))?;
    if !key.is_empty() {
        let selected = value["helps"]
            .get(key)
            .cloned()
            .ok_or_else(|| Error::validation(format!("Unknown command '{key}'")))?;
        value["selected"] = selected;
    }
    Ok(value)
}

#[allow(clippy::vec_init_then_push)]
pub fn main_help() -> Result<String> {
    let file = data()?;
    let mut lines = Vec::new();
    lines.push(bold(
        "rabi - Isolated Git workspaces for parallel coding agents",
    ));
    lines.push(String::new());
    lines.push(dim(
        "Manages local Git workspaces so many coding agents and humans can work independently in one repository",
    ));
    lines.push(String::new());
    lines.push(bold("USAGE"));
    lines.push("  rabi <command> [options]".to_string());
    lines.push(String::new());
    lines.push(bold("COMMANDS"));
    let max_name = file
        .commands
        .iter()
        .map(|command| command.name.len())
        .max()
        .unwrap_or(0);
    for command in &file.commands {
        lines.push(format!(
            "  {}{}",
            cyan(&format!("{:width$}", command.name, width = max_name + 2)),
            command.description
        ));
    }
    lines.push(String::new());
    lines.push(bold("GLOBAL OPTIONS"));
    for option in &file.global_options {
        lines.push(format!(
            "  {}  {}",
            cyan(&option_name(option)),
            option.description
        ));
    }
    lines.push(String::new());
    lines.push(dim(
        "Run 'rabi <command> --help' for more information on a command.",
    ));
    Ok(lines.join("\n"))
}

#[allow(clippy::vec_init_then_push)]
pub fn command_help(key: &str) -> Result<Option<String>> {
    let file = data()?;
    let Some(help) = file.helps.get(key) else {
        return Ok(None);
    };
    let mut lines = Vec::new();
    lines.push(bold(&format!("rabi {}", help.name)));
    lines.push(dim(&help.description));
    lines.push(String::new());
    lines.push(bold("USAGE"));
    lines.push(format!("  {}", help.usage));
    lines.push(String::new());
    if !help.options.is_empty() {
        lines.push(bold("OPTIONS"));
        let max_option = help
            .options
            .iter()
            .map(|option| option_name(option).len())
            .max()
            .unwrap_or(0);
        for option in &help.options {
            let name = option_name(option);
            let description = wrap_text(&option.description, max_option + 4);
            let padded = format!("{name:width$}", width = max_option + 2);
            let first = description.first().map(String::as_str).unwrap_or("");
            lines.push(format!("  {}{first}", cyan(&padded)));
            for extra in description.iter().skip(1) {
                lines.push(format!("  {:width$}{extra}", "", width = max_option + 2));
            }
            if let Some(example) = &option.example {
                let indent = " ".repeat(max_option + 4);
                lines.push(format!("  {indent}{}", dim(&format!("Example: {example}"))));
            }
        }
        lines.push(String::new());
    }
    if !help.examples.is_empty() {
        lines.push(bold("EXAMPLES"));
        for example in &help.examples {
            lines.push(format!("  {}", dim(&example.description)));
            lines.push(format!("    $ {}", example.command));
            lines.push(String::new());
        }
    }
    if !help.notes.is_empty() {
        lines.push(bold("NOTES"));
        for note in &help.notes {
            for line in wrap_text(note, 2) {
                lines.push(format!("  {line}"));
            }
        }
        lines.push(String::new());
    }
    Ok(Some(lines.join("\n")))
}

fn option_name(option: &CommandOption) -> String {
    let mut flags = Vec::new();
    if let Some(short) = &option.short {
        flags.push(format!("-{short}"));
    }
    flags.push(format!("--{}", option.name));
    for alias in &option.aliases {
        flags.push(format!("--{alias}"));
    }
    let mut rendered = flags.join(", ");
    if option.kind == "string" {
        rendered.push(' ');
        rendered.push_str(if option.multiple {
            "<value>..."
        } else {
            "<value>"
        });
    }
    rendered
}

fn wrap_text(text: &str, indent: usize) -> Vec<String> {
    let max_width = width().saturating_sub(indent).saturating_sub(2);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split(' ') {
        if !current.is_empty() && current.len() + word.len() + 1 > max_width {
            lines.push(std::mem::take(&mut current));
            current = word.to_string();
        } else if current.is_empty() {
            current = word.to_string();
        } else {
            current.push(' ');
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

fn width() -> usize {
    if !std::io::stdout().is_terminal() {
        return 80;
    }
    terminal_size::terminal_size()
        .map(|(width, _)| width.0 as usize)
        .unwrap_or(80)
}

fn bold(text: &str) -> String {
    styled("1", text)
}

fn dim(text: &str) -> String {
    styled("2", text)
}

fn cyan(text: &str) -> String {
    styled("36", text)
}

fn styled(code: &str, text: &str) -> String {
    if colors_enabled() {
        format!("\u{1b}[{code}m{text}\u{1b}[0m")
    } else {
        text.to_string()
    }
}

fn colors_enabled() -> bool {
    color_enabled(
        std::env::var("NO_COLOR").ok().as_deref(),
        std::env::var("FORCE_COLOR").ok().as_deref(),
        std::io::stdout().is_terminal(),
    )
}

fn color_enabled(no_color: Option<&str>, force_color: Option<&str>, stdout_is_tty: bool) -> bool {
    if no_color.is_some_and(|value| !value.is_empty()) {
        return false;
    }
    if let Some(value) = force_color {
        return value != "0" && !value.eq_ignore_ascii_case("false");
    }
    stdout_is_tty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_options_use_two_dashes() {
        for key in ["list", "create", "adopt", "exec", "remove", "path"] {
            let help = command_help(key).expect("help data").expect(key);
            assert!(
                !help.contains("----"),
                "{key} rendered a four-dash option:\n{help}"
            );
        }
        let list = command_help("list").expect("help").expect("list");
        assert!(list.contains("--names"));
        assert!(list.contains("--kind"));
        assert!(list.contains("--json"));
        assert!(list.contains("-R, --repo <value>"));
        assert!(!list.contains("Put this before the workspace name"));
        let exec = command_help("exec").expect("help").expect("exec");
        let exec = exec.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(exec.contains("--repo <value>"));
        assert!(exec.contains("put this before SEL"));
        assert!(exec.contains("rabi exec -- git status"));
        let where_help = command_help("path").expect("help").expect("path");
        let where_help = where_help.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(!where_help.contains("Put this before the workspace name"));
        assert!(where_help.contains("may appear with the other options"));
        let checkout = command_help("create").expect("help").expect("checkout");
        assert!(checkout.contains("--base <value>"));
    }

    #[test]
    fn rendered_help_names_the_rabi_command() {
        let main = main_help().expect("main help");
        assert!(main.contains("rabi - Isolated Git workspaces for parallel coding agents"));
        assert!(main.contains("rabi <command> [options]"));
        assert!(main.contains("Run 'rabi <command> --help'"));
        assert!(!main.contains("Phantom"), "{main}");
        assert!(!main.contains("ptm"), "{main}");
        assert!(!main.contains("PTM"), "{main}");
        assert!(!mentions_phantom_command(&main), "{main}");
        let file = data().expect("help data");
        assert!(!file.helps.is_empty());
        for key in file.helps.keys() {
            let help = command_help(key).expect("render").expect(key);
            assert!(
                help.contains("rabi "),
                "{key} help does not name rabi:\n{help}"
            );
            assert!(
                !help.contains("Phantom"),
                "{key} still names Phantom:\n{help}"
            );
            assert!(!help.contains("ptm"), "{key} still names ptm:\n{help}");
            assert!(!help.contains("PTM"), "{key} still names PTM:\n{help}");
            assert!(
                !mentions_phantom_command(&help),
                "{key} still tells the user to run phantom:\n{help}"
            );
        }
    }

    #[test]
    fn help_names_workspaces_and_rabi_env() {
        let main = main_help().expect("main help");
        assert!(main.contains("Create a workspace"));
        assert!(main.contains("List all workspaces"));
        assert!(main.contains("Retire workspaces"));
        assert!(main.contains("Drop workspace records"));
        for key in ["create", "remove", "exec"] {
            let help = command_help(key).expect("render").expect(key);
            for variable in [
                "RABI=1",
                "RABI_WORKSPACE",
                "RABI_WORKSPACE_ID",
                "RABI_WORKSPACE_PATH",
                "RABI_REPO_PATH",
            ] {
                assert!(help.contains(variable), "{key}: {variable}: {help}");
            }
        }
    }

    fn mentions_phantom_command(help: &str) -> bool {
        const COMMANDS: &[&str] = &[
            "<command>",
            "--version",
            "-v",
            "create",
            "attach",
            "list",
            "path",
            "remove",
            "exec",
            "config",
            "version",
            "repo",
            "clone",
            "github",
            "gh",
        ];
        help.split("phantom ")
            .skip(1)
            .any(|rest| COMMANDS.iter().any(|command| rest.starts_with(command)))
    }

    #[test]
    fn color_follows_tty_no_color_and_force_color() {
        assert!(!color_enabled(None, None, false));
        assert!(color_enabled(None, None, true));
        assert!(!color_enabled(Some("1"), None, true));
        assert!(color_enabled(Some(""), None, true));
        assert!(color_enabled(None, Some("1"), false));
        assert!(color_enabled(None, Some(""), false));
        assert!(!color_enabled(None, Some("0"), true));
        assert!(!color_enabled(None, Some("false"), true));
        assert!(!color_enabled(Some("1"), Some("1"), true));
    }
}
