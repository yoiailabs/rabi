use super::*;
use std::os::unix::fs::PermissionsExt;

const TASK: &str = "# Example task\nMake a committed change.";
const FAKE: &str = r#"#!/bin/sh
set -eu
root=$RABI_TEST_ROOT
count=0
if [ -f "$root/calls" ]; then count=$(cat "$root/calls"); fi
count=$((count + 1))
printf '%s' "$count" > "$root/calls"
printf '%s\n' "$@" > "$root/call-$count.args"
answer=
session="thread-$count"
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; answer=$1 ;;
        resume) shift; session=$1; [ "$RABI_TEST_CASE" != resume_failure ] || exit 7 ;;
    esac
    shift
done
prompt=$(cat)
printf '%s\n' "$prompt" > "$root/call-$count.prompt"
# The count and current step must be durable before the agent starts.
gitdir=$(git rev-parse --git-common-dir)
cp "$gitdir"/rabi/runs/run_*/run.json "$root/call-$count.run.json"
if [ "$RABI_TEST_CASE" != missing_id ]; then
    printf '{"type":"thread.started","thread_id":"%s"}\n' "$session"
fi
printf 'diagnostic\n' >&2
[ "$RABI_TEST_CASE" != nonzero ] || exit 9
case "$prompt" in
    DRAFT_PULL_REQUEST*) printf 'Draft PR: https://github.com/test/repo/pull/42\n' > "$answer" ;;
    SUMMARY*) printf 'Final summary.\nVerification gap: integration checks were not run.\n' > "$answer" ;;
    REVIEW*)
        case "$RABI_TEST_CASE" in
            verdict) printf 'No verdict here\n' > "$answer" ;;
            shared_limit) printf 'Review issue.\n判定: 要修正\n' > "$answer" ;;
            review_repair|fresh_fix) if [ ! -f "$root/reviewed" ]; then
                touch "$root/reviewed"
                printf 'Please repair the review issue.\n判定: 要修正\n' > "$answer"
            else printf '判定: 問題なし\n\n' > "$answer"; fi ;;
            *) printf 'Reviewed.\n判定: 問題なし\n\n' > "$answer" ;;
        esac ;;
    *)
        case "$RABI_TEST_CASE" in
            dirty_diff)
                printf 'working-tree-only\n' > tracked.txt
                printf 'index-only\n' > staged.txt
                git add staged.txt
                printf 'untracked-content-not-for-prompt\n' > new-only.txt
                printf 'ignored-content-not-for-prompt\n' > ignored.txt ;;
            dirty|allowed) printf 'uncommitted\n' > loose.txt ;;
            ignored) printf 'ignored\n' > ignored.txt ;;
            *) printf '%s\n' "$count" >> change.txt
               git add change.txt
               git -c user.name=Test -c user.email=test@example.test commit -qm 'feat: example'
               ;;
        esac
        if [ "$RABI_TEST_CASE" = missing_answer ]; then exit 0; fi
        if [ "$RABI_TEST_CASE" = blank_report ]; then : > "$answer"; exit 0; fi
        printf 'Implementation report.\n' > "$answer" ;;
esac
"#;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("rabi-executor-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Each scenario gets a separate process and HOME/PATH. No global environment
// mutation or serialization of unrelated unit tests is needed.
fn scenario(name: &str) {
    let fixture = Fixture::new();
    let bin = fixture.0.join("bin");
    fs::create_dir(&bin).unwrap();
    fs::write(bin.join("codex"), FAKE).unwrap();
    fs::set_permissions(bin.join("codex"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let output = crate::test_git::command(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "executor::tests::worker",
            "--ignored",
            "--nocapture",
        ])
        .env("RABI_TEST_ROOT", &fixture.0)
        .env("RABI_TEST_CASE", name)
        .env("HOME", &fixture.0)
        .env("PATH", path)
        .env("RABI_TRUST_ALL", "1")
        .env("XDG_CONFIG_HOME", fixture.0.join("config"))
        .env("XDG_STATE_HOME", fixture.0.join("state"))
        .env("XDG_DATA_HOME", fixture.0.join("data"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{name}:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn immediate_pass_and_records() {
    scenario("pass");
}
#[test]
fn regression_head_base_keeps_implementation_diff() {
    scenario("base_head");
}
#[test]
fn regression_allowed_dirty_changes_reach_next_agent() {
    scenario("dirty_diff");
}
#[test]
fn regression_fresh_repairs_receive_checks_and_review() {
    scenario("fresh_fix");
}
#[test]
fn definition_commands_execute_without_cli_checks() {
    scenario("list_commands");
}
#[test]
fn empty_report_is_accepted() {
    scenario("blank_report");
}
#[test]
fn failed_check_repairs_in_implementation_session() {
    scenario("repair");
}
#[test]
fn review_repairs_continue_implementation() {
    scenario("review_repair");
}
#[test]
fn repair_loop_is_shared_between_checks_and_reviews() {
    scenario("shared_limit");
}
#[test]
fn two_repairs_then_limit() {
    scenario("limit");
}
#[test]
fn max_steps_does_not_start_next_step() {
    scenario("max_steps");
}
#[test]
fn invalid_review_contract_stops() {
    scenario("verdict");
}
#[test]
fn missing_session_stops() {
    scenario("missing_id");
}
#[test]
fn nonzero_agent_stops() {
    scenario("nonzero");
}
#[test]
fn resume_failure_stops_without_retry() {
    scenario("resume_failure");
}
#[test]
fn missing_answer_stops() {
    scenario("missing_answer");
}
#[test]
fn dirty_agent_stops() {
    scenario("dirty");
}
#[test]
fn allow_dirty_continues() {
    scenario("allowed");
}
#[test]
fn ignored_files_are_clean() {
    scenario("ignored");
}
#[test]
fn draft_pull_request_is_an_ordinary_agent_step() {
    scenario("draft_pull_request");
}

#[test]
fn read_only_final_report_reaches_draft_pull_request_prompt() {
    scenario("draft_pull_request_summary");
}

#[test]
fn read_only_report_without_edit_reaches_draft_pull_request_prompt() {
    scenario("draft_pull_request_summary_only");
}

#[test]
fn unsupported_provider_preflight() {
    scenario("unsupported");
}
#[test]
fn check_commands_validation_preflight() {
    scenario("checks_validation");
}
#[test]
fn workflow_trust_preflight() {
    scenario("untrusted");
}
#[test]
fn prepare_captures_setup_and_stops_on_dirty() {
    scenario("setup_dirty");
}
#[test]
fn prepare_failure_retains_workspace() {
    scenario("setup_failure");
}
#[test]
fn dirty_check_step_stops() {
    scenario("check_dirty");
}
#[test]
fn missing_transition_stops() {
    scenario("missing_transition");
}
#[test]
fn unrun_continue_source_stops() {
    scenario("unrun_continue");
}
#[test]
fn codex_spawn_failure_stops() {
    scenario("spawn_failure");
}
#[test]
fn check_spawn_failure_stops() {
    scenario("check_spawn_failure");
}
#[test]
fn explicit_stopped_terminal() {
    scenario("stopped");
}

fn inspection(yaml: String) -> workflow::Inspection {
    let definition: workflow::Definition = serde_norway::from_str(&yaml).unwrap();
    let diagnostics = definition.validate();
    assert!(diagnostics.is_empty(), "{diagnostics:?}");
    workflow::Inspection {
        entry: workflow::builtin(),
        yaml,
        definition: Some(definition),
        diagnostics,
    }
}

#[test]
#[ignore = "invoked in an isolated process by scenario tests"]
fn worker() {
    let root = PathBuf::from(std::env::var_os("RABI_TEST_ROOT").unwrap());
    let case = std::env::var("RABI_TEST_CASE").unwrap();
    let repo = root.join("repo");
    fs::create_dir(&repo).unwrap();
    git::run(&["init", "--template=", "-b", "main"], Some(&repo)).unwrap();
    crate::test_git::configure_repo(&repo);
    git::run(&["config", "user.name", "Test"], Some(&repo)).unwrap();
    git::run(&["config", "user.email", "test@example.test"], Some(&repo)).unwrap();
    fs::write(repo.join(".gitignore"), "ignored.txt\n").unwrap();
    fs::write(repo.join("tracked.txt"), "original\n").unwrap();
    if case.starts_with("setup_") {
        fs::write(
            repo.join(".rabi.toml"),
            if case == "setup_dirty" {
                "[hooks]\nsetup = ['echo hook-output; echo hook-error >&2; touch hook-file']\n"
            } else {
                "[hooks]\nsetup = ['echo hook-failed >&2; exit 12']\n"
            },
        )
        .unwrap();
    }
    git::run(&["add", "."], Some(&repo)).unwrap();
    git::run(&["commit", "-qm", "chore: initial"], Some(&repo)).unwrap();
    let initial_head = git::head_commit(&repo).unwrap();
    for name in ["REVIEW.md", "AGENTS.md", "CLAUDE.md"] {
        fs::write(repo.join(name), "Review guidance.\n").unwrap();
    }
    git::run(&["add", "."], Some(&repo)).unwrap();
    git::run(&["commit", "-qm", "docs: review guidance"], Some(&repo)).unwrap();
    let initial_head = git::head_commit(&repo).unwrap_or(initial_head);
    let context = crate::context::create_at(&repo).unwrap();
    let yaml = if case.starts_with("draft_pull_request") {
        workflow::DEFAULT_YAML.to_string()
    } else {
        workflow::DEFAULT_YAML.replace("pass: draft-pull-request", "pass: succeeded")
    };
    // Prefix the built-in instructions so the fake can distinguish the role.
    let mut definition: workflow::Definition = serde_norway::from_str(&yaml).unwrap();
    for (id, step) in &mut definition.steps {
        if let StepKind::Agent { instruction, .. } = &mut step.kind {
            *instruction = format!(
                "{}\n{instruction}",
                match id.as_str() {
                    "implement" => "IMPLEMENT",
                    "fix" => "FIX",
                    "draft-pull-request" => "DRAFT_PULL_REQUEST",
                    _ => "REVIEW",
                }
            );
        }
    }
    if case.starts_with("draft_pull_request_summary") {
        let mut summary = definition.steps["implement"].clone();
        if let StepKind::Agent {
            mode, instruction, ..
        } = &mut summary.kind
        {
            *mode = workflow::Mode::ReadOnly;
            *instruction = "SUMMARY".into();
        }
        summary
            .transitions
            .insert("ok".into(), Target::Step("draft-pull-request".into()));
        definition.steps.insert("summary".into(), summary);
        let (previous, result) = if case == "draft_pull_request_summary_only" {
            ("prepare", "ok")
        } else {
            ("review", "pass")
        };
        definition
            .steps
            .get_mut(previous)
            .unwrap()
            .transitions
            .insert(result.into(), Target::Step("summary".into()));
    }
    if matches!(case.as_str(), "allowed" | "dirty_diff") {
        // All later steps see the same permitted dirty file.
        for step in definition.steps.values_mut() {
            step.allow_dirty = true;
        }
    }
    if case == "fresh_fix" {
        if let StepKind::Agent { context, .. } = &mut definition.steps.get_mut("fix").unwrap().kind
        {
            *context = workflow::Context::default();
        }
    }
    if case == "max_steps" {
        definition.limits.max_steps = 2;
    }
    if case == "missing_transition" {
        definition
            .steps
            .get_mut("implement")
            .unwrap()
            .transitions
            .clear();
    }
    if case == "unrun_continue" {
        definition
            .steps
            .get_mut("prepare")
            .unwrap()
            .transitions
            .insert("ok".into(), Target::Step("fix".into()));
    }
    if case == "stopped" {
        definition
            .steps
            .get_mut("prepare")
            .unwrap()
            .transitions
            .insert("ok".into(), Target::Step("stopped".into()));
    }
    let checks: Vec<String> = match case.as_str() {
        "repair" | "resume_failure" | "shared_limit" | "fresh_fix" => vec![
            "test \"$(cat change.txt)\" != 1".into(),
            "echo second-command".into(),
        ],
        "limit" => vec![
            "echo check-failed >&2; exit 3".into(),
            "touch should-not-run".into(),
        ],
        "review_repair" => vec!["test -f change.txt".into()],
        "check_dirty" => vec!["touch check-file".into()],
        _ => vec![
            "echo checked; echo check-diagnostic >&2".into(),
            "echo second-command".into(),
        ],
    };
    if case == "check_spawn_failure" {
        // Successful agent removes only this test's PATH link to sh.
        fs::write(
            root.join("bin/codex"),
            FAKE.replace(
                "printf 'Implementation report.\\n' > \"$answer\"",
                "printf 'Implementation report.\\n' > \"$answer\"; rm \"$RABI_TEST_ROOT/bin/sh\"",
            ),
        )
        .unwrap();
    }
    if case == "checks_validation" {
        let cli = inspection(serde_norway::to_string(&definition).unwrap());
        assert!(execute(
            &context,
            Request {
                workflow: &cli,
                task: TASK,
                checks: &[],
                base: "main",
                name: "topic"
            }
        )
        .is_err());
        if let StepKind::Check { commands } = &mut definition.steps.get_mut("check").unwrap().kind {
            *commands = Commands::List(vec!["true".into()]);
        }
    }
    if case == "unsupported" {
        for provider in [workflow::Provider::Claude, workflow::Provider::Cursor] {
            let mut other = definition.clone();
            other.steps.remove("review");
            other
                .steps
                .get_mut("check")
                .unwrap()
                .transitions
                .insert("pass".into(), Target::Step("succeeded".into()));
            other.agent.as_mut().unwrap().provider = provider;
            let workflow = inspection(serde_norway::to_string(&other).unwrap());
            let error = execute(
                &context,
                Request {
                    workflow: &workflow,
                    task: TASK,
                    checks: &checks,
                    base: "main",
                    name: "topic",
                },
            )
            .unwrap_err();
            assert!(error.to_string().contains("not supported yet"));
        }
        assert!(!context.worktrees_directory.join("topic").exists());
        assert!(!repo.join(".git/rabi/runs").exists());
        return;
    }
    let checks = if case == "list_commands" {
        if let StepKind::Check { commands } = &mut definition.steps.get_mut("check").unwrap().kind {
            *commands = Commands::List(checks);
        }
        Vec::new()
    } else {
        checks
    };
    let workflow = inspection(serde_norway::to_string(&definition).unwrap());
    if case == "untrusted" {
        std::env::remove_var("RABI_TRUST_ALL");
    }
    if case == "spawn_failure" {
        fs::remove_file(root.join("bin/codex")).unwrap();
    }
    if matches!(case.as_str(), "check_spawn_failure" | "spawn_failure") {
        // Constrain PATH after repo setup. Only our sh disappears; Codex and
        // all other utilities needed by the fixture remain callable.
        for tool in ["sh", "git", "cat", "cp", "rm"] {
            std::os::unix::fs::symlink(format!("/usr/bin/{tool}"), root.join("bin").join(tool))
                .unwrap();
        }
        std::env::set_var("PATH", root.join("bin"));
    }
    let result = execute(
        &context,
        Request {
            workflow: &workflow,
            task: TASK,
            checks: &checks,
            base: if case == "base_head" { "HEAD" } else { "main" },
            name: "topic",
        },
    );
    if matches!(case.as_str(), "untrusted" | "checks_validation") {
        assert!(result.is_err());
        assert!(!context.worktrees_directory.join("topic").exists());
        assert!(!repo.join(".git/rabi/runs").exists());
        return;
    }
    let record = result.unwrap();
    let (saved, dir) = run::load(&repo, &record.id).unwrap();
    assert_eq!(saved, record);
    assert!(record.finished_at.is_some());
    assert!(record.current_step.is_none());
    assert_eq!(fs::read_to_string(dir.join("task.md")).unwrap(), TASK);
    assert_eq!(
        fs::read_to_string(dir.join("workflow.yaml")).unwrap(),
        workflow.yaml
    );
    assert_eq!(
        fs::read_to_string(dir.join("worklog.md"))
            .unwrap()
            .lines()
            .count() as u64,
        record.steps_started
    );
    let mut attempts = fs::read_dir(dir.join("steps"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    attempts.sort();
    assert_eq!(attempts.len() as u64, record.steps_started);
    for (i, path) in attempts.iter().enumerate() {
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with(&format!("{:03}-", i + 1)));
        for file in ["stdout.log", "stderr.log", "result.json"] {
            assert!(path.join(file).is_file(), "{path:?}/{file}");
        }
        let attempt: Attempt =
            serde_json::from_slice(&fs::read(path.join("result.json")).unwrap()).unwrap();
        assert_eq!(attempt.attempt as usize, i + 1);
        assert!(attempt.finished_at.is_some());
        if !case.starts_with("setup_") {
            assert!(attempt.head.is_some());
        }
        if attempt.agent.is_some() {
            let prompt = fs::read_to_string(path.join("prompt.md")).unwrap();
            if attempt.step == "implement" || attempt.step == "fix" {
                assert!(prompt.contains("Do not push or create PRs."));
            }
            if attempt.step == "review" {
                assert!(prompt.ends_with("Even when there are no findings, the final line must be exactly 判定: 問題なし or 判定: 要修正.\n"));
            }
            assert!(path.join("prompt.md").is_file());
            assert!(path.join("answer.md").is_file());
            assert!(fs::read_to_string(path.join("prompt.md"))
                .unwrap()
                .contains(TASK));
        }
    }
    let expected = match case.as_str() {
        "pass"
        | "repair"
        | "review_repair"
        | "allowed"
        | "ignored"
        | "list_commands"
        | "blank_report"
        | "base_head"
        | "dirty_diff"
        | "fresh_fix"
        | "draft_pull_request"
        | "draft_pull_request_summary"
        | "draft_pull_request_summary_only" => None,
        "limit" | "max_steps" | "shared_limit" => Some("limit"),
        "verdict" => Some("contract_invalid"),
        "dirty" | "check_dirty" | "setup_dirty" => Some("dirty_workspace"),
        "stopped" => Some("stopped"),
        _ => Some("operation_failed"),
    };
    assert_eq!(
        record
            .stop_reason
            .as_ref()
            .map(|reason| reason.kind.as_str()),
        expected,
        "{record:?}"
    );
    assert_eq!(
        record.state,
        if expected.is_some() {
            run::State::Stopped
        } else {
            run::State::Succeeded
        }
    );
    match case.as_str() {
        "base_head" => {
            assert_eq!(record.base.as_deref(), Some(initial_head.as_str()));
            let implementation = run::load_attempt(&repo, &record.id, "implement", 2).unwrap();
            assert_ne!(implementation.head, record.base);
            let prompt = fs::read_to_string(dir.join("steps/004-review/prompt.md")).unwrap();
            assert!(prompt.contains("diff --git a/change.txt b/change.txt"));
            assert!(prompt.contains("+1"));
            let start: Record =
                serde_json::from_slice(&fs::read(root.join("call-1.run.json")).unwrap()).unwrap();
            assert_eq!(start.base, record.base);
        }
        "dirty_diff" => {
            let prompt = fs::read_to_string(dir.join("steps/004-review/prompt.md")).unwrap();
            assert!(prompt.contains("diff --git a/tracked.txt b/tracked.txt"));
            assert!(prompt.contains("+working-tree-only"));
            assert!(prompt.contains("+index-only"));
            assert!(prompt.contains("# Untracked paths\nnew-only.txt\n"));
            assert!(!prompt.contains("untracked-content-not-for-prompt"));
            assert!(!prompt.contains("ignored.txt"));
            assert!(!prompt.contains("ignored-content-not-for-prompt"));
        }
        "fresh_fix" => {
            assert_eq!(record.loops["repair"].used, 2);
            for (call, number) in [(2, 4), (4, 7)] {
                let args = fs::read_to_string(root.join(format!("call-{call}.args"))).unwrap();
                assert!(!args.lines().any(|arg| arg == "resume"));
                let prompt = fs::read_to_string(root.join(format!("call-{call}.prompt"))).unwrap();
                assert!(prompt.contains("# Latest checks\nCommand:"));
                assert!(prompt.contains(if call == 2 { "Exit: 1" } else { "Exit: 0" }));
                let fix = run::load_attempt(&repo, &record.id, "fix", number).unwrap();
                assert_eq!(fix.agent.unwrap().continued_from, None);
                if call == 4 {
                    assert!(prompt.contains("# Latest review\nPlease repair the review issue."));
                }
            }
            let initial = fs::read_to_string(root.join("call-1.prompt")).unwrap();
            assert!(!initial.contains("# Latest checks"));
            assert!(!initial.contains("# Latest review"));
        }
        "pass" | "ignored" | "allowed" => {
            assert_eq!(record.steps_started, 4);
            assert_eq!(record.loops["repair"].used, 0);
            let implementation = run::load_attempt(&repo, &record.id, "implement", 2).unwrap();
            let agent = implementation.agent.unwrap();
            assert_eq!(agent.session_id.as_deref(), Some("thread-1"));
            assert_eq!(agent.provider, workflow::Provider::Codex);
            assert_eq!(agent.model.as_deref(), Some("gpt-6.1-sol"));
            assert_eq!(agent.effort.as_deref(), Some("high"));
            assert_eq!(agent.continued_from, None);
            let review_args = fs::read_to_string(root.join("call-2.args")).unwrap();
            assert!(!review_args.lines().any(|arg| arg == "resume"));
            assert!(review_args.contains("--sandbox\nread-only\n"));
            assert!(!review_args.lines().any(|arg| arg == "--approve-for-me"));
            let args = fs::read_to_string(root.join("call-1.args")).unwrap();
            assert!(args.lines().any(|arg| arg == "--approve-for-me"));
            assert!(!args.lines().any(|arg| arg == "--sandbox"));
            assert!(!args.lines().any(|arg| arg == "--add-dir"));
            assert!(!review_args.lines().any(|arg| arg == "--add-dir"));
            assert!(args.contains("-m\ngpt-6.1-sol\n-c\nmodel_reasoning_effort=\"high\"\n"));
            assert!(!args.contains("service_tier"));
            let start: Record =
                serde_json::from_slice(&fs::read(root.join("call-1.run.json")).unwrap()).unwrap();
            assert_eq!(start.steps_started, 2);
            assert_eq!(start.current_step.as_deref(), Some("implement"));
            assert_eq!(start.state, run::State::Running);
            let check = run::load_attempt(&repo, &record.id, "check", 3).unwrap();
            assert_eq!(check.checks.len(), 2);
            assert!(check.checks.iter().all(|check| check.exit_code == Some(0)));
            assert_eq!(check.result, "pass");
            assert!(dir.join("steps/003-check/command-002/stdout.log").exists());
            let prompt = fs::read_to_string(dir.join("steps/004-review/prompt.md")).unwrap();
            assert!(prompt.contains("Latest checks\nCommand:"));
            assert!(prompt.contains("check-diagnostic"));
            if case == "pass" {
                assert!(prompt.contains("+1"));
            }
        }
        "repair" | "review_repair" => {
            assert_eq!(record.loops["repair"].used, 1);
            let number = if case == "repair" { 4 } else { 5 };
            let fix = run::load_attempt(&repo, &record.id, "fix", number).unwrap();
            let agent = fix.agent.unwrap();
            assert_eq!(agent.continued_from.as_deref(), Some("implement"));
            assert_eq!(agent.session_id.as_deref(), Some("thread-1"));
            let call = if case == "repair" { 2 } else { 3 };
            let args = fs::read_to_string(root.join(format!("call-{call}.args"))).unwrap();
            assert!(args.contains("resume\nthread-1\n-\n"));
            assert!(args.starts_with("exec\n"));
            assert!(args.find("--approve-for-me").unwrap() < args.find("resume").unwrap());
            assert!(!args.lines().any(|arg| arg == "--sandbox"));
            assert!(!args.lines().any(|arg| arg == "--add-dir"));
            let prompt = fs::read_to_string(root.join(format!("call-{call}.prompt"))).unwrap();
            assert!(prompt.contains("Latest checks"));
            if case == "review_repair" {
                assert!(prompt.contains("Please repair the review issue."));
                let first = run::load_attempt(&repo, &record.id, "review", 4).unwrap();
                let second = run::load_attempt(&repo, &record.id, "review", 7).unwrap();
                let prompt = fs::read_to_string(dir.join("steps/007-review/prompt.md")).unwrap();
                assert!(prompt.contains(&format!(
                    "git diff {} {}",
                    first.head.unwrap(),
                    second.head.unwrap()
                )));
                assert!(prompt.contains("# Review round 2 / 3"));
                assert!(prompt.contains("Please repair the review issue."));
                assert!(prompt.contains("Fix response (fix):\nImplementation report."));
                assert!(!prompt.contains("+1\n"));
                for name in ["REVIEW.md", "AGENTS.md", "CLAUDE.md"] {
                    assert!(prompt.contains(
                        Path::new(&record.workspace.path)
                            .join(name)
                            .to_str()
                            .unwrap()
                    ));
                }
                for tag in ["[Important]", "[Nit]", "[Pre-existing]", "file:line"] {
                    assert!(prompt.contains(tag));
                }
            }
            let check = run::load_attempt(&repo, &record.id, "check", 3).unwrap();
            if case == "repair" {
                assert_eq!(check.checks.len(), 1);
                assert_eq!(check.checks[0].exit_code, Some(1));
            }
        }
        "limit" => {
            assert!(record
                .stop_reason
                .as_ref()
                .unwrap()
                .message
                .contains("Check failure exhausted"));
            assert_eq!(record.loops["repair"].used, 2);
            assert_eq!(record.steps_started, 7);
            assert_eq!(fs::read_to_string(root.join("calls")).unwrap(), "3");
            assert!(!Path::new(&record.workspace.path)
                .join("should-not-run")
                .exists());
            for call in [2, 3] {
                assert!(fs::read_to_string(root.join(format!("call-{call}.args")))
                    .unwrap()
                    .contains("resume\nthread-1"));
            }
        }
        "shared_limit" => {
            assert!(record
                .stop_reason
                .as_ref()
                .unwrap()
                .message
                .contains("Review limit reached"));
            assert!(record
                .stop_reason
                .as_ref()
                .unwrap()
                .message
                .contains("Reconsider the approach"));
            assert_eq!(record.loops["repair"].used, 2);
            assert_eq!(record.steps_started, 9);
            for call in [3, 5] {
                let args = fs::read_to_string(root.join(format!("call-{call}.args"))).unwrap();
                assert!(!args.lines().any(|arg| arg == "resume"));
            }
        }
        "max_steps" => {
            assert_eq!(record.steps_started, 2);
            assert!(!dir.join("steps/003-check").exists());
        }
        "dirty" => assert!(record.stop_reason.unwrap().message.contains("loose.txt")),
        "check_dirty" => assert!(record.stop_reason.unwrap().message.contains("check-file")),
        "setup_dirty" | "setup_failure" => {
            assert_eq!(record.steps_started, 1);
            assert!(record.workspace.id.is_some());
            assert!(Path::new(&record.workspace.path).is_dir());
            let stderr = fs::read_to_string(dir.join("steps/001-prepare/stderr.log")).unwrap();
            assert!(stderr.contains(if case == "setup_dirty" {
                "hook-error"
            } else {
                "hook-failed"
            }));
        }
        "missing_id" | "nonzero" | "missing_answer" | "spawn_failure" => {
            assert_eq!(record.steps_started, 2)
        }
        "resume_failure" => {
            assert_eq!(record.steps_started, 4);
            assert_eq!(fs::read_to_string(root.join("calls")).unwrap(), "2");
        }
        "draft_pull_request" | "draft_pull_request_summary" | "draft_pull_request_summary_only" => {
            // No remote or gh is installed: successful publication is the agent's
            // report, with no rabi publication, lookup, or metadata extraction.
            assert!(record.pr.is_none());
            assert!(!dir.join("pr-body.md").exists());
            assert!(crate::metadata::load(&repo)
                .unwrap()
                .values()
                .all(|m| m.links.pr.is_none()));
            let prompt = fs::read_to_string(attempts.last().unwrap().join("prompt.md")).unwrap();
            for text in [
                "Run branch: topic",
                "Base branch: main",
                "Remote: origin",
                "DRAFT PR",
                "Summary / Testing / Risk / How to revert",
                "without force",
                "risk is not assessed by rabi",
                "report the PR URL",
            ] {
                assert!(prompt.contains(text), "Missing {text}: {prompt}");
            }
            if case.starts_with("draft_pull_request_summary") {
                assert!(prompt.contains(
                    "Final summary.\nVerification gap: integration checks were not run."
                ));
                assert!(!prompt.contains("Implementation report."));
                assert!(!prompt.contains("Fix response (summary)"));
            } else {
                assert!(prompt.contains("Implementation report."));
            }
            if case != "draft_pull_request_summary_only" {
                let review = run::load_attempt(&repo, &record.id, "review", 4).unwrap();
                assert!(prompt.contains(&format!(
                    "verdict 判定: 問題なし, reviewed HEAD {}, rounds 1",
                    review.head.unwrap()
                )));
                assert!(prompt.contains("Exit: 0"));
                assert!(prompt.contains("check-diagnostic"));
            }
            let args = fs::read_to_string(root.join(format!(
                "call-{}.args",
                if case == "draft_pull_request_summary_only" {
                    2
                } else if case == "draft_pull_request_summary" {
                    4
                } else {
                    3
                }
            )))
            .unwrap();
            assert!(args.contains("--approve-for-me"));
            assert!(!args.contains("--sandbox"));
            assert!(!args.contains("resume"));
        }
        "unrun_continue" => {
            assert_eq!(record.steps_started, 2);
            assert!(!root.join("calls").exists());
        }
        "stopped" => assert_eq!(record.steps_started, 1),
        _ => {}
    }
}
