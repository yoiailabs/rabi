use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use crate::config;
use crate::error::{Error, Result};
use crate::git;
use crate::output::visible_text;

pub const RUN_PROMPT: &str = "Trust this project and run its hooks? [y/N]";
pub const TRUST_PROMPT: &str = "Trust this project? [y/N]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Continue the command and run no hooks.
    SkipHooks,
    Run,
    Ask,
    /// Refuse the command before it changes the repository.
    Refuse,
    SkipCandidate,
}

#[derive(Debug, Clone, Copy)]
pub struct Facts {
    pub has_hooks: bool,
    pub trusted: bool,
    pub trust_all: bool,
    pub no_hooks: bool,
    pub json: bool,
    pub tty: bool,
    pub prompt: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct HookControl {
    pub no_hooks: bool,
    pub json: bool,
    pub prompt: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum HookKind {
    Setup,
    PreRemove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProject {
    pub path: String,
    pub trusted_at: String,
}

pub fn decide(facts: Facts) -> Decision {
    if !facts.has_hooks || facts.no_hooks {
        return Decision::SkipHooks;
    }
    if facts.trust_all || facts.trusted {
        return Decision::Run;
    }
    if !facts.prompt {
        return Decision::SkipCandidate;
    }
    if facts.json || !facts.tty {
        return Decision::Refuse;
    }
    Decision::Ask
}

pub fn trust_all() -> bool {
    std::env::var("RABI_TRUST_ALL").ok().as_deref() == Some("1")
}

pub fn answer_is_yes(line: &str) -> bool {
    let answer = line.trim();
    answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes")
}

/// A prompt is only safe for a person at a terminal. Agents and CI get a pty
/// often enough that asking would hang the process.
pub fn prompts() -> bool {
    !crate::runtime::non_interactive()
}

#[cfg(test)]
fn may_prompt(ci: bool, no_input: bool, stdin_tty: bool, stderr_tty: bool) -> bool {
    !ci && !no_input && stdin_tty && stderr_tty
}

/// Commands the caller should execute. Empty means run nothing and continue.
pub fn authorize(
    git_root: &Path,
    setup: &[String],
    pre_remove: &[String],
    kind: HookKind,
    control: HookControl,
) -> Result<Vec<String>> {
    let relevant = match kind {
        HookKind::Setup => setup,
        HookKind::PreRemove => pre_remove,
    };
    if relevant.is_empty() || control.no_hooks {
        return Ok(Vec::new());
    }
    if trust_all() {
        return Ok(relevant.to_vec());
    }
    let trusted = is_trusted_root(git_root)?;
    let facts = Facts {
        has_hooks: !relevant.is_empty(),
        trusted,
        trust_all: trust_all(),
        no_hooks: control.no_hooks,
        json: control.json,
        tty: prompts(),
        prompt: control.prompt,
    };
    match decide(facts) {
        Decision::SkipHooks => Ok(Vec::new()),
        Decision::Run => Ok(relevant.to_vec()),
        Decision::Ask => {
            let project = project_label(git_root);
            if confirm(&project, setup, pre_remove, RUN_PROMPT)? {
                remember(git_root)?;
                Ok(relevant.to_vec())
            } else {
                Err(untrusted(&project, setup, pre_remove))
            }
        }
        Decision::Refuse | Decision::SkipCandidate => {
            Err(untrusted(&project_label(git_root), setup, pre_remove))
        }
    }
}

/// An untrusted repository with pre-remove hooks is skipped by gc.
pub fn blocks_gc(git_root: &Path, pre_remove: &[String], no_hooks: bool) -> Result<bool> {
    if no_hooks || pre_remove.is_empty() || trust_all() {
        return Ok(false);
    }
    Ok(!is_trusted_root(git_root)?)
}

pub fn list_trusted() -> Result<Vec<TrustedProject>> {
    let records = load()?;
    Ok(records
        .into_iter()
        .map(|(path, trusted_at)| TrustedProject { path, trusted_at })
        .collect())
}

pub fn trust_root(git_root: &Path) -> Result<TrustedProject> {
    let path = common_dir_key(git_root)?;
    let key = path.to_string_lossy().to_string();
    let trusted_at = trusted_at_now();
    let saved_key = key.clone();
    let saved_at = trusted_at.clone();
    update(|records| {
        records.insert(key, trusted_at);
        true
    })?;
    Ok(TrustedProject {
        path: saved_key,
        trusted_at: saved_at,
    })
}

/// `true` when a record was removed.
pub fn untrust_root(git_root: &Path) -> Result<bool> {
    let path = common_dir_key(git_root)?;
    let key = path.to_string_lossy().to_string();
    update(|records| records.remove(&key).is_some())
}

pub struct ProjectView {
    pub project: String,
    pub git_root: PathBuf,
    pub setup: Vec<String>,
    pub pre_remove: Vec<String>,
    pub env: crate::execution_env::Environment,
}

pub fn load_hooks(start: &Path) -> Result<ProjectView> {
    let git_root = git::git_root(start)?;
    let source = git::top_level(start).unwrap_or_else(|_| git_root.clone());
    let loaded = config::load_config_from(&source, &git_root)?;
    let env = crate::execution_env::load(&git_root)?;
    Ok(ProjectView {
        project: project_label(&git_root),
        git_root,
        env,
        setup: config::setup_commands(loaded.as_ref()).to_vec(),
        pre_remove: config::pre_remove_commands(loaded.as_ref()).to_vec(),
    })
}

pub fn ask(project: &ProjectView, question: &str) -> Result<bool> {
    confirm(
        &project.project,
        &project.setup,
        &project.pre_remove,
        question,
    )
}

pub fn project_label(git_root: &Path) -> String {
    match fs::canonicalize(git_root) {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(_) => git_root.to_string_lossy().to_string(),
    }
}

pub fn format_hooks(setup: &[String], pre_remove: &[String]) -> String {
    format!(
        "{}\n{}",
        format_list("setup", setup),
        format_list("pre-remove", pre_remove)
    )
}

pub fn refusal_message(project: &str, setup: &[String], pre_remove: &[String]) -> String {
    format!(
        "Untrusted project: {}\n{}\nRefusing to run project hooks. Run `rabi trust add` or pass `--no-setup` (create) / `--no-hooks` (setup, delete, gc).",
        visible_text(project),
        format_hooks(setup, pre_remove)
    )
}

fn untrusted(project: &str, setup: &[String], pre_remove: &[String]) -> Error {
    Error::Untrusted(refusal_message(project, setup, pre_remove))
}

fn confirm(project: &str, setup: &[String], pre_remove: &[String], question: &str) -> Result<bool> {
    crate::output::errln!("Project: {}", visible_text(project));
    crate::output::errln!("{}", format_hooks(setup, pre_remove));
    crate::output::stderr(format_args!("{question} "));
    crate::output::check_stderr()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(prompt_io)?;
    Ok(answer_is_yes(&line))
}

fn prompt_io(error: std::io::Error) -> Error {
    Error::msg(format!("Failed to read the trust prompt: {error}"))
}

fn remember(git_root: &Path) -> Result<()> {
    let path = common_dir_key(git_root)?;
    let key = path.to_string_lossy().to_string();
    let trusted_at = trusted_at_now();
    update(|records| {
        records.insert(key, trusted_at);
        true
    })?;
    Ok(())
}

fn update(body: impl FnOnce(&mut BTreeMap<String, String>) -> bool) -> Result<bool> {
    let path = crate::paths::state_dir()?.join("trust.toml");
    update_at(&path, body)
}

fn update_at(
    path: &Path,
    body: impl FnOnce(&mut BTreeMap<String, String>) -> bool,
) -> Result<bool> {
    let Some(dir) = path.parent().map(Path::to_path_buf) else {
        return Err(Error::msg(format!("Failed to write {}", path.display())));
    };
    fs::create_dir_all(&dir)
        .map_err(|error| Error::msg(format!("Failed to write {}: {error}", path.display())))?;
    let mut records = load_at(path)?;
    let dirty = body(&mut records);
    if dirty {
        write_records(path, &records)?;
    }
    Ok(dirty)
}

pub(crate) fn is_trusted_root(git_root: &Path) -> Result<bool> {
    let path = common_dir_key(git_root)?;
    let records = load()?;
    Ok(records.contains_key(&path.to_string_lossy().to_string()))
}

pub fn common_dir_key(git_root: &Path) -> Result<PathBuf> {
    git::canonical_common_dir(git_root)
}

fn trust_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("trust.toml"))
}

fn state_dir() -> Result<PathBuf> {
    crate::paths::state_dir()
}

fn load() -> Result<BTreeMap<String, String>> {
    load_at(&trust_path()?)
}

fn load_at(path: &Path) -> Result<BTreeMap<String, String>> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(Error::msg(format!(
                "Failed to read {}: {error}",
                path.display()
            )));
        }
    };
    parse_store(path, &content)
}

fn write_records(path: &Path, records: &BTreeMap<String, String>) -> Result<()> {
    crate::atomic::write(path, render_store(records).as_bytes())
        .map_err(|error| Error::msg(format!("Failed to write {}: {error}", path.display())))
}

fn parse_store(path: &Path, content: &str) -> Result<BTreeMap<String, String>> {
    if content.trim().is_empty() {
        return Ok(BTreeMap::new());
    }
    let document = content
        .parse::<DocumentMut>()
        .map_err(|error| Error::msg(format!("Failed to read {}: {error}", path.display())))?;
    let Some(item) = document.get("project") else {
        return Ok(BTreeMap::new());
    };
    let Some(tables) = item.as_array_of_tables() else {
        return Err(Error::msg(format!(
            "Failed to read {}: project must be a list of tables",
            path.display()
        )));
    };
    let mut records = BTreeMap::new();
    for table in tables {
        let stored = table.get("path").and_then(Item::as_str).ok_or_else(|| {
            Error::msg(format!(
                "Failed to read {}: a project entry is missing path",
                path.display()
            ))
        })?;
        let trusted_at = table
            .get("trustedAt")
            .and_then(Item::as_str)
            .ok_or_else(|| {
                Error::msg(format!(
                    "Failed to read {}: a project entry is missing trustedAt",
                    path.display()
                ))
            })?;
        records.insert(stored.to_string(), trusted_at.to_string());
    }
    Ok(records)
}

fn render_store(records: &BTreeMap<String, String>) -> String {
    let mut array = ArrayOfTables::new();
    for (path, trusted_at) in records {
        let mut table = Table::new();
        table["path"] = toml_edit::value(path.as_str());
        table["trustedAt"] = toml_edit::value(trusted_at.as_str());
        array.push(table);
    }
    let mut document = DocumentMut::new();
    document["project"] = Item::ArrayOfTables(array);
    document.to_string()
}

fn format_list(name: &str, commands: &[String]) -> String {
    if commands.is_empty() {
        return format!("{name}: (none)");
    }
    let mut text = format!("{name}:");
    for command in commands {
        text.push_str("\n  ");
        text.push_str(&visible_command(command));
    }
    text
}

fn visible_command(command: &str) -> String {
    visible_text(command)
}

fn trusted_at_now() -> String {
    let now = chrono::Utc::now();
    format!("{}Z", now.format("%Y-%m-%dT%H:%M:%S%.3f"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(has_hooks: bool, trusted: bool, trust_all: bool, no_hooks: bool) -> Facts {
        Facts {
            has_hooks,
            trusted,
            trust_all,
            no_hooks,
            json: false,
            tty: true,
            prompt: true,
        }
    }

    #[test]
    fn a_repository_without_hooks_does_not_ask() {
        assert_eq!(
            decide(facts(false, false, false, false)),
            Decision::SkipHooks
        );
    }

    #[test]
    fn no_hooks_skips_even_when_the_repository_is_untrusted() {
        assert_eq!(decide(facts(true, false, false, true)), Decision::SkipHooks);
    }

    #[test]
    fn trust_all_runs_hooks_without_a_record() {
        assert_eq!(decide(facts(true, false, true, false)), Decision::Run);
    }

    #[test]
    fn a_trusted_repository_runs_hooks() {
        assert_eq!(decide(facts(true, true, false, false)), Decision::Run);
    }

    #[test]
    fn an_interactive_terminal_asks() {
        assert_eq!(decide(facts(true, false, false, false)), Decision::Ask);
    }

    #[test]
    fn json_and_a_closed_stdin_refuse() {
        let mut json = facts(true, false, false, false);
        json.json = true;
        assert_eq!(decide(json), Decision::Refuse);
        let mut closed = facts(true, false, false, false);
        closed.tty = false;
        assert_eq!(decide(closed), Decision::Refuse);
    }

    #[test]
    fn gc_skips_an_untrusted_repository_and_does_not_ask() {
        let mut gc = facts(true, false, false, false);
        gc.prompt = false;
        gc.tty = true;
        assert_eq!(decide(gc), Decision::SkipCandidate);
        gc.no_hooks = true;
        assert_eq!(decide(gc), Decision::SkipHooks);
        gc.no_hooks = false;
        gc.has_hooks = false;
        assert_eq!(decide(gc), Decision::SkipHooks);
    }

    #[test]
    fn ci_agent_and_a_closed_stream_do_not_prompt() {
        assert!(may_prompt(false, false, true, true));
        assert!(!may_prompt(true, false, true, true));
        assert!(!may_prompt(false, true, true, true));
        assert!(!may_prompt(false, false, false, true));
        assert!(!may_prompt(false, false, true, false));
    }

    #[test]
    fn yes_is_the_only_affirmative_answer() {
        assert!(answer_is_yes("y"));
        assert!(answer_is_yes("Y"));
        assert!(answer_is_yes(" yes "));
        assert!(!answer_is_yes(""));
        assert!(!answer_is_yes("n"));
        assert!(!answer_is_yes("no"));
        assert!(!answer_is_yes("yep"));
    }

    #[test]
    fn the_store_round_trips_path_and_trusted_at() {
        let mut records = BTreeMap::new();
        records.insert(
            "/repo/.git".to_string(),
            "2026-10-20T00:00:00.000Z".to_string(),
        );
        records.insert(
            "/other/.git".to_string(),
            "2026-10-20T01:00:00.000Z".to_string(),
        );
        let rendered = render_store(&records);
        let parsed = parse_store(Path::new("trust.toml"), &rendered).expect("parse");
        assert_eq!(parsed, records);
        assert!(rendered.contains("trustedAt"));
        assert!(rendered.contains("[[project]]"));
    }

    #[test]
    fn control_characters_cannot_rewrite_the_prompt() {
        let text = format_hooks(&["ok\n\u{1b}[2J\u{202e}\u{2028}rm -rf /".to_string()], &[]);
        assert!(text.contains("\\u{a}"), "{text}");
        assert!(text.contains("\\u{1b}"), "{text}");
        assert!(text.contains("\\u{202e}"), "{text}");
        assert!(text.contains("\\u{2028}"), "{text}");
        assert!(!text.contains('\u{1b}'), "{text}");
        assert!(!text.contains('\u{202e}'), "{text}");
        assert!(!text.contains('\u{2028}'), "{text}");
        let command_line = text
            .lines()
            .find(|line| line.contains("ok"))
            .expect("command");
        assert!(command_line.contains("rm -rf /"), "{command_line}");
    }

    #[test]
    fn a_missing_field_is_an_error() {
        let error = parse_store(
            Path::new("trust.toml"),
            "[[project]]\npath = \"/repo/.git\"\n",
        )
        .expect_err("missing trustedAt");
        assert!(error.to_string().contains("trustedAt"), "{error}");
    }
}
