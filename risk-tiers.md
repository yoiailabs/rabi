# Merge risk tiers

This policy applies to development of the rabi repository. It is operated by maintainers and the development bot; rabi has no risk-classification or merge command.

Every task requires a fresh read-only risk-classifier subagent, separate from the implementer and ordinary reviewer, even when no PR is requested. Classify the substance of the change: its behavioral and security impact, never its author or touched paths alone. Read the full proposed diff from the recorded initial merge-base, including intended uncommitted changes, and relevant callers against this policy. Unknown risk is high.

| Tier | human_review | Changed behavior | Merge rule |
|---|---|---|---|
| low | no | Documentation/tests-only changes that do not direct unsafe actions, or other demonstrably behavior-neutral changes; no high-risk condition below. | The development bot may merge after required CI is green, Codex review passes, every finding has a response, and no Important remains open. |
| medium | no | Bug fixes, internal refactors, or backward-compatible additions that preserve user-facing contracts and security boundaries, including calling an existing stop/process helper on a newly detected error path; no high-risk condition below. | Same gates as low; the development bot may merge. |
| high | yes | Incompatible user-facing contract changes (CLI output, exit codes, JSON schema, config, selectors, shell integration); actual changes to trust, hooks, delete/remove (including gc), or sync security boundaries; installer, CI, release, toolchain, or dependency changes that can ship bad binaries, change what is published, or rewrite published history; CI or release changes to what runs with credentials, token permissions, secret exposure, or triggers for untrusted code; real data-loss or lost-commits risk; process spawn/kill changes that expand the blast radius beyond existing helpers, such as new kill paths or killing processes the run did not start. | Shuhei must approve the specific HEAD SHA. No bot merge. Required CI and Codex review must pass, every finding must have a response, and no Important may remain open. |

Record the assessment in the parent's report, work log, and PR Risk section (when a PR exists) using this fixed YAML block. Replace every placeholder, keep all six keys, use a path list, and quote `human_review` as a string. In `reason`, name the behavior that changes (or explain why none does) and the evidence supporting the tier; add details immediately after the block if needed. Describe uncertainty explicitly, using `"none"` only when justified.

```yaml
tier: <low|medium|high>
human_review: "<yes|no>"
reason: "<reason and evidence>"
affected_paths:
  - "<path>"
uncertainty: "<remaining uncertainty or none>"
assessed_sha: "<full git HEAD SHA>"
```

Under current policy, high always means `human_review: "yes"`; low and medium always mean `human_review: "no"`. For yes, finish authorized local verification, review, commits, and reporting, then stop and report the human decision needed for that SHA. Do not merge. For no, the bot may merge subject to the existing gates and authorization; classification does not grant permission to merge or take other external actions.

An assessment of uncommitted changes is provisional and material changes invalidate it. Every new HEAD requires fresh read-only classification of the full proposed diff. Assess the final committed HEAD and record its exact SHA before reporting completion, including tasks with no PR.

Read changes in the following areas carefully. These paths are inspection prompts, not automatic high-risk triggers:

- `src/trust.rs`; setup and hooks in `src/setup.rs`, `src/config.rs`, and `.rabi.toml`.
- Process execution in `src/process.rs`, `src/runtime.rs`, `src/run/lifecycle.rs`, and provider code.
- Worktree removal, gc, and preservation in `src/worktree.rs`, `src/gc.rs`, and `src/git.rs`.
- `src/sync.rs` and `src/sync/**`.
- `install.sh`, build scripts (`build.rs`, `build_git.rs`), and `.github/workflows/**`.
- Toolchain, version/release files, and Cargo dependency changes (`rust-toolchain.toml`, `Cargo.toml`, `Cargo.lock`).

Touching `src/run/lifecycle.rs`, `src/process.rs`, or any listed path alone does not force high. State which sensitive behavior newly changes, or why none does. Do not elevate solely because a sensitive file appears in the diff, or lower solely because a diff is small or uses an unlisted path: sensitive behavior moved elsewhere is judged by the same criteria. Documentation that directs unsafe actions is not automatically low.

For example, a #916-shaped fix that detects a session-ID mismatch and calls the existing `Group::stop` to stop the parent is medium when contracts and helper semantics are preserved. Changing `Group::stop` signals or timing for all callers, adding a kill path beyond existing helper semantics, or killing processes the run did not start is high.

Any change that actually affects trust, hooks, delete/remove (including gc), or sync behavior requires a separate fresh read-only security review, focused on security, in addition to the ordinary Codex review. This trigger is behavioral, not path-based; when in doubt, obtain the security review. Address every finding and resolve every Important before merging.

Before merging, confirm required CI, Codex review, any required security review, and the risk assessment cover the current HEAD. Approval covers one HEAD SHA. New commits require updated checks, fresh review, risk assessment, and renewed Shuhei approval for high risk. Risk classification does not replace ordinary or security review. Follow [re-review rules](REVIEW.md) without resetting the round count.

The permitted merger uses `gh pr merge --squash --match-head-commit <sha>` with that verified SHA. For high risk, a human merges after Shuhei approves that SHA; the development bot does not merge it.
