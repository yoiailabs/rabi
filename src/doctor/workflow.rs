//! Conservative workflow heuristics without a YAML dependency. Trigger parsing
//! interprets only the on header and declarations at one event indent. Nested
//! configuration is opaque; comments and collection/quote checks are line-local.
//! Permissions, raw secret references and condition checks are independent.
use super::Finding;

pub(super) struct Workflow {
    pub name: String,
    pub contents: String,
}

struct Line {
    indent: usize,
    text: String,
    key: String,
    value: String,
}

fn uncomment(line: &str) -> String {
    let mut quote = None;
    let mut escaped = false;
    let mut result = String::new();
    for ch in line.chars() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && quote == Some('"') {
            escaped = true;
        } else if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if quote.is_none()
            && ch == '#'
            && (result.is_empty() || result.ends_with(char::is_whitespace))
        {
            break;
        }
        result.push(ch);
    }
    result
}

fn tokens(value: &str) -> impl Iterator<Item = &str> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|s| !s.is_empty())
}

fn lines(contents: &str) -> Vec<Line> {
    let mut scalar_indent = None;
    contents
        .lines()
        .filter_map(|raw| {
            let indent = raw.len() - raw.trim_start().len();
            let in_scalar =
                scalar_indent.is_some_and(|parent| raw.trim().is_empty() || indent > parent);
            if !in_scalar {
                scalar_indent = None;
            }
            let text = if in_scalar {
                raw.to_owned()
            } else {
                uncomment(raw)
            };
            let trimmed = text.trim();
            if trimmed.is_empty() || matches!(trimmed, "---" | "...") {
                return None;
            }
            let (key, value) = trimmed.split_once(':').unwrap_or(("", ""));
            if !in_scalar && value.trim().starts_with(['|', '>']) {
                scalar_indent = Some(indent);
            }
            Some(Line {
                indent,
                text: trimmed.to_owned(),
                key: key.trim().trim_matches(['\'', '"']).to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

fn block(lines: &[Line], index: usize) -> &[Line] {
    let end = (index + 1..lines.len())
        .find(|&i| lines[i].indent <= lines[index].indent)
        .unwrap_or(lines.len());
    &lines[index..end]
}

#[derive(Default)]
struct Triggers {
    pull_request: bool,
    pull_request_target: bool,
    uncertain: bool,
}

impl Triggers {
    fn event(&mut self, name: &str) {
        self.pull_request |= name == "pull_request";
        self.pull_request_target |= name == "pull_request_target";
    }
}

fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn indentation(raw: &str) -> (usize, bool) {
    let indent = raw.len() - raw.trim_start().len();
    (
        indent,
        raw.as_bytes()[..indent].iter().any(|byte| *byte != b' '),
    )
}

fn empty_trigger_line(raw: &str) -> bool {
    raw.trim().is_empty() || raw.trim_start().starts_with('#')
}

// This state exists for one structural line only. It removes comments and
// checks that a quoted scalar/flow collection opened here also closes here.
// No nested configuration line is ever passed to this function.
fn trigger_line(text: &str) -> (&str, bool) {
    let mut quote = None;
    let mut flow = Vec::new();
    let mut node_start = true;
    let mut previous: Option<char> = None;
    let mut malformed = false;
    let mut characters = text.char_indices().peekable();
    while let Some((offset, ch)) = characters.next() {
        if let Some(delimiter) = quote {
            if delimiter == '"' && ch == '\\' {
                if characters.next().is_none() {
                    return (text, false);
                }
            } else if ch == delimiter {
                if delimiter == '\'' && characters.peek().is_some_and(|(_, next)| *next == '\'') {
                    characters.next();
                } else {
                    quote = None;
                    node_start = false;
                }
            }
        } else if ch == '#' && previous.is_none_or(char::is_whitespace) {
            return (text[..offset].trim_end(), !malformed && flow.is_empty());
        } else if node_start && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if matches!(ch, '[' | '{') && (node_start || !flow.is_empty()) {
            flow.push(ch);
            node_start = true;
        } else if matches!(ch, ']' | '}') && !flow.is_empty() {
            malformed |= flow.pop() != Some(if ch == ']' { '[' } else { '{' });
            node_start = false;
        } else if ch == ':'
            && (!flow.is_empty()
                || characters
                    .peek()
                    .is_none_or(|(_, next)| next.is_whitespace()))
            || ch == ',' && !flow.is_empty()
        {
            node_start = true;
        } else if !ch.is_whitespace() {
            node_start = false;
        }
        previous = Some(ch);
    }
    (
        text.trim_end(),
        !malformed && quote.is_none() && flow.is_empty(),
    )
}

fn inline_events(value: &str, result: &mut Triggers) {
    if let Some(inner) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        let inner = inner.trim();
        let inner = inner.strip_suffix(',').unwrap_or(inner).trim_end();
        for name in inner.split(',').map(str::trim) {
            if plain_name(name) {
                result.event(name);
            } else {
                result.uncertain = true;
            }
        }
    } else if plain_name(value) {
        result.event(value);
    } else {
        result.uncertain = true;
    }
}

// Consume only the on header and event-level lines. Deeper indentation is
// event configuration, so its prose, comments and YAML indicators are opaque.
fn on_events(raw: &[&str], start: usize) -> (Triggers, usize) {
    let mut result = Triggers::default();
    let (header, complete) = trigger_line(raw[start].trim());
    let value = header.split_once(':').map_or("", |(_, value)| value.trim());
    result.uncertain = !complete;
    if !value.is_empty() {
        inline_events(value, &mut result);
        return (result, start + 1);
    }
    let Some(first) = (start + 1..raw.len()).find(|&i| !empty_trigger_line(raw[i])) else {
        result.uncertain = true;
        return (result, raw.len());
    };
    let (event_indent, bad_indent) = indentation(raw[first]);
    result.uncertain |= bad_indent;
    // YAML also permits an indentless sequence directly below on:.
    let indentless_sequence = event_indent == 0 && raw[first].starts_with("- ");
    if event_indent == 0 && !indentless_sequence {
        result.uncertain = true;
        return (result, first);
    }
    for (i, &line) in raw.iter().enumerate().skip(first) {
        if empty_trigger_line(line) {
            continue;
        }
        let (indent, bad_indent) = indentation(line);
        result.uncertain |= bad_indent;
        if indent < event_indent || indentless_sequence && indent == 0 && !line.starts_with("- ") {
            result.uncertain |= indent > 0;
            return (result, i);
        }
        if indent > event_indent {
            continue;
        }
        let (code, complete) = trigger_line(line.trim());
        result.uncertain |= !complete;
        if let Some(name) = code.strip_prefix("- ") {
            if plain_name(name.trim()) {
                result.event(name.trim());
            } else {
                result.uncertain = true;
            }
        } else if let Some((name, value)) = code.split_once(':') {
            let name = name.trim();
            let value = value.trim();
            let (_, complete_value) = trigger_line(value);
            if plain_name(name)
                && !value.starts_with(['&', '*', '!', '|', '>'])
                && !value.starts_with("${{")
                && complete_value
            {
                result.event(name);
            } else {
                result.uncertain = true;
            }
        } else {
            result.uncertain = true;
        }
    }
    (result, raw.len())
}

fn triggers(contents: &str) -> Triggers {
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);
    let raw = contents.lines().collect::<Vec<_>>();
    let mut result = Triggers::default();
    let mut seen = false;
    // A root line with an inline value cannot have indented children. An
    // indented line after it means a multiline scalar or flow collection
    // that this scan cannot follow, so it could hide later events.
    let mut inline_root = false;
    let mut i = 0;
    while i < raw.len() {
        let line = raw[i];
        if !empty_trigger_line(line) {
            let (indent, bad_indent) = indentation(line);
            result.uncertain |= bad_indent;
            if indent > 0 {
                result.uncertain |= inline_root;
            }
            if indent == 0 && !matches!(line.trim(), "---" | "...") {
                inline_root = line.split_once(':').is_some_and(|(_, value)| {
                    let (value, _) = trigger_line(value.trim());
                    let value = value.trim();
                    !value.is_empty() && !value.starts_with(['|', '>'])
                });
                if let Some((key, _)) = line.split_once(':') {
                    let key = key.trim();
                    if matches!(key, "on" | "\"on\"" | "'on'") {
                        result.uncertain |= seen;
                        seen = true;
                        let (events, end) = on_events(&raw, i);
                        result.pull_request |= events.pull_request;
                        result.pull_request_target |= events.pull_request_target;
                        result.uncertain |= events.uncertain;
                        i = end;
                        continue;
                    }
                    // Escaped/complex root keys or a root merge could supply on.
                    // true is not decoded as on by this deliberately limited scan.
                    let literal = key
                        .strip_prefix('"')
                        .and_then(|key| key.strip_suffix('"'))
                        .or_else(|| {
                            key.strip_prefix('\'')
                                .and_then(|key| key.strip_suffix('\''))
                        })
                        .unwrap_or(key);
                    result.uncertain |= !plain_name(literal) || literal == "true";
                } else {
                    result.uncertain = true;
                }
            }
        }
        i += 1;
    }
    result.uncertain |= !seen;
    result.pull_request |= result.uncertain;
    result.pull_request_target |= result.uncertain;
    result
}

fn references_secrets(contents: &str) -> bool {
    // Scan raw text, including YAML comments and block scalars: expressions in
    // shell comments still undergo Actions interpolation. Match context names
    // case-insensitively; only the exact built-in token reference is exempt.
    for (offset, _) in contents.char_indices().filter(|(offset, ch)| {
        (ch.is_ascii_alphanumeric() || *ch == '_')
            && (*offset == 0
                || !contents.as_bytes()[offset - 1].is_ascii_alphanumeric()
                    && contents.as_bytes()[offset - 1] != b'_')
    }) {
        let tail = &contents[offset..];
        let token = tail
            .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
            .next()
            .unwrap_or("");
        if token.eq_ignore_ascii_case("secrets") {
            let builtin = "secrets.GITHUB_TOKEN";
            if !tail.starts_with(builtin)
                || tail[builtin.len()..]
                    .starts_with(|ch: char| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return true;
            }
        }
    }
    false
}

fn writable(section: &[Line]) -> bool {
    let value = &section[0].value;
    let joined = section
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    if tokens(&joined).any(|token| token == "write") {
        return true;
    }
    if joined.contains(['&', '*', '!']) || joined.contains("${{") {
        return true;
    }
    if value == "{}" || matches!(value.trim_matches(['\'', '"']), "read-all" | "none") {
        return false;
    }
    // A known mapping containing only read/none values is safe. Unknown scalar
    // values (including malformed YAML) should not silently suppress a warning.
    if !value.is_empty() && !(value.starts_with('{') && value.ends_with('}')) {
        return true;
    }
    let values = if value.is_empty() {
        section[1..]
            .iter()
            .map(|line| line.value.as_str())
            .collect::<Vec<_>>()
    } else {
        value
            .trim_matches(['{', '}'])
            .split(',')
            .map(|entry| {
                entry
                    .split_once(':')
                    .map(|(_, value)| value.trim())
                    .unwrap_or("")
            })
            .collect()
    };
    values.is_empty()
        || values
            .iter()
            .any(|value| !matches!(value.trim_matches(['\'', '"']), "read" | "none"))
}

pub(super) fn evaluate(workflow: &Workflow) -> Vec<Finding> {
    let lines = lines(&workflow.contents);
    let mut findings = Vec::new();
    let triggers = triggers(&workflow.contents);
    let detail = format!(
        "{}; conservative line/indent workflow scan{}",
        workflow.name,
        if triggers.uncertain {
            "; uncertain trigger set"
        } else {
            ""
        }
    );
    if triggers.pull_request_target {
        findings.push(Finding::warning(
            "pull_request_target_trigger",
            format!(
                "{} may run pull_request_target with base-branch privileges",
                workflow.name
            ),
            &detail,
        ));
    }
    if !triggers.pull_request {
        return findings;
    }
    let top = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == 0 && line.key == "permissions");
    if top.is_none_or(|(i, _)| writable(block(&lines, i))) {
        findings.push(Finding::warning(
            "pull_request_write_permissions",
            format!(
                "{} has write or inherited/uncertain pull_request permissions",
                workflow.name
            ),
            &detail,
        ));
    }
    if let Some((jobs, _)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == 0 && line.key == "jobs")
    {
        let jobs = block(&lines, jobs);
        if !jobs[0].value.is_empty() {
            findings.push(Finding::warning(
                "pull_request_write_permissions",
                format!(
                    "{} has uncertain inline or aliased jobs permissions",
                    workflow.name
                ),
                &detail,
            ));
        }
        let indent = jobs[1..].iter().map(|line| line.indent).min();
        for (i, job) in jobs
            .iter()
            .enumerate()
            .skip(1)
            .filter(|(_, line)| Some(line.indent) == indent)
        {
            let section = block(jobs, i);
            let child_indent = section[1..].iter().map(|line| line.indent).min();
            for (p, _) in
                section.iter().enumerate().skip(1).filter(|(_, line)| {
                    Some(line.indent) == child_indent && line.key == "permissions"
                })
            {
                if writable(block(section, p)) {
                    findings.push(Finding::warning(
                        "pull_request_write_permissions",
                        format!(
                            "{} job {} grants write or uncertain pull_request permissions",
                            workflow.name, job.key
                        ),
                        &detail,
                    ));
                }
            }
            // Flow-style jobs are not safely decoded by this heuristic.
            if !job.value.is_empty() || section[1..].iter().any(|line| line.key == "<<") {
                findings.push(Finding::warning(
                    "pull_request_write_permissions",
                    format!(
                        "{} job {} has uncertain inline permissions",
                        workflow.name, job.key
                    ),
                    &detail,
                ));
            }
        }
    }
    let secret = references_secrets(&workflow.contents);
    if secret {
        findings.push(Finding::warning(
            "pull_request_secrets",
            format!(
                "{} references secrets beyond secrets.GITHUB_TOKEN",
                workflow.name
            ),
            &detail,
        ));
    }
    let filtered = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == 0 && line.key == "on")
        .is_some_and(|(i, _)| {
            block(&lines, i)
                .iter()
                .any(|line| tokens(&line.text).any(|token| token == "paths"))
        });
    let conditional = lines.iter().enumerate().any(|(i, line)| {
        // Include literal/folded block scalar continuations of the if value.
        if line.key == "if" {
            block(&lines, i)
                .iter()
                .any(|part| part.text.to_ascii_lowercase().contains("draft"))
        } else {
            tokens(&line.text).any(|token| token == "if")
                && line.text.to_ascii_lowercase().contains("draft")
        }
    });
    if filtered || conditional {
        findings.push(Finding::info("required_job_conditional", format!("{} filters pull_request paths or draft jobs; a required check that is filtered or skipped never reports success; rabi's preflight treats a missing check as failure", workflow.name), &detail));
    }
    if triggers.uncertain {
        for finding in &mut findings {
            finding
                .message
                .push_str(" (trigger set could not be fully parsed)");
        }
    }
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_uncertainty_table() {
        let cases = [
            ("schedule cron", "on:\n  schedule:\n    - cron: \"0 3 * * *\"\n", false, false, false),
            ("dispatch prose", "on:\n  workflow_dispatch:\n    inputs:\n      version:\n        description: Which version (optional)? Choose ? carefully\n", false, false, false),
            ("quoted prose apostrophe", "on:\n  workflow_dispatch:\n    inputs:\n      note:\n        description: 'It''s optional * not an alias'\n", false, false, false),
            ("dispatch apostrophe", "on:\n  workflow_dispatch:\n    inputs:\n      note:\n        description: What's optional?\n", false, false, false),
            ("merge after prose apostrophe", "on:\n  workflow_dispatch:\n    inputs:\n      note:\n        description: What's optional?\n  <<: *events\n", true, true, true),
            ("quoted negative branch", "on:\n  push:\n    branches: [\"!gh-pages\"]\n", false, false, false),
            ("release trigger shape", "on:\n  workflow_dispatch:\n    inputs:\n      make_latest:\n        description: Latest choice (prereleases use false)\n        type: choice\n        options: ['true', 'false', legacy]\n  pull_request:\n    branches: [main]\npermissions: {}\n", true, false, false),
            ("repository release workflow", include_str!("../../.github/workflows/release-compile.yml"), true, false, false),
            ("escaped event", "on: \"pull\\u005frequest_target\"\n", true, true, true),
            ("escaped event key", "on:\n  \"pull\\u005frequest_target\":\n", true, true, true),
            ("continued escaped event", "on: \"push,\n  pull\\u005frequest_target\"\n", true, true, true),
            ("single quoted literal escape", "on: 'pull\\u005frequest_target'\n", true, true, true),
            ("anchor", "on: &events push\n", true, true, true),
            ("alias", "on: *events\n", true, true, true),
            ("tag", "on: !!str push\n", true, true, true),
            ("merge key", "on:\n  <<: *events\n", true, true, true),
            ("complex key", "on:\n  ? [push]\n  : {}\n", true, true, true),
            ("expression", "on: ${{ events }}\n", true, true, true),
            ("event scalar block", "on:\n  push: >-\n    folded text\n", true, true, true),
            ("wrapped double quote hides later event", "on:\n  workflow_dispatch:\n    inputs:\n      v:\n        description: \"abc\nx: y\"\n  pull_request_target:\npermissions: {}\n", true, true, true),
            ("wrapped single quote hides later event", "on:\n  workflow_dispatch:\n    inputs:\n      v:\n        description: 'abc\nx: y'\n  pull_request_target:\npermissions: {}\n", true, true, true),
            ("wrapped flow mapping hides later event", "on:\n  workflow_dispatch:\n    inputs:\n      v:\n        options: {a: 1,\n\"b\": 2}\n  pull_request_target:\npermissions: {}\n", true, true, true),
            ("root inline values then jobs", "on:\n  push:\nname: CI # comment\npermissions: {}\njobs:\n  test:\n    runs-on: ubuntu-latest\n", false, false, false),
            ("root block scalar", "on:\n  push:\nrun-name: >-\n  folded\n", false, false, false),
            ("byte order mark", "\u{feff}on:\n  push:\npermissions: {}\n", false, false, false),
            ("nested prose block", "on:\n  workflow_dispatch:\n    inputs:\n      text:\n        description: |\n          Are * and & and ! allowed (yes)?\n", false, false, false),
        ];
        for (name, contents, pr, target, uncertain) in cases {
            let actual = triggers(contents);
            assert_eq!(
                (
                    actual.pull_request,
                    actual.pull_request_target,
                    actual.uncertain
                ),
                (pr, target, uncertain),
                "{name}"
            );
            let findings = evaluate(&Workflow {
                name: "workflow.yml".into(),
                contents: contents.into(),
            });
            if !pr && !target {
                assert!(findings.is_empty(), "{name}: {findings:?}");
            }
            assert_eq!(
                findings
                    .iter()
                    .any(|finding| finding.code == "pull_request_target_trigger"),
                target,
                "{name}"
            );
            if uncertain {
                assert!(!findings.is_empty(), "{name}");
                assert!(
                    findings
                        .iter()
                        .all(|finding| finding.detail.contains("uncertain")
                            && finding
                                .message
                                .contains("trigger set could not be fully parsed")),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn plain_scalar_description_and_following_event_table() {
        let descriptions = [
            "Paths (e.g., *.md)",
            "Glob, *.md",
            "one, two, & three",
            "Hello, !world",
            "Ready, ?",
            "pick a, 'b",
            "ratio 1:'x",
            r#"opts [a, "b"#,
            r#"Don't # note, "x"#,
            r#"pick a, "b"#,
            "opts {a, 'b",
            "Build & deploy",
            "Use * for all",
            r#"say "hi\n" now"#,
            "pick 'a",
            "5 'inch",
            r#"pick "a"#,
            r#"5 "inch"#,
        ];
        let events = [
            ("", false),
            ("  \"pull\\u005frequest_target\":\n", true),
            ("  <<: *events\n", true),
            (
                "  \"pull\\u005frequest_target\":\n    branches: [it's]\n",
                true,
            ),
            (
                "  \"pull\\u005frequest_target\":\n    branches: [\"it's\"]\n",
                true,
            ),
            ("  <<: *events\n    branches: [it's]\n", true),
            ("  <<: *events\n    branches: [\"it's\"]\n", true),
        ];
        for description in descriptions {
            for (event, uncertain) in events {
                let contents = format!("on:\n  workflow_dispatch:\n    inputs:\n      choice:\n        description: {description}\n{event}permissions: {{}}\n");
                let trigger = triggers(&contents);
                assert_eq!(
                    (
                        trigger.pull_request,
                        trigger.pull_request_target,
                        trigger.uncertain
                    ),
                    (uncertain, uncertain, uncertain),
                    "{description}/{event}"
                );
                let findings = evaluate(&Workflow {
                    name: "workflow.yml".into(),
                    contents,
                });
                if uncertain {
                    assert!(
                        findings
                            .iter()
                            .any(|finding| finding.code == "pull_request_target_trigger"),
                        "{description}/{event}"
                    );
                    assert!(
                        findings
                            .iter()
                            .all(|finding| finding.detail.contains("uncertain")
                                && finding
                                    .message
                                    .contains("trigger set could not be fully parsed")),
                        "{description}/{event}"
                    );
                } else {
                    assert!(findings.is_empty(), "{description}: {findings:?}");
                }
            }
        }
    }

    #[test]
    fn nested_quoted_values_do_not_affect_triggers_table() {
        for (value, uncertain) in [
            ("'pick a", false),
            ("\"pick a", false),
            ("'pick\n          a'", false),
            ("\"pick\n          a\"", false),
        ] {
            let contents = format!("on:\n  workflow_dispatch:\n    inputs:\n      choice:\n        description: {value}\npermissions: {{}}\n");
            let trigger = triggers(&contents);
            assert_eq!(trigger.uncertain, uncertain, "{value}");
            assert_eq!(trigger.pull_request_target, uncertain, "{value}");
        }
    }

    #[test]
    fn event_structure_table() {
        let cases = [
            ("plain scalar", "on: pull_request\n", true, false, false),
            ("plain flow names", "on: [push, pull_request] # pull_request_target\n", true, false, false),
            ("quoted on key", "\"on\":\n  pull_request:\n", true, false, false),
            ("single quoted on key", "'on':\n  pull_request:\n", true, false, false),
            ("event comments", "on: # comment\n  # first comment\n  push: # pull_request_target\n", false, false, false),
            ("block sequence", "on:\n  - push\n  - pull_request\n", true, false, false),
            ("indentless sequence", "on:\n- push\n- pull_request\npermissions: {}\n", true, false, false),
            ("event options inline", "on:\n  push: {branches: [\"main\", \"it's\"], paths: ['src/**']}\n", false, false, false),
            ("event quoted value", "on:\n  push: \"a # literal\" # pull_request_target\n", false, false, false),
            ("nested indirection is configuration", "on:\n  workflow_dispatch:\n    inputs: *input_options\n    description: ${{ text }}\n", false, false, false),
            ("block ends at root", "on:\n  push:\npermissions: {}\njobs:\n  pull_request_target:\n", false, false, false),
            ("event flow sequence closes", "on:\n  push: [branches, paths]\n", false, false, false),
            ("event flow sequence opens", "on:\n  push: [branches,\n  pull_request_target]\n", true, true, true),
            ("event flow mapping opens", "on:\n  push: {branches: [main],\n  \"pull\\u005frequest_target\": {}}\n", true, true, true),
            ("event scalar opens", "on:\n  push: \"text\n  \"pull\\u005frequest_target\":\n", true, true, true),
            ("event merge", "on:\n  <<: *events\n", true, true, true),
            ("event anchor key", "on:\n  &anchor\n", true, true, true),
            ("event alias key", "on:\n  *alias\n", true, true, true),
            ("event tag key", "on:\n  !tag push:\n", true, true, true),
            ("event complex key", "on:\n  ? pull_request_target\n", true, true, true),
            ("event alias value", "on:\n  push: *options\n", true, true, true),
            ("event anchor value", "on:\n  push: &options {}\n", true, true, true),
            ("event tag value", "on:\n  push: !options {}\n", true, true, true),
            ("event expression value", "on:\n  push: ${{ options }}\n", true, true, true),
            ("inline mapping", "on: {push: {}}\n", true, true, true),
            ("inline sequence opens", "on: [push,\n  pull_request]\n", true, true, true),
            ("header literal scalar", "on: |\n  push\n", true, true, true),
            ("header folded scalar", "on: >-\n  push\n", true, true, true),
            ("event literal scalar", "on:\n  push: |\n    text\n", true, true, true),
            ("partial dedent", "on:\n    push:\n  pull_request_target:\n", true, true, true),
            ("tab event", "on:\n\tpull_request_target:\n", true, true, true),
            ("tab configuration", "on:\n  workflow_dispatch:\n    \tdescription: text\n", true, true, true),
            ("tab header", "\ton: push\n", true, true, true),
            ("escaped root on", "\"o\\u006e\": push\n", true, true, true),
            ("root merge", "on: push\n<<: *root\n", true, true, true),
            ("boolean root key unsupported", "true: push\n", true, true, true),
            ("duplicate on", "on: push\non: pull_request\n", true, true, true),
            ("empty on", "on:\npermissions: {}\n", true, true, true),
        ];
        for (name, contents, pr, target, uncertain) in cases {
            let actual = triggers(contents);
            assert_eq!(
                (
                    actual.pull_request,
                    actual.pull_request_target,
                    actual.uncertain
                ),
                (pr, target, uncertain),
                "{name}"
            );
        }
    }

    #[test]
    fn workflow_table() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("next-line scalar", "on:\n  pull_request\n", &["pull_request_target_trigger", "pull_request_write_permissions"]),
            ("next-line list", "on:\n  [push, pull_request_target]\n", &["pull_request_target_trigger", "pull_request_write_permissions"]),
            ("continued list", "on: [push,\n  pull_request]\n", &["pull_request_target_trigger", "pull_request_write_permissions"]),
            ("inherited secrets", "on: pull_request\npermissions: {}\njobs:\n  call:\n    secrets: inherit\n", &["pull_request_secrets"]),
            ("whole secrets context", "on: pull_request\npermissions: {}\nenv:\n  DATA: ${{ toJSON(secrets) }}\n", &["pull_request_secrets"]),
            ("uppercase secrets", "on: pull_request\npermissions: {}\nenv:\n  DATA: ${{ SECRETS.DEPLOY }}\n", &["pull_request_secrets"]),
            ("block scalar shell comment", "on: pull_request\npermissions: {}\njobs:\n  check:\n    steps:\n      - run: |\n          # ${{ secrets.DEPLOY }}\n", &["pull_request_secrets"]),
            ("folded draft condition", "on: pull_request\npermissions: {}\njobs:\n  check:\n    if: >-\n      github.event.pull_request.draft == false\n", &["required_job_conditional"]),
            ("literal draft condition", "on: pull_request\npermissions: {}\njobs:\n  check:\n    if: |\n      github.event.pull_request.draft == false\n", &["required_job_conditional"]),
            ("unknown trigger line", "on:\n  ??? unknown\n", &["pull_request_target_trigger", "pull_request_write_permissions"]),
            ("nested event token", "on:\n  push:\n    branches: [pull_request_target]\npermissions: {}\n", &[]),
            ("raw commented secret", "on: pull_request\npermissions: {}\n# ${{ secrets.DEPLOY }}\n", &["pull_request_secrets"]),
            ("not a secrets token", "on: pull_request\npermissions: {}\nenv:\n  DATA: ${{ notsecrets.DEPLOY }}\n", &[]),
            ("unicode before secret", "on: pull_request\npermissions: {}\n# λ ${{ secrets.DEPLOY }}\n", &["pull_request_secrets"]),
            ("literal hash in condition", "on: pull_request\npermissions: {}\njobs:\n  check:\n    if: |-\n      # draft\n", &["required_job_conditional"]),
            ("folded ordinary condition", "on: pull_request\npermissions: {}\njobs:\n  check:\n    if: >-\n      always()\n", &[]),
            ("inline jobs", "on: pull_request\npermissions: {}\njobs: {check: {permissions: write-all}}\n", &["pull_request_write_permissions"]),
            ("merged job", "on: pull_request\npermissions: {}\njobs:\n  check:\n    <<: *shared\n", &["pull_request_write_permissions"]),
            ("push", "on: push\npermissions: read-all\njobs:\n  test:\n    runs-on: ubuntu-latest\n", &[]),
            ("read mapping", "on: pull_request\npermissions:\n  contents: read\n", &[]),
            ("inline read", "on: [push, pull_request]\npermissions: {contents: read}\n", &[]),
            ("write all", "on: pull_request\npermissions: write-all\n", &["pull_request_write_permissions"]),
            ("job write", "on: pull_request\npermissions: {}\njobs:\n  test:\n    permissions:\n      contents: write\n", &["pull_request_write_permissions"]),
            ("job read", "on: pull_request\npermissions: {}\njobs:\n  test:\n    permissions:\n      contents: read\n", &[]),
            ("inherited", "on: [push, pull_request]\njobs:\n  test:\n    permissions: read-all\n", &["pull_request_write_permissions"]),
            ("target scalar", "on: pull_request_target\n", &["pull_request_target_trigger"]),
            ("map quoted keys", "'on':\n  'pull_request':\n    types: [opened]\n'permissions':\n  'contents': 'read'\n", &["pull_request_target_trigger"]),
            ("target map", "on:\n  pull_request_target:\n    types: [opened]\n", &["pull_request_target_trigger"]),
            ("comments", "on: push # pull_request_target\n# on: pull_request\n# secrets.PRIVATE\npermissions: read-all\n", &[]),
            ("commented write", "on: pull_request\npermissions:\n  contents: read # write\n", &[]),
            ("only github token", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: ${{ secrets.GITHUB_TOKEN }}\n", &[]),
            ("secret", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: '${{ secrets.DEPLOY_KEY }}'\n", &["pull_request_secrets"]),
            ("secret token prefix", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: ${{ secrets.GITHUB_TOKEN_EXTRA }}\n", &["pull_request_secrets"]),
            ("quoted hash", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: '# ${{ secrets.PRIVATE }}'\n", &["pull_request_secrets"]),
            ("paths ignore", "on:\n  pull_request:\n    paths-ignore:\n      - docs/**\npermissions: {}\n", &["required_job_conditional"]),
            ("paths", "on: {pull_request: {paths: ['src/**']}}\npermissions: {}\n", &["pull_request_target_trigger", "required_job_conditional"]),
            ("draft false", "on: pull_request\npermissions: {}\njobs:\n  test:\n    if: github.event.pull_request.draft == false\n", &["required_job_conditional"]),
            ("ordinary if", "on: pull_request\npermissions: {}\njobs:\n  test:\n    if: always()\n", &[]),
            ("aliased permissions", "on: pull_request\npermissions: *shared\n", &["pull_request_write_permissions"]),
            ("uncertain triggers", "on: *events\npermissions: {}\n", &["pull_request_target_trigger"]),
        ];
        for (name, contents, expected) in cases {
            let file = Workflow {
                name: "workflow.yml".into(),
                contents: (*contents).into(),
            };
            let findings = evaluate(&file);
            let actual = findings
                .iter()
                .map(|finding| finding.code)
                .collect::<Vec<_>>();
            assert_eq!(&actual, expected, "{name}");
            if *name == "unknown trigger line" || *name == "uncertain triggers" {
                assert!(findings
                    .iter()
                    .all(|finding| finding.detail.contains("uncertain")));
            }
            if *name == "job write" {
                assert!(findings[0].message.contains("job test"));
            }
        }
    }
}
