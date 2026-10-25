# Merge risk tiers

This policy applies to development of the rabi repository. It is operated by maintainers and the development bot; rabi has no risk-classification or merge command.

Classify the change, never the author. Read the full PR diff and relevant callers in a fresh read-only session. Record the tier, reason, affected paths, evidence, uncertainty, and assessed HEAD SHA in the work log and PR Risk section. Unknown risk is high.

| Tier | Changed behavior | Merge rule |
|---|---|---|
| low | Demonstrably preserves existing behavior and security boundaries, such as accurate documentation or isolated tests; no high-risk condition below. | The development bot may merge after required CI is green, Codex review passes, every finding has a response, and no Important remains open. |
| medium | Broader internal work, an isolated addition, or a backward-compatible change to existing behavior (for example a bug fix or added output that keeps existing contracts); security boundaries preserved; no high-risk condition below. | Same gates as low; the development bot may merge. |
| high | Breaks existing behavior: an incompatible change to CLI output, exit codes, JSON schema, config, selectors, or shell integration that existing users or scripts rely on; security-sensitive changes (trust, hooks, delete/remove, sync, installer, CI/release config); release/tag/publish; possible data loss, lost commits, or published-history rewrite. | Shuhei must approve the specific HEAD SHA. No bot merge. Required CI and Codex review must pass, every finding must have a response, and no Important may remain open. |

The following paths are high unless the diff demonstrably does not touch the sensitive behavior. Record evidence for any exception; paths are prompts to inspect behavior, not a complete classifier:

- `src/trust.rs`; setup and hooks in `src/setup.rs`, `src/config.rs`, and `.rabi.toml`.
- Process execution in `src/process.rs`, `src/runtime.rs`, `src/run/lifecycle.rs`, and provider code.
- Worktree removal, gc, and preservation in `src/worktree.rs`, `src/gc.rs`, and `src/git.rs`.
- `src/sync.rs` and `src/sync/**`.
- `install.sh`, build scripts (`build.rs`, `build_git.rs`), and `.github/workflows/**`.
- Toolchain, version/release files, and Cargo dependency changes (`rust-toolchain.toml`, `Cargo.toml`, `Cargo.lock`).

Sensitive behavior moved elsewhere is still high. Run code (`src/run*`) is not automatically medium or high: classify its content using the same criteria. Documentation that directs unsafe actions is not automatically low.

Any change touching trust, hooks, delete/remove (including gc), or sync requires a separate fresh read-only security review, focused on security, in addition to the ordinary Codex review. Address every finding and resolve every Important before merging.

Before merging, confirm required CI, Codex review, any required security review, and the risk assessment cover the current HEAD. Approval covers one HEAD SHA. New commits require updated checks, fresh review, risk assessment, and renewed Shuhei approval for high risk. Follow [re-review rules](REVIEW.md) without resetting the round count.

The permitted merger uses `gh pr merge --squash --match-head-commit <sha>` with that verified SHA. For high risk, a human merges after Shuhei approves that SHA; the development bot does not merge it.
