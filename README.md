# rabi

rabi is an open-source Rust CLI for a software factory: record intent with `rabi task new`, then use `rabi task advance` to drive implementation, prechecks, draft PR publication, CI, review, classification, approval, merge and safe cleanup. Agents edit, check and commit locally; deterministic rabi commands publish and merge behind explicit gates. Ordinary Git workspaces keep each task's files and branch separate.

[Japanese](README.ja.md) · [Upgrading to v8](docs/upgrading-to-v8.md) · [v8.0.0 release notes](docs/release-notes/v8.0.0.md)

## Install

Install from [GitHub Releases](https://github.com/yoiailabs/rabi/releases) with the release installer (`curl` and `tar` required):

```sh
curl -fsSL https://github.com/yoiailabs/rabi/releases/latest/download/install.sh | sh
```

The installer verifies SHA-256 and installs to `~/.local/bin` without sudo. Linux and macOS x64/arm64 are supported; use Linux binaries in WSL. Add `sh -s -- --verify-attestation` in place of `sh` to verify provenance with `gh`. For reproducible installs, pin `RABI_VERSION=<TAG>` and use that tag's installer URL; see [installation](docs/installation.md).

## Quick start

Allow five minutes for setup; agent work and CI take longer. Use an existing GitHub clone with a locally available `main` base, Git 2.44+, authenticated Git and `gh`, and installed/logged-in Codex and Claude CLIs. Codex implements by default, Claude reviews by default and is the only classifier provider. Substitute your base branch and project checks below.

First, have a maintainer add `.rabi.toml` prechecks:

```toml
[precheck]
commands = ["cargo fmt --check", "cargo clippy --locked --all-targets -- -D warnings", "cargo test --locked"]
```

Add `.rabi/policy.toml`, replacing the check names/app with your actual CI jobs and `maintainer-login` with an authorized maintainer's GitHub login:

```toml
schema = 1

[ci]
required = ["Rust fmt", "Rust clippy", "Rust test"]
app = "github-actions"

[merge]
auto = ["low", "medium"]

[approval]
approvers = ["maintainer-login"]

[classifier]
doc = "risk-tiers.md"
```

Write `risk-tiers.md` with your review and escalation criteria: define low/medium/high, require high for uncertainty, and say which changes need maintainer approval. Have a maintainer commit and publish these three files to the base branch **before** creating tasks. Prechecks must match the committed remote base; policy is read from base commits. Changes to `.rabi.toml`, `.rabi/policy.toml` or `.github/**` hit built-in floors and block task publication for human handling.

Review hooks, prechecks and declared environment before granting trust. Start from the primary checkout and keep that shell there so cleanup can remove the task workspace:

```sh
cd your-repo
rabi trust add
rabi task new "Improve the command documentation" --branch task/docs --base main
# Set task_id to the ID printed above.
task_id='task_<uuid>'
rabi create task/docs --base main --no-setup
rabi task implement "$task_id"
rabi task advance "$task_id" --wait
rabi task show "$task_id" --json
```

`task new` only records intent and reserves an unused branch. `create` supplies the linked checkout; `--no-setup` skips copies and setup hooks. `task implement` starts the first local implementation attempt. `advance` then progresses one step per invocation, or continues and polls with `--wait`: implement → precheck → publish (draft PR) → CI → review → ready → classify → approval/auto-merge → cleanup. Review findings can return to implementation within a shared fix budget (default 3). For each required check name and app, the newest run must have succeeded.

`rabi task new --issue N` imports an issue only after checking author write/maintain/admin permission. `rabi task intake --label L [--limit N] [--dry-run]` matches labels case-insensitively, checks the latest label actor and skips linked issues. Missing or non-user (for example bot) label actors use `label_actor_unknown`; permission refusals, including collaborator-permission HTTP 404, use `intake_not_permitted`. Intake records tasks without launching agents; external text is sanitized, keeps its beginning within a size cap with a trailing truncation marker, and is nonce-framed as untrusted data in later prompts. Continue with the checkout, `task implement` and explicit `task advance` steps above. See [external intake](docs/commands.md#external-issue-intake).

Exit 10 means waiting for CI or maintainer approval; exit 4 means blocked/refused. Read `task.state.reason` and events in `task show --json`; an exit-0 step does not by itself mean merged. See the [state and recovery tables](docs/commands.md#task-lifecycle) and [exit codes](docs/exit-codes.md).

`task implement` also exits 4 with `stopReason: "metadata_changed"` if its post-launch Git metadata recheck fails. Inspect and repair Git registration before retrying; see [implementation outcomes](docs/json.md#local-task-store).

## Approval

Only policy-listed low/medium tiers without a floor may merge without approval; the default auto set is empty. High, floors, and missing/failed/uncertain classification require a maintainer. Review output is never approval. Automation never runs `rabi task approve`, unsets `RABI_AGENT` to bypass it, or posts a success `rabi/approval` status.

A configured maintainer, outside rabi-launched agents, inspects the exact current PR head and gates, sets `reviewed_sha` to its full 40-hex SHA, then records approval:

```sh
rabi task status task/docs --json
rabi task approve task/docs --sha "$reviewed_sha"
rabi task advance "$task_id" --wait
```

The latest `rabi/approval` status must be success from `[approval] approvers` on that exact SHA. A new head needs renewed approval. Advance posts pending only, then reruns the full preflight and merges with `--match-head-commit`. `--wait` polls approval for up to 24h by default and returns 10 on expiry. Cleanup retains dirty, stashed or unpushed workspaces and all local branches; remote branch deletion is off by default. See [approval and merge safety](docs/safety.md#task-approval-and-merge).

## Workspaces

Use workspace commands for investigations, manual edits or existing branches without the task lifecycle. The primary checkout supplies copied files and is protected from removal. Select workspaces by name, `.`, path or recorded PR/issue link. Commands target one repository: the current checkout or `-R/--repo` with a checkout root path or canonical `host/owner/repo` at its existing clone location. Prefix relative paths with `./`; repository registration and discovery are removed.

```sh
rabi create topic --base main --purpose 'investigate a change'
rabi exec topic -- git status --short
rabi list
rabi sync topic --dry-run
rabi remove topic --dry-run
```

`sync` fetches even with `--dry-run`; it never pushes. `remove` checks preservation and keeps unpreserved branches. Dirty work and unpreserved-history deletion need separate explicit permissions. Ignored files are deleted with the workspace, including nested repositories in ignored directories. Workspace commands do not detect another agent's use; coordinate cleanup. See [workspace commands](docs/commands.md#selecting-and-inspecting-workspaces) and [removal safety](docs/safety.md).

## Parent sessions

`rabi run` launches or resumes one trusted parent session in an existing linked workspace. The parent manages its progress; run does not enforce the task lifecycle or declare task completion. Codex is the default; `--provider cursor|claude` selects another provider on first/fresh launches. `--fresh` retains older records, `--timeout` defaults to `2h`, and optional `--skill VALUE` passes instructions verbatim without lookup. Use `SEL --` to select a workspace; `-R` requires it.

```sh
rabi run topic -- "Investigate the change and commit locally"
rabi run topic --
rabi run --fresh --provider claude --timeout 30m topic -- "Continue the investigation"
```

Run shares the task agent boundary: inherited GitHub tokens, SSH agent/askpass and `GIT_*` are stripped; gh configuration is empty and Git credentials/prompts are disabled. Agents only edit, check and commit; publication runs outside them. Codex has network/web disabled and approval never; Claude denies push/gh/web tools. This reduces accidental credential use, not access by a determined same-user process. [Provider restrictions and limits](docs/safety.md#task-agent-launch-boundary) also describe Cursor's sandbox behavior. `list`/`show` expose run summaries; handoff memory carries context between sessions.

## Command index

Use `rabi help` for syntax or `rabi help --json` for the full command/flag contract. Global options include `-R/--repo`, `--json`, `--output text|json`, `--no-input`, `-h/--help` and `-v/--version`.

| Area | Commands |
|---|---|
| Task records | `rabi task`, `rabi task new`, `rabi task intake`, `rabi task list`, `rabi task show`, `rabi task abandon` |
| Factory steps | `rabi task implement`, `rabi task advance`, `rabi task publish`, `rabi task review` |
| Gates | `rabi task risk`, `rabi task classify`, `rabi task status`, `rabi task approve`, `rabi task merge` |
| Workspaces | `rabi create`, `rabi list`, `rabi show`, `rabi path`, `rabi set`, `rabi exec` |
| Preparation and retirement | `rabi setup`, `rabi adopt`, `rabi forget`, `rabi sync`, `rabi remove`, `rabi gc` |
| Sessions and memory | `rabi run`, `rabi handoff`, `rabi handoff show`, `rabi handoff path`, `rabi handoff add` |
| Trust | `rabi trust`, `rabi trust add`, `rabi trust remove`, `rabi trust list` |
| User settings | `rabi config`, `rabi config get`, `rabi config set`, `rabi config unset`, `rabi config list` |
| Tools | `rabi clone`, `rabi doctor`, `rabi help`, `rabi completions`, `rabi shell-init`, `rabi --version` |

## Documentation

[Commands and task states](docs/commands.md) · [Configuration and policy](docs/configuration.md) · [JSON contract](docs/json.md) · [Exit codes](docs/exit-codes.md) · [Safety](docs/safety.md) · [Hooks and trust](docs/hooks.md) · [Sync](docs/sync.md) · [Shell integration](docs/shell.md) · [Upgrading to v8](docs/upgrading-to-v8.md) · [Changelog](CHANGELOG.md)

## Agent skill

The [rabi usage skill](skills/rabi/SKILL.md) explains task orchestration, local-only agent work, blocked reasons and workspace recovery. Install the CLI separately using the release installer above, then copy `skills/rabi/` into your agent provider's skills directory.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development and verification. Open pull requests against `main`.

## License

[MIT](LICENSE).
