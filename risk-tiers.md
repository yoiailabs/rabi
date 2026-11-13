# Approval policy

By default, every PR may be approved and merged automatically by automation once required CI is green and review passes for the current HEAD. A maintainer is informed afterwards.

This default includes source code, documentation, policy and gate documents (`risk-tiers.md`, `CONTRIBUTING.md`, `REVIEW.md`, `AGENTS.md`, the PR template, and `.rabi/policy.toml`), repository hook/env configuration (`.rabi.toml`), and GitHub-only release changes (version bumps, tags, and GitHub Releases). These are not escalation reasons by themselves.

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

For this repository, tool path tiers and floors (including the built-in `.github/**`, `.rabi/policy.toml`, and `.rabi.toml` floors) are advisory hints until merge tooling enforces this policy. The written assessment sets the tier and must explain in `reason` why it is lower than a tool floor when applicable.

An assessment of uncommitted changes is provisional. Assess the final committed HEAD; a new HEAD requires reassessment and updated CI and review. Approval covers one full HEAD SHA, so high changes require renewed approval for a new HEAD.

## Merging escalated changes

A maintainer approves the exact full HEAD SHA after the gates pass. Until `rabi task merge` is available, merge after that approval with `gh pr merge --squash --match-head-commit <sha>`. All merges, including low and medium, use this command with the verified full SHA.

Once `rabi task approve` and a non-dry-run `rabi task merge` are available, they record maintainer approval and enforce these gates. Automation may record or act on maintainer approval only when a maintainer approved that exact full HEAD SHA: never on its own initiative, for a different SHA, or after HEAD changed following approval. Run `rabi task merge` from a build of `main` or a release, never from the change being merged.
