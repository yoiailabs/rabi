//! Pure merge preflight: no Git, filesystem, or network access.
use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{json, Value};

use super::{
    classify::{self, ChangedEntry, Classification},
    policy::{Policy, Tier},
};
use crate::github::facts::Snapshot;

pub(crate) struct Local {
    pub head: Option<String>,
    pub kinds: Option<Vec<ChangedEntry>>,
    pub policy_matches_base_tip: Option<bool>,
    pub classification_record: Option<super::assessment::Record>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Pass,
    Fail,
    NotEvaluated,
}

#[derive(Serialize)]
pub(crate) struct Item {
    pub id: &'static str,
    pub result: Outcome,
    pub reason: String,
    pub detail: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Evaluation {
    pub items: Vec<Item>,
    pub verdict: &'static str,
    pub classification: Option<Classification>,
}

fn item(id: &'static str, pass: bool, reason: impl Into<String>, detail: Value) -> Item {
    Item {
        id,
        result: if pass { Outcome::Pass } else { Outcome::Fail },
        reason: reason.into(),
        detail,
    }
}
fn pending(id: &'static str, reason: &str, detail: Value) -> Item {
    Item {
        id,
        result: Outcome::NotEvaluated,
        reason: reason.into(),
        detail,
    }
}

pub(super) fn classify_files(
    facts: &Snapshot,
    policy: &Policy,
    kinds: Option<&[ChangedEntry]>,
) -> std::result::Result<Classification, &'static str> {
    classify_files_inner(facts, policy, kinds, true)
}

fn classify_files_inner(
    facts: &Snapshot,
    policy: &Policy,
    kinds: Option<&[ChangedEntry]>,
    path_tiers: bool,
) -> std::result::Result<Classification, &'static str> {
    if facts.files_truncated {
        return Err("GitHub file list is truncated");
    }
    if let Some(kinds) = kinds {
        let paths = |entries: &[ChangedEntry]| -> BTreeSet<Vec<u8>> {
            entries
                .iter()
                .flat_map(|entry| std::iter::once(entry.path.clone()).chain(entry.old_path.clone()))
                .collect()
        };
        let remote: BTreeSet<_> = facts
            .files
            .iter()
            .flat_map(|file| {
                std::iter::once(file.filename.as_bytes().to_vec()).chain(
                    file.previous_filename
                        .as_ref()
                        .map(|path| path.as_bytes().to_vec()),
                )
            })
            .collect();
        if paths(kinds) != remote || kinds.len() != facts.files.len() {
            return Err("Local diff path set differs from GitHub file list");
        }
    }
    let entries: Vec<_> = facts
        .files
        .iter()
        .map(|file| {
            let kind = kinds.and_then(|entries| {
                entries
                    .iter()
                    .find(|entry| entry.path == file.filename.as_bytes())
            });
            ChangedEntry {
                path: file.filename.as_bytes().to_vec(),
                old_path: file
                    .previous_filename
                    .as_ref()
                    .map(|path| path.as_bytes().to_vec()),
                status: match file.status.as_str() {
                    "added" => "A",
                    "removed" => "D",
                    "modified" | "changed" => "M",
                    "renamed" => "R",
                    "copied" => "C",
                    other => other,
                }
                .into(),
                old_mode: kind.map_or(0, |entry| entry.old_mode),
                new_mode: kind.map_or(0, |entry| entry.new_mode),
                added: file.additions,
                deleted: file.deletions,
                binary: kind.is_some_and(|entry| entry.binary),
                unknown: kind.map_or_else(
                    || vec!["unknown:kinds".into()],
                    |entry| entry.unknown.clone(),
                ),
            }
        })
        .collect();
    let mut classification = if path_tiers {
        classify::classify(policy, &entries)
    } else {
        classify::safeguards(policy, &entries)
    };
    if kinds.is_none() {
        classification.tier = Tier::High;
        if !classification
            .reasons
            .iter()
            .any(|reason| reason == "unknown:kinds")
        {
            classification.reasons.push("unknown:kinds".into());
        }
    }
    Ok(classification)
}

pub(crate) fn evaluate(
    facts: &Snapshot,
    policy: &Policy,
    local: &Local,
    base: Option<&str>,
) -> Evaluation {
    let pr = &facts.pr;
    let expected_base = base.unwrap_or(&facts.default_branch);
    let mut problems = Vec::new();
    if pr.state != "open" {
        problems.push("PR is not open");
    }
    if pr.draft {
        problems.push("PR is draft");
    }
    if !pr.same_repository {
        problems.push("PR head is a fork or unavailable repository");
    }
    if pr.base_ref != expected_base {
        problems.push("PR base differs from expected branch");
    }
    let mut items = vec![item(
        "pr_open",
        problems.is_empty(),
        if problems.is_empty() {
            "PR is open, ready for review, same repository and expected base".into()
        } else {
            problems.join("; ")
        },
        json!({"expectedBase": expected_base}),
    )];
    items.push(if facts.inconsistent {
        item(
            "head_consistent",
            false,
            "PR head moved during collection",
            json!({"head":pr.head_sha,"finalHead":facts.final_head_sha}),
        )
    } else if let Some(head) = &local.head {
        item(
            "head_consistent",
            head == &pr.head_sha,
            if head == &pr.head_sha {
                "PR head is stable and matches local HEAD"
            } else {
                "local HEAD differs from PR head"
            },
            json!({"localHead":head,"prHead":pr.head_sha}),
        )
    } else {
        pending(
            "head_consistent",
            "PR head is stable; local comparison not evaluated for pr:N",
            Value::Null,
        )
    });
    let checks: Vec<_> = policy.ci.required.iter().map(|required| {
        let name = required.name();
        let app = required.app(&policy.ci);
        let latest = facts.check_runs.iter().filter(|run| run.name == name && run.app.as_deref() == Some(app)).max_by_key(|run| run.id);
        let pass = latest.is_some_and(|run| run.status == "completed" && run.conclusion.as_deref() == Some("success"));
        let observed = latest.map_or_else(|| "missing".into(), |run| format!("{}:{}", run.status, run.conclusion.as_deref().unwrap_or("none")));
        let ignored_runs: Vec<_> = facts.check_runs.iter().filter(|run| run.name == name && run.app.as_deref() != Some(app)).collect();
        let ignored_statuses: Vec<_> = facts.statuses.iter().filter(|status| status.context == name).collect();
        json!({"name":name,"app":app,"pass":pass,"observed":observed,"ignoredCheckRuns":ignored_runs,"ignoredStatuses":ignored_statuses})
    }).collect();
    let passed = !facts.checks_incomplete
        && !checks.is_empty()
        && checks.iter().all(|check| check["pass"] == true);
    let reason = if checks.is_empty() {
        "no_required_checks".into()
    } else {
        checks
            .iter()
            .map(|check| {
                format!(
                    "{}: {}{}",
                    check["name"].as_str().unwrap_or(""),
                    check["observed"].as_str().unwrap_or(""),
                    if check["ignoredCheckRuns"]
                        .as_array()
                        .is_some_and(|rows| !rows.is_empty())
                        || check["ignoredStatuses"]
                            .as_array()
                            .is_some_and(|rows| !rows.is_empty())
                    {
                        " (other apps and commit statuses ignored)"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("; ")
    };
    let reason = if facts.checks_incomplete {
        format!("GitHub check runs are incomplete: collected count differs from total_count, or total_count is missing or changed; {reason}")
    } else {
        reason
    };
    items.push(item(
        "required_checks",
        passed,
        reason,
        json!({"checks":checks,"checksIncomplete":facts.checks_incomplete}),
    ));
    let record = local.classification_record.as_ref();
    let (classification, risk) = match classify_files_inner(
        facts,
        policy,
        local.kinds.as_deref(),
        record.is_none(),
    ) {
        Ok(mut classification) => {
            if let Some(record) = record {
                // Only deterministic escalation survives; advisory rule/default
                // tiers never change a fresh policy-document verdict.
                classification.tier = if classification.tier == Tier::High {
                    Tier::High
                } else {
                    record.tier
                };
                classification.floor |= record.floor;
                for reason in &record.reasons {
                    if !classification.reasons.contains(reason) {
                        classification.reasons.push(reason.clone());
                    }
                }
            }
            let risk = item(
                "risk_tier",
                true,
                "Risk tier computed",
                json!({"tier":classification.tier,"floor":classification.floor,"reasons":classification.reasons}),
            );
            (Some(classification), risk)
        }
        Err(reason) => (None, item("risk_tier", false, reason, Value::Null)),
    };
    items.push(risk);
    let automatic = classification
        .as_ref()
        .is_some_and(|risk| !risk.floor && policy.merge.auto.contains(&risk.tier));
    // Statuses are collected for this SHA, newest first. Never search past a
    // newer failure, pending status, or unapproved issuer for an older success.
    let latest = facts
        .statuses
        .iter()
        .find(|status| status.context == "rabi/approval");
    let (approved, reason) = if local.policy_matches_base_tip.is_none() {
        (
            false,
            "Cannot determine base branch tip or read its policy; retry or update the PR",
        )
    } else if local.policy_matches_base_tip == Some(false) {
        (
            false,
            "Policy changed on base branch since PR base; update the PR",
        )
    } else if automatic {
        (true, "auto tier")
    } else if classification.is_none() {
        (false, "Risk classification unavailable")
    } else if policy.approval.approvers.is_empty() {
        (false, "No approval.approvers configured")
    } else if facts.inconsistent {
        (
            false,
            "PR head moved; approval must match the current head SHA",
        )
    } else {
        match latest {
            None => (false, "No rabi/approval status on the current head SHA"),
            Some(status) if status.sha != pr.head_sha => (false, "Approval is for an older SHA"),
            Some(status) if status.state != "success" => {
                (false, "Latest rabi/approval status is not success")
            }
            Some(status)
                if !status.creator.as_ref().is_some_and(|creator| {
                    policy
                        .approval
                        .approvers
                        .iter()
                        .any(|login| login.eq_ignore_ascii_case(creator))
                }) =>
            {
                (
                    false,
                    "Latest rabi/approval creator is not an allowed approver",
                )
            }
            Some(_) => (
                true,
                "Latest rabi/approval status approves this exact head SHA",
            ),
        }
    };
    items.push(item(
        "approval",
        approved,
        reason,
        classification.as_ref().map_or(
            Value::Null,
            |risk| json!({"tier":risk.tier,"floor":risk.floor}),
        ),
    ));
    items.push(pending(
        "review_record",
        "No task review records yet",
        Value::Null,
    ));
    items.push(match record {
        Some(record) => item(
            "classification_record",
            true,
            "Classification record matches exact PR head and base policy/document blobs",
            json!(record),
        ),
        None => pending(
            "classification_record",
            "No matching task classification record (missing, unreadable, or stale)",
            Value::Null,
        ),
    });
    let requested: Vec<_> = facts
        .reviews
        .iter()
        .filter(|review| review.state == "CHANGES_REQUESTED")
        .collect();
    let no_requests =
        requested.is_empty() && facts.review_decision.as_deref() != Some("CHANGES_REQUESTED");
    items.push(item(
        "no_changes_requested",
        no_requests,
        if no_requests {
            "No changes requested"
        } else {
            "CHANGES_REQUESTED"
        },
        json!({"reviews":requested,"reviewDecision":facts.review_decision}),
    ));
    let mergeable = pr.mergeable == Some(true)
        && matches!(
            pr.mergeable_state.as_str(),
            "clean" | "unstable" | "has_hooks"
        );
    let reason = if pr.mergeable.is_none() || pr.mergeable_state == "unknown" {
        format!(
            "GitHub has not computed mergeability (state: {}); rerun later",
            pr.mergeable_state
        )
    } else if mergeable {
        format!("GitHub reports mergeable (state: {})", pr.mergeable_state)
    } else {
        format!(
            "GitHub mergeability rejected (mergeable: {:?}, state: {})",
            pr.mergeable, pr.mergeable_state
        )
    };
    items.push(item(
        "mergeable",
        mergeable,
        reason,
        json!({"mergeable":pr.mergeable,"state":pr.mergeable_state}),
    ));
    let verdict = if items
        .iter()
        .any(|item| matches!(item.result, Outcome::Fail))
    {
        "blocked"
    } else if items
        .iter()
        .any(|item| matches!(item.result, Outcome::NotEvaluated))
    {
        "incomplete"
    } else {
        "ready"
    };
    Evaluation {
        items,
        verdict,
        classification,
    }
}

impl Evaluation {
    pub fn permits_merge(&self, external_review: bool) -> bool {
        self.items.iter().all(|item| match item.result {
            Outcome::Pass => true,
            Outcome::Fail => false,
            Outcome::NotEvaluated => {
                external_review && matches!(item.id, "review_record" | "classification_record")
            }
        })
    }

    pub fn print(&self) {
        use crate::output::{outln, visible_text};
        for item in &self.items {
            let detail = if matches!(item.id, "risk_tier" | "approval") {
                self.classification.as_ref().map_or(String::new(), |risk| {
                    format!(
                        " (tier: {}, floor: {}, reasons: {})",
                        risk.tier,
                        risk.floor,
                        visible_text(&risk.reasons.join(", "))
                    )
                })
            } else {
                String::new()
            };
            outln!(
                "{} {}: {}{}",
                match item.result {
                    Outcome::Pass => "PASS",
                    Outcome::Fail => "FAIL",
                    Outcome::NotEvaluated => "N/A",
                },
                item.id,
                visible_text(&item.reason),
                detail
            );
        }
        outln!("Verdict: {}", self.verdict);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const POLICY: &str = "schema = 1\ndefault-tier = 'low'\n[ci]\nrequired = ['Rust test']";

    #[test]
    fn preflight_table() {
        // Each row starts from a serializable snapshot fixture and changes one fact.
        let cases = [
            ("success", "", Value::Null, true, "", "incomplete"),
            (
                "incomplete checks",
                "/checksIncomplete",
                json!(true),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "blocked",
                "/pr/mergeableState",
                json!("blocked"),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "behind",
                "/pr/mergeableState",
                json!("behind"),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "unrecognized",
                "/pr/mergeableState",
                json!("unrecognized"),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "draft state",
                "/pr/mergeableState",
                json!("draft"),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "skipped",
                "/checkRuns/0/conclusion",
                json!("skipped"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "neutral",
                "/checkRuns/0/conclusion",
                json!("neutral"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "failure",
                "/checkRuns/0/conclusion",
                json!("failure"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "cancelled",
                "/checkRuns/0/conclusion",
                json!("cancelled"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "timed out",
                "/checkRuns/0/conclusion",
                json!("timed_out"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "action required",
                "/checkRuns/0/conclusion",
                json!("action_required"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "stale",
                "/checkRuns/0/conclusion",
                json!("stale"),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "queued",
                "/checkRuns/0/status",
                json!("queued"),
                true,
                "required_checks",
                "blocked",
            ),
            ("rerun", "", Value::Null, true, "required_checks", "blocked"),
            (
                "path filter missing",
                "/checkRuns",
                json!([]),
                true,
                "required_checks",
                "blocked",
            ),
            (
                "wrong app and status",
                "",
                Value::Null,
                true,
                "required_checks",
                "blocked",
            ),
            (
                "truncated",
                "/filesTruncated",
                json!(true),
                true,
                "risk_tier",
                "blocked",
            ),
            (
                "head moved",
                "/inconsistent",
                json!(true),
                true,
                "head_consistent",
                "blocked",
            ),
            (
                "local mismatch",
                "",
                Value::Null,
                true,
                "head_consistent",
                "blocked",
            ),
            (
                "fork",
                "/pr/sameRepository",
                json!(false),
                true,
                "pr_open",
                "blocked",
            ),
            (
                "draft",
                "/pr/draft",
                json!(true),
                true,
                "pr_open",
                "blocked",
            ),
            (
                "closed",
                "/pr/state",
                json!("closed"),
                true,
                "pr_open",
                "blocked",
            ),
            (
                "wrong base",
                "/pr/baseRef",
                json!("release"),
                true,
                "pr_open",
                "blocked",
            ),
            (
                "empty required",
                "",
                Value::Null,
                true,
                "required_checks",
                "blocked",
            ),
            (
                "missing ci",
                "",
                Value::Null,
                true,
                "required_checks",
                "blocked",
            ),
            (
                "mergeable null",
                "/pr/mergeable",
                Value::Null,
                true,
                "mergeable",
                "blocked",
            ),
            (
                "mergeable unknown",
                "/pr/mergeableState",
                json!("unknown"),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "conflicting",
                "/pr/mergeable",
                json!(false),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "dirty",
                "/pr/mergeableState",
                json!("dirty"),
                true,
                "mergeable",
                "blocked",
            ),
            (
                "requested",
                "/reviews",
                json!([{"reviewer":"reviewer-1","state":"CHANGES_REQUESTED","id":1}]),
                true,
                "no_changes_requested",
                "blocked",
            ),
            (
                "decision",
                "/reviewDecision",
                json!("CHANGES_REQUESTED"),
                true,
                "no_changes_requested",
                "blocked",
            ),
            ("unknown kinds", "", Value::Null, false, "", "incomplete"),
            (
                "floor",
                "/files/0/filename",
                json!(".github/workflows/ci.yml"),
                true,
                "",
                "incomplete",
            ),
            (
                "path mismatch",
                "",
                Value::Null,
                true,
                "risk_tier",
                "blocked",
            ),
            (
                "explicit base",
                "/pr/baseRef",
                json!("release"),
                true,
                "",
                "incomplete",
            ),
            ("pr selector", "", Value::Null, true, "", "incomplete"),
            (
                "unstable",
                "/pr/mergeableState",
                json!("unstable"),
                true,
                "",
                "incomplete",
            ),
            (
                "has hooks",
                "/pr/mergeableState",
                json!("has_hooks"),
                true,
                "",
                "incomplete",
            ),
        ];
        for (name, pointer, value, available, failed, verdict) in cases {
            let mut fixture: Value =
                serde_json::from_str(include_str!("../../tests/fixtures/task_facts.json")).unwrap();
            if !pointer.is_empty() {
                *fixture.pointer_mut(pointer).unwrap() = value;
            }
            if name == "rerun" {
                let mut rerun = fixture["checkRuns"][0].clone();
                rerun["id"] = json!(11);
                rerun["status"] = json!("in_progress");
                rerun["conclusion"] = Value::Null;
                fixture["checkRuns"]
                    .as_array_mut()
                    .unwrap()
                    .insert(0, rerun);
            }
            if name == "wrong app and status" {
                fixture["checkRuns"][0]["app"] = json!("another-app");
                fixture["statuses"].as_array_mut().unwrap().push(
                    json!({"sha":"2222222222222222222222222222222222222222","context":"Rust test","state":"success","creator":"issuer-1"}));
            }
            let facts: Snapshot = serde_json::from_value(fixture).unwrap();
            let policy_text = match name {
                "empty required" => "schema=1\n[ci]\nrequired=[]",
                "missing ci" => "schema=1",
                _ => POLICY,
            };
            let policy = Policy::parse(&format!(
                "{policy_text}\n[approval]\napprovers=['maintainer']"
            ))
            .unwrap();
            let kinds = available.then(|| {
                facts
                    .files
                    .iter()
                    .map(|file| ChangedEntry {
                        path: if name == "path mismatch" {
                            b"other".to_vec()
                        } else {
                            file.filename.as_bytes().to_vec()
                        },
                        old_path: None,
                        status: "M".into(),
                        old_mode: 0o100644,
                        new_mode: 0o100644,
                        added: 0,
                        deleted: 0,
                        binary: false,
                        unknown: Vec::new(),
                    })
                    .collect()
            });
            let head = if name == "pr selector" {
                None
            } else {
                Some(if name == "local mismatch" {
                    "different".into()
                } else {
                    facts.pr.head_sha.clone()
                })
            };
            let report = evaluate(
                &facts,
                &policy,
                &Local {
                    head,
                    kinds,
                    policy_matches_base_tip: Some(true),
                    classification_record: None,
                },
                (name == "explicit base").then_some("release"),
            );
            assert_eq!(report.verdict, verdict, "{name}");
            let ids = [
                "pr_open",
                "head_consistent",
                "required_checks",
                "risk_tier",
                "approval",
                "review_record",
                "classification_record",
                "no_changes_requested",
                "mergeable",
            ];
            for (item, id) in report.items.iter().zip(ids) {
                assert_eq!(item.id, id, "{name}");
                let expected = if id == failed
                    || (id == "approval" && (report.classification.is_none() || facts.inconsistent))
                {
                    "fail"
                } else if matches!(id, "review_record" | "classification_record")
                    || (name == "pr selector" && id == "head_consistent")
                {
                    "not_evaluated"
                } else {
                    "pass"
                };
                assert_eq!(
                    serde_json::to_value(&item.result).unwrap(),
                    expected,
                    "{name}: {id}"
                );
            }
            assert_eq!(report.items.len(), ids.len());
            if name == "unknown kinds" {
                let risk = report.classification.as_ref().unwrap();
                assert_eq!(risk.tier, Tier::High);
                assert!(risk.reasons.contains(&"unknown:kinds".into()));
            }
            if name == "floor" {
                let risk = report.classification.as_ref().unwrap();
                assert_eq!(risk.tier, Tier::High);
                assert!(risk.floor);
                assert_eq!(report.items[4].detail["floor"], true);
            }
            if name == "wrong app and status" {
                let detail = &report.items[2].detail["checks"][0];
                assert_eq!(detail["ignoredCheckRuns"].as_array().unwrap().len(), 1);
                assert_eq!(detail["ignoredStatuses"].as_array().unwrap().len(), 1);
                assert!(report.items[2].reason.contains("ignored"));
            }
            if matches!(name, "empty required" | "missing ci") {
                assert_eq!(report.items[2].reason, "no_required_checks");
            }
            if name == "incomplete checks" {
                assert!(report.items[2].reason.contains("incomplete"));
                assert_eq!(report.items[2].detail["checksIncomplete"], true);
            }
            if matches!(name, "blocked" | "behind" | "unrecognized" | "draft state") {
                assert!(report.items[8].reason.contains(&facts.pr.mergeable_state));
                assert_eq!(report.items[8].detail["state"], facts.pr.mergeable_state);
            }
            if name == "local mismatch" {
                assert_eq!(report.items[1].reason, "local HEAD differs from PR head");
            }
            if name == "mergeable null" {
                assert!(report.items[8]
                    .reason
                    .contains("GitHub has not computed mergeability"));
                assert!(report.items[8].reason.contains("rerun later"));
            }
        }
    }
    #[test]
    fn approval_table() {
        for (name, tier, automatic, floor, approvers, statuses, pass) in [
            ("auto tier", "low", true, false, false, vec![], true),
            ("auto medium", "medium", true, false, false, vec![], true),
            (
                "high approved",
                "high",
                false,
                false,
                true,
                vec![("success", "maintainer", false)],
                true,
            ),
            (
                "wrong issuer",
                "high",
                false,
                false,
                true,
                vec![("success", "other", false)],
                false,
            ),
            (
                "old SHA",
                "high",
                false,
                false,
                true,
                vec![("success", "maintainer", true)],
                false,
            ),
            (
                "newer failure",
                "high",
                false,
                false,
                true,
                vec![
                    ("failure", "maintainer", false),
                    ("success", "maintainer", false),
                ],
                false,
            ),
            (
                "newer pending",
                "high",
                false,
                false,
                true,
                vec![
                    ("pending", "maintainer", false),
                    ("success", "maintainer", false),
                ],
                false,
            ),
            (
                "newer other issuer",
                "high",
                false,
                false,
                true,
                vec![
                    ("success", "other", false),
                    ("success", "maintainer", false),
                ],
                false,
            ),
            (
                "empty approvers",
                "high",
                false,
                false,
                false,
                vec![("success", "maintainer", false)],
                false,
            ),
            ("missing status", "high", false, false, true, vec![], false),
            (
                "floor in auto tier",
                "low",
                true,
                true,
                false,
                vec![],
                false,
            ),
        ] {
            let mut facts: Snapshot =
                serde_json::from_str(include_str!("../../tests/fixtures/task_facts.json")).unwrap();
            if floor {
                facts.files[0].filename = ".rabi.toml".into();
            }
            facts.statuses = statuses
                .into_iter()
                .map(|(state, creator, old)| crate::github::facts::CommitStatus {
                    sha: if old {
                        facts.pr.base_sha.clone()
                    } else {
                        facts.pr.head_sha.clone()
                    },
                    context: "rabi/approval".into(),
                    state: state.into(),
                    creator: Some(creator.into()),
                })
                .collect();
            let mut policy = Policy::parse(&format!(
                "schema=1\ndefault-tier='{tier}'\n[merge]\nauto={}\n[approval]\napprovers={}",
                if automatic { "['low','medium']" } else { "[]" },
                if approvers { "['maintainer']" } else { "[]" }
            ))
            .unwrap();
            // Floors currently force high; keep the explicit !floor guard tested
            // even if an in-memory policy contains that tier in auto.
            if floor {
                policy.merge.auto.push(Tier::High);
            }
            let kinds = Some(vec![ChangedEntry {
                path: facts.files[0].filename.as_bytes().to_vec(),
                old_path: None,
                status: "M".into(),
                old_mode: 0o100644,
                new_mode: 0o100644,
                added: 2,
                deleted: 1,
                binary: false,
                unknown: vec![],
            }]);
            let mut local = Local {
                head: Some(facts.pr.head_sha.clone()),
                kinds,
                policy_matches_base_tip: Some(true),
                classification_record: None,
            };
            for freshness in [Some(false), None] {
                local.policy_matches_base_tip = freshness;
                let stale = evaluate(&facts, &policy, &local, None);
                assert!(
                    matches!(stale.items[4].result, Outcome::Fail),
                    "{name}: {freshness:?}"
                );
                assert!(stale.items[4].reason.contains("base branch"));
            }
            local.policy_matches_base_tip = Some(true);
            let report = evaluate(&facts, &policy, &local, None);
            let approval = &report.items[4];
            assert_eq!(
                matches!(approval.result, Outcome::Pass),
                pass,
                "{name}: {}",
                approval.reason
            );
            assert!(!matches!(approval.result, Outcome::NotEvaluated), "{name}");
            if automatic && !floor {
                assert_eq!(approval.reason, "auto tier");
            }
        }
    }

    #[test]
    fn matching_record_cannot_lower_deterministic_safeguards() {
        for case in [
            "floor",
            "binary",
            "symlink",
            "submodule",
            "exec-bit",
            "unknown",
        ] {
            let mut facts: Snapshot =
                serde_json::from_str(include_str!("../../tests/fixtures/task_facts.json")).unwrap();
            if case == "floor" {
                facts.files[0].filename = ".github/config".into();
            }
            let policy = Policy::parse("schema=1\ndefault-tier='low'\n[merge]\nauto=['low','medium']\n[ci]\nrequired=['Rust test']").unwrap();
            let entry = ChangedEntry {
                path: facts.files[0].filename.as_bytes().to_vec(),
                old_path: None,
                status: "M".into(),
                old_mode: 0o100644,
                new_mode: match case {
                    "symlink" => 0o120000,
                    "submodule" => 0o160000,
                    "exec-bit" => 0o100755,
                    _ => 0o100644,
                },
                added: 2,
                deleted: 1,
                binary: case == "binary",
                unknown: if case == "unknown" {
                    vec!["unknown:raw".into()]
                } else {
                    vec![]
                },
            };
            let record = serde_json::from_value(json!({
                "schema":1,"headSha":facts.pr.head_sha,"baseSha":facts.pr.base_sha,
                "mergeBaseSha":facts.pr.base_sha,"policyBlobSha":null,"docBlobSha":null,
                "tier":"low","floor":false,"reasons":["Completed assessment"],
                "uncertainty":"none","provider":"codex","createdAt":"2026-01-01T00:00:00Z"
            }))
            .unwrap();
            let evaluation = evaluate(
                &facts,
                &policy,
                &Local {
                    head: Some(facts.pr.head_sha.clone()),
                    kinds: Some(vec![entry]),
                    policy_matches_base_tip: Some(true),
                    classification_record: Some(record),
                },
                None,
            );
            assert_eq!(
                evaluation.classification.as_ref().unwrap().tier,
                Tier::High,
                "{case}"
            );
            assert!(
                matches!(evaluation.items[4].result, Outcome::Fail),
                "{case}"
            );
            assert!(
                matches!(evaluation.items[6].result, Outcome::Pass),
                "{case}"
            );
            assert!(!evaluation.permits_merge(true), "{case}");
        }
    }
}
