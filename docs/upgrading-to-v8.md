# Upgrading to rabi v8

[README](../README.md) · [Commands](commands.md) · [Changelog](../CHANGELOG.md#v800-2026-11-14)

v8 adds a software-factory task lifecycle while retaining ordinary Git workspaces
and resumable parent sessions. This guide covers every breaking change listed in
[v8.0.0](../CHANGELOG.md#v800-2026-11-14). Existing workspace records, task/handoff
state and current run records remain in their existing locations; no bulk state
migration is required. Removed registry files are ignored, not migrated.

## Upgrade binaries before configuration

Install the intended v8 release with its GitHub Releases installer. Pin the
installer and binary to the same published tag with `RABI_VERSION=<TAG>`; see
[installation](installation.md). Upgrade every invocation location, including
interactive shells, CI and agent launchers, before sharing new configuration.
Check the binary selected on PATH with `rabi --version` and `rabi help --json`.

Older binaries reject each of these `.rabi.toml` sections as an unknown key:

| New section | Migration |
|---|---|
| `[precheck]` | Upgrade before adding committed pre-publication commands. Have a maintainer establish them on the base first; task publication refuses changed declarations |
| `[review]` | Upgrade before setting reviewer provider/model preferences in primary |
| `[factory]` | Upgrade before setting fix-budget, CI timeout or approval timeout in primary |
| `[env]` | Upgrade before declaring trusted run/exec PATH prefixes or variables in primary; never store secrets |

For a rollback, remove unsupported declarations before using an older binary.
[Configuration](configuration.md) documents defaults and validation. Prechecks
and task providers do not apply repository `[env]`; only run and exec use it.

## Replace registration and discovery

Multi-repository registration and automatic discovery have been removed.
Workspace commands now operate on exactly one repository per invocation.

| Removed or changed interface | Migration |
|---|---|
| `rabi repo add`, `rabi repo list`, `rabi repo show`, `rabi repo forget` | Use the current checkout, or `-R ./your-repo`; inspect repository context with `rabi show --json` |
| Repository registry | Existing state under `$XDG_STATE_HOME/rabi/repos/` (default `~/.local/state/rabi/repos/`) and old `projects.json` files is ignored and left untouched. Optionally delete only that obsolete state after checking it; keep `repositories/` and Git common-directory workspace/run records |
| `list --all-repos` and JSON `data.repos` | Invoke `rabi -R REF list --json` separately for each repository and read `data.workspaces` |
| `discover-ghq` configuration and automatic ghq discovery | Run `rabi config unset discover-ghq` or remove that key from user `config.toml`. Other commands report the removed key until it is deleted. Select repositories explicitly |
| Repository-option completion | Enter a filesystem path or canonical clone name after `-R/--repo`; completion no longer queries a registry or suggests registered names |

Clone-root defaults still follow `ghq.root` → `GHQ_ROOT` → `~/ghq`. This resolves
one expected clone location; it does not scan directories or run ghq discovery.

## Update repository references and clone consumers

`-R/--repo` accepts a checkout **root** path or canonical `host/owner/repo`
(`owner/repo` uses the default host). Paths must start with `/`, `./`, `../` or
`~` (also `.` and `..`); prefix relative paths with `./`. Linked checkout roots
are accepted and resolve to primary. Subdirectory and `.git` paths are rejected.
Replace registered IDs, bare names, suffix matches and discovery-based references
with an explicit checkout root or canonical name.

```sh
rabi -R ./your-repo list --json
rabi -R owner/repo show --json
```

Canonical names map directly to an existing clone-root location whose hosted
`origin` must match. Local relative origin paths do not establish hosted identity,
for either canonical selection or reuse of an existing clone. Use an explicit
filesystem path for such a checkout, or configure its intended hosted origin
before canonical selection/clone reuse. Canonical selection never clones or fetches.

`rabi clone` keeps the ghq-compatible layout but writes no registry entry and
reports no registry-related failure, including partial exit 6. Consume the
returned `data.repo.root` or canonical clone name for later commands; remove
registration-ID and registry-failure handling. Warning-code consumers must replace
`repo_discovery` with `clone_root` for clone-root fallback warnings. See
[repository targeting](configuration.md#targeting-a-repository) and
[clone JSON](json.md#clone-data).

## Replace skill discovery

`rabi skill` and `rabi skill list`, their discovery JSON and their completions
are removed. Inspect your provider's skill directories directly and remove
scripts parsing discovery output. The [rabi usage skill](../skills/rabi/SKILL.md)
is still available as instructions for agents.

`rabi run --skill VALUE` still passes VALUE verbatim without resolving a file.
It is now optional on first and `--fresh` launches. Omit it when no extra
instruction is needed; new runs store an empty skill string and inject no skill
instruction. On resume, omission retains the previously recorded value.

```sh
rabi run topic -- "Implement the local change and commit"
rabi run --fresh topic -- "Start a new local approach"
rabi run --skill project-guidance topic -- "Follow the project instructions"
```

These examples require an existing trusted linked workspace named `topic`.

## Move remote operations out of parent sessions

Plain `rabi run` now applies the shared task credential boundary after repository
environment and handoff setup, on all providers, including resume. GitHub token,
SSH agent/askpass and inherited `GIT_*` variables are stripped; `GH_CONFIG_DIR`
is empty; Git credential helpers/prompts are disabled; `RABI_AGENT=1` is set.
Environment summaries report only surviving declarations. Neither `--no-env`
nor `--no-handoff` bypasses the boundary.

Move pushes, PR creation and merges out of agent sessions into authorized,
deterministic rabi task commands. Keep local edits, checks and commits inside the
agent. Do not restore credentials, remove the marker or post approval statuses
from automation. A configured maintainer records exact-head approval outside
rabi-launched agents. User-configured MCP servers in Claude/Cursor run parents
can retain their own credentials; the boundary covers inherited gh/Git
credentials and is not same-user access control.

Codex parents now use the task implement workspace-write restrictions, with
network/web disabled, approval never, user configuration/rules ignored and narrow
Git metadata write grants for local commits. Claude parents disallow push, gh
and web tools. Move remote/network-dependent operations outside these parents;
prepare local tools/dependencies before launch. Review
[the agent boundary and its limits](safety.md#task-agent-launch-boundary).

Codex also verifies linked Git metadata before granting write access and refuses
redirected pointers or symlinked grant directories. Repair the checkout's Git
worktree registration/backlink and metadata directories before retrying; do not
broaden sandbox grants to bypass validation. Retain local work while repairing.

New Cursor sandbox policies contain an empty network allowlist rather than
GitHub host entries. Existing `.cursor/sandbox.json` policies are preserved.
Review an existing policy and remove prior allow entries when desired; use rabi
for publication. This change does not rewrite user policies or remove Cursor's
automatic review behavior. See [parent sessions](commands.md#parent-sessions).

## Verify task publication destinations

`task publish` requires a recorded HTTPS or Git SSH push URL matching the GitHub
repository independently resolved by `gh` for the task checkout. Local paths,
other transports, non-GitHub destinations and proven repository mismatches block
as `destination_not_github` before push. GitHub Enterprise uses the host resolved
by gh. This restriction applies to task publication, not every workspace command.

Configure the intended hosted push remote and verify gh's repository identity
before `task new`, which captures the push URL once. Later remote edits do not
rewrite that saved URL. For an existing mismatched task, inspect its intent and
local commits with a maintainer and create a correctly targeted task/branch if
needed; do not hand-edit task state or redirect publication. Authentication,
network or ambiguous-identity lookup failures remain retryable errors, not proof
of a different destination. See [publication](commands.md#task-publish).

## Use the tool-less classifier provider

Codex classification support is removed because its CLI cannot currently
guarantee tool-less invocation without an injected model catalog; rabi injects
none. `task classify --provider` supports only `claude`. Unsupported flag values
exit 2, while unsupported `[classifier] provider` values fail policy parsing.

Replace `--provider codex` with `--provider claude` or omit it, and change any
base-policy provider to `claude` or omit it. Install and authenticate Claude;
configure `[classifier] doc` to point to the written policy committed on base.
Reviewers and implementers can still use Codex. Classification failures or
uncertainty produce high, never automatic approval. See
[classification](commands.md#task-classify).

## Make merge evaluation explicit

`rabi task merge` now attempts a guarded merge by default. Add `--dry-run` to all
inspection-only scripts. Approval is evaluated rather than remaining N/A, and
real merge preflight refusals exit 4. A completed dry run still exits 0 for
`blocked`, `incomplete` or `ready`; inspect `data.verdict` and individual items.

```sh
rabi task merge topic --dry-run --json
```

Real merge needs current CI, consistent heads, fresh base policy, event-bound
earned review, matching classification, approval/auto eligibility, no requested
changes and known mergeability. `--external-review` covers only the two missing
review/classification record items after explicit external review; it cannot
waive approval or other gates. Automation never runs approve or posts success
approval. New HEADs require updated evidence and renewed maintainer approval
when required. See [merge preflight](commands.md#task-status-and-merge-preflight).
