# 🤝 Contributing to rabi

Thank you for your interest in contributing to rabi! This guide will help you get started with development.

## 📋 Table of Contents

- [Development Setup](#development-setup)
- [Development Guidelines](#development-guidelines)
- [Testing](#testing)
- [Code Quality](#code-quality)
- [Pull Request Process](#pull-request-process)
- [Documentation](#documentation)
- [Release Process](#release-process)
- [Additional Resources](#additional-resources)

## 🛠️ Development Setup

### Prerequisites

- Rust 1.99.1 (`rust-toolchain.toml`).
- [mise](https://mise.jdx.dev/) is optional. `mise.toml` only aliases `fmt`, `clippy`, and `test`. It does not install Rust or other tools.

Development happens on `main`. Open pull requests against `main`.

Dependabot does not bump the Rust toolchain. The version is pinned in `rust-toolchain.toml`, in the `toolchain:` input of `.github/workflows/ci.yml` and `release-compile.yml`, and in `RUSTUP_TOOLCHAIN` in `release-compile.yml`. Update all of those together, then run the checks below on the new compiler. `Cargo.toml` `rust-version` is the MSRV (1.89, required by uuid 1.28) and changes only when that floor moves.

### Getting Started

```bash
git clone https://github.com/yoiailabs/rabi.git
cd rabi
cargo run --locked -- --help
```

The built binary is `rabi` (`rabi --help`, `rabi create`). The Cargo package name is `rabi`. Retired interfaces have no compatibility shims before v7 stable; unknown commands, flags, config keys, and hook fields use normal validation.

### Development Workflow

```bash
cargo test --locked
cargo fmt --all
cargo clippy --locked --all-targets -- -D warnings
```

`mise run fmt`, `mise run clippy`, and `mise run test` match the CI checks (`cargo fmt --all -- --check`, Clippy, and `cargo test --locked`).

## 📝 Development Guidelines

### Language Requirements

- **Write files, issues, and pull requests in English**, except for the Japanese README translation
- This ensures the project is accessible to the global community

### Code Style

- Follow existing code conventions and patterns
- Write new code in Rust. Non-test code returns `Result` and does not call `unwrap()`, `expect()`, or `panic!`
- Follow the Single Responsibility Principle
- Keep modules focused and testable

### Architecture Principles

- **Single Responsibility Principle**: Each module has one clear responsibility
- **Separation of Concerns**: CLI, business logic, and git operations are separated
- **Testability**: Core modules are framework-agnostic and easily testable
- **No Code Duplication**: Common operations are centralized
- **Clear Dependencies**: Dependencies flow from CLI → Core (including Git operations)

## 🧪 Testing

### Running Tests

```bash
cargo test --locked
cargo test --locked --test cli
```

### Writing Tests

- Add tests for all new features
- Follow existing test patterns
- Use descriptive test names
- Test both success and error cases

## ✨ Code Quality

### Before Committing

Always run the following command before committing:

```bash
cargo fmt --all
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

### Security Best Practices

- Never introduce code that exposes or logs secrets and keys
- Never commit secrets or keys to the repository
- Be careful with user input validation

## 🚀 Pull Request Process

- Clear description of changes
- Tests for new functionality
- Documentation updates if applicable
- All checks passing (`cargo fmt`, `cargo clippy --locked`, and `cargo test --locked`)
- Follow existing code style

### Review process

This process applies to this repository. Use [REVIEW.md](REVIEW.md) for review output and [risk-tiers.md](risk-tiers.md) for merge decisions; these are development practices, not rabi CLI features.

1. Implement and run the required checks above. For every task, including tasks with no PR, obtain a fresh read-only risk classification from a subagent separate from the implementer and ordinary reviewer. It reads the full proposed diff from a recorded initial merge-base and relevant callers against [risk-tiers.md](risk-tiers.md), judging behavioral and security substance rather than paths or authors, with the changed behavior and evidence in `reason`. Before opening a draft PR against `main`, also obtain a fresh read-only Codex review of the full diff from that merge-base to HEAD. Supply the task, completion criteria, repository guidance, and actual check results.
2. Findings use [Important], [Nit], or [Pre-existing], with file:line evidence and bug class grouping. Respond to every finding: fixed, or declined with a reason. Fix the whole bug class, not just the reported instance. Any unresolved Important means a revise verdict (`判定: 要修正`); a declined Important needs reviewer agreement to be resolved.
3. After fixes, rerun affected checks and review in a fresh read-only session. From round 2, review only the diff since the last reviewed HEAD, with necessary context and all prior findings and responses as input. Keep finding IDs and update their status. If history/base changes invalidate comparison, stop and explicitly restart full-diff review without resetting the round count.
4. Allow at most three review rounds total. If round 3 does not pass, stop and reconsider the approach or splitting the change instead of a fourth round. Do not reset the count to continue the same approach.
5. Any change that actually affects trust, hooks, delete/remove (including gc), or sync behavior also needs a separate fresh read-only review focused on security. Judge this trigger by behavior, not touched paths; when in doubt, obtain the review. Address its findings under the same response and Important rules before merging.
6. After review passes, assess the final committed HEAD in a fresh read-only risk-classification session, including when no PR is requested. A provisional assessment of uncommitted changes is invalidated by material changes; every changed HEAD requires reclassification of the full proposed diff. Include the fixed six-key YAML block from [risk-tiers.md](risk-tiers.md) in the parent report, work log, and PR Risk section when applicable: `tier`, `human_review`, `reason`, `affected_paths`, `uncertainty`, and `assessed_sha`. Record evidence in `reason` or next to the block; `assessed_sha` must be the exact current HEAD.
7. When a PR is requested, open the draft using the four template sections: Summary, Testing, Risk, How to revert. Record commands and observed results, gaps, reviewed HEAD SHA, and review results. Log the PR, reviewed SHAs, rounds-to-pass (or stopped and why), findings, and responses in the work log; put rounds-to-pass in Testing.

Under current policy, low/medium means `human_review: "no"`: the development bot may merge with authorization after required CI is green, Codex review passes, every finding has a response, and no Important remains open. High means `human_review: "yes"`: finish authorized local checks, review, commits, and reporting, then stop and report the need for a human decision. Shuhei must approve the specific HEAD SHA and the bot cannot merge it. Unknown risk is high. Approval covers one SHA. Before merging, follow the current-HEAD checks and guarded squash command in [risk-tiers.md](risk-tiers.md); new commits require updated checks, review, risk assessment, and renewed approval when required.

Changes to risk classification or merge-gate rules are high with `human_review: "yes"` under canonical [risk-tiers.md](risk-tiers.md), whether they relax or tighten the gates.

## 📚 Documentation

When contributing documentation:

- Keep language clear and concise
- Keep `README.md` concise; put detailed reference material in [`docs/`](docs/commands.md)
- Keep `README.ja.md` aligned in sections, commands, flags, and examples; fenced code blocks must be identical
- Verify CLI guidance against `help-data.json`, the implementation, and isolated example runs
- Run `cargo test --locked --test docs` for README consistency
- Check for broken links when moving reference sections

## 🚀 Release Process

### Versioning

During the v7 beta, minor releases (7.x) may contain breaking changes, following the same rule as SemVer 0.x; patch releases (7.x.y) contain fixes only. List breaking changes in [CHANGELOG.md](CHANGELOG.md) under `Changed (Breaking)` in the `Unreleased` section.

To release a new version of rabi:

1. **Open a version pull request against `main`**

   Change the version by hand in a dedicated pull request. Do not bump it in a feature pull request. Edit `version` in `Cargo.toml` and the `rabi` package entry in `Cargo.lock` so they match.

   ```bash
   # After editing Cargo.toml, refresh only the rabi entry in Cargo.lock:
   cargo update -p rabi --precise <version>
   ```

   Title the pull request `chore(version): vX.Y.Z`, for example `chore(version): v7.0.0`. `.github/workflows/tag-on-merge.yml` creates that git tag when the pull request merges into `main`. `ci.yml` and `release-compile.yml` do not bump the version.

2. **Review and merge the version pull request**
   - Confirm `Cargo.toml` and `Cargo.lock` show the same version
   - Obtain Shuhei's approval for the specific HEAD SHA and merge under [risk-tiers.md](risk-tiers.md); release/tag changes are high risk and cannot be merged by the bot
   - The merge tags `vX.Y.Z` on `main`. Publishing a GitHub release is a separate step

3. **Create a GitHub release (builds release binaries)**

   Publish stable 7.x releases without `--prerelease` and mark them Latest with `--latest`, even during the v7 beta. For prereleases, use `--prerelease --latest=false`.

   ```bash
   # Create a stable release with automatically generated notes
   gh release create v<version> \
     --title "rabi v<version>" \
     --generate-notes \
     --target main \
     --latest

   # Example for v7.0.0:
   gh release create v7.0.0 \
     --title "rabi v7.0.0" \
     --generate-notes \
     --target main \
     --latest

   # Example prerelease:
   gh release create v7.1.0-beta.1 \
     --title "rabi v7.1.0-beta.1" \
     --generate-notes \
     --target main \
     --prerelease --latest=false
   ```

   `.github/workflows/release-compile.yml` builds linux/darwin x64/arm64 binaries and uploads `rabi-v<version>-<os>-<arch>.tar.gz` to the release. Each archive contains only the `rabi` binary. `install.sh` supports only `rabi-v<version>-<os>-<arch>.tar.gz` archives. v7.0.0-alpha.10, v7.0.0-alpha.9 and v7.0.0-alpha.8 must each be installed with that release's own `install.sh` at `https://github.com/yoiailabs/rabi/releases/download/<TAG>/install.sh` (replace `<TAG>` with the release tag); v7.0.0-alpha.7 and older have no installer and must be installed manually. npm is not published. Monitor the workflow run in GitHub Actions.

4. **Update release notes for clarity**
   - Review the auto-generated release notes using `gh release view v<version>`
   - Check PR descriptions for important details using `gh pr view <number>`
   - Update the release notes to be more user-friendly:
     - Group changes by category (Features, Bug Fixes, Improvements, Documentation)
     - Add usage examples for new features with code blocks
     - Credit external contributors inline (e.g., "Thanks @username!")
     - Include PR numbers for all changes
     - Include "New Contributors" section with PR numbers

   ````bash
   # Edit the release notes
   gh release edit v<version> --notes "$(cat <<'EOF'
   ## 🚀 What's New in v<version>

   <Brief overview of major changes>

   ### ✨ New Features

   #### Feature Name (#PR) - Thanks @contributor!
   Description and usage example:
   ```bash
   # Example command
   ````

   ### 🛠️ Improvements
   - **Improvement** (#PR) - Description
   - **Another improvement** (#PR) - Description

   ### 🐛 Bug Fixes
   - Fixed issue description (#PR) - Thanks @contributor!

   ### 📚 Documentation
   - Documentation updates (#PR)

   ### 🙏 New Contributors

   Welcome to our new contributors!
   - @username - Contribution description (#PR)

   ***

   **Full Changelog**: https://github.com/yoiailabs/rabi/compare/v<previous>...v<version>
   EOF
   )"

   ```

   ```

## 🙏 Thank You!

Your contributions make rabi better for everyone. If you have questions, feel free to:

- Open an issue for bugs or feature requests
- Start a discussion for general questions
- Ask in pull request comments
