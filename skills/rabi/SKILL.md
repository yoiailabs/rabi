---
name: rabi
description: Use the rabi CLI to manage Git worktrees and isolated checkouts per task when humans or parallel coding agents share a repository. Covers workspace creation, inspection, synchronization, trusted setup, and safe cleanup.
license: MIT
metadata:
  author: yoiailabs
---

# rabi

rabi manages local Git workspaces: ordinary linked worktrees plus task metadata.
Use a separate workspace for each task so agents and humans can edit independently.
Keep the primary checkout clean; do task edits, tests, and commits in a workspace.
Use Git for commits and pushes, and your usual tools for opening or merging PRs.

Workspaces isolate files and branches, not processes, ports, databases, or credentials.
rabi does not track ownership or detect whether another agent is using a workspace.
Choose descriptive names and `--purpose`, retain the workspace name or path, and
coordinate before changing or removing another task's checkout.

## Install and check the CLI

The skill does not install the rabi binary. Follow the
[README installation instructions](https://github.com/yoiailabs/rabi/blob/main/README.md#install)
or the [installation reference](https://github.com/yoiailabs/rabi/blob/main/docs/installation.md).
The release installer needs `curl` and `tar`:

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh
rabi --version
rabi --help
```

Use `rabi help COMMAND` for the installed version's text help, or
`rabi help --json` for its command, flag, exit, error, and warning contract.
Substitute your repository's base branch and real PR/issue numbers in examples.

## Choose a repository

Inside an existing clone, rabi uses that repository without requiring registration.
Register it to select it by name from elsewhere:

```sh
cd your-repo
rabi repo add .
rabi repo list
rabi repo show
```

Alternatively, clone and register in a ghq-compatible directory layout:

```sh
rabi clone owner/repo
```

`rabi clone` accepts a URL, `host/owner/repo`, or `owner/repo`.
Use global `-R/--repo REF` to select a repository by path or an unambiguous
registered/discovered name. For example, from outside the checkout:

```sh
rabi -R /absolute/path/to/your-repo list
```

## Create a task workspace

From the primary checkout, create a new branch with an explicit base and purpose:

```sh
rabi create task/auth --base main --purpose 'implement authentication'
rabi path task/auth
rabi exec task/auth -- git status --short
```

Names may contain `/`. The base defaults to the remote default branch, then the
local default branch. A new-branch name already observed locally or on a remote
refuses with exit 4 (`exists_remote`); rabi does not fetch to check it.

These are independent source examples:

```sh
rabi create --branch origin/existing
rabi create --pr 123
rabi create --issue 456 --base main
rabi create task/config --base main --copy-file '.env.local'
rabi create task/auth --base main --reuse
```

- `--branch` uses an existing local or locally observed remote-tracking branch;
  qualify it as `remote/branch` when needed. It does not fetch.
- `--pr NUMBER|URL` checks out a GitHub PR. `--issue NUMBER|URL` alone creates
  `issues/N` and defaults the purpose to the issue title. Both require authenticated
  `gh`; with `--branch` or `--pr`, `--issue` only adds an issue link.
- `--base`, `--branch`, and `--pr` are mutually exclusive.
- `--copy-file` adds a gitignore pattern and can repeat. Only ignored matching
  files are copied from primary; existing destination files are preserved.
- `--reuse` returns an existing workspace only when its requested source matches.
  It never recopies files or reruns setup, including after failed setup. A mismatch
  still exits 4 (`exists`).

Review the repository's preparation rules before creation; see trust below.

## Locate, inspect, and record intent

```sh
rabi list
rabi list --kind workspace --names
rabi show task/auth --json
rabi set task/auth --purpose 'review authentication' --pr 123 --base main
rabi set task/auth --unset issue
rabi path task/auth
rabi exec task/auth -- git diff --stat
```

`list` includes primary; `--kind workspace` filters task checkouts. `show` reports
recorded intent and observed state, including dirty files and setup state.
`set` updates only the supplied fields. Repeat `--unset` to clear several of
`purpose`, `base`, `pr`, or `issue`. Setting PR/issue links records metadata without
a GitHub lookup. Clearing the base makes sync report `base-unknown` until restored.

Text `path` prints only the absolute path. Check its success before changing
directory; `exec` runs in the workspace without moving your shell. Always put `--`
before the child command, and put rabi options before the selector:

```sh
rabi -R /absolute/path/to/your-repo exec task/auth -- git status --short
```

`exec` preserves child output and exit status and rejects explicit JSON output.
`show`, `path`, `list`, and `exec` do not rewrite workspace metadata.

## Selectors

Workspace selectors are a name, `.` (checkout containing cwd), `pr:N`, `issue:N`,
a recorded PR/issue URL, or an absolute path / path starting with `./` or `../`.
Linked selectors find recorded links; they never create a workspace.
Missing matches exit 3; ambiguous matches exit 2. `pr:N` and `issue:N` are scoped
to the selected repository. `.` with a different `-R` is a usage error (exit 2).

`show`, `path`, `set`, `setup`, and `exec` default to `.`.
`sync` without selectors affects only the current checkout.
`remove` requires explicit selectors. Use names or absolute paths when operating
from primary on another task's workspace.

Full syntax: [command reference](https://github.com/yoiailabs/rabi/blob/main/docs/commands.md).

## Trust, copying, and hooks

The invoking checkout's `.rabi.toml` can define project preparation and teardown:

```toml
[hooks]
setup = ["git status --short"]
pre-remove = ["git diff --quiet"]
```

Replace these example commands with the project's setup and teardown steps.
With `-R`, hook configuration comes from selected primary instead.
Setup runs in the target after copying; pre-remove runs there before removal.
Hooks must tolerate reruns. `.worktreeinclude` selects ignored files to copy
from primary, and `--copy-file` adds patterns for an invocation.

Review hook commands before granting execution trust:

```sh
rabi trust add
rabi trust list
```

`trust add` displays hooks and grants trust without asking. Trust applies to the
Git common-directory path, covers all linked workspaces, and survives re-cloning
at the same path until `rabi trust remove` revokes it.

JSON and noninteractive commands do not prompt. Untrusted setup/removal hooks
refuse with exit 4 (`untrusted`) before changes. `gc` skips untrusted hooks.
To create without any copying or hooks, use `--no-setup`:

```sh
rabi create task/manual --base main --no-setup
rabi setup task/manual --no-hooks --copy-file '.env.local'
```

`--no-setup` conflicts with `--copy-file`. Later `setup` can prepare the workspace;
`setup --no-hooks` copies only. `remove --no-hooks` and `gc --no-hooks` skip teardown
without granting trust. If create exits 6 after setup fails, retain the workspace
and use the returned setup retry command after fixing the cause.

Details: [hooks, trust, and setup retries](https://github.com/yoiailabs/rabi/blob/main/docs/hooks.md).

## Sync with the base

```sh
rabi sync task/auth --dry-run --json
rabi sync task/auth
rabi sync --main
```

Sync fetches involved remotes and integrates the recorded base. It never pushes,
changes upstream settings, or runs hooks. Even `--dry-run` fetches, but it leaves
checkouts, indexes, local branches, and records unchanged.

Default auto strategy rebases unpublished work and merges published work;
user `sync.strategy` or `--auto`, `--rebase`, or `--merge` can select the strategy.
A published rebase refuses with exit 4 (`published`) unless rewrite permission
is enabled. `--rewrite-published` grants permission without selecting rebase;
use it only when rewriting that published history is intended.

`--main` fast-forwards primary only while it is on the default branch. From
primary, plain `sync` acts as `--main`; from a workspace, it syncs only that workspace.
`--all` selects every workspace; add `--main` to include primary. Coordinate this
broad selection with other users. `--all` cannot combine with explicit selectors.

Conflicts exit 5 and roll back unless `--keep-conflicts` was requested for exactly
one workspace target. Exit 6 means another selected target changed before failure,
conflict, or refusal. Inspect per-item results before retrying.

Details: [sync strategies and recovery](https://github.com/yoiailabs/rabi/blob/main/docs/sync.md).

## Remove and collect workspaces

Return to primary before retiring a task checkout, inspect its state, and plan:

```sh
rabi show task/auth --json
rabi remove task/auth --dry-run --json
rabi remove task/auth
rabi gc --json
```

Remove refuses primary. It deletes a named branch only with preservation evidence:
integration into its recorded base (default fallback when absent), exact upstream
tip agreement, or verified merged-PR evidence. Otherwise it keeps the branch with
a warning. `--keep-branch` always retains the branch while removing the checkout.

Modified/staged tracked files and non-ignored untracked files require explicit
`--discard-changes` permission. Populated submodules or retained submodule data
also need that permission, even when clean; review contents and unpushed commits.
Ignored files are deleted with the workspace without a dirty warning, including
nested repositories inside ignored directories. Unpreserved detached history is
protected; `--delete-unpreserved-branch` allows losing unpreserved history.
These are separate permissions: do not infer either from permission for the other.

`gc` only plans until `--apply`. Its default criteria are integrated work, closed
PRs, and orphaned state; it skips protected, dirty, or unpreserved candidates.
Inspect the plan and coordinate before `rabi gc --apply`. Age filters only narrow
evidence-based candidates. `rabi forget SEL` drops metadata while retaining files,
branches, and Git registration if that is the intended outcome instead.

After partial remove (exit 6), inspect results and fix the cause, then use
`error.details.retryCommand` (also shown in text) in bash or zsh. Repeating the
original command may exit 3 because completed targets have already gone.

Details: [removal safety and retries](https://github.com/yoiailabs/rabi/blob/main/docs/safety.md).

## Machine-readable results and recovery

Use `--json` or `--output json` on JSON-capable commands. Each emits one schema 2
envelope on stdout with `schema`, `command`, `ok`, `data`, `warnings`, and `error`.
Diagnostics go to stderr. Ignore unknown fields; inspect `data` even on failure
because create, remove, sync, and other commands can retain partial results.
Check warning codes as well as `ok`; a warning alone does not mean failure.

`--no-input` disables prompts; JSON, CI, and non-terminal stdin/stderr also disable
them. `exec`, `completions`, and `shell-init` reject explicit JSON output.

| Exit | Meaning |
|---|---|
| 0 | Success, including gc plans and protected skips |
| 1 | Operational failure |
| 2 | Usage error or ambiguous selector |
| 3 | Target/repository not found or outside a repository |
| 4 | Safety refusal, such as dirty, published, or untrusted |
| 5 | Sync conflict |
| 6 | Persistent changes followed by failure or partial success |
| 124 | Run timeout |
| 130 | Run interrupted |

Standalone `exec` returns its child's status instead of an envelope.
Read [JSON objects and partial results](https://github.com/yoiailabs/rabi/blob/main/docs/json.md)
and [exit codes and recovery](https://github.com/yoiailabs/rabi/blob/main/docs/exit-codes.md)
when automating error handling.

`rabi run` optionally launches or resumes a trusted Codex session in the current
workspace; use `rabi run SEL -- [TASK | -]` to select a workspace without cd and
`-R REPO` with SEL to select its repository. Without `--`, a single positional
remains task text. `list` and `show` expose the current parent run state, session
id, and latest launch paths; inspection never changes run files or locks.
A first/fresh run requires `--skill VALUE`. `rabi skill list` lists
checkout and user skill directories independently of run. See the command
reference for those optional features.
