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
fn parent_session_guidance_and_discovery_replace_the_workflow_contract() {
    let help: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    for name in [
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
        for flag in ["--skill", "--fresh", "--timeout"] {
            assert!(document.contains(flag));
        }
        assert!(document.contains("rabi skill list"));
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
