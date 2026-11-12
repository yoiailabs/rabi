//! Pure classification of committed diff entries. No filesystem or Git access.
use serde::Serialize;

use super::policy::{glob_matches, Policy, Tier};

#[derive(Debug, Clone)]
pub(crate) struct ChangedEntry {
    pub path: Vec<u8>,
    pub old_path: Option<Vec<u8>>,
    pub status: String,
    pub old_mode: u32,
    pub new_mode: u32,
    pub added: u64,
    pub deleted: u64,
    pub binary: bool,
    pub unknown: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Classification {
    pub tier: Tier,
    pub floor: bool,
    pub reasons: Vec<String>,
    pub files: Vec<FileRisk>,
    pub totals: Totals,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileRisk {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub old_path: Option<String>,
    pub status: String,
    pub tier: Tier,
    pub floor: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Totals {
    pub files: usize,
    pub changed_lines: u64,
}

fn add_reason(reasons: &mut Vec<String>, reason: impl Into<String>) {
    let reason = reason.into();
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

fn raise(file: &mut FileRisk, tier: Tier, reason: impl Into<String>) {
    file.tier = file.tier.max(tier);
    add_reason(&mut file.reasons, reason);
}

fn classify_path(policy: &Policy, path: &[u8], file: &mut FileRisk) {
    // These ASCII built-ins remain identifiable even when the rest of a path
    // cannot be decoded. Unknown encodings must not remove a built-in floor.
    let builtin = match path {
        b".rabi/policy.toml" => Some(".rabi/policy.toml"),
        b".rabi.toml" => Some(".rabi.toml"),
        _ if path.starts_with(b".github/") => Some(".github/**"),
        _ => None,
    };
    let Ok(path) = std::str::from_utf8(path) else {
        raise(file, Tier::High, "unknown:path-encoding");
        if let Some(pattern) = builtin {
            file.floor = true;
            raise(file, Tier::High, format!("floor:builtin:{pattern}"));
        }
        return;
    };
    let mut matched = false;
    for rule in &policy.rules {
        for pattern in &rule.paths {
            if glob_matches(pattern, path) {
                matched = true;
                if rule.floor {
                    file.floor = true;
                    raise(file, Tier::High, format!("floor:rule:{pattern}"));
                } else if let Some(tier) = rule.tier {
                    raise(file, tier, format!("rule:{pattern}"));
                }
            }
        }
    }
    if !matched {
        raise(file, policy.default_tier, "default-tier");
    }
    if let Some(pattern) = builtin {
        file.floor = true;
        raise(file, Tier::High, format!("floor:builtin:{pattern}"));
    }
}

fn known_status(status: &str) -> bool {
    match status {
        "A" | "D" | "M" | "T" => true,
        _ => status
            .strip_prefix('R')
            .or_else(|| status.strip_prefix('C'))
            .is_some_and(|score| {
                score.is_empty() || score.parse::<u8>().is_ok_and(|value| value <= 100)
            }),
    }
}

pub(crate) fn classify(policy: &Policy, entries: &[ChangedEntry]) -> Classification {
    let mut result = Classification {
        tier: Tier::Low,
        floor: false,
        reasons: Vec::new(),
        files: Vec::with_capacity(entries.len()),
        totals: Totals {
            files: entries.len(),
            changed_lines: 0,
        },
    };
    for entry in entries {
        let mut file = FileRisk {
            path: String::from_utf8_lossy(&entry.path).into_owned(),
            old_path: entry
                .old_path
                .as_deref()
                .map(|path| String::from_utf8_lossy(path).into_owned()),
            status: entry.status.clone(),
            tier: Tier::Low,
            floor: false,
            reasons: Vec::new(),
        };
        if let Some(path) = &entry.old_path {
            classify_path(policy, path, &mut file);
        }
        classify_path(policy, &entry.path, &mut file);
        if !known_status(&entry.status) {
            raise(&mut file, Tier::High, "unknown:status");
        }
        for reason in &entry.unknown {
            raise(&mut file, Tier::High, reason.clone());
        }
        for (applies, tier, reason) in [
            (entry.binary, policy.kinds.binary, "kind:binary"),
            (
                entry.old_mode == 0o120000 || entry.new_mode == 0o120000,
                policy.kinds.symlink,
                "kind:symlink",
            ),
            (
                entry.old_mode == 0o160000 || entry.new_mode == 0o160000,
                policy.kinds.submodule,
                "kind:submodule",
            ),
            (
                // Mode zero for additions/deletions is also a mode transition.
                entry.old_mode != entry.new_mode
                    && (entry.old_mode == 0o100755 || entry.new_mode == 0o100755),
                policy.kinds.exec_bit,
                "kind:exec-bit",
            ),
        ] {
            if applies {
                raise(&mut file, tier, reason);
            }
        }
        if !entry.binary {
            match entry
                .added
                .checked_add(entry.deleted)
                .and_then(|lines| result.totals.changed_lines.checked_add(lines))
            {
                Some(total) => result.totals.changed_lines = total,
                None => {
                    result.totals.changed_lines = u64::MAX;
                    raise(&mut file, Tier::High, "unknown:line-count");
                }
            }
        }
        result.tier = result.tier.max(file.tier);
        result.floor |= file.floor;
        for reason in &file.reasons {
            add_reason(&mut result.reasons, reason.clone());
        }
        result.files.push(file);
    }
    for (exceeded, reason) in [
        (
            policy
                .limits
                .max_changed_lines
                .is_some_and(|limit| result.totals.changed_lines > limit),
            "limit:max-changed-lines",
        ),
        (
            policy.limits.max_files.is_some_and(|limit| {
                u64::try_from(entries.len()).map_or(true, |files| files > limit)
            }),
            "limit:max-files",
        ),
    ] {
        if exceeded {
            result.tier = Tier::High;
            add_reason(&mut result.reasons, reason);
        }
    }
    if entries.is_empty() {
        add_reason(&mut result.reasons, "no changes");
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> ChangedEntry {
        ChangedEntry {
            path: path.as_bytes().to_vec(),
            old_path: None,
            status: "M".into(),
            old_mode: 0o100644,
            new_mode: 0o100644,
            added: 1,
            deleted: 1,
            binary: false,
            unknown: Vec::new(),
        }
    }

    struct Case {
        name: &'static str,
        policy: &'static str,
        entries: Vec<ChangedEntry>,
        tier: Tier,
        floor: bool,
        files: Vec<(Tier, bool, Vec<&'static str>)>,
        reasons: Vec<&'static str>,
    }

    #[test]
    fn classification_table() {
        use Tier::{High, Low, Medium};
        let low = "schema = 1\ndefault-tier = 'low'";
        let docs = "schema = 1\n[[rule]]\npaths = ['docs/**']\ntier = 'low'";
        let mut renamed = entry("docs/new");
        renamed.status = "R100".into();
        renamed.old_path = Some(b"secret/old".to_vec());
        let mut copied = renamed.clone();
        copied.status = "C100".into();
        let mut deleted = entry("docs/deleted");
        deleted.status = "D".into();
        deleted.new_mode = 0;
        let mut binary = entry("image");
        binary.binary = true;
        let mut symlink = entry("link");
        symlink.old_mode = 0o120000;
        let mut submodule = entry("module");
        submodule.new_mode = 0o160000;
        let mut executable = entry("script");
        executable.status = "A".into();
        executable.old_mode = 0;
        executable.new_mode = 0o100755;
        let mut chmod = executable.clone();
        chmod.status = "M".into();
        chmod.old_mode = 0o100644;
        let mut noexec = chmod.clone();
        noexec.old_mode = 0o100755;
        noexec.new_mode = 0o100644;
        let mut unknown = entry("docs/unknown");
        unknown.status = "X".into();
        let mut non_utf8 = entry("docs/a");
        non_utf8.path.push(0xff);
        let mut non_utf8_floor = entry(".github/a");
        non_utf8_floor.path.push(0xff);
        let mut raw_unknown = entry("docs/raw");
        raw_unknown.unknown.push("unknown:raw".into());
        let mut cases = vec![
            Case { name: "defaults", policy: "schema = 1", entries: vec![entry("docs/a")], tier: High, floor: false, files: vec![(High, false, vec!["default-tier"])], reasons: vec!["default-tier"] },
            Case { name: "default override", policy: low, entries: vec![entry("a")], tier: Low, floor: false, files: vec![(Low, false, vec!["default-tier"])], reasons: vec!["default-tier"] },
            Case { name: "multiple matching rules", policy: "schema = 1\n[[rule]]\npaths = ['docs/**']\ntier = 'low'\n[[rule]]\npaths = ['docs/a']\ntier = 'medium'", entries: vec![entry("docs/a")], tier: Medium, floor: false, files: vec![(Medium, false, vec!["rule:docs/**", "rule:docs/a"])], reasons: vec!["rule:docs/**", "rule:docs/a"] },
            Case { name: "floor", policy: "schema = 1\n[[rule]]\npaths = ['script']\nfloor = true", entries: vec![entry("script")], tier: High, floor: true, files: vec![(High, true, vec!["floor:rule:script"])], reasons: vec!["floor:rule:script"] },
            Case { name: "rename old side", policy: docs, entries: vec![renamed], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "rule:docs/**"])], reasons: vec!["default-tier", "rule:docs/**"] },
            Case { name: "copy old side", policy: docs, entries: vec![copied], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "rule:docs/**"])], reasons: vec!["default-tier", "rule:docs/**"] },
            Case { name: "delete", policy: docs, entries: vec![deleted], tier: Low, floor: false, files: vec![(Low, false, vec!["rule:docs/**"])], reasons: vec!["rule:docs/**"] },
            Case { name: "binary", policy: low, entries: vec![binary], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "kind:binary"])], reasons: vec!["default-tier", "kind:binary"] },
            Case { name: "symlink", policy: low, entries: vec![symlink], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "kind:symlink"])], reasons: vec!["default-tier", "kind:symlink"] },
            Case { name: "submodule", policy: low, entries: vec![submodule], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "kind:submodule"])], reasons: vec!["default-tier", "kind:submodule"] },
            Case { name: "exec add", policy: low, entries: vec![executable], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "kind:exec-bit"])], reasons: vec!["default-tier", "kind:exec-bit"] },
            Case { name: "exec mode change", policy: low, entries: vec![chmod], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "kind:exec-bit"])], reasons: vec!["default-tier", "kind:exec-bit"] },
            Case { name: "exec removed", policy: low, entries: vec![noexec], tier: High, floor: false, files: vec![(High, false, vec!["default-tier", "kind:exec-bit"])], reasons: vec!["default-tier", "kind:exec-bit"] },
            Case { name: "empty", policy: "schema = 1", entries: vec![], tier: Low, floor: false, files: vec![], reasons: vec!["no changes"] },
            Case { name: "unknown status", policy: docs, entries: vec![unknown], tier: High, floor: false, files: vec![(High, false, vec!["rule:docs/**", "unknown:status"])], reasons: vec!["rule:docs/**", "unknown:status"] },
            Case { name: "unknown encoding", policy: low, entries: vec![non_utf8], tier: High, floor: false, files: vec![(High, false, vec!["unknown:path-encoding"])], reasons: vec!["unknown:path-encoding"] },
            Case { name: "unknown raw", policy: docs, entries: vec![raw_unknown], tier: High, floor: false, files: vec![(High, false, vec!["rule:docs/**", "unknown:raw"])], reasons: vec!["rule:docs/**", "unknown:raw"] },
        ];
        cases.push(Case {
            name: "unknown encoding retains builtin floor",
            policy: low,
            entries: vec![non_utf8_floor],
            tier: High,
            floor: true,
            files: vec![(
                High,
                true,
                vec!["unknown:path-encoding", "floor:builtin:.github/**"],
            )],
            reasons: vec!["unknown:path-encoding", "floor:builtin:.github/**"],
        });
        for (path, reason) in [
            (".rabi/policy.toml", "floor:builtin:.rabi/policy.toml"),
            (".rabi.toml", "floor:builtin:.rabi.toml"),
            (".github/workflows/test.yml", "floor:builtin:.github/**"),
        ] {
            cases.push(Case {
                name: "builtin floor",
                policy: "schema = 1\n[[rule]]\npaths = ['**']\ntier = 'low'",
                entries: vec![entry(path)],
                tier: High,
                floor: true,
                files: vec![(High, true, vec!["rule:**", reason])],
                reasons: vec!["rule:**", reason],
            });
        }
        for case in cases {
            let policy = Policy::parse(case.policy).expect(case.name);
            let result = classify(&policy, &case.entries);
            assert_eq!(
                (result.tier, result.floor),
                (case.tier, case.floor),
                "{}",
                case.name
            );
            assert_eq!(result.reasons, case.reasons, "{}", case.name);
            assert_eq!(result.files.len(), case.files.len(), "{}", case.name);
            for (actual, expected) in result.files.iter().zip(case.files) {
                assert_eq!(
                    (
                        actual.tier,
                        actual.floor,
                        actual
                            .reasons
                            .iter()
                            .map(String::as_str)
                            .collect::<Vec<_>>()
                    ),
                    expected,
                    "{}",
                    case.name
                );
            }
        }
    }

    #[test]
    fn limits_table() {
        for (lines, files, tier, reasons) in [
            (2, 1, Tier::Low, vec!["default-tier"]),
            (
                1,
                1,
                Tier::High,
                vec!["default-tier", "limit:max-changed-lines"],
            ),
            (4, 1, Tier::High, vec!["default-tier", "limit:max-files"]),
            (
                3,
                1,
                Tier::High,
                vec!["default-tier", "limit:max-changed-lines", "limit:max-files"],
            ),
        ] {
            let entries = if lines > 2 {
                vec![entry("a"), entry("b")]
            } else {
                vec![entry("a")]
            };
            let policy = Policy::parse(&format!("schema = 1\ndefault-tier = 'low'\n[limits]\nmax-changed-lines = {lines}\nmax-files = {files}")).unwrap();
            let result = classify(&policy, &entries);
            assert_eq!(result.tier, tier);
            assert_eq!(result.reasons, reasons);
            assert!(!result.floor);
            assert_eq!(result.totals.files, entries.len());
            assert_eq!(result.totals.changed_lines, entries.len() as u64 * 2);
            assert!(result.files.iter().all(|file| file.tier == Tier::Low));
        }
    }

    #[test]
    fn rename_and_copy_keep_the_old_paths_floor() {
        let policy = Policy::parse("schema = 1\ndefault-tier = 'low'").unwrap();
        for status in ["R100", "C100"] {
            let mut changed = entry("docs/new");
            changed.status = status.into();
            changed.old_path = Some(b".rabi/policy.toml".to_vec());
            let result = classify(&policy, &[changed]);
            assert_eq!(result.tier, Tier::High);
            assert!(result.floor);
            assert!(result.files[0].floor);
            assert_eq!(
                result.files[0].reasons,
                ["default-tier", "floor:builtin:.rabi/policy.toml"]
            );
        }
    }

    #[test]
    fn deleting_an_executable_keeps_its_kind_tier() {
        let policy = Policy::parse("schema = 1\ndefault-tier = 'low'").unwrap();
        let mut changed = entry("script");
        changed.status = "D".into();
        changed.old_mode = 0o100755;
        changed.new_mode = 0;
        let result = classify(&policy, &[changed]);
        assert_eq!(result.tier, Tier::High);
        assert_eq!(result.files[0].reasons, ["default-tier", "kind:exec-bit"]);
    }

    #[test]
    fn configured_kind_tiers_and_binary_totals() {
        let policy = Policy::parse("schema = 1\ndefault-tier = 'low'\n[kinds]\nbinary = 'medium'\nsymlink = 'low'\nsubmodule = 'medium'\nexec-bit = 'low'").unwrap();
        for (old_mode, new_mode, binary, tier, reason) in [
            (0o100644, 0o100644, true, Tier::Medium, "kind:binary"),
            (0o120000, 0o100644, false, Tier::Low, "kind:symlink"),
            (0o100644, 0o160000, false, Tier::Medium, "kind:submodule"),
            (0o100644, 0o100755, false, Tier::Low, "kind:exec-bit"),
        ] {
            let mut changed = entry("a");
            changed.old_mode = old_mode;
            changed.new_mode = new_mode;
            changed.binary = binary;
            let result = classify(&policy, &[changed]);
            assert_eq!(result.tier, tier);
            assert_eq!(result.files[0].reasons, ["default-tier", reason]);
            assert_eq!(result.totals.changed_lines, if binary { 0 } else { 2 });
        }
    }
}
