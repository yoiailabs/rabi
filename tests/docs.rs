const README: &str = include_str!("../README.md");
const README_JA: &str = include_str!("../README.ja.md");

#[test]
fn user_skill_has_matching_name_frontmatter() {
    let skill = include_str!("../skills/rabi/SKILL.md");
    let mut lines = skill.lines();
    assert_eq!(lines.next(), Some("---"));
    let frontmatter: Vec<_> = lines.by_ref().take_while(|line| *line != "---").collect();
    assert!(frontmatter.contains(&"name: rabi"));
    assert!(lines.next().is_some(), "Skill body is missing");
}

#[test]
fn product_and_package_are_named_rabi() {
    for document in [README, README_JA] {
        assert_eq!(document.lines().next(), Some("# rabi"));
    }
    assert_eq!(env!("CARGO_PKG_NAME"), "rabi");
}

fn fenced_blocks(document: &str) -> Vec<Vec<&str>> {
    let mut blocks = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in document.lines() {
        if line.starts_with("```") {
            if let Some(mut block) = current.take() {
                block.push(line);
                blocks.push(block);
            } else {
                current = Some(vec![line]);
            }
        } else if let Some(block) = &mut current {
            block.push(line);
        }
    }
    assert!(current.is_none(), "Unclosed code fence");
    blocks
}

#[test]
fn japanese_readme_does_not_copy_english_prose() {
    let english_lines: std::collections::HashSet<_> = README.lines().map(str::trim).collect();
    let mut in_code = false;
    for (index, line) in README_JA.lines().enumerate() {
        let line = line.trim();
        if line.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        // Headings, Markdown markers, and short labels may legitimately match.
        // Include command descriptions in tables as well as ordinary paragraphs.
        if in_code || line.starts_with('#') || line.split_whitespace().count() < 4 {
            continue;
        }
        assert!(
            !english_lines.contains(line),
            "README.ja.md:{} contains untranslated English prose: {line}",
            index + 1
        );
    }
}

#[test]
fn readmes_cover_the_help_contract_and_share_examples() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    for command in help["helps"].as_object().unwrap().keys() {
        let entry = format!("`rabi {command}`");
        for (name, document) in [("README.md", README), ("README.ja.md", README_JA)] {
            assert!(document.contains(&entry), "{name} is missing {entry}");
        }
    }
    assert_eq!(fenced_blocks(README), fenced_blocks(README_JA));
}

#[test]
fn user_documentation_has_no_temporary_phase_promises() {
    for (name, document) in [
        ("README.md", README),
        ("README.ja.md", README_JA),
        ("help-data.json", include_str!("../help-data.json")),
        ("AGENTS.md", include_str!("../AGENTS.md")),
        ("CHANGELOG.md", include_str!("../CHANGELOG.md")),
    ] {
        for marker in ["pending P", "until P"] {
            assert!(!document.contains(marker), "{name} still contains {marker}");
        }
    }
}

#[test]
fn parent_session_guidance_matches_the_simplified_run_contract() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    for name in [
        "skill",
        "skill list",
        "workflow",
        "workflow list",
        "workflow show",
        "workflow check",
        "run start",
        "run show",
    ] {
        assert!(help["helps"].get(name).is_none());
    }
    for document in [README, README_JA, include_str!("../docs/commands.md")] {
        for flag in ["--skill", "--fresh", "--timeout", "--provider"] {
            assert!(document.contains(flag));
        }
        assert!(!document.contains("rabi skill list"));
        for old in [
            "rabi workflow",
            "rabi run start",
            "rabi run show",
            "docs/workflows.md",
        ] {
            assert!(!document.contains(old));
        }
    }
    for code in ["124", "130"] {
        assert!(help["exitCodes"][code].is_string());
    }
}

#[test]
fn task_implement_guidance_documents_claude_auto_mode_and_denied_prompts() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let notes = help["helps"]["task implement"]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for (name, document) in [
        ("task implement help", notes.as_str()),
        ("docs/commands.md", include_str!("../docs/commands.md")),
    ] {
        for setting in ["--permission-mode auto", "--permission-prompts none"] {
            assert!(document.contains(setting), "{name} is missing {setting}");
        }
        assert!(
            !document.contains("acceptEdits"),
            "{name} has stale permissions"
        );
    }
}

#[test]
fn task_implement_guidance_pins_metadata_changed_exit_and_json_outcome() {
    for (name, document) in [
        ("commands", include_str!("../docs/commands.md")),
        ("exit codes", include_str!("../docs/exit-codes.md")),
        ("JSON", include_str!("../docs/json.md")),
    ] {
        for outcome in [
            "exit 4 (`refused`)",
            "data.exitCode: 4",
            "data.stopReason: \"metadata_changed\"",
            "postHead: null",
            "newCommits: null",
            "dirty: null",
            "ok: false",
            "error.code: \"refused\"",
        ] {
            assert!(document.contains(outcome), "{name} is missing {outcome}");
        }
    }
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let notes = help["helps"]["task implement"]["notes"].as_array().unwrap();
    let metadata = notes
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|note| note.contains("metadata_changed"))
        .expect("Task implement help must document metadata changes");
    for outcome in ["exit 4 (refused)", "null postHead/newCommits/dirty"] {
        assert!(metadata.contains(outcome), "help is missing {outcome}");
    }
    for document in [README, README_JA, include_str!("../skills/rabi/SKILL.md")] {
        assert!(document.contains("stopReason: \"metadata_changed\""));
    }
}

#[test]
fn task_advance_guidance_covers_waiting_limits_and_untrusted_logs() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    assert!(help["helps"]["task advance"].is_object());
    assert!(help["exitCodes"]["10"].is_string());
    for flag in ["--wait", "--ci-timeout"] {
        assert!(include_str!("../docs/commands.md").contains(flag));
    }
    for key in ["fix-budget", "ci-timeout"] {
        assert!(include_str!("../docs/configuration.md").contains(key));
    }
    assert!(include_str!("../docs/safety.md").contains("CI job logs are untrusted data"));
    assert!(include_str!("../docs/exit-codes.md").contains("10 (`waiting`)"));
}

#[test]
fn task_review_guidance_covers_boundary_and_deterministic_outcomes() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let review = &help["helps"]["task review"];
    assert_eq!(review["usage"], "rabi task review [SEL] [options]");
    let notes = review["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for (name, document) in [
        ("review help", notes.as_str()),
        ("commands", include_str!("../docs/commands.md")),
        ("safety", include_str!("../docs/safety.md")),
    ] {
        for requirement in [
            "read-only",
            "--add-dir",
            "review_invalid",
            "review_diff_too_large",
            "review_pass_invalid",
            "never an approval",
            "nonce",
        ] {
            assert!(document.contains(requirement), "{name}: {requirement}");
        }
    }
    assert!(include_str!("../docs/configuration.md").contains("[review]"));
    assert!(include_str!("../docs/json.md").contains("rabi/reviews/"));
    assert!(README.contains("approval, merge and safe cleanup"));
    assert!(README_JA.contains("承認・マージ・安全なクリーンアップ"));
    let advance = help["helps"]["task advance"]["notes"].as_array().unwrap();
    assert!(advance.iter().any(|note| {
        let note = note.as_str().unwrap();
        note.contains("--wait") && note.contains("approval-timeout")
    }));
}

#[test]
fn task_landing_guidance_covers_approval_merge_and_cleanup() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let notes = help["helps"]["task advance"]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for (name, document) in [
        ("help", notes.as_str()),
        ("commands", include_str!("../docs/commands.md")),
        ("safety", include_str!("../docs/safety.md")),
    ] {
        for requirement in [
            "approval-timeout",
            "delete-branch",
            "--match-head-commit",
            "stashed",
            "auto",
            "floor",
        ] {
            assert!(document.contains(requirement), "{name}: {requirement}");
        }
    }
    assert!(include_str!("../docs/exit-codes.md").contains("maintainer approval"));
    assert!(include_str!("../docs/configuration.md").contains("approval-timeout = \"24h\""));
    assert!(include_str!("../docs/json.md").contains("mergeCommitSha"));
}

#[test]
fn run_guidance_documents_optional_skill_and_shared_agent_boundary() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let skill = help["helps"]["run"]["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|option| option["name"] == "skill")
        .unwrap();
    assert!(skill["description"].as_str().unwrap().contains("Optional"));
    assert!(!help["commands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command["name"] == "skill"));
    let notes = help["helps"]["run"]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for (name, document) in [
        ("run help", notes.as_str()),
        ("commands", include_str!("../docs/commands.md")),
        ("safety", include_str!("../docs/safety.md")),
    ] {
        for setting in [
            "GH_CONFIG_DIR",
            "RABI_AGENT=1",
            "sandbox_workspace_write.network_access=false",
            "Bash(git push:*)",
            "Bash(gh:*)",
            "WebFetch",
            "WebSearch",
            "empty network allowlist",
        ] {
            assert!(document.contains(setting), "{name} is missing {setting}");
        }
    }
}

#[test]
fn repository_selection_guidance_has_no_registry_or_cross_repository_help() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    for command in ["repo", "repo add", "repo list", "repo show", "repo forget"] {
        assert!(help["helps"].get(command).is_none(), "{command}");
    }
    let contract = include_str!("../help-data.json");
    for obsolete in [
        "registered ID",
        "unambiguous name suffix",
        "all-repos",
        "discover-ghq",
        "and register it",
    ] {
        assert!(!contract.contains(obsolete), "{obsolete}");
    }
    for (name, document) in [
        ("README.md", README),
        ("README.ja.md", README_JA),
        ("commands", include_str!("../docs/commands.md")),
        ("configuration", include_str!("../docs/configuration.md")),
        ("skill", include_str!("../skills/rabi/SKILL.md")),
    ] {
        assert!(document.contains("host/owner/repo"), "{name}");
        for obsolete in [
            "rabi repo add",
            "rabi repo list",
            "rabi repo show",
            "rabi repo forget",
            "registered/discovered name",
        ] {
            assert!(!document.contains(obsolete), "{name}: {obsolete}");
        }
    }
    let configuration = include_str!("../docs/configuration.md");
    for requirement in ["ignored", "untouched", "deleted manually", "origin"] {
        assert!(configuration.contains(requirement), "{requirement}");
    }
    let json = include_str!("../docs/json.md");
    assert!(!json.contains("`--all-repos` also adds `repos`"));
    let breaking = include_str!("../CHANGELOG.md")
        .split("\n## ")
        .find(|section| section.starts_with("v8.0.0"))
        .unwrap();
    for removal in ["registry", "--all-repos", "discover-ghq", "-R"] {
        assert!(breaking.contains(removal), "{removal}");
    }
}
