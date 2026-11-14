# JSON contract

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

`--json` (an alias for `--output json`) prints exactly one schema 2 envelope to **stdout**. Every JSON command uses the same contract, including errors. Parsers must ignore unknown fields. Progress, hooks, Git output go to **stderr**. Human output is selected with `--output text`.

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

`command` uses space-separated names such as `set` and `task list`. `data` contains the command result without a top-level `schema`; only `--version` uses `data.version` for the application version. `error` is null on success, otherwise an object with stable `code`, human `message` and optional `details`. Partial results remain in `data`, including setup failures and per-item remove/gc/sync results. Per-item errors stay on their items. `warnings` contains objects with stable `code` and `message`, including metadata read/update failures and unavailable PR status; warnings may also appear on stderr and include contextual fields such as `workspace`.

`refused` is the fallback when a safety refusal has no more specific cause. The same statuses apply to text output. Standalone `exec` preserves its child's exit status, including 128+signal. Create setup failures exit 6 and retain the workspace plus hook status in `data.setup.exitCode`. Metadata save and copy failures after creation also exit 6. Remove checks every selector and safety guard before removing anything. A later failure exits 6 after persistent removal, otherwise the cause exit code. Per-item results remain available for retries. Remove reports unknown selectors as exit 3 before any hooks or removal, including with `--dry-run`. After exit 6, use `error.details.retryCommand` (also shown in text) to retry only unfinished work; repeating the original command may exit 3 for completed targets.

`RABI_OUTPUT=json|text` sets the default; empty means unset and other values exit 2. Explicit output flags override it. Unknown commands and missing commands after common options also return a usage envelope when JSON is selected. `list --names`/`--paths` select text even with `RABI_OUTPUT=json`; explicitly combining it with JSON is a usage error. `exec`, `completions`, `shell-init` and text help through `--help` ignore `RABI_OUTPUT`. `--version` and `help` support JSON and honor the output mode. Explicit JSON output for `exec`, `completions` or `shell-init` is a usage error; arguments after the required `--` belong to the child.

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

Former repository registrations and their `proj_` IDs are no longer read or written. Existing registry files are ignored and left untouched; see [storage cleanup](configuration.md#targeting-a-repository). Workspace records and their `repo: {root, name}` context remain in use. Trust and user config remain single files replaced atomically.

## Per-command data

These are the keys inside envelope `data`; nested workspace objects use the contract above.

| Command | Data |
|---|---|
| `create` | `workspace`, `source`, `setup`, `copy`, `metadata` |
| `run` | `run: RECORD, launch: LAUNCH, answer: string` |
| `show`, `set`, `adopt` | `workspace` |
| `path` | `path` |
| `list` | `workspaces` (current or selected repository only) |
| `setup` | `workspace`, `setup`, `copy` |
| `forget` | `items: [{workspace, ok}]` |
| `remove` | `items`; plans also include `plan` |
| `gc` | `plan`, `applied`, `items` |
| `sync` | `plan`, `items`, `main` |
| `config get`, `config list` | `config` (effective values, or source objects with `--sources`) |
| `config set` | `key`, `value` |
| `config unset` | `key`, `unset` |
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

Removed repository commands have no JSON data contract; invoking them returns a usage error. Cross-repository `list` output and its top-level `data.repos` are removed; use one invocation per repository. The clone-root fallback warning is `clone_root` (formerly `repo_discovery`).

## Clone data

`clone --json` uses the common schema 2 envelope. `repo.root` is the absolute primary checkout path. `repo.name` is the canonical `host/owner/repo` name, with a lowercased host (including any port). `repo.remoteUrl` is the clone URL with the same lowercased host. `http` and `https` userinfo is removed, including a token used as the username (`https://github.com/<owner>/<repo>.git` for `owner/repo` and `host/owner/repo`). `ssh` and scp-style URLs keep the login name. `cloned` is true when this command created the checkout. `updated` is true only when `--update` fetched an existing checkout. Git's progress stays on stderr. No registration ID, registry source, or registry timestamps are returned. Clone does not access registry state and cannot fail because that state is corrupt or unwritable.

A failed clone uses the common error envelope. A destination that is not the expected repository is `usage` / exit code 2. A path component the filesystem rejects, such as a name longer than the filename limit, is also `usage` / exit code 2. If Git cannot read its configuration while inspecting an existing checkout, the error includes Git's message and uses exit code 1. A Git failure during clone or fetch is `command_failed` with exit code 1.

## Help and version discovery

`rabi help [COMMAND...]` prints the same text as `--help`. `rabi help --json` puts the full contents of `help-data.json` in `data`, including `commands`, `helps`, `globalOptions`, `exitCodes`, `errorCodes`, `warningCodes`, and `schemaVersion: 2`. `rabi help show --json` adds `data.selected` for that command. This is separate from the envelope’s `schema`.

`--version` and `-v` show the application version. `rabi --version --json` uses `command: "--version"`; `data.version` contains the application version and `data.display` contains the text version line.

## Parent sessions

`list` and `show` add `run` to each workspace object. It is null when no current pointer exists. Invalid or unreadable pointer or state data also produces null and a `run_read` warning without failing the command; the same applies to invalid or unreadable locks when checking a running record. Other commands' workspace objects do not need this field. This summary reads the existing run store and never acquires, creates, removes, or rewrites a lock, pointer, or state file.

The summary is `{id,provider,state,sessionId,skill,launches,lastLaunch}`. `launches` is the number of recorded launches, not the latest launch number. `lastLaunch` is null when none are recorded, otherwise `{n,startedAt,finishedAt,exitCode,stopReason,message,dir,promptPath,eventsPath,stderrPath,answerPath}`. `dir` and file paths are absolute, and paths are null when the corresponding directory or file does not exist; an answer may be absent while the parent is running. Session and unfinished launch fields may be null. `skill` is an empty string when no skill was supplied for the run; omitting it on resume retains the recorded value. `state` uses `running`, `idle`, `stopped`, or `error` as below, but a record saying running is displayed as `stopped` unless its lock's launcher pid or parent process group is alive. This is a read-only observation; it does not repair the stored state or claim task completion.

`run` returns `data: {run, launch, answer}`. `run` is the schema-2 internal state: `{schema:2,id,workspace:{id,name,path,branch},provider:"codex"|"cursor"|"claude",sessionId,skill,state,launches:[...]}`. Unknown workspace id/branch and sessionId are null. State is `running`, `idle` after a completed parent launch, `stopped` after parent failure/interruption/timeout, or `error` after launcher/protocol errors. It never means task success.

Each launch is `{n,startedAt,finishedAt,exitCode,stopReason,skill,sessionId,message}`. `n` starts at 1 and increases; leftover launch directories are skipped, so numbers may have gaps; timestamps are ISO UTC; unfinished results are null. `exitCode` is the observed parent status (signals use 128+signal), while the rabi status is 124 for timeout and 130 for interrupt. `stopReason` is `completed`, `parent_failed`, `interrupted`, `timed_out` or `error`. `message` records diagnostic text or is null. A mismatched resume saves the observed launch.sessionId while retaining run.sessionId before stopping the running parent process group. After reaping the parent, stopReason and run.state are `error`, rabi exits 1, and exitCode retains the observed parent status. A mismatch found only by the final log scan after parent exit produces the same error fields without a stop. The top-level `launch` repeats the current launch; `answer` is the final answer file contents (empty if absent). Failed launches retain data with `ok:false` and an error. Errors before launch have no launch data.

`provider` is `codex`, `cursor`, or `claude` in both the full record and the read-only summary. Launches and `lastLaunch` may add `insecureToolCalls`, a count of completed Cursor tool-call events whose requested sandbox policy is `TYPE_INSECURE_NONE`; zero is omitted and defaults to zero when reading older records. A nonzero count also adds an audit note to `message`, including on failure, without independently changing the exit status. It includes rejected requests with that policy; it does not assert successful execution. Optional `requestedSessionId` is omitted when absent; Codex and Cursor allocate their ids and do not pre-request one. Claude new launches record a generated UUID v4 in `requestedSessionId`; resume launches omit it. The parent `sessionId` is observed from the stream, never copied from the request. Claude only saves it for resume after validating init `permissionMode: "auto"`; a new session that fails this validation remains without a parent id. These fields are additive; record schema remains 2.

Cursor finalization scans the log once after the parent is reaped, independently of the 64 KiB live session observer limit. Its limit is 16 MiB per line, including the newline when present. A valid final line without a trailing newline is accepted. Nonempty malformed/truncated JSON or an oversized line is a protocol error, including after an earlier valid result. The last `type: "result"` wins and requires a string `result`, boolean `is_error`, and nonempty `session_id`; the id must agree with the first observed stream id. The answer is atomically saved to `answer.md`. Exit 0 with no valid result records `stopReason: "error"` and rabi exits 1. Exit 0 with `is_error: true` records `exitCode: 0`, `stopReason: "parent_failed"`, and rabi exits 1. A nonzero parent exit keeps its real status; timeout and interrupt retain 124 and 130 even if the log is incomplete.

Claude finalization uses the same 16 MiB line limit, last-result selection, atomic answer writing, and status precedence. Its live observer also allows 16 MiB per line and per poll so large init metadata is validated; unreadable, malformed, or oversized live events trigger a controlled error stop. It validates init permission mode and agreement between first, init, and result session ids; an inconsistent new session remains without a resumable parent id. Null or missing result strings produce no answer file, matching Cursor; an ordinary exit 0 with such a malformed result is a protocol error. Timeout and interrupt retain their reason and rabi status even when Claude exits 0 with `is_error: true` and a null result. Unknown-session resume failures without init keep their nonzero parent exit and `parent_failed` reason. No record schema change is needed.

Records live at `<git-common-dir>/rabi/runs/run_<uuid v4>/state.json`. Per-launch `launches/<n>/` contains `prompt.md`, raw `events.jsonl`, `stderr.log`, and the provider's `answer.md` when produced. Current pointers are `<common>/rabi/runs/current/<key>` containing a run id; key is the lowercase hexadecimal SHA-256 digest of the UTF-8 absolute checkout path, matching `run.workspace.path`. Branch or display-name changes do not change this identity. `<key>.lock` initially contains the launcher pid; after spawn it atomically records both launcher pid and parent process group id as whitespace-separated integers. A group id of zero marks spawning in progress; if the launcher dies before publishing the actual id, the lock is unverifiable and requires inspection. A surviving launcher or process group prevents takeover. State and pointer writes use the shared atomic writer. Old schema-1 directories and users' `.rabi/workflows/` are left untouched and unused. Workspace metadata schema 2 is unchanged.

## Workspace handoff

`handoff show` returns `data: {intent: string|null, entries: [...], path: string}`. `handoff path` returns `{path}`. `handoff add` returns the entry directly in `data`. All use the standard schema-2 envelope, including `RABI_OUTPUT=json`. `list/show` workspace JSON is unchanged by handoff.

Each entry is `{schema:1,id:"ho_<uuid v4>",createdAt:"<RFC3339 UTC milliseconds, Z>",kind:"checkpoint|decision|note|request",text,source?}`. Optional `source` is `{runId,launch,provider:"codex|cursor|claude"}`. Required fields are schema, id, createdAt, kind and text. Append/file order is authoritative. `request` is reserved for run capture; human and parent additions are at most 8 KiB, requests at most 64 KiB. Intent is stored verbatim without a size limit; oversized requests are skipped with `handoff_request_skipped` without failing the launch. Unknown, unterminated or invalid lines are skipped with `handoff_invalid_line` without rewriting files.

Launch records optionally add `handoff: {injectedEntryIds:[...],bytes:N,snapshotAt?:"<RFC3339 UTC>"}`; disabled or unavailable memory is omitted (read as null), and old records remain readable with internal run schema 2. `bytes` measures the rendered UTF-8 prefix, including framing and reminder. The snapshot is read under the memory append lock; `snapshotAt` records the time immediately before that read. Resume uses the last launch with handoff and either an observed launch `sessionId` or `exitCode:0` as delivery evidence, comparing foreign entries with `createdAt > snapshotAt`. Old handoff records without `snapshotAt` fall back to that launch's `startedAt`. Launches without delivery evidence do not advance the baseline; no qualifying launch means a full first/fresh snapshot. Ids describe the selected snapshot, not a delivery acknowledgment. `handoff_unrecorded` signals skipped memory; `handoff_identity` signals an empty read or archival on a write after identity mismatch. Memory is separate retained XDG state; run and workspace stores remain in the git common directory.

## Task risk

`rabi task risk --json` uses the common schema 2 envelope with `command: "task risk"`. Its `data` has:

| Field | Meaning |
|---|---|
| `base` | `{ref, sha}`: requested/default ref and resolved commit SHA |
| `mergeBase`, `head` | Full merge-base and workspace HEAD commit SHAs |
| `policy` | `{present, source}`: base SHA plus `:.rabi/policy.toml`, or `built-in defaults (no policy file)` |
| `tier`, `floor` | Overall `low\|medium\|high` and whether any file requires a human |
| `reasons` | Stable, deduplicated contributing reason strings, including limit reasons or `no changes` |
| `files` | Changed files in Git raw diff order, each `{path, oldPath?, status, tier, floor, reasons}` |
| `totals` | `{files, changedLines}`: changed-entry count and total added+deleted lines (binary counts zero) |

`status` retains Git's status and any similarity score (for example `R100`); `oldPath` appears for renames/copies. Both paths are evaluated, and unknown encodings force high and floor with `floor:unknown:path-encoding`; replacement characters in displayed paths are never used for matching. Per-file reasons include applicable path rules/defaults, floors, kinds, and unknown data. Overall reasons collect file reasons plus limits. Reason forms and glob behavior are documented in [configuration](configuration.md#repository-risk-policy).

Only committed changes are classified; the working tree and index are not read. Successful high/floor classifications still have `ok: true`, `error: null`, and exit 0. Invalid policy uses exit 1 with `error.code: "failed"`; failed Git inspection or ref resolution uses exit 1 with `git_failed`. Malformed invocation uses exit 2 with `usage`. No new error codes are introduced.

## Task classify

`rabi task classify --json` uses the schema 2 envelope with `command: "task classify"`. Data has the same `base`, `mergeBase`, `head`, `policy`, `tier`, `floor`, `reasons`, `files`, and `totals` fields as risk, plus `record`. The overall verdict applies the written policy; per-file entries retain the advisory path classification. Path rule/default tiers cannot raise or lower the overall verdict.

`record` is the atomically saved schema-1 object:

| Field | Meaning |
|---|---|
| `schema` | `1` |
| `headSha`, `baseSha`, `mergeBaseSha` | Full commit IDs used for assessment; the classified range is `mergeBaseSha..headSha`, with policy/document blobs read at `baseSha` |
| `policyBlobSha`, `docBlobSha` | Base policy and document blob IDs, null when absent or unavailable |
| `tier`, `floor`, `reasons`, `uncertainty` | Final tier, deterministic floor, nonempty explanations, and `none` or uncertainty text |
| `provider` | `claude`; null when no provider is selected for launch |
| `createdAt` | UTC RFC 3339 assessment timestamp |

The record lives at `<git-common-dir>/rabi/classifications/<head-sha>.json` and is replaced on rerun. It binds the assessment to the exact head, merge-base, and base policy/document blob IDs; a different base commit is compatible only when its PR merge-base and both blob IDs still match. Floors and high deterministic safeguards skip the provider; missing policy documents, oversized diffs, provider failures, malformed answers, and uncertainty escalate to high. These are successful verdicts with exit 0, `ok: true`, and a `classifier:*` reason where applicable. Provider stderr is never included. Git/policy errors retain risk's codes; record-write errors exit 1. See [commands](commands.md#task-classify).

## Task status and merge preflight

Task commands use the common schema 2 envelope. A completed dry-run evaluation has `ok: true`, `error: null`, and exit 0 for every verdict. Data field names use camelCase; item IDs and the `not_evaluated` enum are stable identifiers.

`task status` (`command: "task status"`) returns a facts snapshot:

| Field | Meaning |
|---|---|
| `repository`, `defaultBranch` | Selected owner/repository and GitHub default branch |
| `baseTipSha` | Current PR base-branch tip from the encoded Git ref GET; null if lookup fails or returns invalid ref/commit data |
| `pr` | `{number, state, draft, baseRef, baseSha, headRef, headSha, headRepository, sameRepository, merged, mergeCommitSha, mergeable, mergeableState, changedFiles, additions, deletions}` |
| `files` | Every fetched `{filename, previousFilename, status, additions, deletions}`; previousFilename is null unless supplied |
| `filesTruncated` | Count differs from changedFiles, or at least 3000 files fetched |
| `checkRuns` | `{id, name, status, conclusion, app, startedAt, completedAt}` on the initial head SHA; app is its slug, missing fields are null |
| `checksIncomplete` | Collected check runs differ from `total_count`, or any page has a missing/changing total; fails required_checks |
| `statuses` | `{sha, context, state, creator}` on that SHA; creator is login or null; separate from checks |
| `reviews` | Effective `{reviewer, state, id}` per reviewer, in reviewer order: latest APPROVED, CHANGES_REQUESTED or DISMISSED; COMMENTED/PENDING do not change state |
| `reviewDecision` | Null for REST snapshots; evaluator also accepts an aggregate decision |
| `inconsistent`, `finalHeadSha` | Whether the final PR head differs from the initial head, and that final SHA |

PR `state` and file statuses retain REST values such as `open` and `renamed`. `headRepository` is null for an unavailable head repository. `mergeable` can be null; null/missing REST mergeable state is normalized to `unknown`. Mergeability passes only for true with state `clean`, `unstable` or `has_hooks`; every other state fails with the observed state. GitHub may return null while computing mergeability lazily; that fails and can be retried later. Times and conclusions may be null. Files/check runs/statuses retain API pagination order; check selection uses the maximum run ID for the exact name and app.

`task merge --dry-run` (`command: "task merge"`) returns:

| Field | Meaning |
|---|---|
| `items` | Ordered `{id, result, reason, detail}` preflight items |
| `verdict` | `blocked` if any fail; otherwise `incomplete` if any not_evaluated; otherwise `ready` |
| `classification` | Existing risk `{tier, floor, reasons, files, totals}`, or null if classification could not complete |

Item order is `pr_open`, `head_consistent`, `required_checks`, `risk_tier`, `approval`, `review_record`, `classification_record`, `no_changes_requested`, `mergeable`. Result is `pass`, `fail` or `not_evaluated`. Missing/empty CI requirements give reason `no_required_checks`. `required_checks.detail.checksIncomplete` mirrors the snapshot flag; incomplete checks fail even when collected required runs succeed. `required_checks.detail.checks` contains `{name, app, pass, observed, ignoredCheckRuns, ignoredStatuses}` for each policy requirement. Observed is `missing` or `status:conclusion` (`none` for null); ignored arrays preserve the facts objects. `risk_tier.detail` contains `{tier, floor, reasons}` when computed, and `approval.detail` contains `{tier, floor}` or null. Other details identify expected base, compared SHAs, changes-requested reviews, or mergeability. Approval first requires byte-identical policy content at the PR base SHA and `baseTipSha` (absence at both is equal). Changed policy, an unavailable tip, or unreadable tip policy fails this item while the remaining items are evaluated. The PR base policy remains in use. Once fresh, approval passes for an auto tier without floor, or the latest successful `rabi/approval` status from a policy approver on the exact head SHA. Otherwise it fails. `classification_record` passes when a valid local record matches the exact PR head, the locally computed PR merge-base, and the policy/document blob IDs at the PR base SHA; its detail contains that record. This verifies the classified range is exactly PR merge-base..head. If the PR merge-base cannot be computed locally, the item is not_evaluated. With a match, the effective tier is the record tier raised by deterministic safeguards, ignoring path rule/default tiers. Missing, invalid, stale, or unverifiable records leave this item not_evaluated and retain path classification. `review_record` passes only for the selected workspace task's earned pass, bound by head SHA and record digest in its `review_passed` event, with the recorded PR and reviewed range matching the verified PR. Base advancement requires the same reviewed merge-base. Bare, replaced, or unbound pass records leave it not_evaluated. Reasons are explanations rather than error codes.

For example, passing facts yield `pr_open: pass`, `required_checks: pass`, `risk_tier: pass`, `no_changes_requested: pass`, `mergeable: pass`, and verdict `ready` when valid review and classification records and approval also pass; absent records leave it `incomplete`. With `pr:N`, `head_consistent` is also not_evaluated when stable; an inconsistent head fails it. Errors use the common envelope and existing [exit codes](exit-codes.md); dry-run evaluation failures remain successful envelopes.

`task approve` (`command: "task approve"`) returns `data: {repository, number, sha, state, context, description, tier}` after a successful POST. `sha` is the lowercase full head SHA; `state` is `success`, `context` is `rabi/approval`, and `description` contains the tier and optional note, truncated to 140 characters. Refusals have `ok: false`, `error.code: "usage"`, null data, and exit 2 without writing.

Real `task merge` uses the same command name as dry-run. A preflight refusal has `ok: false`, `error.code: "refused"`, exit 4, and the unchanged evaluation `{items, verdict, classification}` in `data`. `--external-review` does not alter the evaluation or verdict; it permits only the two record items to remain not_evaluated. Other N/A items still refuse.

After a merge invocation, `data` is `{preflight, merged, mergeCommitSha, mergedHeadSha, facts}`. `preflight` is the original evaluation; `facts` is the reread status snapshot. `merged` is GitHub's merged flag; `mergeCommitSha` is the reported merge commit or null. `mergedHeadSha` is the reread PR head SHA when merged, otherwise null. A gh failure or an unmerged reread retains this data with `ok: false`, `error.code: "failed"` and exit 1. If rereading itself fails, the original evaluation remains in `data` and the collection error is reported; merge outcome is unknown. Snapshot `pr.merged` and `pr.mergeCommitSha` are taken from the final PR read, as is `pr.state`.

## Doctor report

`rabi doctor --json` returns `data: {repository, defaultBranch, findings, counts: {warning, info}}`. Repository is a `host/owner/name` string or null; defaultBranch is the branch name or null. Findings are `{code, level, message, detail}`, with level `warning` or `info`. They belong to `data.findings`, not envelope `warnings`. Missing data produces informational `could_not_check` findings; findings never change `ok: true` or exit 0. Repository selection and usage errors retain the normal error envelope and status. Each committed workflow file produces an informational `workflow_review_manually` finding for manual review; doctor does not parse workflow YAML. See [doctor findings](safety.md#doctor-findings).

## Local task store

`task new`, `task show` and `task abandon` return `data: {task, recordedState, events}`. `task` is the record with its **displayed** state, while `recordedState` preserves its saved state. `task list` returns `data: {tasks: [{id, state, branch, intent}]}`; intent is its first line, and unreadable/corrupt records have null branch and intent. `--all` includes terminal records. Warnings use `record_mismatch` for invalid records or branch/state disagreement, and `internal` for unreadable records. List/show never rewrite records or events. Invalid or unreadable task records and event files fail show/abandon with exit 1 (`failed`), naming the task ID. Creation fails closed with the same code and task ID if an unreadable record prevents checking branch reservations.

Files live outside Git at `<state_dir>/repositories/<sha256 hex of canonical git common-dir bytes>/tasks/task_<uuid>/task.json` and `events.jsonl`. `state_dir()` uses `$XDG_STATE_HOME/rabi`, falling back to `~/.local/state/rabi` for unset, empty or relative XDG values. The shared repository namespace publishes and validates schema 1 `repository.json` (`commonDir` is the canonical Git common directory), just like handoff memory. A corrupt manifest or mismatched identity fails with exit 1 (`failed`) without replacing it. Directories are 0700 and files 0600. `task.json` is atomically replaced under the task's `.lock` (Unix `flock`); `events.jsonl` is only appended under the same lock. Creation also locks `tasks/.lock` to serialize reservations. It builds each new task in `tasks/.staging-<uuid>/`, syncs both files, then renames that directory to `task_<uuid>/`. Incomplete staging directories remain invisible to listing, ID resolution and reservation checks. Readers do not lock. For existing tasks, events are synced before record replacement, so an interrupted replacement may leave an extra event; this pair of files is not a single atomic transaction.

Task file schema 1 and command data both use camelCase field names:

| Field | Value |
|---|---|
| `schema` | Integer 1 |
| `id` | `task_<UUID v4>`, matching its directory |
| `createdAt`, `updatedAt` | UTC RFC3339 timestamps |
| `state` | `{name: STATE}`, or `{name: "blocked", reason: CODE}` |
| `intent` | Nonempty text, preserved verbatim |
| `branch` | Reserved branch name; creation does not create the branch |
| `baseRef` | Locally resolvable base ref captured at creation |
| `pushRemoteUrl` | Push URL captured once; userinfo, query and fragment stripped |
| `workspacePath` | Null until `task implement` binds an existing linked checkout |
| `prNumber` | PR number recorded by `task publish`, otherwise null |

States are `new`, `implementing`, `prechecking`, `publishing`, `ci`, `reviewing`, `ready`, `classifying`, `awaiting_approval`, `merging`, `merged`, `blocked`, and `abandoned`. Blocked reason codes are `no_progress`, `repeated_failure`, `no_ci`, `ci_timeout`, `conflict`, `diverged`, `destination_not_github`, `policy_invalid`, `provider_failed`, `evidence_missing`, `floor_changed`, `precheck_changed`, `environment`, `review_invalid`, `review_diff_too_large`, `review_pass_invalid`, `budget`, `internal`, `record_mismatch`, `merge_preflight`, and `merge_failed`.

Each event is one JSON object and newline: `{at, from, to, reason}` with optional string `detail` and optional objects `agent`, `precheck` and `review`. `at` is a UTC RFC3339 timestamp, `from`/`to` are state objects, and `reason` is text. Creation appends `{from: null, to: {name: "new"}, reason: "created"}`. Abandon appends the previous saved state and `{name: "abandoned"}`, using the supplied reason or `abandoned`. Previous lines remain unchanged.

The [transition table](commands.md#local-task-store) allows the forward chain through implementation, checks, publication, CI, review and classification; classification goes directly to merging or through awaiting approval. Prechecking, CI, reviewing, classifying and awaiting approval can return to implementing. Any non-terminal state can become blocked or abandoned; blocked can return to any non-terminal state. Merged and abandoned are terminal. Invalid edges use exit 2 (`usage`). These states are bookkeeping claims, not GitHub evidence, and do not authorize merges or launches. Advance and standalone merge additionally require the earned head-bound review pass referenced by its task event (standalone merge permits `--external-review` when it is absent); the remaining merge preflight relies on fresh evidence.

For non-terminal tasks other than `blocked`, output derives `blocked(record_mismatch)` when a new task's local branch already exists or a later state's branch is missing. A stored `blocked` state keeps its reason regardless of branch existence. `recordedState` and the saved record are preserved. No other Git or GitHub derivation is performed.

`task implement` returns `data: {taskId, launchId, provider, exitCode, preHead, postHead, newCommits, dirty, stopReason}`. `provider` is `codex` or `claude`; post-launch Git facts are nullable when inspection fails. The command reports a failed attempt through the ordinary envelope while retaining the launch data. Provider output goes to stderr.

Implementation appends `implementation_started` and `implementation_finished` events. Their optional `agent` object carries launch details: the start event includes `launchId`, `provider`, `preHead` and pre-launch `dirty`; the finished event carries the command result fields. Only the first start changes state, from `new` to `implementing`; subsequent events preserve the current state, including concurrent abandonment. A separate nonblocking `.implement.lock` excludes concurrent implementers for the same task and remains held through process cleanup and outcome recording. It does not lock out plain `rabi run` or unrelated workspace commands.

`task publish` returns `data: {taskId, head, prNumber, state}` on success. Precheck events add `precheck: {head, passed, failingCommand, outputTail}`; command events also include a `reason` and `failingCommandHash` (SHA-256 of the original failing command, null on success). The displayed command and output tail are sanitized, and output tails are bounded to 4096 bytes. The hash distinguishes commands whose sanitized display is identical without recording their original text. Older events may omit it. An absent declaration still records a successful precheck for HEAD. Publication records transitions through `prechecking`, `publishing` and `ci`, saves `prNumber`, and shares `.implement.lock` with implementation attempts. An interrupted `publishing` task rechecks the current HEAD and reconciles its saved destination before writing again. A failed precheck returns directly from `publishing` to `implementing` without a blocked or `record_mismatch` event. Task records still do not authorize merge preflight.

## Task advance

`task advance` uses `command: "task advance"` and returns `data: {taskId, state, reason, head, fixesUsed, fixBudget, fixesRemaining}`. `state` uses the task state object (`{name}` or `{name: "blocked", reason}`). `head` is the inspected commit SHA when available, otherwise null. `reason` explains the last step; an externally merged PR reports `merged` while retaining its recorded state. `fixesUsed` is the task's persistent failure-driven relaunch count, including conflict recovery and review revisions. `fixBudget` is the effective `[factory] fix-budget` (default 3); `fixesRemaining` is that budget minus usage, clamped to zero. `task show` also returns `data.fixBudget` and `data.fixesRemaining`, with usage in `data.task.advance.fixesUsed`. These budget fields reflect current primary-checkout configuration without rewriting the task. If that configuration cannot be read or validated, `task show` still succeeds with `data.fixBudget: null` and `data.fixesRemaining: null`, preserves the task and event history, and emits a `task_config_unavailable` warning on stderr and in the envelope’s `warnings` array. Exit 0 has `ok: true`; waiting (exit 10) and refusals (exit 4) retain the result with `ok: false` and the matching error. Publication errors preserve their existing statuses. Ordinary provider failures block as `provider_failed` with exit 4; provider timeout and interruption also block as `provider_failed` while retaining 124 and 130. Cancellation during prechecks or CI-log retrieval stops `--wait` with 130 without launching another implementer. `--wait` emits one final envelope, with provider output on stderr.

Advance bookkeeping is an optional, defaulted `advance` object in schema-1 task records, so existing tasks can be read without migration. It tracks the evaluated head, CI head and start time, consumed failure signatures, fix count, pending feedback, and the immediately previous fix attempt's head-independent fingerprint. CI fingerprints combine sorted failing check names with a hash of the normalized sanitized output tail; review fingerprints combine sorted path/severity pairs with normalized finding-body hashes; conflict fingerprints use the base SHA; precheck fingerprints combine the failing command hash (a hash of sanitized command text for older events without one) with the same normalized output-tail hash. [Output normalization](commands.md#task-advance) removes volatile log prefixes, timestamps, durations, hex IDs and temporary paths before hashing. Repeating the immediately previous fingerprint blocks as `repeated_failure` before another launch, even on a new HEAD, and records a `repeated_failure` event. An exhausted budget reports `budget` first. Existing head-bound signatures still prevent the same signature from relaunching after an interrupted invocation. The fingerprint and budget reservation are persisted before launch. Events record every state transition and its reason; launch events retain their existing shapes. A nonblocking `.advance.lock` serializes advance invocations; each step also holds the existing implementation and publication locks, preventing concurrent implementers, publishers or abandonment during the step. Polling releases these operation locks between steps. These records coordinate progress and do not authorize merge.

Advance reports may additionally contain `mergeCommitSha`, `cleanup: {outcome, reason}`, and `remoteBranchDeleted` after merge. `remoteBranchDeleted` is present only when policy requested remote deletion; false means the branch was retained or deletion could not be verified. The merge receipt and cleanup outcome persist in `data.task.advance`; task and review history remain available after workspace removal. `awaiting_approval` returns exit 10 and `error.code: "waiting"`, including when the approval deadline expires. Pending status publication is tracked by head so polling and repeated invocations do not duplicate it. Exact-head approval timers persist across invocations and reset on head movement. A failed merge preflight reports `blocked(merge_preflight)` with the gate explanation in `reason`; no write retry loop runs.

## Task review

`task review` uses `command: "task review"` and returns the same `{taskId, state, reason, head, fixesUsed, fixBudget, fixesRemaining}` data shape as `task advance`. A pass moves through `ready` to `classifying`; a revise returns to `implementing`. Refusals retain the report in `data` with `ok: false`; invalid reviewer output after one retry reports `blocked(review_invalid)`. The reviewer never emits a GitHub approval.

The provider's final answer is a strict JSON object in the requested nonce frame:

```json
{"sha":"<reviewed-head-sha>","verdict":"pass","findings":[{"id":"R1","severity":"nit","path":"src/example.rs","line":12,"body":"Clarify this condition."}]}
```

`verdict` is `pass` or `revise`; `severity` is `important` or `nit`. Pass requires no important findings; revise requires at least one. An inconsistent answer gets one retry, then blocks as `review_invalid`. The SHA must match the exact reviewed head. Finding bodies are nonblank and at most 4096 bytes, matching the sanitizer tail limit; if redaction expansion requires a cut, the body starts with an explicit `[truncated]` marker. A diff larger than 400 KiB blocks as `review_diff_too_large` before a reviewer runs.

Rabi sanitizes finding strings before atomically writing valid review records to `<git-common-dir>/rabi/reviews/<head-sha>.json`. Each schema-1 record has `{schema, headSha, baseSha, mergeBaseSha, provider, sha, verdict, findings, createdAt}`. In `reviewing`, all three commit bindings must match before reuse. The `review_passed` transition event adds `review: {headSha, recordId}`, binding the earned pass to the head and the SHA-256 of the serialized record. In `ready` or `classifying`, only that pass can be reconciled, even if the base has moved; a missing or invalid event/record blocks as `review_pass_invalid` without launching a reviewer. Records for another head are ignored. A marked PR comment summarizes sanitized, size-capped findings, including nits; reruns edit the existing comment owned by the authenticated GitHub account. Copied markers from other authors are ignored. Review findings are advisory data for deterministic task transitions and framed implementer feedback, never approval evidence.
