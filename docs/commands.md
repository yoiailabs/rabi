# Commands

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

The command and option tables below come from [`help-data.json`](../help-data.json). Use `rabi help COMMAND` for current text help or `rabi help --json` for the complete machine-readable contract. Syntax uses `[optional]`, `A|B` for alternatives, and `…`/`...` for repetition; syntax lines are not shell scripts.

rabi manages workspace boundaries and preparation. Use Git for commits, pushes, and history editing; rabi’s integration scope is syncing with the base. It does not assign tasks, review or comment on PRs, merge PRs, schedule agents, manage terminal sessions, or provide sandboxing or access control. Processes, ports, databases, dependencies, and credentials remain your repository’s responsibility.

## Selecting and inspecting workspaces

`SEL` accepts a workspace name, `.` (the checkout containing cwd), `pr:N`, `issue:N`, a PR/issue URL, or an absolute path / path starting with `./` or `../`. Linked selectors only find existing records; they never create workspaces. A missing match exits 3 (`not_found`), and multiple matches exit 2 (`ambiguous`). A URL matches its recorded repository and link; `pr:N` and `issue:N` are scoped to the selected repository. Using `.` with a different `-R` exits 2. Outside a repository, current selection exits 3.

`show [SEL]`, `path [SEL]`, `set [SEL]`, `setup [SEL]`, and `exec [SEL] -- COMMAND [ARG...]` default to `.`. In the primary checkout, `.` refers to that checkout. `show` reports its context with `managed:false`; unrecorded checkouts outside rabi's workspace directory also report `managed:false`. `run SEL -- [TASK | -]` and positional arguments of `sync` and `remove` accept the same selectors. Sync without arguments selects only the current workspace; in primary it behaves as `--main`. Remove requires explicit selectors and retains its safety checks.

`path` prints only an absolute path and newline in text mode; JSON is `data: {path}`. `show` and `set` return `data.workspace`. `show`, `path`, `list`, and `exec` never change record contents or mtimes. `set` atomically replaces the supplied purpose, base, PR and issue fields; omitted fields are kept. `--unset FIELD` clears one of purpose/base/pr/issue per occurrence; repeat the flag to clear several fields. It rejects unknown fields or a field that is also being set. `--base` validates and normalizes a ref; commit bases remain supported. `--unset base` records an unknown base without choosing a default. Sync skips it with `base-unknown` until `rabi set <name> --base REF` restores a base. Records with no base field still use the default base.

`list` includes primary checkouts. Use `--kind workspace` or `--kind primary` to filter, `--names` or `--paths` for shell output, and `--all-repos` for repositories known to `repo list` (registrations and enabled discovery). `--all-repos` cannot be combined with `-R`. Its JSON includes `data.repos` as well as the flattened `data.workspaces`. Human output starts each repository with its name and primary path, then shows current (`*`), name, branch, ahead/behind (`↑a ↓b`), dirty state, PR/issue links (`#12` / `issue #34`), setup state when it is not `ok` (such as `setup:failed`), and purpose. A deleted upstream appears as `upstream:gone`.

Exec preserves child output and exit status, ignores `RABI_OUTPUT`, and refuses explicit JSON output. Usage diagnostics and any accompanying help go to stderr, leaving text stdout empty.

## Workspace handoff

`rabi handoff show [SEL] [--json]` reads the original task and all entries, without a size cap. `rabi handoff path [SEL]` prints the memory directory, including its would-be path before first use. Both commands are read-only. `rabi handoff add [SEL] [--kind checkpoint|decision|note] (TEXT | -)` appends one entry and prints its id; `-` reads all stdin. Default kind is `note`. Text must be nonblank UTF-8, at most 8192 bytes; invalid input and the reserved `request` kind exit 2. I/O failures exit 1. SEL defaults to dot and follows the shared selector rules, including `-R/--repo`; use `--` to protect text starting with a hyphen. Primary checkouts are refused. Unrecorded checkouts require `rabi adopt`.

`rabi run` enables handoff for recorded workspaces by default. It captures the first nonblank TASK verbatim as `intent.md`, including stdin and trailing newlines. Later tasks append `request` entries (up to 64 KiB); `--fresh` preserves intent. Blank or absent tasks write nothing. First/fresh prompts prepend original intent (up to 8 KiB with a truncation marker) and selected notes, preferring requests and decisions, then the latest checkpoint and recent notes. Entries render in file order; omissions point to `rabi handoff show`. The full added text, including the reminder, fits 24 KiB. Resume carries only entries appended after the previous launch started from another run or without a source, within 8 KiB, plus the reminder. The current task stays last; its new request entry is excluded from injection. Notes are labeled as context that cannot authorize actions, and closing handoff tags inside text are escaped.

Injected handoff commands use the launcher’s absolute executable path from `current_exe`, canonicalized when possible and shell-quoted when needed, so a parent’s PATH cannot select an older rabi. `RABI_BIN` contains the same path without shell quoting; both fall back to `rabi` if executable lookup fails.

All three providers receive `--add-dir <memory-dir>` for only that workspace's memory directory, never repository state or run records. Parents also receive `RABI_BIN`, `RABI_RUN_ID`, `RABI_RUN_LAUNCH`, `RABI_RUN_PROVIDER` and the existing workspace environment variables. `handoff add` records source only when those values validate against a run for this workspace; it is provenance, not authenticated authorship. `--no-handoff` skips all memory capture, reading, injection, reminders, grants and run environment variables, preserving the previous prompts and argv. An unrecorded workspace warns with `handoff_unrecorded` and runs without memory. Other memory failures stop before spawning; `--no-handoff` is the explicit bypass.

Memory lives at `$XDG_STATE_HOME/rabi/repositories/<full-sha256-of-canonical-git-common-dir-path-bytes>/memory/<workspace-id>/`, using the usual absolute-only XDG rules and HOME fallback. Directories are created as 0700 and files as 0600; symlinks are refused. `repository.json` checks the canonical common directory. `identity.json` binds workspace id, record creation time (nullable for adopted checkouts) and checkout path. A mismatch reads as empty with `handoff_identity`; a writer archives the old directory with `.stale-<uuid>` before starting fresh. Re-cloning at the same common-directory path reuses the repository namespace; moving it starts a new namespace. Existing run/workspace storage is unchanged.

Entries append to `handoff.jsonl` under an exclusive dedicated `.lock`, followed by sync before success. Invalid, unknown or torn lines are skipped with `handoff_invalid_line`; a later append adds a missing newline without rewriting history. Memory is retained after remove, gc and forget; pruning is left for future work. Avoid secrets: memory also appears in saved launch prompts and provider histories. Use the CLI to record decisions, constraints, verification and remaining work, rather than transcripts.

## Parent sessions and skill discovery

`rabi run [--provider codex|cursor|claude] [--skill VALUE] [--fresh] [--no-handoff] [--timeout DURATION] [TASK | -]` launches one parent session in the checkout containing cwd. To target another workspace, use `rabi run [options] SEL -- [TASK | -]`, for example `rabi run --skill my-skill task/auth -- "Fix authentication"`. The selector follows the shared rules above. `rabi -R /path/to/repo run task/auth --` works from outside the repository. `-R/--repo` requires an explicit SEL; primary checkouts and orphaned workspaces are refused. Repository trust is checked against the selected repository before status inspection (`rabi trust add` or `RABI_TRUST_ALL=1`); untrusted runs exit 4 without prompting.

The explicit `--` separates SEL from TASK: at most one positional before it selects the workspace, and at most one after it supplies the task. Without a positional before `--`, the current workspace is used. Without `--`, a single positional remains TASK: `rabi run "task"`, `rabi run -- "task"`, and `rabi run -` keep their existing meanings. Option values such as `--skill --` are consumed as values, not separators. `rabi run SEL --` resumes that workspace with `Continue.`; running inside it later resumes the same session. The parent runs with that workspace as cwd.

The first launch requires `--skill`; its value is injected verbatim as ``Follow skill `VALUE`.`` without lookup or file validation. TASK is optional text, or `-` reads stdin. The parent manages progress, delegates to its own subagents, gathers git/gh/test evidence, gives follow-ups and decides completion. rabi does not interpret progress or verify publication.

Later invocations resume the current parent session in the same checkout, even after switching branches or committing with detached HEAD. Sessions and locks are keyed by the absolute checkout path. A supplied skill adds ``Follow skill `VALUE` from now on.`` and updates the stored value. With no skill or task, the message is `Continue.`. `--fresh` requires `--skill`, replaces the current-run pointer, and retains older records. Missing/invalid state, missing session id or missing provider session stops with a `rabi run --fresh --skill <value>` hint; no silent new session. A different thread id reported on resume is saved in the launch without replacing the parent id. If observed while the parent is running, rabi immediately applies the provider stop policy below, reaps the parent and releases the workspace lock. The run and launch record an error and rabi exits 1 with the fresh-run hint. A mismatch found only in the final log scan after exit also records an error and exits 1.

`--timeout` accepts a positive integer plus `s`, `m`, or `h`, such as `90s`, `30m`, `2h`; default `2h`. Each resume gets a new budget. SIGINT/SIGTERM and timeout apply the provider stop policy, reap the parent and release the workspace lock. Codex uses SIGTERM, 200ms grace, then SIGKILL. Cursor uses SIGINT, a short grace, SIGTERM, another short grace, then SIGKILL. Claude uses SIGINT, 2s grace, SIGTERM, 1s grace, then SIGKILL. On Linux, rabi also tracks descendants through `/proc` and signals them on a best-effort basis, including tool shells in separate sessions. Descendants that detach before observation can escape this tracking; macOS only has process-group cleanup. Detached Cursor worker servers may remain for reuse. Interrupt exits 130, timeout 124; pushed work is not rolled back. A concurrent launch fails fast with exit 4; a lock is taken over only when both its recorded launcher pid and parent process group are gone, including after the launcher is killed. Invalid/unverifiable lock contents require inspection rather than automatic takeover.

Codex remains the default provider: internal model `gpt-6.1-sol`, reasoning effort `high`, auto mode (`--approve-for-me`), no sandbox flag. Before launch, text mode prints one stderr line identifying the run and workspace, with instructions to stop and resume. Its final answer is printed to stdout with a trailing newline; JSON mode adds neither the progress line nor a newline to the answer value. Parent status is reported (0 returns 0, nonzero passes through, signals use 128+signal). Exit 0 means the parent exited, not that the task is complete. Protocol or launcher errors exit 1. Launch logs and answers are retained; see [JSON and records](json.md).

Select `--provider cursor`, `--provider claude`, or `--provider codex` on a first run or with `--fresh`. Resume always uses `Record.provider`; repeating the same `--provider` confirms it, while a different value exits 2 before any state change and asks for `--fresh --provider VALUE`. There is no config-file default provider. Cursor resolves `cursor-agent` on PATH first, then `agent`; install Cursor CLI and log in before running.

Cursor argv is `cursor-agent -p --output-format stream-json --workspace WS --trust --sandbox enabled --auto-review --model grok-4.7-high`, with `--resume ID` on resume. The saved prompt is stdin. Resume uses the exact recorded workspace path and requires `$HOME/.cursor/chats/<md5-of-absolute-workspace-path>/<ID>/meta.json` to parse with `hasConversation: true`; missing or invalid metadata stops with the fresh-run hint. This check prevents Cursor from silently creating an empty chat for an unknown id. rabi observes `session_id` from the stream without `create-chat`, and checks that the result id agrees with the first observed id.

Before launching Cursor, rabi creates `.cursor/sandbox.json` with `{"networkPolicy":{"allow":["github.com","*.github.com","*.githubusercontent.com"]}}` when absent, and ensures `/.cursor/sandbox.json` is listed in the repository common directory's `info/exclude`. Existing sandbox paths are left unchanged, including symlinks; rabi also avoids writing through a symlinked `.cursor` directory. Their network policy remains the user's responsibility. A symlinked common-directory `info` or `info/exclude` is refused rather than written through. Setup is idempotent. `--auto-review` lets Cursor's review service approve tool requests, including some requests to run outside the sandbox. rabi counts completed `tool_call` events requesting `TYPE_INSECURE_NONE` and adds an audit message when the count is nonzero; that count alone never fails a run. The count includes rejected requests carrying that policy and does not prove a command succeeded. rabi neither adds nor removes commit attribution trailers; Cursor CLI configuration controls them.

After reaping Cursor, rabi scans `events.jsonl` once and atomically writes the last result's string `.result` to `answer.md`. A successful parent exit without a valid result is a protocol error (exit 1); `is_error: true` is a parent failure even when the recorded parent `exitCode` is 0. Nonzero parent exits retain their real status, and interrupt/timeout retain 130/124. See [JSON and records](json.md) for stream limits and audit fields.

Claude Code resolves only `claude` on PATH; install Claude Code and run `claude` to log in before launching. New argv is `claude -p --output-format stream-json --verbose --session-id UUID --permission-mode auto --permission-prompts none --model opus --effort high`; resume replaces `--session-id UUID` with `--resume ID`. The saved prompt is stdin and cwd is the workspace. rabi generates a UUID v4 for each new session, records it as the launch's `requestedSessionId`, and only saves a resumable parent id after observing the stream and validating `system/init.permissionMode` as `auto`. A different observed id stops the parent with an error; an inconsistent new session is not saved as resumable. A missing or different init permission mode also stops it with an error and guidance to check Claude settings such as `disableAutoMode`; there is no bypass fallback. Claude does not prepare workspace settings files.

Claude session probing looks for `<config>/projects/*/ID.jsonl`, where `<config>` is nonempty `$CLAUDE_CONFIG_DIR`, otherwise `$HOME/.claude`. A matching file confirms presence. Absence is unknown because Claude's storage layout is internal, so rabi attempts resume and reports any launch failure without starting a fresh session automatically. Session ids are validated before looking up files.

Claude uses the same result finalization and status precedence as Cursor. A null or missing result string produces no `answer.md`; on an ordinary successful exit it is a protocol error. Interrupt and timeout still return 130 and 124 even when Claude emits `is_error: true` with a null result and exits 0. A nonzero parent exit remains a parent failure, including unknown-session errors without an init event.

`list` and `show` expose the current parent run without changing its pointer, state, or lock. JSON workspace objects include `run` (null when absent or unreadable, with a warning for invalid data), containing the provider, run state, session id, skill, launch count, and latest launch details and existing absolute log/answer paths. `running` requires both a running record and a live lock holder pid or process group; a stale running record is displayed as `stopped`. Text `list` adds `run:STATE`; `show` adds Run, Session, Last launch, Answer, and Log lines when available. `--names` and `--paths` keep their existing output. These states describe the parent process, not task completion.

`rabi skill list [--json]` scans direct child directories containing `SKILL.md`, in order: current checkout `.agents/skills`, `.codex/skills`, `.claude/skills`, then `~/.agents/skills`, `$CODEX_HOME/skills` (default `~/.codex/skills`), `~/.claude/skills`. Names are sorted within each location. Missing directories are skipped; all duplicate names are listed. No frontmatter parsing. Text is `name<TAB>source<TAB>path`; sources are `checkout:.agents`, `checkout:.codex`, `checkout:.claude`, `user:.agents`, `user:.codex`, `user:.claude`. Discovery needs no trust, executes no skills, accepts no `-R`, and is never used by run.

## Creating, adopting, and forgetting

`create [NAME]` makes a new branch from `--base`, defaulting to the remote default branch and then the local default branch. An omitted name is generated. A name that is already a local branch or locally observed remote-tracking branch exits 4 (`exists_remote`), without fetching; use `--branch` to use that branch. A qualified `--branch remote/name` uses local `name` or creates it tracking the observed remote ref. Multiple remote matches exit 2 and list the choices.

`--pr NUMBER|URL` uses the PR head branch, with `owner/branch` for forks. Forks and deleted heads use `pull/N/head` without setting an upstream. `--issue NUMBER|URL` alone creates `issues/N` and defaults the purpose to the issue title. With `--pr` or `--branch`, it only adds a link. An explicit NAME overrides the workspace name; an existing branch retains its branch name. `--base`, `--branch`, and `--pr` are mutually exclusive. `--detach` accepts `--base` and `--issue`.

An existing workspace exits 4 (`exists`). With `--reuse`, a workspace matching the requested base, branch or PR and any requested issue returns successfully without copying files or running hooks, even after a failed setup. A mismatch still exits 4. Repeated reuse preserves creation details and existing purpose. `--no-setup` skips copying and hooks and cannot be combined with `--copy-file`.

`adopt PATH` records an existing linked Git worktree of the selected repository without running setup. It accepts `--name`, `--base`, `--purpose`, `--pr`, and `--issue`. `--base` optionally records the sync target for the existing checkout; when omitted, the target is unknown (`null`). Unknown creation time and creation commit are null. An already recorded path exits 4; a path that is not a linked checkout of this repository exits 3.

`forget SEL...` drops only records, including intent on the primary checkout. Files, branches and Git worktree registrations remain. It accepts the shared workspace selectors and resolves every selector before dropping any records.

In a single-branch or otherwise narrow-refspec clone, `create --pr` writes `branch.<b>.remote/merge` so `git pull` works, but `@{upstream}` and ahead/behind do not resolve because rabi does not change `remote.origin.fetch`.

## Existing branches, detached checkouts, and reuse

Run these in primary with an existing `origin/existing` tracking ref. `branch-example` uses a new branch; `snapshot` has detached HEAD. Reuse returns the matching workspace without setup:

```sh
rabi create branch-example --base origin/main
rabi create --branch origin/existing
rabi create snapshot --detach --base main
rabi create branch-example --base origin/main --reuse
```

## Several agents in one repository

Start in primary with a local `main` branch and a fetchable `origin` remote. Each agent records its workspace name or absolute path; handing off work means sharing that identifier.

```sh
rabi create feature/auth --base main --purpose 'agent 1: authentication' --json
rabi create feature/tests --base main --purpose 'agent 2: tests' --json
rabi exec feature/auth -- git status --short
rabi show feature/auth --json
rabi path feature/tests
rabi set feature/auth --purpose 'review authentication' --pr 774 --base main
rabi show pr:774 --json
rabi set feature/auth --unset purpose --unset pr --unset issue
rabi list --kind workspace --names
rabi list --kind primary --paths
rabi list --all-repos --json
rabi sync --all --dry-run --json
rabi sync --all --json
```

Setting a link number stores metadata without a GitHub lookup. Creating from a PR or issue uses authenticated `gh`; these independent examples follow the command help and require real links from your repository:

```sh
rabi create --pr 123
rabi create --issue 456 --base main
```

To record a checkout made by Git, run this from primary, with `primary` set to its absolute path:

```sh
git worktree add "$primary/../review" -b review main
rabi adopt "$primary/../review" --name review --base main --purpose 'review existing work'
rabi forget review
```

Forget leaves the checkout and branch in place. See [safety](safety.md) for retirement and [hooks](hooks.md) for preparation.

## Take an issue through cleanup

This workflow requires authenticated `gh` and real issue/PR numbers from your repository. Start in a repository with default base `origin/main`. The initial creation needs GitHub; after editing, use Git and gh to commit, push, and open the PR before recording its link.

```sh
export RABI_NO_INPUT=1
rabi create --issue 812 --base origin/main --json
cd "$(rabi path issues/812)"
# Edit, test, commit, push, and open a PR with Git and gh.
rabi set --pr 853 --json
rabi sync --json
rabi show pr:853 --json
primary=$(rabi repo show --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["repo"]["root"])')
cd "$primary"
rabi remove pr:853 --json
rabi gc --json
```

After integration, remove deletes a branch only with preservation evidence; otherwise it keeps the branch with a warning. Sync auto mode merges published work when integration is needed. A conflict exits 5 and rolls back unless `--keep-conflicts` was requested. After partial removal (exit 6), fix the cause and use the returned retry command. See [safety](safety.md) and [sync](sync.md).

## Global options

| Option | Meaning |
|---|---|
| `-R, --repo <value>` | Select a repository by path, registered ID, canonical host/owner/repo name, or an unambiguous name suffix. May precede the command. For exec, put it before the workspace name. |
| `--output <value>` | Select text or json on JSON-capable commands; overrides RABI_OUTPUT |
| `--json` | Alias for --output json on JSON-capable commands |
| `--no-input` | Never prompt; also enabled by CI, JSON, RABI_NO_INPUT, or no stdin/stderr TTY |
| `-h, --help` | Show text help |
| `-v, --version` | Show the application version; supports --json |

`-R` may precede the command. For `exec`, put rabi options before SEL and always use `--` before the child command. `exec`, `completions`, and `shell-init` reject explicit JSON; the script commands ignore `RABI_OUTPUT`. See [JSON output exceptions](json.md).

## Command syntax and specific options

Common options above apply where listed in command help. Each table below lists only command-specific options.

### rabi create

Create a workspace from a new branch, existing branch, pull request, issue, or detached commit

Usage: `rabi create [NAME] [--base REF | --branch BRANCH | --pr REF] [--issue REF] [--detach] [options]`

| Option | Meaning |
|---|---|
| `--copy-file <value>` | Extra gitignore pattern of files to copy from the primary checkout. Can be used multiple times. Only gitignored matches are copied |
| `--base <value>` | Base ref for a new branch or detached checkout; defaults to the remote default branch, then the local default branch |
| `--purpose <value>` | Free-text description of the task this workspace is for |
| `--pr <value>` | Create from a GitHub pull request number or URL, linking the PR; requires authenticated gh |
| `--issue <value>` | Link a GitHub issue number or URL; without --branch/--pr, default name is issues/N and purpose is its title |
| `--no-setup` | Skip all setup: no copying or hooks; records setup.state skipped. Cannot be combined with --copy-file |
| `--branch <value>` | Use an existing local branch or locally observed remote-tracking branch; no fetch. Pass remote/branch to select a remote |
| `--detach` | Create a detached checkout at --base |
| `--reuse` | Return an existing workspace only when its source matches; skip all setup. Mismatches exit 4 exists |

### rabi list

List all workspaces

Usage: `rabi list [--repo <ref>] [options]`

| Option | Meaning |
|---|---|
| `--names` | Output only workspace names (for scripts and automation) |
| `--kind <value>` | Filter by workspace or primary |
| `--paths` | Output only absolute paths, one per line |
| `--all-repos` | List repositories known to repo list; JSON adds data.repos. Cannot be combined with --repo. |

### rabi gc

Plan stale workspace cleanup; remove eligible candidates with --apply

Usage: `rabi gc [--merged] [--closed-pr] [--orphaned] [--older-than D] [--apply] [--keep-branch] [--no-hooks]`

| Option | Meaning |
|---|---|
| `--apply` | Apply the plan; otherwise return plan:true with exit 0 |
| `--merged` | Select workspaces integrated into their recorded base, including squash/rebase verified by exact tree entries and merged PR evidence |
| `--closed-pr` | Select linked PRs that are closed or merged; missing gh is reported as unavailable |
| `--orphaned` | Select workspaces whose directory is missing, whose git registration is prunable, or whose branch ref is gone. A metadata record whose directory is already gone is dropped without deleting a directory git does not list |
| `--older-than <value>` | Narrow evidence-based candidates to known createdAt at least D ago (positive integer plus m, h, d, or w); age alone never selects a workspace |
| `--keep-branch` | Always keep branches when removing checkouts |
| `--no-hooks` | Remove matching workspaces without running pre-remove hooks. This does not trust the repository. gc never asks for trust |

### rabi trust

Manage repository hook trust

Usage: `rabi trust <add|remove|list> [options]`

### rabi trust remove

Remove hook trust for a repository

Usage: `rabi trust remove [REPO] [options]`

### rabi exec

Execute a command in a workspace directory

Usage: `rabi exec [--repo <ref>] [SEL] -- COMMAND [ARG...]`

### rabi config

Read and write user configuration

Usage: `rabi config <get|set|unset|list> [options]`

### rabi config get

Show an effective config value

Usage: `rabi config get KEY [--sources] [options]`

| Option | Meaning |
|---|---|
| `--sources` | Include source (config, env, default) and detail for every effective value |

### rabi config set

Set a user config value

Usage: `rabi config set KEY VALUE [options]`

### rabi config unset

Unset a user config value

Usage: `rabi config unset KEY [options]`

### rabi clone

Clone a Git repository into the ghq-compatible layout and register it

Usage: `rabi clone <url|host/owner/repo|owner/repo> [options]`

| Option | Meaning |
|---|---|
| `--branch <value>` | Branch to check out when the repository is cloned |
| `--depth <value>` | Create a shallow clone with this many commits. Must be a positive integer |
| `--update` | Fetch origin when the repository already exists at the destination |

### rabi --version

Show the application version

Usage: `rabi --version [--json]`

### rabi repo

Register, discover, and inspect repositories

Usage: `rabi repo <add|list|show|forget> [options]`

### rabi repo add

Register a Git repository as a rabi repo

Usage: `rabi repo add [path] [options]`

### rabi repo list

List registered and discovered rabi repos

Usage: `rabi repo list [--registered|--discovered] [--names|--paths] [options]`

| Option | Meaning |
|---|---|
| `--names` | Output only repo names |
| `--paths` | Output only repo root paths |
| `--registered` | Only explicitly registered repos; do not discover |
| `--discovered` | Only discovered repos without a registration |

### rabi repo forget

Remove a Git repository from rabi's registry

Usage: `rabi repo forget <id|name|path> [options]`

### rabi sync

Fetch and sync selected workspaces; advance primary explicitly with --main

Usage: `rabi sync [SEL... | --all | --main] [--auto | --rebase | --merge] [--rewrite-published] [--keep-conflicts] [--dry-run]`

| Option | Meaning |
|---|---|
| `--all` | Sync every workspace in this repository |
| `--main` | Also fast-forward primary, only when on the default branch; may combine with SEL or --all |
| `--auto` | Rebase unpublished commits and merge published commits; overrides sync.strategy |
| `--rewrite-published` | Allow rebasing published history; permission only, does not select rebase |
| `--dry-run` | Fetch and report plan:true without changing checkouts, local branches, or workspace records |
| `--rebase` | Choose the rebase strategy |
| `--merge` | Merge the base instead of choosing automatically |
| `--keep-conflicts` | Leave conflicts in progress; exactly one workspace target required |

### rabi repo show

Inspect a repository and its primary checkout

Usage: `rabi repo show [REF] [options]`

### rabi config list

Show all effective config values

Usage: `rabi config list [--sources] [options]`

| Option | Meaning |
|---|---|
| `--sources` | Include source (config, env, default) and detail for every effective value |

### rabi trust add

Show hooks and trust a repository without prompting

Usage: `rabi trust add [REPO] [options]`

### rabi trust list

List trusted repository common directories

Usage: `rabi trust list [options]`

### rabi help

Show command help or the complete machine-readable command contract

Usage: `rabi help [COMMAND...] [--json]`

### rabi show

Inspect a workspace and its intent and observed state

Usage: `rabi show [--repo <ref>] [SEL] [options]`

### rabi path

Output only the filesystem path of a workspace

Usage: `rabi path [--repo <ref>] [SEL] [options]`

### rabi set

Update workspace purpose, base and links

Usage: `rabi set [--repo <ref>] [SEL] [options]`

| Option | Meaning |
|---|---|
| `--purpose <value>` | Replace the recorded purpose |
| `--pr <value>` | Replace the linked pull request. A number or an http(s) URL |
| `--issue <value>` | Replace the linked issue. A number or an http(s) URL |
| `--base <value>` | Set and validate the base ref used by sync; stored as a full ref name |
| `--unset <value>` | --unset FIELD clears one of purpose, base, pr or issue per occurrence. Repeat the flag to clear several fields. Cannot set and unset the same field. |

### rabi adopt

Manage an existing linked checkout without running setup

Usage: `rabi adopt PATH [--name NAME] [--base REF] [--purpose TEXT] [--pr REF] [--issue REF]`

| Option | Meaning |
|---|---|
| `--name <value>` | Workspace name; defaults to the observed checkout name |
| `--base <value>` | Optionally record the sync target for the existing checkout; unknown (null) when omitted |
| `--purpose <value>` | Free-text description of the task this workspace is for |
| `--pr <value>` | Intent to record |
| `--issue <value>` | Intent to record |

### rabi forget

Drop workspace records while preserving checkouts and branches

Usage: `rabi forget SEL...`

### rabi setup

Prepare a workspace again: copy ignored files from the primary checkout, then run trusted setup hooks

Usage: `rabi setup [SEL] [--copy-file PAT] [--no-hooks] [--source-checkout PATH]`

| Option | Meaning |
|---|---|
| `--copy-file <value>` | Extra gitignore pattern of files to copy from the primary checkout. Can be used multiple times. Only gitignored matches are copied |
| `--source-checkout <value>` | Read .rabi.toml from this checkout in the same repository, preserving the original hooks when retrying setup . from the target workspace. Copies still come from the primary checkout |
| `--no-hooks` | Copy files only; skip setup hooks without recording trust |

### rabi remove

Retire workspaces, deleting branches only when preserved

Usage: `rabi remove SEL... [--keep-branch] [--delete-unpreserved-branch] [--discard-changes] [--no-hooks] [--source-checkout PATH] [--dry-run]`

| Option | Meaning |
|---|---|
| `--source-checkout <value>` | Read pre-remove hooks from this checkout in the selected repository; preserves the original hook source when retrying from primary |
| `--discard-changes` | Allow losing modified/staged tracked files, non-ignored untracked files, and submodule contents |
| `--delete-unpreserved-branch` | Allow deleting an unpreserved branch or detached commits |
| `--dry-run` | Return a plan without running hooks or changing files, refs, or records |
| `--keep-branch` | Delete the workspace but keep its branch |
| `--no-hooks` | Delete the workspace without running pre-remove hooks. This does not trust the repository |

### rabi completions

Output a completion script for bash, zsh or fish

Usage: `rabi completions bash|zsh|fish`

### rabi shell-init

Output a shell function for rabi cd in bash, zsh or fish

Usage: `rabi shell-init bash|zsh|fish`
