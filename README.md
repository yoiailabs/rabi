# rabi

rabi (ラビ, from "rabbit") — isolated Git workspaces for parallel coding agents.

> [!WARNING]
> rabi is under active development (v7 beta). During the v7 beta, minor releases (7.x) may contain breaking changes, following the same rule as SemVer 0.x; patch releases (7.x.y) contain fixes only. Breaking changes are listed in [CHANGELOG](CHANGELOG.md).

Give each task an isolated Git workspace (a linked worktree), so coding agents and humans can work in the same repository in parallel. rabi records why each checkout exists, provides machine-readable output, and checks preservation before cleanup. Unpreserved commits and dirty work require explicit permission to discard.

[日本語](README.ja.md) · [Command reference](docs/commands.md) · [JSON contract](docs/json.md)

## Install

Install the latest v7 release. The installer always verifies SHA-256 and needs `curl` and `tar`.

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh
```

Use `sh -s -- --verify-attestation` to also verify artifact provenance (requires `gh`).

Supports Linux and macOS on x64/arm64; use Linux binaries in WSL. Installs to `~/.local/bin` without sudo or shell-profile changes.

Or build from source with Rust 1.89+:

```sh
cargo install --git https://github.com/yoiailabs/rabi --tag v7.0.0 --locked rabi
```

npm `@phantompane/cli` and Homebrew `phantom` are the older TypeScript v6.

Agents and CI can pin with `RABI_VERSION=<TAG>` and the same tag's installer URL. See [installation](docs/installation.md) for pinned installs, older releases, manual verification, options, and updates.

## Quick start

Start in an existing clone with a `main` branch (substitute your base branch). `sync` needs access to any involved remote. This walkthrough makes no code changes, so cleanup can delete the preserved task branch. Repositories with hooks need [trust or an explicit skip](docs/hooks.md).

```sh
cd your-repo
rabi create task/auth --base main --purpose 'implement authentication'
rabi exec task/auth -- git status --short
rabi list
rabi sync task/auth
rabi remove task/auth --dry-run
rabi remove task/auth
```

`exec` runs in the workspace without moving your shell. To enter it yourself, use `cd "$(rabi path task/auth)"` before removal, or load the optional [shell helper and completions](docs/shell.md) for `rabi cd task/auth`.

## Core concepts

- **Workspace:** a task checkout plus metadata such as purpose, base, and PR/issue links. Names can contain `/`. Select by name, `.`, path, `pr:N`, `issue:N`, or a recorded PR/issue URL.
- **Primary checkout:** the repository’s original checkout, which supplies copied files. rabi refuses to remove it; `sync --main` explicitly advances it on the default branch.
- **Records and management:** records live in the Git common directory. rabi also sees unrecorded checkouts; `adopt` records an existing linked checkout, while `forget` removes only records. Unrecorded checkouts outside the configured workspace directory are unmanaged and protected from removal.
- **Preservation:** removal deletes a branch only with preservation evidence, otherwise keeps it with a warning. Dirty files require `--discard-changes`; deleting unpreserved history requires `--delete-unpreserved-branch`. Ignored files are deleted with the workspace, including nested repositories in ignored directories. `gc` plans until `--apply` and skips unsafe candidates.

Read [removal safety and retries](docs/safety.md) and [sync strategies](docs/sync.md) before automating cleanup or integration. There is no ownership tracking, internal locking, or in-use detection; coordinate with other agents.

## Commands

| Command | Purpose |
|---|---|
| `rabi create` | Create from a new/existing branch, PR, issue, or detached commit |
| `rabi adopt` | Record an existing linked checkout |
| `rabi list` | List workspaces and primary checkouts |
| `rabi show` | Inspect intent and observed state |
| `rabi path` | Print an absolute workspace path |
| `rabi set` | Update purpose, base, and PR/issue links |
| `rabi setup` | Copy ignored files and rerun trusted setup hooks |
| `rabi exec` | Run a command in a workspace (requires `--`) |
| `rabi sync` | Fetch and integrate the base; `--main` advances primary |
| `rabi remove` | Retire explicit workspaces with safety checks |
| `rabi forget` | Drop records while keeping files and branches |
| `rabi gc` | Plan cleanup; `--apply` performs it |
| `rabi clone` | Clone into a ghq-compatible layout and register |
| `rabi repo` | Repository registry commands |
| `rabi repo add` | Register a repository |
| `rabi repo list` | List registered and discovered repositories |
| `rabi repo show` | Inspect a repository |
| `rabi repo forget` | Drop a registration |
| `rabi run` | Workflow execution commands |
| `rabi run start` | Run a trusted workflow in a new workspace |
| `rabi run show` | Read progress, PR and stop reason |
| `rabi workflow` | Workflow definition commands |
| `rabi workflow list` | List built-in and repository definitions |
| `rabi workflow show` | Show YAML and resolved agent settings |
| `rabi workflow check` | Validate definitions without executing them |
| `rabi trust` | Hook and workflow execution trust |
| `rabi trust add` | Show hooks/workflows and grant trust without prompting |
| `rabi trust remove` | Revoke trust |
| `rabi trust list` | List trusted Git common directories |
| `rabi config` | User configuration commands |
| `rabi config get` | Read an effective value |
| `rabi config set` | Set a value |
| `rabi config unset` | Remove an override |
| `rabi config list` | List effective values (`--sources` shows origins) |
| `rabi help` | Text help or the complete JSON command contract |
| `rabi completions` | Generate bash/zsh/fish completions |
| `rabi shell-init` | Generate the shell helper for `rabi cd` |
| `rabi --version` | Print the application version (`-v` also works) |

Global options: `-R/--repo REF`, `--json`, `--output text|json`, `--no-input`, `-h/--help`, and `-v/--version`. For `exec`, put rabi options before the selector and the child command after `--`. See [all options and workflows](docs/commands.md).

## For agents

Use `--json` or `--output json` for one schema 2 envelope: `schema`, `command`, `ok`, `data`, `warnings`, and `error`. Parse `data` even on failure because it can contain partial results; ignore unknown fields. Use `rabi help --json` to discover commands, flags, and error/warning codes.

`--no-input` disables rabi prompts. It is also implied by JSON output, nonempty `CI`, `RABI_NO_INPUT` other than empty or `0`, or stdin/stderr without a terminal. Untrusted hooks refuse before changes; `gc` skips them. Standalone `exec` preserves child output and status; `exec`, `completions`, and `shell-init` reject explicit JSON.

| Exit | Meaning |
|---|---|
| 0 | Success |
| 1 | Operational failure |
| 2 | Usage error or ambiguous selector |
| 3 | Target or repository not found |
| 4 | Safety refusal |
| 5 | Sync conflict |
| 6 | Partial persistent changes |

See [JSON objects and per-command data](docs/json.md) and [exit codes and recovery](docs/exit-codes.md).

## Workflow definitions

`rabi workflow list`, `rabi workflow show NAME`, and `rabi workflow check [NAME]` inspect the built-in `default` and `.rabi/workflows/<name>.yaml` without executing them or requiring trust. `-R` uses the selected primary checkout; otherwise definitions come from the invoking checkout. `rabi trust add` lists repository workflows alongside hooks and grants the same repository execution trust. See [workflow definitions](docs/workflows.md).

`rabi run start TASK --name NAME --base REF --check CMD` executes a trusted workflow in the foreground. Agents commit, then a final `draft-pull-request` agent step pushes to origin and opens a draft PR. On push or draft PR failure, the prompt tells the agent to leave an uncommitted, untracked `DRAFT_PULL_REQUEST_FAILED.txt`; the dirty-workspace rule stops the run and `run show` names the file. rabi does not check PR existence; reports stay in the agent logs. Inspect saved results with `rabi run show RUN_ID`. Only Codex runs today. Provider/model overrides apply to all agent steps. See [workflow execution](docs/workflows.md).

```sh
rabi trust add
rabi run start task.md --name fix/example --base origin/main --check 'cargo test --locked'
rabi run show run_<uuid>
```

## Configuration

`rabi config` manages user settings: `workspace-root`, `clone-root`, `discover-ghq`, `sync.strategy`, and `sync.rewrite-published`. `-R/--repo REF` targets a repository by path or registered/discovered name. See [configuration, paths, and repository registrations](docs/configuration.md).

## Hooks and trust

Repository `.rabi.toml` defines setup and pre-remove hooks; `.worktreeinclude` selects ignored files to copy from primary. Review hooks and workflow definitions before granting trust with `rabi trust add`. Trust applies to the Git common-directory path and survives a re-clone at that path until revoked. See [hooks, trust, and setup retries](docs/hooks.md).

## FAQ

**Why not plain `git worktree`?** Git provides the linked checkouts. rabi adds task metadata, repository discovery, preparation, base synchronization, and evidence-based cleanup. Branches and worktrees remain ordinary Git.

**Does it run agents or isolate services?** Workspaces separate files and branches. rabi does not schedule agents, manage terminal sessions, or isolate processes, ports, databases, credentials, or access between agents.

## Status

Beta (see the versioning policy above). Breaking changes are listed in [CHANGELOG](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and checks. Open pull requests against `main`.

## License

[MIT](LICENSE).
