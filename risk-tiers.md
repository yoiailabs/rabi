# Approval policy

By default, every PR may be approved and merged automatically by automation once required CI is green and review passes for the current HEAD. A maintainer is informed afterwards.

This default includes source code, documentation, policy and gate documents (`risk-tiers.md`, `CONTRIBUTING.md`, `REVIEW.md`, `AGENTS.md`, the PR template, and `.rabi/policy.toml`), repository hook/env configuration (`.rabi.toml`), and GitHub-only release changes (version bumps, tags, and GitHub Releases). These are not escalation reasons by themselves.

Read the full proposed diff against its merge-base and relevant context. Escalate to **high** only when the change:

- Can affect pay-as-you-go cost paid by the repository or project: GitHub Actions minutes, workflows, runners, CI configuration, infrastructure, or paid APIs or services that the project pays for. Running coding agents or provider sessions (for example Claude Code or Codex, via `rabi run` or `rabi task classify`) on the invoking user's own provider account is not a cost escalation by itself.
- Adds an integration with a new external service, network endpoint, or hosted service dependency. Existing GitHub/git usage is not new.
- May add personal or confidential information: secrets, tokens, credentials, real names, personal emails, local absolute paths, hostnames, or private repository names.
- Publishes to or writes to any external service other than GitHub.

Also escalate to high if the risk assessment or review could not be completed, failed, or is uncertain.

| Tier | human_review | Meaning |
|---|---|---|
| low | no | Not escalated; behavior-neutral changes may use this tier. Merge automatically after the gates pass. |
| medium | no | Not escalated; default for other changes. Merge automatically after the gates pass. |
| high | yes | Escalated; a maintainer must approve the exact HEAD SHA before merge. |

Security issues or bugs found by review or CI must be fixed before merge regardless of tier. Auto-approval never skips them. Respond to every finding and resolve every Important under [REVIEW.md](REVIEW.md).

Record the assessment in the PR Risk section when a PR exists. Keep all six keys, explain the changed behavior and evidence in `reason`, and describe any uncertainty.

```yaml
tier: <low|medium|high>
human_review: "<yes|no>"
reason: "<reason and evidence>"
affected_paths:
  - "<path>"
uncertainty: "<remaining uncertainty or none>"
assessed_sha: "<full git HEAD SHA>"
```

`rabi task merge` merges `.rabi/policy.toml` `merge.auto` tiers without approval and requires a recorded maintainer approval for floors (the built-in `.github/**`, `.rabi/policy.toml`, and `.rabi.toml` floors, plus any listed in the policy). Path tiers are a coarse default; the written assessment decides whether a change is escalated. Until rabi records its own review and assessment, run `rabi task merge --external-review` only after the [REVIEW.md](REVIEW.md) review passes and the written assessment finds no escalation condition, or after a maintainer approved an escalated change. A change under a floor that meets no escalation condition (for example, the PR template or `.rabi/policy.toml`) does not need maintainer approval; merge it with `gh pr merge --squash --match-head-commit <sha>` after the same gates, and say in the assessment why the floor does not escalate it.

An assessment of uncommitted changes is provisional. Assess the final committed HEAD; a new HEAD requires reassessment and updated CI and review. Approval covers one full HEAD SHA, so high changes require renewed approval for a new HEAD.

## Merging escalated changes

A maintainer approves the exact full HEAD SHA after the gates pass and records it with `rabi task approve`, run by an approver outside rabi-launched agents. Automation must not record approval: it must not unset `RABI_AGENT`, run `rabi task approve`, or post a `rabi/approval` status itself. Automation may only act on an approval already recorded for the exact current HEAD SHA, by running `rabi task merge --external-review` from a build of `main` or a release, never from the change being merged. All merges, including low and medium, use `--match-head-commit` with the verified full SHA.
