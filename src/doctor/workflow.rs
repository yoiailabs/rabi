//! Deliberately conservative YAML heuristic, not a YAML parser. We recognize
//! indentation, quoted keys, scalar/list/map triggers and inline permissions.
//! Comments outside quotes are removed. Aliases, tags, expressions and other
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
    contents
        .lines()
        .filter_map(|raw| {
            let text = uncomment(raw);
            let trimmed = text.trim();
            if trimmed.is_empty() || matches!(trimmed, "---" | "...") {
                return None;
            }
            let (key, value) = trimmed.split_once(':').unwrap_or(("", ""));
            Some(Line {
                indent: raw.len() - raw.trim_start().len(),
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

fn event(lines: &[Line], name: &str) -> bool {
    let Some((i, on)) = lines
        .iter()
        .enumerate()
        .find(|(_, line)| line.indent == 0 && line.key == "on")
    else {
        // A missing/unrecognized trigger cannot establish that PR execution is safe.
        return true;
    };
    let section = block(lines, i);
    if tokens(&on.value).any(|token| token == name)
        || section[1..].iter().any(|line| {
            line.key == name
                || (line.text.starts_with('-') && tokens(&line.text).any(|token| token == name))
        })
    {
        return true;
    }
    // YAML anchors/tags/expressions may hide a PR trigger.
    let uncertain = |value: &str| value.starts_with(['&', '*', '!']) || value.contains("${{");
    uncertain(&on.value)
        || (on.value.is_empty() && section.len() == 1)
        || section[1..].iter().any(|line| line.key == "<<")
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
    let detail = format!("{}; conservative line/indent workflow scan", workflow.name);
    if event(&lines, "pull_request_target") {
        findings.push(Finding::warning(
            "pull_request_target_trigger",
            format!(
                "{} may run pull_request_target with base-branch privileges",
                workflow.name
            ),
            &detail,
        ));
    }
    if !event(&lines, "pull_request") {
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
    let secret = lines.iter().any(|line| {
        line.text.split("secrets.").skip(1).any(|rest| {
            let name = rest
                .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
                .next()
                .unwrap_or("");
            name != "GITHUB_TOKEN"
        }) || line.text.contains("secrets[")
    });
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
    let conditional = lines
        .iter()
        .any(|line| tokens(&line.text).any(|token| token == "if") && line.text.contains("draft"));
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
            if *name == "job write" {
                assert!(findings[0].message.contains("job test"));
            }
        }
    }
}
