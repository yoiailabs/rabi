# Changelog

## Unreleased

### Added

- Run trusted workflows in the foreground with `rabi run start` and inspect saved progress with `rabi run show`. Agents commit, push and open a draft PR through ordinary agent steps. Review prompts include incremental rounds and prior responses.

- Inspect and validate built-in and repository YAML workflow definitions with `rabi workflow list/show/check`. `rabi trust add` lists repository workflows alongside hooks under the same execution trust.

### Changed

- Rename the default and security-review workflows’ final agent step to `draft-pull-request`; it still pushes only the run branch and opens a draft PR.

- Remove workflow system actions `push` and `draft-pr` (unknown actions now fail validation). Default workflow version 7 ends with a fresh `draft-pull-request` agent edit step. rabi passes publication facts and leaves PR body creation, pushing and PR creation to the agent, without verification or URL extraction/recording. Existing run schema 1 remains readable.

- Codex workflow edit steps, including continued sessions, run in auto mode (`--approve-for-me`) so agents can commit in linked workspaces. Read-only steps keep `--sandbox read-only`.

### Fixed

- Instruct the default and security-review `draft-pull-request` agents to leave an uncommitted, untracked `DRAFT_PULL_REQUEST_FAILED.txt` on push or draft PR creation failure, stopping the run through the existing dirty-workspace rule without rabi-side PR checks.

- End every review prompt with the mandatory exact verdict-line instruction, including when no findings are reported.

- Display multiline run stop reasons with indented continuation lines, preserving control-character escaping and the original JSON message.

## v7.0.0 (2026-10-24)

### Changed

- v7.0.0 is the first v7 release published as a stable (non-prerelease) GitHub release and becomes Latest. The unpinned installer now installs v7.0.0 without `--prerelease`.
- v7 is in beta: rabi is under active development. During the v7 beta, minor releases (7.x) may contain breaking changes, like semver 0.x; patch releases contain fixes only. Breaking changes are listed in this changelog.
- No functional changes since v7.0.0-alpha.11. v7 is a Rust rewrite and is not compatible with v6. The npm package `@phantompane/cli` and Homebrew `phantom` remain on v6.

## v7.0.0-alpha.11 (2026-10-24)

### Changed

- Move the repository to `yoiailabs/rabi`; old repository URLs redirect to the new location. `install.sh` now verifies attestations against `yoiailabs/rabi`. Attestations for v7.0.0-alpha.10 and earlier remain under the old organization; verify them manually with `gh attestation verify <file> --owner phantompane`.

## v7.0.0-alpha.10 (2026-10-24)

### Changed (Breaking)

- Rename the product, command and Cargo package from mabo to **rabi** (ラビ, from "rabbit"). Install from source with `cargo install --git https://github.com/phantompane/phantom --tag <TAG> --locked rabi`, using a tag from the first release containing rabi or later.
- Read and write only rabi paths: user config at `~/.config/rabi` (`$XDG_CONFIG_HOME/rabi`), state at `$XDG_STATE_HOME/rabi` (default `~/.local/state/rabi`), new workspace directories at `$XDG_DATA_HOME/rabi/workspaces` (default `~/.local/share/rabi/workspaces`), and workspace records at `<git-common-dir>/rabi` (normally `.git/rabi`). Repository config is `.rabi.toml`; environment variables are `RABI_*` and the child-process marker is `RABI=1`. Project directory markers and clone staging prefixes are `.rabi-common-dir` and `.rabi-clone-`.
- Release assets are `rabi-v<version>-<os>-<arch>.tar.gz`, containing only `rabi`. `install.sh` accepts only `rabi-` archives. Install v7.0.0-alpha.9 or v7.0.0-alpha.8 with that release's own installer at `https://github.com/phantompane/phantom/releases/download/<TAG>/install.sh`; v7.0.0-alpha.7 and older have no installer.
- No aliases, fallbacks or automatic migration: mabo data, config and environment variables are ignored. Alpha users must move their own configuration and records or recreate them. For default paths, with no existing destination, for example:

  ```sh
  mv ~/.config/mabo ~/.config/rabi
  mv ~/.local/state/mabo ~/.local/state/rabi
  # In each primary checkout (use the Git common directory for linked checkouts):
  mv .git/mabo .git/rabi
  mv .mabo.toml .rabi.toml
  ```

  Adjust paths for XDG overrides. Existing checkout paths saved in records can stay in place. If reusing a project workspace directory in the new default data layout, rename its `.mabo-common-dir` to `.rabi-common-dir`. To relocate checkouts, use `git worktree move`, drop stale records with `rabi forget OLD_PATH`, then record new paths with `rabi adopt NEW_PATH`; moving directories alone leaves Git registrations and saved absolute paths stale. Update scripts, hook commands and shell integration to use rabi and `RABI_*`.

## v7.0.0-alpha.9 (2026-10-24)

### Changed

- The product is now named mabo, and the Cargo package is `mabo`. Install from source with `cargo install --git https://github.com/phantompane/phantom --tag <TAG> --locked mabo`.
- **Breaking:** `install.sh` supports only `mabo-v<version>-<os>-<arch>.tar.gz` archives. v7.0.0-alpha.8 must be installed with its own `install.sh` at `https://github.com/phantompane/phantom/releases/download/v7.0.0-alpha.8/install.sh`; v7.0.0-alpha.7 and older have no installer and must be installed manually. Pinned installs require exactly one valid checksum entry for the expected archive; unpinned installs consider only releases with the `mabo-` archive for the platform.

### Removed

- Legacy storage compatibility was removed: data, state, config and metadata live only under `mabo/` paths, and legacy `maboroshi/` directories, `worktrees.json` / `projects.json` stores and old worktree layouts are no longer read or migrated.
- Compatibility shims for retired commands, flags, config keys, hook keys and environment variables were removed; retired commands, flags and keys now fail as ordinary unknown input, and retired environment variables receive no special handling.

## v7.0.0-alpha.8 (2026-10-22)

### Added

- `install.sh` installs and updates `mabo` with an exact version pin. Unpinned installs select stable v7 releases; prereleases require a pin or explicit opt-in. The installer always verifies SHA-256, optionally verifies GitHub artifact attestations, and replaces the binary atomically without sudo, prompts, or shell profile edits. It requires `curl`, and every network call has a time limit.
- Release assets now include `SHA256SUMS` and `install.sh`, with GitHub artifact attestations for the archives, checksum file, and installer.

### Changed

- Linux release binaries are now static musl builds, supporting Alpine and older glibc distributions; previously they required glibc 2.34+. Asset names are unchanged.

## v7.0.0-alpha.7 (2026-10-22)

### Added

- `remove` and `gc --merged` now accept GitHub merged PR evidence after local preservation checks fail. Linked PRs require a verified repository and a merged head equal to or containing the local tip; ancestry uses existing objects only. Partial clones (`extensions.partialClone` configured or any remote with `promisor=true`) require an exact tip match and skip PR object and ancestry probes on every Git version. Unlinked branches require exactly one merged PR with the same head name and exact tip. Bounded, non-prompting `gh` queries are cached per command; uncertainty keeps the local verdict. Plans use the same checks, and item `preservationReason` plus text details identify merged PR proof. GitHub Enterprise hosts on plain HTTP or a non-default port cannot prove a merged PR yet; their branches are kept.

### Breaking changes

This release redesigns the Rust `mabo` CLI around workspaces. Update scripts written for alpha.6 before upgrading. The installed binary remains `mabo`.

- Creation is unified under `create --branch`, `--pr`, `--issue`, and `--detach`. The old `attach`, `github`/`gh`, and create-time `--exec` interfaces are removed. `create --pr` and `--issue` now resolve GitHub sources using authenticated `gh`.
- Use `show`, `path`, and `set` for inspection and intent; use `repo`, `config`, and `trust add|remove|list` for the outer commands. `-R/--repo` replaces `-p/--project`. Removed commands and flags exit 2 with migration hints rather than acting as aliases.
- `remove` replaces `delete`. Working-file loss (`--discard-changes`) and deleting unpreserved history (`--delete-unpreserved-branch`) are separate permissions. Branches are deleted only when preservation is proven; otherwise they stay with a warning. Ignored files are clean and are deleted with the checkout, including nested Git repositories inside ignored directories. Populated submodules or retained submodule data still require `--discard-changes`.
- `sync` selects the current workspace by default. Advancing primary is explicit with `--main`; `--all` selects every workspace. Auto rebases unpublished work and merges published work; `--rewrite-published` permits a published rebase without selecting that strategy. Sync checks fetch destinations before any fetch or integration, including in dry runs.
- Machine output uses the schema 2 envelope and the shared exit table below. Workspace keys and exported environment variables use workspace vocabulary. Ownership, `lastUsedAt`, internal locks, and `MABO_OPERATION_LOCK` are removed.
- Hooks are `[hooks] setup` and `pre-remove`. Copy sources are always the primary checkout. `create --no-setup` skips copying and hooks; rerun preparation with `setup`. State is imported on the first write, and new workspace directories use `workspaces/`; existing directories stay in place.

### Migration from alpha.6

`B` marks an incompatible change. “Migrated” describes automatic state/config migration; “new” is additive. Unchanged flags listed under a renamed command still require changing the command name. See [README.md](README.md) for current examples and the full JSON contract.

| Old (alpha.6) | New | Breaking |
|---|---|---|
| `create [name]` | `create [NAME]` | B: flags change |
| `create --base REF` | `create --base REF` | — |
| `create --owner X` / `attach --owner` / `gh checkout --owner` | Removed (record intent with `--purpose`) | B |
| `create --purpose/--pr/--issue` | `--purpose` still records intent; `--pr` selects the PR head; `--issue` looks up the issue, deriving `issues/N` and its title when used alone | B: PR/issue flags now resolve GitHub sources through `gh` |
| `create --copy-file P` | `create --copy-file P` (passed through to setup) | — |
| `create --no-hooks` | `create --no-setup` | B |
| `create -x/--exec CMD` | `create NAME …` then `exec NAME -- CMD` (name the new workspace; a bare `exec -- CMD` runs in the current checkout) | B (removed) |
| `create --json` | `--json` (envelope schema 2) | B |
| `attach B` / `attach remote/B` | `create --branch B` / `create --branch remote/B` | B |
| `github checkout N` (PR) / `gh checkout N` | `create --pr N` | B |
| `github checkout N` (issue) | `create --issue N` | B |
| `github checkout N --base X` | `create --issue N --base X` | B |
| `list` | `list` (primary checkout included, `kind`) | B (output) |
| `list --no-default` | `list --kind workspace` | B |
| `list --names` | `list --names` | — |
| `list --owner X` | Removed | B |
| (none) | `list --all-repos`, `list --paths`, `list --kind` | new |
| `where NAME` | `path NAME` | B |
| `where NAME --json` | `show NAME --json` | B |
| `delete N…` | `remove N…` | B |
| `delete -f` | `remove --discard-changes` and/or `--delete-unpreserved-branch` | B |
| `delete --current` | `remove .` | B |
| `delete --keep-branch` | `remove --keep-branch` | B: command renamed; flag retained |
| `delete --no-hooks` | `remove --no-hooks` | B: command renamed; flag retained |
| `sync` (in the main checkout) | `sync` (same as `--main`) | — |
| `sync` (in a workspace: also ff'd the default branch) | `sync` (that workspace only); `sync . --main` advances the primary too | B |
| `sync N…` | Same | — |
| `sync --mine` | Removed (`sync` in the workspace, or name it) | B |
| `sync --all` (ownership-based scope) | `sync --all` (every workspace) | B |
| `sync --no-default` | (default) | B |
| `sync --rebase / --merge` | Same (+ `--auto`) | — |
| `sync --rebase --force` | `sync --rebase --rewrite-published` | B |
| `sync --keep-conflicts` | Same | — |
| `sync --json` (`schema:1`) | Envelope schema 2 | B |
| `gc` | `gc` (plan) | — |
| `gc --apply` | `gc --apply` | — |
| `gc -f/--force` | `remove … --discard-changes` per target | B (removed) |
| `gc --merged / --orphaned` | Same | — |
| `gc --pr` | `gc --closed-pr` | B |
| `gc --older-than D` (lastUsedAt, a criterion) | `gc --older-than D` (createdAt, a narrowing filter) | B (meaning) |
| `gc --owner X` | Removed | B |
| `gc --keep-branch` / `gc --no-hooks` | Same | — |
| `trust [path]` / `trust --yes` | `trust add [REPO]` | B |
| `trust --list` | `trust list` | B |
| `untrust [path]` | `trust remove [REPO]` | B |
| `exec NAME CMD…` | `exec NAME -- CMD…` (`--` required, NAME optional) | B |
| `meta set N --owner X` | Removed | B |
| `meta set N --purpose/--pr/--issue/--base` | `set N …` | B |
| `preferences get/set/remove K` | `config get/set/unset K` | B |
| config keys `worktrees-directory` / `root` / `ghq-discovery` | `workspace-root` / `clone-root` / `discover-ghq` | Migrated: old file keys warn and still work; rewritten on the next config write; new names win |
| config keys `keep-branch` / `directory-name-separator` | Ignored with warnings and removed on the next config write; use the `--keep-branch` flag for retirement | B |
| `version` command / `-v` | `--version` / `-v` (only the command is dropped) | B |
| `clone …` | Same | output only |
| `project add/list/remove` | `repo add/list/forget` (+ `repo show`) | B |
| `project list --json` (v2) | `repo list --json` (envelope) | B |
| `-p/--project` | `-R/--repo` | B |
| env `MABO_NAME` / `MABO_PATH` / `MABO_ROOT_PATH` | `MABO_WORKSPACE` / `MABO_WORKSPACE_PATH` / `MABO_REPO_PATH` | B |
| env `MABO_AGENT_ID` | Removed (no longer sets an owner or non-interactive mode; agents use `--no-input` / `MABO_NO_INPUT=1`, or run without a stdin/stderr TTY) | B |
| env `MABO_TRUST_ALL` / `CI` | Same | — |
| (none) | `MABO_OUTPUT`, `MABO_NO_INPUT`, `MABO_WORKSPACE_ID` | new |
| `.mabo.toml [hooks] post-create` / `pre-delete` | `[hooks] setup` / `pre-remove` (the old keys give an error with a rename hint) | B |
| `.worktreeinclude` (copied from where the command runs) | Same file (copied from the primary checkout) | B (source) |
| `worktrees.json` v1 | `workspaces/<id>.json` schema 2 (imported automatically) | Migrated |
| `$XDG_DATA_HOME/maboroshi/worktrees/` | `…/workspaces/` for new workspaces only | — |
| Exit codes 2 = not found, 3 = validation/unpushed, 4 = not a repo | 0 success, 1 operational failure, 2 usage/ambiguous, 3 not found/not a repo, 4 safety refusal, 5 conflict, 6 partial | B |
| Repo operation lock (infinite wait), metadata lock, registry lock, `MABO_OPERATION_LOCK` | Removed; atomic per-record writes (same-record concurrent updates use the last write) | B (behavior) |

### Additional behavior and commands

- `show [SEL]`, `path [SEL]`, `set [SEL]`, `setup [SEL]`, and `exec [SEL] -- COMMAND` default to `.`. Selectors also accept absolute paths or paths starting with `./` or `../`, plus recorded `pr:N`, `issue:N`, and PR/issue URLs. Ambiguous selectors exit 2; missing selectors exit 3. Inspection and exec do not rewrite or import records.
- `setup` copies ignored matches without overwriting target files, then runs trusted setup hooks. Setup state is shared across workspace results: pending, running, ok, failed, skipped, unknown, or observed interrupted. Failed create preparation leaves the checkout and exits 6 with a retry hint preserving the source checkout and copy patterns. `--reuse` never retries setup. `setup/remove --source-checkout PATH` preserves hook configuration when retrying from another checkout of the same repository.
- `adopt PATH` records an existing linked checkout without copying or hooks, with unknown creation details. `forget SEL...` drops only records and preserves checkout files, branches, and Git registrations.
- `remove --dry-run` is read-only and runs no hooks. All selectors are preflighted: one unknown selector aborts the entire removal with exit 3 `not_found`, including dry runs. A completed removal drops its record, so repeating the original command can exit 3. `alreadyGone:true` requires a record and/or an eligible Git registration; a branch alone is insufficient.
- A partial removal exits 6 and returns one exact `error.details.retryCommand` (also printed as “Retry with”). Fix the cause and run that command in bash or zsh: it selects only unfinished paths and preserves permission flags, output mode, and hook source. A branch deletion failure after checkout removal keeps the branch with `branchKeptReason:"delete_failed"`, a warning, and Git's error in `detail`; this alone exits 0 and is not retried.
- Branch preservation requires an independent common-directory ref. Squash/rebase checks compare complete tree-entry transitions (paths, modes, full before/after object IDs) against non-merge commits on the base first-parent history, with distinct evidence in order for rebased commits. Textual patch IDs, a retiring branch's own ref, detached HEAD, and bare commit IDs are insufficient. Uncertain history stays.
- `gc` plans by default and requires `--apply` to mutate. With no criteria, it enables merged, closed-PR, and orphaned checks; age only narrows by `createdAt` and excludes unknown creation times. Dirty/unpreserved candidates are skipped with targeted remove hints. Ignored files follow the same clean rule as remove/list/show. Repeating `gc --apply` is safe.
- `sync --dry-run` fetches involved remotes, but changes no checkout, index, local branch, or workspace record. Unknown future commit IDs are null. Combined plans use the tips planned for primary and earlier workspaces; unknown planned bases propagate a dependency explanation. JSON is `{plan,items,main}`; `integrated` replaces `merged`, and items expose strategy and rewrite permission sources separately.
- Before fetching, sync refuses positive fetch refspecs writing outside `refs/remotes/`, including explicit tag destinations and mirrors (exit 4 `refused`). Missing/empty destinations are allowed; negative exclusions do not override the refusal. Visible symbolic refs under `refs/remotes/` or `refs/tags/` pointing outside `refs/remotes/` are also refused. Dangling symbolic refs omitted by Git enumeration are a known detection limit. Normal Git pruning, automatic tag following, and configured remote HEAD behavior remain in effect.
- Publication checks include observed remote-tracking refs, configured fetch destinations, and upstreams, including independent local refs and destinations of remotes not fetched by this sync. Personal `sync.strategy` and `sync.rewrite-published` belong in user config, never `.mabo.toml`.
- `completions bash|zsh|fish` generates commands and flags from `help-data.json`, querying `command mabo list --names` for workspace selectors. `shell-init bash|zsh|fish` defines only the `mabo cd [SEL] [-R REF]` helper. Load completions separately; zsh requires `compinit`. Both commands emit raw scripts, ignore `MABO_OUTPUT`, and reject explicit JSON or missing/unknown shells with exit 2.
- `help --json` exposes the complete help contract under `data`, including `schemaVersion:2`, exit codes and error codes. `--version --json` keeps the application version in `data.version`. `exec` keeps the child's output, stdin and exit status; script-producing commands and exec ignore `MABO_OUTPUT` and reject explicit JSON.

### Exit codes

| Exit | Meaning |
|---|---|
| 0 | Success (including gc plans with protected candidates) |
| 1 | Operational failure |
| 2 | Usage error or ambiguous selector |
| 3 | Target or repository context not found |
| 4 | Safety refusal |
| 5 | Sync conflict (restored by default; retained with `--keep-conflicts`) |
| 6 | Partial completion after persistent changes |

The JSON envelope remains `{schema:2,command,ok,data,warnings,error}`. Failures retain partial data; `error` has `code`, `message`, and optional `details`. Standalone exec preserves child statuses.
