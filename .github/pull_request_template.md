## Summary

- Problem and resulting behavior:

## Testing

- Commands and observed results:
- Reviewed HEAD SHA and review result:
- Separate read-only security review result, if performed:
- Not verified / remaining gaps:

## Risk

Replace every placeholder with the risk assessment of the current committed HEAD; see [risk-tiers.md](https://github.com/yoiailabs/rabi/blob/main/risk-tiers.md). Judge behavioral and security substance, not touched paths; name the changed behavior and supporting evidence in `reason`. Keep all six keys and quote `human_review` as a string.

```yaml
tier: <low|medium|high>
human_review: "<yes|no>"
reason: "<reason and evidence>"
affected_paths:
  - "<path>"
uncertainty: "<remaining uncertainty or none>"
assessed_sha: "<full git HEAD SHA>"
```

- Low/medium requires `human_review: "no"`: automation may approve and merge after required CI is green, review passes, every finding has a response, and no Important remains unresolved. A maintainer is informed afterwards.
- High requires `human_review: "yes"`: a maintainer must approve this exact full HEAD SHA for the escalation conditions in [risk-tiers.md](https://github.com/yoiailabs/rabi/blob/main/risk-tiers.md), including failed, incomplete, or uncertain assessment or review. Until `rabi task merge` is available, merge after that approval with `gh pr merge --squash --match-head-commit <sha>`.
- Once `rabi task approve` and a non-dry-run `rabi task merge` are available, they record maintainer approval and enforce these gates. Automation may record or act on maintainer approval only when a maintainer approved that exact full HEAD SHA: never on its own initiative, for a different SHA, or after HEAD changed following approval. Run `rabi task merge` from a build of `main` or a release, never from the change being merged.
- Security issues and bugs must be fixed before merge at every tier. All merges use `gh pr merge --squash --match-head-commit <sha>` with the verified full SHA.
- Approval covers one SHA; a new HEAD requires reassessment and renewed approval for high risk.

## How to revert

- Commit or PR to revert and verification after reverting:
- Effects a code revert cannot undo (deleted files, rewritten history, published assets), or none:
