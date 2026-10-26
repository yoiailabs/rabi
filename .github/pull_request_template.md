## Summary

- Problem and resulting behavior:

## Testing

- Commands and observed results:
- Reviewed HEAD SHA and review result:
- Separate fresh read-only security review result, if required:
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

- Low/medium requires `human_review: "no"`: a maintainer may merge after required CI is green, review passes, every finding has a response, and no Important remains unresolved.
- High (including unknown risk) requires `human_review: "yes"`: maintainer approval of this exact HEAD SHA is required before merge; automation must not merge it. Changes to risk classification or merge-gate rules are high under [risk-tiers.md](https://github.com/yoiailabs/rabi/blob/main/risk-tiers.md), whether they relax or tighten the gates.
- Approval covers one SHA; a new HEAD requires reassessment and renewed approval for high risk.

## How to revert

- Commit or PR to revert and verification after reverting:
- Effects a code revert cannot undo (deleted files, rewritten history, published assets), or none:
