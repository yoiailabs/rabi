# JSON contract

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

`--json` (an alias for `--output json`) prints exactly one schema 2 envelope to **stdout**. Every JSON command uses the same contract, including errors and `repo list`. Parsers must ignore unknown fields. Progress, hooks, Git output go to **stderr**. Human output is selected with `--output text`.

## Envelope examples

These outputs were captured in an isolated repository. Only the temporary path prefix is shortened to `/tmp/…`; object fields are retained. `rabi path json-example --json` after creating that workspace returned:

```json
{
  "command": "path",
  "data": {
    "path": "/tmp/…/d/rabi/workspaces/your-repo-e26da761/json-example"
  },
  "error": null,
  "ok": true,
  "schema": 2,
  "warnings": []
}
```

An absent workspace (`rabi show topic --json`) returned exit 3:

```json
{
  "command": "show",
  "data": null,
  "error": {
    "code": "not_found",
    "message": "Workspace 'topic' not found"
  },
  "ok": false,
  "schema": 2,
  "warnings": []
}
```

`command` uses space-separated names such as `set` and `repo list`. `data` contains the command result without a top-level `schema`; only `--version` uses `data.version` for the application version. `error` is null on success, otherwise an object with stable `code`, human `message` and optional `details`. Partial results remain in `data`, including setup failures and per-item remove/gc/sync results. Per-item errors stay on their items. `warnings` contains objects with stable `code` and `message`, including metadata read/update failures and unavailable PR status; warnings may also appear on stderr and include contextual fields such as `workspace`.

`refused` is the fallback when a safety refusal has no more specific cause. The same statuses apply to text output. Standalone `exec` preserves its child's exit status, including 128+signal. Create setup failures exit 6 and retain the workspace plus hook status in `data.setup.exitCode`. Metadata save and copy failures after creation also exit 6. Remove checks every selector and safety guard before removing anything. A later failure exits 6 after persistent removal, otherwise the cause exit code. Per-item results remain available for retries. Remove reports unknown selectors as exit 3 before any hooks or removal, including with `--dry-run`. After exit 6, use `error.details.retryCommand` (also shown in text) to retry only unfinished work; repeating the original command may exit 3 for completed targets.

`RABI_OUTPUT=json|text` sets the default; empty means unset and other values exit 2. Explicit output flags override it. Unknown commands and missing commands after common options also return a usage envelope when JSON is selected. `list --names`/`--paths` (and repo `--names`/`--paths`) select text even with `RABI_OUTPUT=json`; explicitly combining it with JSON is a usage error. `exec`, `completions`, `shell-init` and text help through `--help` ignore `RABI_OUTPUT`. `--version` and `help` support JSON and honor the output mode. Explicit JSON output for `exec`, `completions` or `shell-init` is a usage error; arguments after the required `--` belong to the child.

rabi never prompts in non-interactive mode: nonempty `CI`, JSON output, `--no-input`, `RABI_NO_INPUT` other than empty or `0`, or stdin/stderr without a TTY. Untrusted hooks refuse before changes with exit 4 and a `rabi trust add` / `create --no-setup` / `setup --no-hooks` hint. Network Git operations receive `GIT_TERMINAL_PROMPT=0`; repository hooks and gh read stdin from `/dev/null`. User commands (standalone `exec`) inherit stdin.

See [exit codes](exit-codes.md) for statuses and [sync](sync.md) for strategy and result fields.

## Workspace object

`current` identifies the checkout containing cwd. `repo` contains the primary root and repository name. `managed` is false for the primary checkout and unrecorded checkouts outside the workspace directory. `upstream` is null when no upstream is configured, otherwise `{ref,ahead,behind,gone}`; ref is fully qualified, and counts are null when the upstream is gone or unavailable. Unknown metadata and observed state are null. For an orphaned record, branch, head, dirty and detached are null. Reads never invent timestamps.

`list`, `show`, `create`, `adopt`, `set`, `remove` share this object. Their data uses `workspace` or `workspaces` keys (including each `items` entry); sync reports its results under `main` and `items`. `path` is absolute. `head` is the full HEAD SHA, or null when Git has no HEAD. `branch` is null when HEAD is detached (`detached` is true). `base` is the commit-ish this command started from when it knows one; otherwise it is the recorded sync base (a full ref such as `refs/remotes/origin/main`), or null when no base is recorded. `list --json` exposes that recorded base. `dirty` is true for modified or staged tracked files (including submodule changes) or non-ignored untracked files, even when Git status display settings hide them. `kind` is `primary` for the repository's primary checkout and `workspace` for other workspaces.

The `data.workspace` object captured from `rabi create json-example --base main --json` (with the [example hooks and copy patterns](hooks.md)) was:

```json
{
  "base": "refs/heads/main",
  "branch": "json-example",
  "createdAt": "2026-10-23T15:52:16.473Z",
  "createdFrom": "11ed8177c3bfbd861c940781d16138629d204881",
  "current": false,
  "detached": false,
  "dirty": false,
  "head": "11ed8177c3bfbd861c940781d16138629d204881",
  "id": "ws_cf988bfc-198c-47a0-a750-5ffb06d8ac09",
  "kind": "workspace",
  "links": {
    "issue": null,
    "pr": null
  },
  "managed": true,
  "name": "json-example",
  "orphaned": false,
  "path": "/tmp/…/d/rabi/workspaces/your-repo-e26da761/json-example",
  "purpose": null,
  "repo": {
    "name": "your-repo",
    "root": "/tmp/…/your-repo"
  },
  "setup": {
    "at": "2026-10-23T15:52:16.520Z",
    "error": null,
    "state": "ok"
  },
  "updatedAt": "2026-10-23T15:52:16.522Z",
  "upstream": null
}
```

`id`, `purpose`, `base`, `createdAt`, `updatedAt`, `createdFrom`, `setup`, and `links` come from `rabi/workspaces/<id>.json` in the Git common directory (`git rev-parse --git-common-dir`). For a normal repository that directory is `.git`. `rabi create` writes a record. `links.pr` and `links.issue` are `{ "number", "url" }` or null. A number stores `url` as null. An http(s) URL stores the URL and a number parsed from `/pull/`, `/pulls/`, `/issues/`, or `/issue/` when one is present. `create --pr` or `create --issue` fills the matching link, including `https://<host>/<owner>/<repo>/pull/<number>` or `.../issues/<number>`. `show`, `path`, `list`, and `exec` read metadata without changing record files. `remove` removes the record after Git removes the workspace. `create` replaces any record already stored at that path. `set` patches intent. Reusing an existing PR branch can add its missing PR link while preserving creation details. `create --json` includes `metadata` as `{ "ok": true, "error": null }`. When saving the record fails after the workspace exists, `ok` is false, `error` explains why, copy and hooks still run, and the process exits 6 with `error.code` set to `partial` and the result retained in `data`. A corrupt workspace record makes `list`, `show`, and `exec` continue with null metadata for that record and a warning; other records remain readable. `create`, `setup`, `adopt`, `forget`, `remove`, `set`, `sync` refuse and leave the file unchanged. Sync validates metadata before fetching or updating any target. Concurrent first registrations for the same path are read as one record: the oldest `createdAt` wins, then the lexicographically smallest `id` breaks ties. All fields come from that record, and the next metadata write removes the extra files. `createdFrom` is the full commit the branch was created from, or the base tip sync last fast-forwarded it to while it had no commits of its own. `create` stores the new branch's HEAD. A record for a branch that already existed stores the oldest `refs/heads` reflog entry, or null when that reflog is missing. `set` and a later checkout leave it unchanged. Adopted records have `createdFrom` null. `rabi set` patches the fields you pass and leaves the others. A workspace Git created outside `rabi`, including the primary checkout, has null metadata fields until `set`, `adopt`, or PR reuse records it. Cleanup details (`prunable`, `locked`) can be added later without a new version.

Workspace records contain `schema: 2`, an `id` of the form `ws_<uuid v4>`, `path`, an optional `name`, and the metadata fields above. JSON workspace objects include `id` (null for an unrecorded workspace). Each record is written atomically through a temporary file in the same directory, fsync, and rename. Deleting a record removes only its file. Concurrent additions of different records preserve both; concurrent updates of the same record use the last write.

Repo registrations live in `$XDG_STATE_HOME/rabi/repos/<id>.json`, falling back to `~/.local/state/rabi/repos/<id>.json`. Their IDs have the form `proj_<uuid>`. Readers deduplicate registrations by root path, keeping the oldest `createdAt` (then the smallest ID on a tie). rabi uses no internal file locks; trust and user config remain single files replaced atomically.

## Per-command data

These are the keys inside envelope `data`; nested workspace objects use the contract above.

| Command | Data |
|---|---|
| `create` | `workspace`, `source`, `setup`, `copy`, `metadata` |
| `run` | `run: RECORD, launch: LAUNCH, answer: string` |
| `skill list` | `skills: [{name,source,path}]` |
| `show`, `set`, `adopt` | `workspace` |
| `path` | `path` |
| `list` | `workspaces`; `--all-repos` also adds `repos` |
| `setup` | `workspace`, `setup`, `copy` |
| `forget` | `items: [{workspace, ok}]` |
| `remove` | `items`; plans also include `plan` |
| `gc` | `plan`, `applied`, `items` |
| `sync` | `plan`, `items`, `main` |
| `config get`, `config list` | `config` (effective values, or source objects with `--sources`) |
| `config set` | `key`, `value` |
| `config unset` | `key`, `unset` |
| `repo add`, `repo forget` | `status`, `repo` |
| `repo show` | `repo` |
| `repo list` | `repos` |
| `trust add` | `path`, `trustedAt` |
| `trust remove` | `path`, `removed` |
| `trust list` | `repos` (common-directory paths and trust timestamps) |
| `clone` | `repo`, `cloned`, `updated` |
| `help` | Complete help contract; command selection adds `selected` |
| `--version` | `version`, `display` |

`show --json` and the result of a create or adopt use the workspace object. `show` returns `data.workspace`.

The other `data` fields from that create invocation were:

```json
{
  "copy": {
    "copied": [
      ".env",
      ".env.local",
      "config/secrets.json"
    ],
    "error": null
  },
  "metadata": {
    "error": null,
    "ok": true
  },
  "setup": {
    "at": "2026-10-23T15:52:16.520Z",
    "command": null,
    "error": null,
    "exitCode": 0,
    "ok": true,
    "ran": true,
    "signal": null,
    "state": "ok"
  },
  "source": {
    "base": "refs/heads/main",
    "kind": "new",
    "reused": false
  }
}
```

`source.kind` is `new`, `branch`, `pr`, `issue`, or `detach`. Sources include the normalized `base`; branch sources add `branch` and `ref`. PR sources add `number`, `url`, `headRef`, `headRepository`, `branch`, and `checkout` (`branch` or `pullHead` on creation). Issue sources add `number`, `url`, and `issue`; an issue combined with another source is recorded as a link. `reused` indicates that no checkout or setup was performed.

`copy.copied` contains the relative paths copied during this invocation. Copy errors leave the workspace in place. `setup.ran` is false when hooks did not run; a successful hook has `exitCode: 0`. A failed hook reports its command, exit code, signal, or startup error. Checkout creation followed by a metadata, copy, or hook failure exits 6 and preserves the workspace object in `data`.

`adopt --json` returns `{workspace: {...}}`. If refreshing the workspace after saving its record fails, it exits 6 (`partial`) and preserves the saved registration fields in `data.workspace`. `forget --json` returns `{items: [{workspace: {...}, ok: true}]}` using the workspace state after record removal.

## Repository data

`repo list --json` uses the common schema 2 envelope. It uses camelCase and absolute `rootPath` values. Each repo includes `canonicalName` (`host/owner/repo` from `origin`, otherwise from a `<root>/<host>/<owner>/<repo>` checkout, otherwise the directory name). `name` stays the directory name. Warnings are recorded in the envelope and on stderr. Parsers must ignore unknown fields, so adding `canonicalName` does not change `schema`.

`repo add --json` and `repo forget --json` return `{ "status", "repo" }` in `data`. Errors use the common envelope.

## Clone data

`clone --json` uses the common schema 2 envelope. `repo.root` is the absolute primary checkout path. `repo.name` is the canonical `host/owner/repo` name, with a lowercased host (including any port). `repo.remoteUrl` is the clone URL with the same lowercased host. `http` and `https` userinfo is removed, including a token used as the username (`https://github.com/<owner>/<repo>.git` for `owner/repo` and `host/owner/repo`). `ssh` and scp-style URLs keep the login name. `cloned` is true when this command created the checkout. `updated` is true only when `--update` fetched an existing checkout. Git's progress stays on stderr.

A failed clone uses the common error envelope. A destination that is not the expected repository is `usage` / exit code 2. A path component the filesystem rejects, such as a name longer than the filename limit, is also `usage` / exit code 2. If Git cannot read its configuration while inspecting an existing checkout, the error includes Git's message and uses exit code 1. A Git failure during clone or fetch is `command_failed` with exit code 1. If repo registration fails after a checkout was created or an existing checkout was fetched, the command exits 6 with `error.code` set to `partial`, retaining the completed result (including `repo.root`, `cloned`, and `updated`) in `data`. The checkout remains available; retrying the command can register it. Registration failure without a clone or fetch exits 1.

## Help and version discovery

`rabi help [COMMAND...]` prints the same text as `--help`. `rabi help --json` puts the full contents of `help-data.json` in `data`, including `commands`, `helps`, `globalOptions`, `exitCodes`, `errorCodes`, `warningCodes`, and `schemaVersion: 2`. `rabi help repo show --json` adds `data.selected` for that command. This is separate from the envelope’s `schema`.

`--version` and `-v` show the application version. `rabi --version --json` uses `command: "--version"`; `data.version` contains the application version and `data.display` contains the text version line.

## Parent sessions and skill discovery

`list` and `show` add `run` to each workspace object. It is null when no current pointer exists. Invalid or unreadable pointer or state data also produces null and a `run_read` warning without failing the command; the same applies to invalid or unreadable locks when checking a running record. Other commands' workspace objects do not need this field. This summary reads the existing run store and never acquires, creates, removes, or rewrites a lock, pointer, or state file.

The summary is `{id,state,sessionId,skill,launches,lastLaunch}`. `launches` is the number of recorded launches, not the latest launch number. `lastLaunch` is null when none are recorded, otherwise `{n,startedAt,finishedAt,exitCode,stopReason,message,dir,promptPath,eventsPath,stderrPath,answerPath}`. `dir` and file paths are absolute, and paths are null when the corresponding directory or file does not exist; an answer may be absent while the parent is running. Session and unfinished launch fields may be null. `state` uses `running`, `idle`, `stopped`, or `error` as below, but a record saying running is displayed as `stopped` unless its lock's launcher pid or Codex process group is alive. This is a read-only observation; it does not repair the stored state or claim task completion.

`run` returns `data: {run, launch, answer}`. `run` is the schema-2 internal state: `{schema:2,id,workspace:{id,name,path,branch},provider:"codex",sessionId,skill,state,launches:[...]}`. Unknown workspace id/branch and sessionId are null. State is `running`, `idle` after parent exit 0, `stopped` after parent failure/interruption/timeout, or `error` after launcher/protocol errors. It never means task success.

Each launch is `{n,startedAt,finishedAt,exitCode,stopReason,skill,sessionId,message}`. `n` starts at 1 and increases; leftover launch directories are skipped, so numbers may have gaps; timestamps are ISO UTC; unfinished results are null. `exitCode` is the observed parent status (signals use 128+signal), while the rabi status is 124 for timeout and 130 for interrupt. `stopReason` is `completed`, `parent_failed`, `interrupted`, `timed_out` or `error`. `message` records diagnostic text or is null. A mismatched resume saves the observed launch.sessionId while retaining run.sessionId before stopping the running parent process group. After reaping the parent, stopReason and run.state are `error`, rabi exits 1, and exitCode retains the observed parent status. A mismatch found only by the final log scan after parent exit produces the same error fields without a stop. The top-level `launch` repeats the current launch; `answer` is the final answer file contents (empty if absent). Failed launches retain data with `ok:false` and an error. Errors before launch have no launch data.

Records live at `<git-common-dir>/rabi/runs/run_<uuid v4>/state.json`. Per-launch `launches/<n>/` contains `prompt.md`, raw `events.jsonl`, `stderr.log`, and the provider's `answer.md` when produced. Current pointers are `<common>/rabi/runs/current/<key>` containing a run id; key is the lowercase hexadecimal SHA-256 digest of the UTF-8 absolute checkout path, matching `run.workspace.path`. Branch or display-name changes do not change this identity. `<key>.lock` initially contains the launcher pid; after spawn it atomically records both launcher pid and Codex process group id as whitespace-separated integers. A group id of zero marks spawning in progress; if the launcher dies before publishing the actual id, the lock is unverifiable and requires inspection. A surviving launcher or process group prevents takeover. State and pointer writes use the shared atomic writer. Old schema-1 directories and users' `.rabi/workflows/` are left untouched and unused. Workspace metadata schema 2 is unchanged.

`skill list` returns `data: {skills:[{name,source,path}]}`. Paths identify skill directories, not `SKILL.md`; sources and order are documented in [commands](commands.md). Duplicates remain separate entries. Neither discovery nor run parses skill contents.
