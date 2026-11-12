//! Deliberately conservative YAML heuristic, not a YAML parser. We recognize
//! indentation, quoted keys, scalar/list/map triggers and inline permissions.
//! Comments outside quotes/scalar blocks are removed. Aliases, tags, expressions and other
//! uncertain trigger/permission forms are treated as potentially unsafe.
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

struct Triggers {
    pull_request: bool,
    pull_request_target: bool,
    uncertain: bool,
}

// Recognize a small structural subset, never infer safety from an unknown
// line. Event tokens are collected from every line, including continuations.
fn known_trigger_line(line: &Line, first: bool) -> bool {
    let mapping = first || !line.key.is_empty();
    if !first
        && mapping
        && !line
            .key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return false;
    }
    let value = if mapping {
        line.value.as_str()
    } else {
        line.text.as_str()
    };
    if value.contains("${{")
        || value.contains("<<")
        || value
            .split([' ', ':', '{', '[', ','])
            .any(|part| part.starts_with(['*', '&', '!']))
    {
        return false;
    }
    if value.is_empty() {
        return mapping;
    }
    if value.starts_with(['|', '>']) {
        return false;
    }
    // Ordinary event configuration mappings and flow maps are understood only
    // lexically. Unexpected punctuation remains uncertain.
    if mapping && (!first || value.starts_with('{')) || value.starts_with("- ") {
        return value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || ch.is_whitespace() || "_-.:/[]{}*,\"'".contains(ch)
        });
    }
    let value = value.strip_prefix("- ").unwrap_or(value);
    value.trim_matches(['[', ']', ',']).split(',').all(|part| {
        let part = part.trim().trim_matches(['\'', '"']);
        !part.is_empty()
            && part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    })
}

fn triggers(lines: &[Line]) -> Triggers {
    let Some((i, _)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == 0 && line.key == "on")
    else {
        return Triggers {
            pull_request: true,
            pull_request_target: true,
            uncertain: true,
        };
    };
    let section = block(lines, i);
    let contains = |name| {
        section
            .iter()
            .any(|line| tokens(&line.text).any(|token| token == name))
    };
    let uncertain = (section.len() == 1 && section[0].value.is_empty())
        || section
            .iter()
            .enumerate()
            .any(|(i, line)| !known_trigger_line(line, i == 0));
    Triggers {
        pull_request: contains("pull_request") || uncertain,
        pull_request_target: contains("pull_request_target") || uncertain,
        uncertain,
    }
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
    let triggers = triggers(&lines);
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
    findings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_table() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("next-line scalar", "on:\n  pull_request\n", &["pull_request_write_permissions"]),
            ("next-line list", "on:\n  [push, pull_request_target]\n", &["pull_request_target_trigger"]),
            ("continued list", "on: [push,\n  pull_request]\n", &["pull_request_write_permissions"]),
            ("inherited secrets", "on: pull_request\npermissions: {}\njobs:\n  call:\n    secrets: inherit\n", &["pull_request_secrets"]),
            ("whole secrets context", "on: pull_request\npermissions: {}\nenv:\n  DATA: ${{ toJSON(secrets) }}\n", &["pull_request_secrets"]),
            ("uppercase secrets", "on: pull_request\npermissions: {}\nenv:\n  DATA: ${{ SECRETS.DEPLOY }}\n", &["pull_request_secrets"]),
            ("block scalar shell comment", "on: pull_request\npermissions: {}\njobs:\n  check:\n    steps:\n      - run: |\n          # ${{ secrets.DEPLOY }}\n", &["pull_request_secrets"]),
            ("folded draft condition", "on: pull_request\npermissions: {}\njobs:\n  check:\n    if: >-\n      github.event.pull_request.draft == false\n", &["required_job_conditional"]),
            ("literal draft condition", "on: pull_request\npermissions: {}\njobs:\n  check:\n    if: |\n      github.event.pull_request.draft == false\n", &["required_job_conditional"]),
            ("unknown trigger line", "on:\n  ??? unknown\n", &["pull_request_target_trigger", "pull_request_write_permissions"]),
            ("nested event token", "on:\n  push:\n    branches: [pull_request_target]\npermissions: {}\n", &["pull_request_target_trigger"]),
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
            ("map quoted keys", "'on':\n  'pull_request':\n    types: [opened]\n'permissions':\n  'contents': 'read'\n", &[]),
            ("target map", "on:\n  pull_request_target:\n    types: [opened]\n", &["pull_request_target_trigger"]),
            ("comments", "on: push # pull_request_target\n# on: pull_request\n# secrets.PRIVATE\npermissions: read-all\n", &[]),
            ("commented write", "on: pull_request\npermissions:\n  contents: read # write\n", &[]),
            ("only github token", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: ${{ secrets.GITHUB_TOKEN }}\n", &[]),
            ("secret", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: '${{ secrets.DEPLOY_KEY }}'\n", &["pull_request_secrets"]),
            ("secret token prefix", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: ${{ secrets.GITHUB_TOKEN_EXTRA }}\n", &["pull_request_secrets"]),
            ("quoted hash", "on: pull_request\npermissions: {}\nenv:\n  TOKEN: '# ${{ secrets.PRIVATE }}'\n", &["pull_request_secrets"]),
            ("paths ignore", "on:\n  pull_request:\n    paths-ignore:\n      - docs/**\npermissions: {}\n", &["required_job_conditional"]),
            ("paths", "on: {pull_request: {paths: ['src/**']}}\npermissions: {}\n", &["required_job_conditional"]),
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
