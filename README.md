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
| `rabi run` | Launch or resume the workspace parent session |
| `rabi skill` | Skill discovery |
| `rabi skill list` | List skill names, sources and paths |
| `rabi trust` | Hook and run execution trust |
| `rabi trust add` | Show hooks and grant trust without prompting |
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

Global options (`run` and `skill` use the current checkout and do not accept `-R`): `-R/--repo REF`, `--json`, `--output text|json`, `--no-input`, `-h/--help`, and `-v/--version`. For `exec`, put rabi options before the selector and the child command after `--`. See [all options and examples](docs/commands.md).

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
| 124 | Run timed out |
| 130 | Run interrupted |

See [JSON objects and per-command data](docs/json.md) and [exit codes and recovery](docs/exit-codes.md).

## Agent skill

Install the [rabi usage skill](skills/rabi/SKILL.md) for Codex, Claude Code, and Cursor with Node.js/npm:

```sh
npx skills add yoiailabs/rabi --skill rabi -a codex -a claude-code -a cursor -g -y
npx skills update rabi
```

`-g` installs globally; add `-g` to the update command to explicitly target global scope. Use `-a '*'` instead of the named agents to install to all supported agents. Keep `--skill rabi`: this repository also contains checkout-local contributor skills under `.codex/skills`. The skill teaches CLI usage; install the rabi binary separately as described above.

## Parent sessions and skills

`rabi run --skill rabi-dev "Fix issue #123"` starts one Codex parent session in the current workspace. The parent manages progress, delegates to its own subagents, gathers evidence and decides completion. Run `rabi run` again to resume the same parent; `--fresh --skill VALUE` starts over and retains old records. `--skill` passes a verbatim instruction without resolving files. `--timeout 30m` changes the per-launch limit (default `2h`). Trust is required; create a workspace before running. See [commands](docs/commands.md).

Choose Cursor with `--provider cursor` or Claude Code with `--provider claude` on the first run or with `--fresh`. Resume follows the recorded provider; a different `--provider` requires `--fresh`. Codex remains the default. Cursor uses sandbox mode with automatic approval review; the review service can permit execution outside the sandbox, which rabi reports in the launch audit. Existing `.cursor/sandbox.json` files are preserved; otherwise rabi creates a GitHub network allowlist. Claude Code requires `claude` on PATH and login, uses model `opus` with effort `high`, and refuses to run unless its reported permission mode is `auto`. See [provider details](docs/commands.md#parent-sessions-and-skill-discovery).

Use `rabi run SEL -- [TASK]` to target a workspace without changing directory; add `-R REPO` from outside its repository. A single positional without `--` remains task text. `rabi list` and `rabi show` display the current parent run state; JSON also includes its session id and latest launch log/answer paths.

`rabi skill list` lists direct child skills from checkout and user skill directories, including duplicates. It is independent of `rabi run`.

```sh
rabi skill list
rabi run --skill rabi-dev "Fix issue #123"
rabi run "Address the review findings"
rabi run -R /path/to/repo task/auth -- "Address the review findings"
rabi run --fresh --skill rabi-dev - < task.md
rabi run --fresh --provider cursor --skill rabi-dev "Fix issue #123"
rabi run --fresh --provider claude --skill rabi-dev "Fix issue #123"
```

## Configuration

`rabi config` manages user settings: `workspace-root`, `clone-root`, `discover-ghq`, `sync.strategy`, and `sync.rewrite-published`. `-R/--repo REF` targets a repository by path or registered/discovered name. See [configuration, paths, and repository registrations](docs/configuration.md).

## Hooks and trust

Repository `.rabi.toml` defines setup and pre-remove hooks; `.worktreeinclude` selects ignored files to copy from primary. Review hooks and parent instructions before granting trust with `rabi trust add`. Trust applies to the Git common-directory path and survives a re-clone at that path until revoked. See [hooks, trust, and setup retries](docs/hooks.md).

## FAQ

**Why not plain `git worktree`?** Git provides the linked checkouts. rabi adds task metadata, repository discovery, preparation, base synchronization, and evidence-based cleanup. Branches and worktrees remain ordinary Git.

**Does it run agents or isolate services?** Workspaces separate files and branches. rabi does not schedule agents, manage terminal sessions, or isolate processes, ports, databases, credentials, or access between agents.

## Status

Beta (see the versioning policy above). Breaking changes are listed in [CHANGELOG](CHANGELOG.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and checks. Open pull requests against `main`.

## License

[MIT](LICENSE).
