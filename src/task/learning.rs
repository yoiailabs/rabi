//! Read-only learning from local records and append-only observations, never merge authority.
use std::collections::BTreeMap;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use super::store::{Event, Store, Task, TaskState};
use crate::error::{Error, Result};

const FINDING_LIMIT: usize = 20;
const EXAMPLE_CHARS: usize = 240;
const CAUSES: &[&str] = &[
    "precheck",
    "ci",
    "conflict",
    "review",
    "changes-requested",
    "unknown",
];

pub(crate) fn parse_since(value: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    value.map(|value| since_at(value, Utc::now())).transpose()
}

fn since_at(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let invalid = || {
        Error::validation("--since requires a positive duration (s, m, h, d, w), YYYY-MM-DD, or RFC3339 timestamp")
    };
    if let Ok(date) = DateTime::parse_from_rfc3339(value) {
        return Ok(date.with_timezone(&Utc));
    }
    if value.len() == 10 {
        if let Ok(date) = NaiveDate::parse_from_str(value, "%Y-%m-%d") {
            return date
                .and_hms_opt(0, 0, 0)
                .map(|date| date.and_utc())
                .ok_or_else(invalid);
        }
    }
    let (digits, unit) = value
        .split_at_checked(value.len().saturating_sub(1))
        .ok_or_else(invalid)?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let multiplier = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => return Err(invalid()),
    };
    let seconds = digits
        .parse::<i64>()
        .ok()
        .and_then(|n| n.checked_mul(multiplier))
        .filter(|n| *n > 0)
        .ok_or_else(invalid)?;
    let duration = chrono::TimeDelta::try_seconds(seconds).ok_or_else(invalid)?;
    now.checked_sub_signed(duration).ok_or_else(invalid)
}

fn timestamp(value: &str, id: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|date| date.with_timezone(&Utc))
        .map_err(|_| Error::msg(format!("Task {id}: invalid recorded timestamp")))
}

fn included(at: DateTime<Utc>, since: &Option<DateTime<Utc>>) -> bool {
    since.is_none_or(|since| at >= since)
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Timing {
    count: u64,
    total_ms: u64,
    mean_ms: Option<f64>,
    min_ms: Option<u64>,
    max_ms: Option<u64>,
}
impl Timing {
    fn add(&mut self, start: DateTime<Utc>, end: DateTime<Utc>, since: &Option<DateTime<Utc>>) {
        if end < start || !included(end, since) {
            return;
        }
        let Ok(ms) = end
            .signed_duration_since(start)
            .num_milliseconds()
            .try_into()
        else {
            return;
        };
        self.count += 1;
        self.total_ms = self.total_ms.saturating_add(ms);
        self.mean_ms = Some(self.total_ms as f64 / self.count as f64);
        self.min_ms = Some(self.min_ms.map_or(ms, |old| old.min(ms)));
        self.max_ms = Some(self.max_ms.map_or(ms, |old| old.max(ms)));
    }
    fn combine(&mut self, other: &Self) {
        self.count += other.count;
        self.total_ms = self.total_ms.saturating_add(other.total_ms);
        self.mean_ms = (self.count > 0).then(|| self.total_ms as f64 / self.count as f64);
        if let Some(min) = other.min_ms {
            self.min_ms = Some(self.min_ms.map_or(min, |old| old.min(min)));
        }
        if let Some(max) = other.max_ms {
            self.max_ms = Some(self.max_ms.map_or(max, |old| old.max(max)));
        }
    }
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct TierOutcome {
    auto_merged: u64,
    awaited_approval: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Finding {
    path: String,
    body_hash: String,
    count: u64,
    example: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskReport {
    task_id: String,
    state: TaskState,
    fix_relaunches: u64,
    fixes_by_cause: BTreeMap<String, u64>,
    review_rounds: u64,
    approval_wait: Timing,
    ci_wait: Timing,
    time_to_merge: Timing,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LearningReport {
    report_version: u32,
    since: Option<String>,
    task_count: usize,
    states: BTreeMap<String, u64>,
    final_states: BTreeMap<String, u64>,
    blocked_reasons: BTreeMap<String, u64>,
    current_blocked_reasons: BTreeMap<String, u64>,
    fix_relaunches: u64,
    fixes_by_cause: BTreeMap<String, u64>,
    review_rounds: u64,
    tier_outcomes: BTreeMap<String, TierOutcome>,
    approval_wait: Timing,
    ci_wait: Timing,
    time_to_merge: Timing,
    top_findings: Vec<Finding>,
    tasks: Vec<TaskReport>,
}

fn counts(keys: &[&str]) -> BTreeMap<String, u64> {
    keys.iter().map(|key| ((*key).into(), 0)).collect()
}

impl LearningReport {
    fn empty(since: &Option<DateTime<Utc>>) -> Self {
        Self {
            report_version: 1,
            since: since.map(|date| date.to_rfc3339()),
            task_count: 0,
            states: counts(&[
                "new",
                "implementing",
                "prechecking",
                "publishing",
                "ci",
                "reviewing",
                "ready",
                "classifying",
                "awaiting_approval",
                "merging",
                "merged",
                "blocked",
                "abandoned",
            ]),
            final_states: counts(&["merged", "abandoned"]),
            blocked_reasons: BTreeMap::new(),
            current_blocked_reasons: BTreeMap::new(),
            fix_relaunches: 0,
            fixes_by_cause: counts(CAUSES),
            review_rounds: 0,
            tier_outcomes: ["low", "medium", "high", "unknown"]
                .into_iter()
                .map(|tier| (tier.into(), TierOutcome::default()))
                .collect(),
            approval_wait: Timing::default(),
            ci_wait: Timing::default(),
            time_to_merge: Timing::default(),
            top_findings: Vec::new(),
            tasks: Vec::new(),
        }
    }

    pub(crate) fn print(&self) {
        use crate::output::outln;
        outln!("Tasks: {}", self.task_count);
        outln!("States: {}", display_counts(&self.states));
        outln!("Blocked: {}", display_counts(&self.blocked_reasons));
        outln!(
            "Fix relaunches: {} ({})",
            self.fix_relaunches,
            display_counts(&self.fixes_by_cause)
        );
        outln!("Review rounds: {}", self.review_rounds);
        for (tier, outcome) in &self.tier_outcomes {
            if outcome.auto_merged + outcome.awaited_approval > 0 {
                outln!(
                    "{}: {} auto-merged, {} awaited approval",
                    tier,
                    outcome.auto_merged,
                    outcome.awaited_approval
                );
            }
        }
        for (name, timing) in [
            ("Approval wait", &self.approval_wait),
            ("CI wait", &self.ci_wait),
            ("Time to merge", &self.time_to_merge),
        ] {
            outln!(
                "{}: {} samples, mean {} ms",
                name,
                timing.count,
                timing
                    .mean_ms
                    .map(|value| format!("{value:.0}"))
                    .unwrap_or_else(|| "n/a".into())
            );
        }
        for finding in &self.top_findings {
            outln!(
                "{}x {} [{}]: {}",
                finding.count,
                crate::output::visible_text(&finding.path),
                finding.body_hash,
                crate::output::visible_text(&finding.example)
            );
        }
    }
}

fn display_counts(counts: &BTreeMap<String, u64>) -> String {
    let values: Vec<_> = counts
        .iter()
        .filter(|(_, count)| **count > 0)
        .map(|(key, count)| format!("{key}={count}"))
        .collect();
    if values.is_empty() {
        "none".into()
    } else {
        values.join(", ")
    }
}

fn block_name(state: TaskState) -> Option<String> {
    if let TaskState::Blocked(reason) = state {
        serde_json::to_value(reason)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
    } else {
        None
    }
}

fn fix_cause(event: &Event) -> Option<&'static str> {
    if event
        .precheck
        .as_ref()
        .is_some_and(|data| data["passed"] == false)
    {
        return Some("precheck");
    }
    match event.reason.as_str() {
        "precheck_failed" | "precheck_feedback" => Some("precheck"),
        "ci_failed" => Some("ci"),
        "sync_with_base" => Some("conflict"),
        "review_revise" => Some("review"),
        "changes_requested" => Some("changes-requested"),
        _ => None,
    }
}

pub(crate) fn report(root: &Path, since: &Option<DateTime<Utc>>) -> Result<LearningReport> {
    let store = Store::locate(root)?;
    let mut inputs = Vec::new();
    for id in store.ids()? {
        let mut task = store.load(&id)?;
        let events = store.events(&id)?;
        task.state = store.derived(&task)?;
        inputs.push((task, events));
    }
    aggregate(inputs, since, root)
}

fn aggregate(
    inputs: Vec<(Task, Vec<Event>)>,
    since: &Option<DateTime<Utc>>,
    root: &Path,
) -> Result<LearningReport> {
    let mut report = LearningReport::empty(since);
    let mut findings: BTreeMap<(String, String), Finding> = BTreeMap::new();
    for (task, events) in inputs {
        let created = timestamp(&task.created_at, &task.id)?;
        let times: Vec<_> = events
            .iter()
            .map(|event| timestamp(&event.at, &task.id))
            .collect::<Result<_>>()?;
        if !included(created, since) && !times.iter().any(|time| included(*time, since)) {
            continue;
        }
        report.task_count += 1;
        *report.states.entry(task.state.name().into()).or_default() += 1;
        if task.state.terminal() {
            *report
                .final_states
                .entry(task.state.name().into())
                .or_default() += 1;
        }
        if let Some(reason) = block_name(task.state) {
            *report.current_blocked_reasons.entry(reason).or_default() += 1;
        }
        let mut item = TaskReport {
            task_id: task.id.clone(),
            state: task.state,
            fix_relaunches: 0,
            fixes_by_cause: counts(CAUSES),
            review_rounds: 0,
            approval_wait: Timing::default(),
            ci_wait: Timing::default(),
            time_to_merge: Timing::default(),
        };
        let mut cause = "unknown";
        let mut reserved = None;
        let mut ci_start = None;
        let mut approval_start = None;
        let mut merged = false;
        let mut gate: Option<(&str, bool, DateTime<Utc>)> = None;
        for (event, at) in events.iter().zip(times) {
            let in_window = included(at, since);
            if matches!(
                event.reason.as_str(),
                "head_advanced" | "implementation_progress" | "head_changed"
            ) {
                cause = "unknown";
                reserved = None;
            }
            if let Some(next) = fix_cause(event) {
                cause = next;
            }
            if event.reason == "fix_reserved" {
                reserved = Some(cause);
            }
            if event.reason == "implementation_started" {
                if let Some(cause) = reserved.take() {
                    if in_window {
                        item.fix_relaunches += 1;
                        *item.fixes_by_cause.entry(cause.into()).or_default() += 1;
                    }
                }
            }
            if let Some(reason) = block_name(event.to).filter(|_| event.from != Some(event.to)) {
                if in_window {
                    *report.blocked_reasons.entry(reason).or_default() += 1;
                }
            }
            if matches!(event.reason.as_str(), "review_revise" | "review_passed") && in_window {
                item.review_rounds += 1;
                if let Some(values) = event
                    .review
                    .as_ref()
                    .and_then(|review| review["findings"].as_array())
                {
                    for value in values {
                        if let (Some(path), Some(body)) =
                            (value["path"].as_str(), value["body"].as_str())
                        {
                            let hash = crate::hash::sha256_hex(
                                super::advance::normalized_output(body).as_bytes(),
                            );
                            let path = super::publish::sanitized(path.as_bytes(), root);
                            let entry = findings
                                .entry((path.clone(), hash.clone()))
                                .or_insert_with(|| Finding {
                                    path,
                                    body_hash: hash,
                                    count: 0,
                                    example: super::publish::sanitized(body.as_bytes(), root)
                                        .split_whitespace()
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                        .chars()
                                        .take(EXAMPLE_CHARS)
                                        .collect(),
                                });
                            entry.count += 1;
                        }
                    }
                }
            }
            if event.to == TaskState::Ci && event.from != Some(TaskState::Ci) {
                ci_start = Some(at);
            }
            if event.from == Some(TaskState::Ci) && event.to != TaskState::Ci {
                if let Some(start) = ci_start.take() {
                    item.ci_wait.add(start, at, since);
                }
            }
            if event.to == TaskState::AwaitingApproval
                && event.from != Some(TaskState::AwaitingApproval)
            {
                approval_start = Some(at);
            }
            if event.from == Some(TaskState::AwaitingApproval)
                && event.to != TaskState::AwaitingApproval
            {
                if let Some(start) = approval_start
                    .take()
                    .filter(|_| event.to == TaskState::Merging)
                {
                    item.approval_wait.add(start, at, since);
                }
            }
            if matches!(event.reason.as_str(), "auto_tier" | "approval_required") {
                let tier = event
                    .classification
                    .as_ref()
                    .and_then(|data| data["tier"].as_str())
                    .filter(|tier| matches!(*tier, "low" | "medium" | "high"))
                    .unwrap_or("unknown");
                gate = Some((tier, event.reason == "auto_tier", at));
            }
            if !merged && event.to == TaskState::Merged && event.from != Some(TaskState::Merged) {
                item.time_to_merge.add(created, at, since);
                merged = true;
                if let Some((tier, true, _)) = gate {
                    if in_window {
                        report
                            .tier_outcomes
                            .entry(tier.into())
                            .or_default()
                            .auto_merged += 1;
                    }
                }
            }
        }
        if let Some((tier, false, at)) = gate {
            if included(at, since) {
                report
                    .tier_outcomes
                    .entry(tier.into())
                    .or_default()
                    .awaited_approval += 1;
            }
        }
        report.fix_relaunches += item.fix_relaunches;
        report.review_rounds += item.review_rounds;
        for (cause, count) in &item.fixes_by_cause {
            *report.fixes_by_cause.entry(cause.clone()).or_default() += count;
        }
        report.approval_wait.combine(&item.approval_wait);
        report.ci_wait.combine(&item.ci_wait);
        report.time_to_merge.combine(&item.time_to_merge);
        report.tasks.push(item);
    }
    report.top_findings = findings
        .into_values()
        .filter(|finding| finding.count > 1)
        .collect();
    report.top_findings.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.body_hash.cmp(&b.body_hash))
    });
    report.top_findings.truncate(FINDING_LIMIT);
    Ok(report)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EventRow {
    task_id: String,
    #[serde(flatten)]
    event: Event,
}
impl EventRow {
    pub(crate) fn print(&self) {
        crate::output::outln!(
            "{} {} {} -> {}: {}",
            self.task_id,
            crate::output::visible_text(&self.event.at),
            self.event.from.map_or("null", TaskState::name),
            self.event.to.name(),
            crate::output::visible_text(&self.event.reason)
        );
    }
}

pub(crate) fn events(
    root: &Path,
    selector: Option<&str>,
    since: &Option<DateTime<Utc>>,
) -> Result<Vec<EventRow>> {
    let store = Store::locate(root)?;
    let ids = match selector {
        Some(selector) => vec![store.resolve(selector)?],
        None => store.ids()?,
    };
    let mut rows = Vec::new();
    for id in ids {
        store.load(&id)?;
        for event in store.events(&id)? {
            if included(timestamp(&event.at, &id)?, since) {
                rows.push(EventRow {
                    task_id: id.clone(),
                    event,
                });
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn time(seconds: i64) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
            + chrono::TimeDelta::seconds(seconds)
    }
    fn task(state: Value) -> Task {
        serde_json::from_value(json!({"schema":1,"id":"task_fixture","createdAt":time(0).to_rfc3339(),"updatedAt":time(100).to_rfc3339(),"state":state,"intent":"fixture","branch":"topic","baseRef":"main","pushRemoteUrl":"fixture","workspacePath":null,"prNumber":null})).unwrap()
    }
    fn event(seconds: i64, from: Option<&str>, to: &str, reason: &str) -> Event {
        serde_json::from_value(json!({"at":time(seconds).to_rfc3339(),"from":from.map(|state| json!({"name":state})),"to":{"name":to},"reason":reason})).unwrap()
    }
    fn sample() -> (Task, Vec<Event>) {
        let mut events = vec![
            event(0, None, "new", "created"),
            event(10, Some("new"), "implementing", "implementation_started"),
            event(20, Some("prechecking"), "implementing", "precheck_feedback"),
            event(21, Some("implementing"), "implementing", "fix_reserved"),
            event(
                22,
                Some("implementing"),
                "implementing",
                "implementation_started",
            ),
            event(
                23,
                Some("implementing"),
                "prechecking",
                "implementation_progress",
            ),
            event(30, Some("publishing"), "ci", "published"),
            event(40, Some("ci"), "reviewing", "ci_passed"),
            event(45, Some("reviewing"), "implementing", "review_revise"),
            event(46, Some("implementing"), "implementing", "fix_reserved"),
            event(
                47,
                Some("implementing"),
                "implementing",
                "implementation_started",
            ),
            event(
                48,
                Some("implementing"),
                "prechecking",
                "implementation_progress",
            ),
            event(50, Some("publishing"), "ci", "published"),
            event(60, Some("ci"), "reviewing", "ci_passed"),
            event(65, Some("reviewing"), "ready", "review_passed"),
            event(
                70,
                Some("classifying"),
                "awaiting_approval",
                "approval_required",
            ),
            event(
                80,
                Some("awaiting_approval"),
                "merging",
                "approval_received",
            ),
            event(90, Some("merging"), "merged", "merged"),
            event(100, Some("merged"), "merged", "cleanup_completed"),
        ];
        for index in [8, 14] {
            events[index].review = Some(
                json!({"findings":[{"path":"src/example.rs","body":" Check  input  at 2026-01-01T00:00:00Z token=fixture-value"}]}),
            );
        }
        events[15].classification = Some(json!({"tier":"high"}));
        (task(json!({"name":"merged"})), events)
    }
    fn decode(inputs: Vec<(Task, Vec<Event>)>, since: Option<DateTime<Utc>>) -> Value {
        serde_json::to_value(aggregate(inputs, &since, Path::new("fixture-workspace")).unwrap())
            .unwrap()
    }

    #[test]
    fn report_math_counts_launches_completed_intervals_and_repeated_findings() {
        let report = decode(vec![sample()], None);
        assert_eq!(report["reportVersion"], 1);
        assert_eq!(report["taskCount"], 1);
        assert_eq!(report["states"]["merged"], 1);
        assert_eq!(report["finalStates"]["merged"], 1);
        assert_eq!(report["fixRelaunches"], 2);
        assert_eq!(report["fixesByCause"]["precheck"], 1);
        assert_eq!(report["fixesByCause"]["review"], 1);
        assert_eq!(report["reviewRounds"], 2);
        assert_eq!(report["tierOutcomes"]["high"]["awaitedApproval"], 1);
        assert_eq!(report["tierOutcomes"]["high"]["autoMerged"], 0);
        assert_eq!(report["ciWait"]["count"], 2);
        assert_eq!(report["ciWait"]["totalMs"], 20000);
        assert_eq!(report["ciWait"]["meanMs"], 10000.0);
        assert_eq!(report["approvalWait"]["totalMs"], 10000);
        assert_eq!(report["timeToMerge"]["count"], 1);
        assert_eq!(report["timeToMerge"]["totalMs"], 90000);
        assert_eq!(report["topFindings"][0]["count"], 2);
        assert_eq!(
            report["topFindings"][0]["bodyHash"].as_str().unwrap().len(),
            64
        );
        assert!(!report["topFindings"][0]["example"]
            .as_str()
            .unwrap()
            .contains("fixture-value"));
        assert_eq!(report["tasks"][0]["ciWait"], report["ciWait"]);
    }

    #[test]
    fn since_filters_observations_and_cohort_but_uses_full_interval_starts() {
        let report = decode(vec![sample()], Some(time(60)));
        assert_eq!(report["taskCount"], 1);
        assert_eq!(report["fixRelaunches"], 0);
        assert_eq!(report["reviewRounds"], 1);
        assert_eq!(report["ciWait"]["count"], 1);
        assert_eq!(report["ciWait"]["totalMs"], 10000);
        assert_eq!(report["timeToMerge"]["totalMs"], 90000);
        assert_eq!(report["topFindings"], json!([]));
        let excluded = decode(vec![sample()], Some(time(101)));
        assert_eq!(excluded["taskCount"], 0);
        assert_eq!(excluded["states"]["merged"], 0);
        assert_eq!(excluded["timeToMerge"]["meanMs"], Value::Null);
    }

    #[test]
    fn failed_precheck_payload_assigns_cause_regardless_of_reason() {
        for reason in [
            "command_failed",
            "timeout",
            "failed",
            "tree_changed",
            "precheck",
            "ci_failed",
            "head_advanced",
            "implementation_progress",
            "head_changed",
            "other",
        ] {
            let mut failed = event(1, Some("prechecking"), "implementing", reason);
            failed.precheck = Some(json!({"passed": false, "reason": reason}));
            let events = vec![
                event(0, Some("ci"), "implementing", "ci_failed"),
                failed,
                event(2, Some("implementing"), "implementing", "fix_reserved"),
                event(
                    3,
                    Some("implementing"),
                    "implementing",
                    "implementation_started",
                ),
            ];
            let report = decode(vec![(task(json!({"name":"implementing"})), events)], None);
            assert_eq!(report["fixRelaunches"], 1, "{reason}");
            assert_eq!(report["fixesByCause"]["precheck"], 1, "{reason}");
            assert_eq!(report["fixesByCause"]["ci"], 0, "{reason}");
            assert_eq!(report["fixesByCause"]["unknown"], 0, "{reason}");
        }
        for passed in [json!(true), Value::Null, json!("false")] {
            let mut observation = event(1, Some("prechecking"), "implementing", "failed");
            observation.precheck = Some(json!({"passed": passed}));
            assert_eq!(fix_cause(&observation), None);
        }
    }

    #[test]
    fn all_fix_causes_and_unknown_are_bound_to_actual_reserved_launches() {
        let mut events = Vec::new();
        for reason in [
            "precheck_feedback",
            "ci_failed",
            "sync_with_base",
            "review_revise",
            "changes_requested",
        ] {
            events.push(event(1, Some("ci"), "implementing", reason));
            events.push(event(
                2,
                Some("implementing"),
                "implementing",
                "fix_reserved",
            ));
            events.push(event(
                3,
                Some("implementing"),
                "implementing",
                "implementation_started",
            ));
            events.push(event(
                4,
                Some("implementing"),
                "prechecking",
                "implementation_progress",
            ));
        }
        events.push(event(
            5,
            Some("implementing"),
            "implementing",
            "fix_reserved",
        ));
        events.push(event(
            6,
            Some("implementing"),
            "implementing",
            "implementation_started",
        ));
        events.push(event(
            7,
            Some("implementing"),
            "implementing",
            "fix_reserved",
        ));
        let report = decode(vec![(task(json!({"name":"implementing"})), events)], None);
        assert_eq!(report["fixRelaunches"], 6);
        for cause in CAUSES {
            assert_eq!(report["fixesByCause"][cause], 1, "{cause}");
        }
    }

    #[test]
    fn outcomes_unknown_legacy_tier_and_block_histogram() {
        let mut auto = event(2, Some("classifying"), "merging", "auto_tier");
        auto.classification = Some(json!({"tier":"medium"}));
        let first = (
            task(json!({"name":"merged"})),
            vec![auto, event(3, Some("merging"), "merged", "merged")],
        );
        let second = (
            task(json!({"name":"awaiting_approval"})),
            vec![event(
                2,
                Some("classifying"),
                "awaiting_approval",
                "approval_required",
            )],
        );
        let blocked = serde_json::from_value(json!({"at":time(3).to_rfc3339(),"from":{"name":"ci"},"to":{"name":"blocked","reason":"budget"},"reason":"budget"})).unwrap();
        let third = (
            task(json!({"name":"blocked","reason":"budget"})),
            vec![blocked],
        );
        let report = decode(vec![first, second, third], None);
        assert_eq!(report["tierOutcomes"]["medium"]["autoMerged"], 1);
        assert_eq!(report["tierOutcomes"]["unknown"]["awaitedApproval"], 1);
        assert_eq!(report["blockedReasons"]["budget"], 1);
        assert_eq!(report["currentBlockedReasons"]["budget"], 1);
    }

    #[test]
    fn open_and_aborted_waits_do_not_invent_completed_approval_samples() {
        let events = vec![
            event(
                1,
                Some("classifying"),
                "awaiting_approval",
                "approval_required",
            ),
            event(
                3,
                Some("awaiting_approval"),
                "implementing",
                "changes_requested",
            ),
            event(4, Some("publishing"), "ci", "published"),
        ];
        let report = decode(vec![(task(json!({"name":"ci"})), events)], None);
        assert_eq!(report["approvalWait"]["count"], 0);
        assert_eq!(report["ciWait"]["count"], 0);
        assert_eq!(report["timeToMerge"]["count"], 0);
    }

    #[test]
    fn findings_are_sorted_bounded_and_examples_are_sanitized_before_truncation() {
        let mut first = event(1, Some("reviewing"), "implementing", "review_revise");
        let findings: Vec<_> = (0..30).map(|index| json!({"path":format!("src/{index:02}.rs"),"body":format!("\u{1b}[31m{} token=fixture-hidden", "word ".repeat(500))})).collect();
        first.review = Some(json!({"findings":findings}));
        let second: Event = serde_json::from_value(serde_json::to_value(&first).unwrap()).unwrap();
        let report = decode(
            vec![(task(json!({"name":"implementing"})), vec![first, second])],
            None,
        );
        assert_eq!(report["topFindings"].as_array().unwrap().len(), 20);
        assert_eq!(report["topFindings"][0]["path"], "src/00.rs");
        for finding in report["topFindings"].as_array().unwrap() {
            let example = finding["example"].as_str().unwrap();
            assert!(example.chars().count() <= EXAMPLE_CHARS);
            assert!(!example.contains("fixture-hidden"));
            assert!(!example.contains('\u{1b}'));
        }
    }

    #[test]
    fn since_validation_rejects_bad_units_dates_zero_and_overflow() {
        for value in [
            "",
            "0s",
            "-1h",
            "2H",
            "2026-02-30",
            "18446744073709551615w",
            "9223372036854775807s",
        ] {
            assert!(since_at(value, time(0)).is_err(), "{value}");
        }
        for (value, expected) in [
            ("1s", time(-1)),
            ("2m", time(-120)),
            ("1h", time(-3600)),
            ("1d", time(-86400)),
            ("1w", time(-604800)),
            ("2026-01-01", time(0)),
            ("2026-01-01T01:00:00+01:00", time(0)),
        ] {
            assert_eq!(since_at(value, time(0)).unwrap(), expected, "{value}");
        }
        assert!(parse_since(None).unwrap().is_none());
    }

    #[test]
    fn reversed_submillisecond_intervals_do_not_become_zero_length_samples() {
        let end = time(0);
        let start = end + chrono::TimeDelta::microseconds(500);
        let mut timing = Timing::default();
        timing.add(start, end, &None);
        assert_eq!(timing.count, 0);
        timing.add(end, start, &None);
        assert_eq!(timing.count, 1);
        assert_eq!(timing.total_ms, 0);
    }

    #[test]
    fn bad_timestamps_fail_instead_of_silently_omitting_evidence() {
        let (task, mut events) = sample();
        events[0].at = "not-a-date".into();
        assert!(aggregate(vec![(task, events)], &None, Path::new("fixture")).is_err());
    }
}
