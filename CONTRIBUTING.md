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

1. Implement and run the required checks above. Classify the full proposed change under [risk-tiers.md](risk-tiers.md), judging behavioral and security impact. Before opening a draft PR against `main`, obtain a read-only review of the full diff against its merge-base, including the task, completion criteria, repository guidance, and actual check results.
2. Findings use [Important], [Nit], or [Pre-existing], with file:line evidence and bug class grouping. Respond to every finding: fixed, or declined with a reason. Fix the whole bug class. Resolve every Important before merging; a declined Important requires reviewer agreement.
3. After fixes, rerun affected checks and have the changes reviewed. Verify prior findings and responses as well as any new issues. Review must cover the current HEAD before merge.
4. Changes that actually affect trust, hooks, delete/remove (including gc), or sync behavior also require a separate read-only security review. Judge this trigger by behavior, not touched paths; when in doubt, obtain it. Address its findings under the same response and Important rules.
5. Assess the final committed HEAD and record the risk assessment using the YAML block in [risk-tiers.md](risk-tiers.md). When a PR is requested, open a draft using the four template sections: Summary, Testing, Risk, How to revert. Include commands and observed results, verification gaps, reviewed HEAD SHA, review results, and the risk assessment.

A maintainer may merge low/medium changes after required CI is green, review passes, every finding has a response, and no Important remains unresolved. High-risk changes require maintainer approval for the specific HEAD SHA before merge; automation (including CI and bots) must not merge them. Unknown risk is high. Approval covers one SHA: new commits require updated checks, review, risk assessment, and renewed approval for high risk. Follow the current-HEAD checks and guarded squash command in [risk-tiers.md](risk-tiers.md).

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

   Title the pull request `chore(version): vX.Y.Z`, for example `chore(version): v7.0.0`. `ci.yml` and `release-compile.yml` do not bump the version. Merging the pull request does not create a tag.

2. **Review and merge the version pull request**
   - Confirm `Cargo.toml` and `Cargo.lock` show the same version
   - Obtain maintainer approval for the specific HEAD SHA and merge under [risk-tiers.md](risk-tiers.md); release/tag changes are high risk and must not be merged by automation
   - Squash-merge the reviewed exact HEAD SHA into `main`; record the full merge commit SHA for the draft target

3. **Create a draft, then dispatch the release build**

   Enable release immutability in Settings → General → Releases → "Enable release immutability". Once published, the assets and tag are locked.

   Stable 7.x releases use `--latest` without `--prerelease`, even during the v7 beta. For prereleases, use `--prerelease --latest=false`. Set these choices on the draft; CI preserves them when publishing.

   ```bash
   # Stable release: use the full 40-hex merge commit SHA, not a branch name.
   gh release create v<version> --draft \
     --target <merge-commit-SHA> \
     --title "rabi v<version>" \
     --generate-notes --latest

   # Prerelease:
   gh release create v7.3.0-beta.1 --draft \
     --target <merge-commit-SHA> \
     --title "rabi v7.3.0-beta.1" \
     --generate-notes --prerelease --latest=false

   # Polish the draft notes (step 4) before dispatching when possible.
   gh workflow run release-compile.yml --ref main -f tag=v<version>
   # Select the dispatched run and watch it:
   gh run list --workflow release-compile.yml
   gh run watch <run-id>
   ```

   Create exactly one release for the tag. A new tag is created from the draft's full target SHA when CI publishes; the SHA must be reachable from `main`. An existing tag is also supported, but it must resolve to the intended release commit and match any full target SHA on the draft. The committed `Cargo.toml` version must match the tag without `v`.

   `.github/workflows/release-compile.yml` builds linux/darwin x64/arm64 binaries; Linux binaries use static musl and are checked on Alpine. CI assembles the four `rabi-v<version>-<os>-<arch>.tar.gz` archives, `install.sh` from the resolved commit, and sorted `SHA256SUMS` covering those five files. It verifies checksums and installs the linux-x64 archive using `install.sh` from a local `file://` mirror, attests all six assets, uploads them to the draft by release ID, verifies the exact asset names, upload states and sizes, then publishes the draft. Each archive contains only the `rabi` binary. `install.sh` supports only `rabi-v<version>-<os>-<arch>.tar.gz` archives. v7.0.0-alpha.10, v7.0.0-alpha.9 and v7.0.0-alpha.8 must each be installed with that release's own `install.sh` at `https://github.com/yoiailabs/rabi/releases/download/<TAG>/install.sh` (replace `<TAG>` with the release tag); v7.0.0-alpha.7 and older have no installer and must be installed manually. npm is not published. PRs to `main` touching `.github/workflows/release-compile.yml`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`, or `install.sh` run a dry run that builds all four targets, assembles and verifies checksums, and smoke-tests the installer without attesting, uploading or publishing; only dry runs may skip a missing `install.sh`.

   If CI fails before publishing, fix the cause and re-dispatch; assets may be replaced while the release is still a draft. Remove any unexpected draft assets manually, or delete the draft. Never try to patch a published release's assets or tag. A deleted immutable release's tag cannot be reused, so ship a new patch version instead.

4. **Polish release notes, preferably before dispatching**

   Edit the draft before dispatching when possible. Title and notes remain editable after publication, as do prerelease and Latest choices.
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
