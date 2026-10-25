# Review guide

This guide applies to the rabi repository. Read AGENTS.md, CLAUDE.md, the task and completion criteria, the diff, and check results. Review behavior and root causes, including guidance made inaccurate by the change.

Prioritize:

- Removal/gc preflight, primary checkout protection, dirty files, submodules, commit preservation, and checks after hooks. Ignored files are intentionally removed with a workspace; that documented behavior alone is not a bug.
- Common-directory trust for hooks and run execution; skill discovery must execute nothing. Check noninteractive refusal and hook-skipping flags.
- Sync publication evidence, rewrite permission, fetch destinations, dry-run guarantees, and conflict recovery.
- CLI output, JSON contracts, exit codes, stream routing, selectors, shell quoting, and partial-failure results.
- Durable records, credentials, argument execution, fresh review context, and read-only enforcement. Documentation must match implemented behavior.

Use these severity tags:

- [Important]: an introduced bug, safety violation, or unmet completion criterion. Any unresolved Important requires revision.
- [Nit]: optional improvement; at most two per review. Omit style-only preferences.
- [Pre-existing]: present at the initial base and outside this change; does not block. Earlier commits in this PR are not pre-existing.

Give every finding an ID, tag, bug class, file:line evidence, triggering conditions, impact, confidence, and fix direction. Group instances of the same bug class and request a fix for the whole class. A filename alone is not evidence. State verification gaps.

Start with actual counts, including unresolved prior Important findings, for example:

```text
0 actionable findings (Important: 0 / Nit: 0 / Pre-existing: 0); nothing to fix.
```

Skip `.git/`, `.turbo/`, and `target/`; check generated artifacts through their source and observable output. Keep Cargo.lock, installer, CI/release changes, and new source paths in scope.

Use a fresh read-only Codex session each round. Round 1 reviews the full diff from the recorded initial merge-base to HEAD. From round 2, review only `git diff PREVIOUS_REVIEW_HEAD HEAD`, plus necessary context and prior findings/responses. Update existing IDs rather than duplicate findings; verify fixes and declined reasons. Every finding needs a response: fixed, or declined with a reason. A declined Important is resolved only when the reviewer accepts the evidence.

Stop after at most three rounds. If round 3 fails, reconsider the approach or split the change instead of starting round 4. If history/base changes invalidate comparison, stop and explicitly reset to a full-diff review; keep the round count. See [the process](CONTRIBUTING.md#review-process) and [risk tiers](risk-tiers.md) for security review and merge gates.

End with exactly one final verdict line in the form the caller requests. This repository's review prompts use `判定: 要修正` (revise) if any Important remains unresolved or evidence is insufficient to judge, otherwise `判定: 問題なし` (pass). Nit and Pre-existing findings alone do not require revision.
