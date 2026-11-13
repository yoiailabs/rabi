# Approval policy

By default, every PR may be approved and merged automatically by automation or `rabi task merge` once required CI is green and review passes for the current HEAD. A maintainer is informed afterwards.

This default includes source code, documentation, policy and gate documents (`risk-tiers.md`, `CONTRIBUTING.md`, `REVIEW.md`, `AGENTS.md`, the PR template, `.rabi/policy.toml`, and `.rabi.toml`), and GitHub-only release changes (version bumps, tags, and GitHub Releases). These are not escalation reasons by themselves.

Read the full proposed diff against its merge-base and relevant context. Escalate to **high** only when the change:

- Can affect pay-as-you-go cost: GitHub Actions workflows, runners, CI configuration, infrastructure, or usage of paid APIs or services.
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

Path rules are only a floor; this assessment decides the tier. An assessment of uncommitted changes is provisional. Assess the final committed HEAD; a new HEAD requires reassessment and updated CI and review. Approval covers one HEAD SHA, so high changes require renewed approval for a new HEAD.

## Merging escalated changes

A maintainer approves the exact HEAD SHA after the gates pass. `rabi task approve` records that approval; automation must not create approval without it. `rabi task merge` then merges with `gh pr merge --squash --match-head-commit <sha>`. Otherwise, squash-merge using that same guarded command after approval. All merges, including low and medium, must use `--match-head-commit` with the verified SHA.
