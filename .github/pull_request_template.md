## Summary

- Problem and resulting behavior:

## Testing

- Commands and observed results:
- Reviewed HEAD SHA, Codex verdict, and rounds-to-pass (or stopped):
- Separate fresh read-only security review result, if required:
- Not verified / remaining gaps:

## Risk

Replace every placeholder with the fresh read-only classifier's assessment of the current committed HEAD; see [risk-tiers.md](https://github.com/yoiailabs/rabi/blob/main/risk-tiers.md). Judge behavioral and security substance, not touched paths; name the changed behavior and supporting evidence in `reason`. Keep all six keys and quote `human_review` as a string.

```yaml
tier: <low|medium|high>
human_review: "<yes|no>"
reason: "<reason and evidence>"
affected_paths:
  - "<path>"
uncertainty: "<remaining uncertainty or none>"
assessed_sha: "<full git HEAD SHA>"
```

- Low/medium requires `human_review: "no"`: the development bot may merge with authorization after required CI is green, Codex review passes, every finding has a response, and no Important remains open.
- High (including unknown risk) requires `human_review: "yes"`: stop and report for a human decision; Shuhei's approval for this specific SHA is required, and the bot must not merge.
- Approval covers one SHA; new HEAD requires reclassification and renewed approval when required.
- If `human_review: "yes"`, approved HEAD SHA and approval reference (or pending):

## How to revert

- Commit or PR to revert and verification after reverting:
- Effects a code revert cannot undo (deleted files, rewritten history, published assets), or none:
