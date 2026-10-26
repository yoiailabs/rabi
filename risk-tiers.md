# Merge risk tiers

This policy applies to development of the rabi repository. Maintainers apply it; rabi has no risk-classification or merge command.

Classify each change by its behavioral and security impact, never its author or touched paths alone. Read the full proposed diff against its merge-base and relevant callers. When in doubt, classify it as high.

| Tier | human_review | Changed behavior | Merge rule |
|---|---|---|---|
| low | no | Documentation/tests-only changes that do not direct unsafe actions, or other demonstrably behavior-neutral changes; no high-risk condition below. | A maintainer may merge after required CI is green, review passes, every finding has a response, and no Important remains unresolved. |
| medium | no | Bug fixes, internal refactors, or backward-compatible additions that preserve user-facing contracts and security boundaries, including calling an existing stop/process helper on a newly detected error path; no high-risk condition below. | Same gates as low; a maintainer may merge. |
| high | yes | Incompatible user-facing contract changes (CLI output, exit codes, JSON schema, config, selectors, shell integration); actual changes to trust, hooks, delete/remove (including gc), or sync security boundaries; installer, CI, release, toolchain, or dependency changes that can ship bad binaries, change what is published, or rewrite published history; CI or release changes to what runs with credentials, token permissions, secret exposure, or triggers for untrusted code; real data-loss or lost-commits risk; process spawn/kill changes that expand the blast radius beyond existing helpers, such as new kill paths or killing processes the run did not start; changes to the risk classification policy, merge risk tiers, or rules deciding when human review, security review, or automated merging is required, whether relaxing or tightening those gates (for example, `risk-tiers.md` itself or equivalent gate wording in `CONTRIBUTING.md`, `AGENTS.md`, `REVIEW.md`, or `.github/pull_request_template.md`). | A maintainer must approve the specific HEAD SHA before merge. Automation (including CI and bots) must not merge high-risk changes. Required CI and review must pass, every finding must have a response, and no Important may remain unresolved. |

A policy-text change that changes merge gates, risk assessment requirements, or human-review rules must not be classified low or medium solely because it is documentation. Purely editorial edits, such as a typo or link fix, can still be low only when they demonstrably leave those rules and gates unchanged; the assessment must explain why they are unchanged.

Record the assessment in the PR Risk section when a PR exists. Use the following YAML block with all placeholders replaced. In `reason`, identify the changed behavior (or explain why none changes) and supporting evidence. Describe any remaining uncertainty.

```yaml
tier: <low|medium|high>
human_review: "<yes|no>"
reason: "<reason and evidence>"
affected_paths:
  - "<path>"
uncertainty: "<remaining uncertainty or none>"
assessed_sha: "<full git HEAD SHA>"
```

High always means `human_review: "yes"`; low and medium mean `human_review: "no"`. Classification does not grant permission to merge or take other external actions. An assessment of uncommitted changes is provisional; assess the final committed HEAD and record its exact SHA. Each new HEAD requires reassessment of the full proposed diff and renewed maintainer approval for high risk.

Read changes in the following areas carefully. These paths are inspection prompts, not automatic high-risk triggers:

- Risk and merge-gate policy in `risk-tiers.md`, `CONTRIBUTING.md`, `AGENTS.md`, `REVIEW.md`, and `.github/pull_request_template.md`.
- `src/trust.rs`; setup and hooks in `src/setup.rs`, `src/config.rs`, and `.rabi.toml`.
- Process execution in `src/process.rs`, `src/runtime.rs`, `src/run/lifecycle.rs`, and provider code.
- Worktree removal, gc, and preservation in `src/worktree.rs`, `src/gc.rs`, and `src/git.rs`.
- `src/sync.rs` and `src/sync/**`.
- `install.sh`, build scripts (`build.rs`, `build_git.rs`), and `.github/workflows/**`.
- Toolchain, version/release files, and Cargo dependency changes (`rust-toolchain.toml`, `Cargo.toml`, `Cargo.lock`).

Touching `src/run/lifecycle.rs`, `src/process.rs`, or any listed path alone does not force high. State which sensitive behavior newly changes, or why none does. Do not elevate solely because a sensitive file appears in the diff, or lower solely because a diff is small or uses an unlisted path: sensitive behavior moved elsewhere is judged by the same criteria. Documentation that directs unsafe actions is not automatically low.

For example, a fix that detects a session-ID mismatch and calls the existing `Group::stop` to stop the parent is medium when contracts and helper semantics are preserved. Changing `Group::stop` signals or timing for all callers, adding a kill path beyond existing helper semantics, or killing processes the run did not start is high.

Any change that actually affects trust, hooks, delete/remove (including gc), or sync behavior requires a separate fresh read-only security review, focused on security, in addition to the ordinary review. This trigger is behavioral, not path-based; when in doubt, obtain the security review. Address every finding and resolve every Important before merging.

Before merging, confirm required CI, review, any required security review, and the risk assessment cover the current HEAD. Approval covers one HEAD SHA. New commits require updated checks, fresh review, risk assessment, and renewed maintainer approval for high risk. Risk classification does not replace ordinary or security review. Follow the [review guide](REVIEW.md).

Use `gh pr merge --squash --match-head-commit <sha>` with that verified SHA. For high risk, merge only after a maintainer approves that SHA; automation must not merge it.
