---
name: rabi-dev
description: Manage a development task in the current rabi workspace as the parent for rabi run. Use when asked to implement or fix work in yoiailabs/rabi with implementer and reviewer subagents, command-based verification, and a final completion decision. Create a PR only when the task requests one.
---

# Rabi Development

## Role and References

Act as the progress manager in the current rabi workspace. Spawn implementer and
reviewer subagents, give follow-up instructions, and decide when the task is done.
Gather your own git, test, and applicable GitHub evidence; subagent summaries alone
do not establish completion. rabi launches, stops, locks, and records the parent;
it does not manage the development workflow or decide task completion.

Read these repository sources before planning; follow their details rather than
creating a YAML or file-based workflow:

- [AGENTS.md](../../../AGENTS.md): repository, code, checks, and release rules.
- [README.md](../../../README.md#parent-sessions-and-skills): parent run usage.
- [docs/commands.md](../../../docs/commands.md#parent-sessions-and-skill-discovery): run and skill contracts.
- [REVIEW.md](../../../REVIEW.md): review scope, findings, rounds, and verdicts.
- [CONTRIBUTING.md](../../../CONTRIBUTING.md#review-process): review and PR process.
- [risk-tiers.md](../../../risk-tiers.md): risk classification and security review.

## Workflow

1. Understand the task, completion criteria, existing changes, and risk. Run
   `git status -sb` and inspect relevant code and history. Record the initial
   merge-base with `main` for review. Classify risk using `risk-tiers.md`, including
   affected paths, evidence, and uncertainty; unknown risk is high. Preserve
   unrelated changes and stay in the current workspace.
2. Make a short plan with implementation scope and verification. Spawn an
   implementer subagent with the task, completion criteria, repository guidance,
   and clear file ownership. Follow up until the intended change is concrete.
3. Verify the result yourself with `git diff`, `git diff --staged`, relevant CLI
   output, and tests. Run the repository checks below and inspect actual results.
   Check docs and help against implementation; do not accept invented flags or
   infer that a test passed from a subagent's report.
4. Spawn a fresh read-only reviewer subagent with the task, completion criteria,
   full proposed diff, actual check results, and risk assessment. Require
   `REVIEW.md` output and its repository context, including `CLAUDE.md`. If trust,
   hooks, delete/remove (including gc), or sync is touched, also obtain a separate
   fresh read-only security review under `risk-tiers.md`.
5. Respond to every finding: fixed, or declined with evidence. Resolve every
   Important; a declined Important needs reviewer agreement. Delegate fixes,
   rerun affected checks, and use a fresh read-only reviewer for each round.
   Follow `REVIEW.md` full-diff/delta scope and retain finding IDs and responses.
   Stop after at most three rounds; if round 3 fails, report and reconsider or
   split the approach. A changed history/base requires an explicit full-diff
   reset without resetting the round count.
6. Once verification and review pass, commit only intended changes with a
   Conventional Commit message. Confirm the commit matches the reviewed changes
   and record its SHA. Never bump the version in a feature task. A requested
   version bump uses a dedicated PR against `main`, editing `Cargo.toml` and the
   `rabi` entry in `Cargo.lock`, titled `chore(version): vX.Y.Z`; see AGENTS.md.
7. Only if the task asks for a PR, use the existing `create-pull-request` skill.
   Before opening a draft against `main`, ensure the required fresh read-only
   review covers the committed HEAD using the recorded initial merge-base and
   subsequent review heads, within the same round limit. Follow CONTRIBUTING.md
   for the template, risk, reviewed SHA, findings, responses, and work log.
   Inspect `gh pr view` and `gh pr checks` yourself to report actual PR and CI
   state. Do not infer permission to merge, publish, or release from a PR request.
8. Report files and behavior changed, subagent roles, commit SHA, actual check
   commands and results, review outcome, and remaining gaps. Include PR state
   when applicable. Decide completion against the original criteria, not merely
   a successful parent exit or a subagent's completion message.

## Repository Checks and Constraints

Before committing, run `cargo fmt --all`, then verify formatting and run the CI
checks from `.github/workflows/ci.yml`:

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Use the pinned Rust toolchain in AGENTS.md. Report any unavailable check and its
exact failure rather than claiming success. `cargo fmt --check` is the direct
format check; CI uses the all-workspace form above.

- Write files, issues, and PRs in English; maintain `README.ja.md` as the translation.
- Keep README.md and README.ja.md aligned when changing either; see CONTRIBUTING.md.
- Source CLI help and generated completions from `help-data.json`, per AGENTS.md.
- Outside tests, do not use `unwrap()`, `expect()`, or `panic!`; propagate `Result`.

## Starting and Resuming

From a trusted linked workspace, users start with
`rabi run --skill rabi-dev "task"`. Primary checkouts are refused; repository
trust is required without prompting. See docs/commands.md for trust details.
`rabi run` resumes the same parent; `rabi run "additional instructions"` resumes
with a message. `rabi run --fresh --skill rabi-dev "task"` starts a new session
and retains older records. To add instructions mid-run, stop with Ctrl-C, then
resume with a new message: concurrent launches fail; there is no message queue.
`rabi skill list` discovers skills independently; run injects the skill value
verbatim without resolving or validating its file.

## Stop and Report

Stop the affected work and report evidence plus the decision or access needed
when progress requires an out-of-scope design decision, an unrequested public
API or config-format change, missing secrets or permissions, or repeated failure
without a known cause. Preserve completed work and explain what remains.
