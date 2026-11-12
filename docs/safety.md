# Removal and garbage collection

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

Retire workspaces with explicit selectors. See [command options](commands.md) for the full syntax.

## Local preservation

Remove runs trusted `pre-remove` hooks before deleting the checkout and rechecks the target after hooks. A branch is preserved when its commits are integrated into its base (normal merge, or squash/rebase proven by exact tree-entry transitions) or its tip exactly matches its own upstream. With no recorded base, retirement checks the repository default base. For local proof, the base or upstream must resolve to an independent ref in the common Git directory, including custom fetch namespaces. A symbolic alias to the retiring branch, detached HEAD, or a bare commit ID does not establish preservation. Preserved branches are deleted; other branches stay with warning `branch_kept_unpreserved` and `branchKeptReason: "unpreserved"`. `--keep-branch` always keeps the branch; `--delete-unpreserved-branch` explicitly permits deleting unpreserved history.

## GitHub merged-PR evidence

After local preservation checks fail, remove and gc can use GitHub evidence for a named branch. A recorded PR (`set --pr` or `create --pr`) must be `MERGED`, and its `headRefOid` must equal the local tip or contain it as an ancestor. Ancestry is checked only when that PR commit already exists locally; nothing is fetched. Partial clones (`extensions.partialClone` configured or any remote with `promisor=true`) require an exact tip match and skip PR object and ancestry probes on every Git version. A recorded URL is passed to `gh` and must match the checkout repository resolved by `gh repo view`; number-only links use that repository. Unlinked branches use one `gh pr list --head BRANCH --state merged --limit 2` query and require exactly one result with the same head name and exact tip ID; ancestry is never used. Repository identity is resolved once per command. Missing or unauthenticated `gh`, failed queries, invalid JSON, ambiguous identities, and timeouts keep the existing local verdict. Each gh call has a 10-second deadline (shorten it with positive `RABI_GH_TIMEOUT_MS`); authentication prompts are disabled and non-interactive stdin is `/dev/null`. Remote responses, including failures, are cached for the command, while local tips and ancestry are rechecked after hooks. JSON items add `preservationReason`: `merged_pr` for GitHub proof, `local` for remove's local proof, otherwise null or absent. Text details identify merged-PR proof. These rules also apply to plans. Known limit: repository identity is compared as `https://HOST/OWNER/REPO` without a port, so GitHub Enterprise hosts served over plain HTTP or on a non-default port never prove a merged PR; those branches are kept (use `git branch -d` after checking).

## Dirty work and explicit permissions

Modified or staged tracked files (including submodule changes) and non-ignored untracked files require `--discard-changes` (otherwise exit 4 `dirty`). Unpreserved detached commits require `--delete-unpreserved-branch` (otherwise exit 4 `unpreserved`). The two permissions are independent. The primary checkout always exits 4 `primary`. `--dry-run` runs no hooks and changes no files, refs, or records; its items have `removed: false`, `wouldRemove`, and `wouldDeleteBranch`, with `plan: true`.

For remove (including `--dry-run`) and gc (plan and `--apply`), dirty uses the same rule as list/show. Ignored files do not count as dirty and are deleted with the workspace without a warning or `--discard-changes`. Known limitation: nested Git repositories inside ignored directories (for example, `vendor/x/.git` under ignored `vendor/`) are also deleted with the workspace.

Even when clean, workspaces with populated submodules or retained submodule data are refused during remove preflight (exit 4 `dirty`, including `--dry-run`) and skipped by gc (plan and `--apply`) with a `rabi remove NAME --discard-changes` hint; review the submodule contents, including unpushed commits, before using that permission.

## Preflight and results

`remove --json` returns the shared schema 2 envelope. `data.items` contains `workspace`, `removed`, `branchDeleted`, `branchKeptReason`, `preservationReason`, `detail`, `alreadyGone`, and `error`. Dry runs add `data.plan`; their items also include `wouldRemove` and `wouldDeleteBranch`.

Per-item errors are structured `{code,message}` objects. A selector with neither a metadata record nor a matching Git worktree exits 3 `not_found`, including with `--dry-run`. Every selector is checked first: one unknown target prevents all hooks and removal, and text output lists every unknown selector. Select by name, `.`, `pr:N`, `issue:N`, PR/issue URL, or an absolute path or path starting with `./` or `../`.

`alreadyGone: true` requires evidence of the workspace. A missing directory with a record, a Git registration inside the workspace directory, or both succeeds with exit 0; only stale records and registrations are removed, without hooks or branch deletion. If a record and files remain but the Git registration is gone, remove acknowledges it with `alreadyGone: true` and leaves the files, record, and branch untouched. A branch alone or an unregistered directory without a record is insufficient evidence.

If branch deletion fails after checkout removal, remove keeps the branch with warning `branch_kept_delete_failed`, `branchKeptReason: "delete_failed"`, and the Git error in the item `detail`. This does not fail the item or stop later targets; branch-only failures exit 0 and have no retry command. Decide on any later branch cleanup with Git.

## Partial completion and retries

A failure after persistent removal exits 6 and retains every item result. Completed removals also remove their records, so repeating the original command may exit 3. Fix the cause, then run `error.details.retryCommand`, also shown as `Retry with` in text. The retry is one invocation: `cd <primary> && rabi -R <primary> remove [flags] -- <unfinished paths>`. It selects only unfinished checkout or record work, preserves permission flags and output mode, and protects selectors with `--`. Run hints in bash or zsh; paths are shell-quoted and control characters use ANSI-C quoting. Metadata cleanup is retried only for recorded targets, using the absolute path spelling saved in the record before removal. Location selectors are fixed before removal. A linked checkout supplying pre-remove hooks is removed last; retries preserve its configuration with `--source-checkout PATH`, which must belong to the selected repository. Once that source is removed, only record cleanup remains and retries disable hooks. Retries add `--no-hooks` when the original command disabled hooks or its source had no pre-remove hooks, preserving hook non-execution even if primary defines hooks. With `-R PRIMARY`, dot also resolves a recorded checkout whose Git registration disappeared.

## Exact squash and rebase evidence

Squash and rebase checks compare complete changed entries: exact paths, file modes, and full before/after blob or gitlink IDs. Evidence comes from non-merge commits on the base's first-parent history. A squash must match one base commit; rebased history must match every workspace commit to a distinct base commit in ancestor-to-tip order. A base commit cannot prove multiple reapplications. Base history remains evidence after later edits. Evidence found only on side histories, unverified merges, ambiguous merge bases, Git failures, and differing file contents keep the branch. These checks do not run external diff or textconv commands, write Git objects, or require `merge-tree --write-tree`.

## Garbage collection

GC defaults to all three criteria: integrated work (`--merged`, including verified squash/rebase history and merged PR evidence), closed or merged linked PRs (`--closed-pr`), and missing directories or registrations (`--orphaned`). `--older-than D` filters by `createdAt`; unknown creation times are excluded and age alone never creates a candidate. D is a positive integer plus `m`, `h`, `d`, or `w`. Without merged PR evidence, a branch still at its creation commit is not merged. Git reflog checks for older and unrecorded checkouts remain conservative.

Without `--apply`, gc returns `plan: true` and exit 0, including protected candidates. Applying follows remove safety rules, skips dirty or unpreserved candidates, and reports a targeted `rabi remove 'NAME' --discard-changes` or `rabi remove 'NAME' --delete-unpreserved-branch` hint. Permissions are never bundled. Hints pin the repository and verify that the selector resolves uniquely. The cd/dot fallback requires an existing directory. For a missing directory, hints use a recorded PR/issue selector when possible; otherwise they show an explicit `git -C REPO worktree remove --force -- PATH` command that discards only that Git registration and its HEAD reflog. Any remaining metadata record can be cleaned by a later orphan gc. Orphan cleanup removes stale records and registrations while preserving branches and any remaining checkout files. Missing gh or unavailable PR status is reported as `unavailable`. Each target is rechecked after hooks; failures after persistent removal exit 6, otherwise 1.

Actions are `wouldRemove`, `removed`, `skipped`, and `failed`. Repeating `gc --apply` is safe; retired workspaces are no longer candidates.

## Orphaned records

Orphaned records have neither a Git worktree registration nor an existing directory. `list` includes them with a trailing `[orphaned]`; the shared JSON workspace object has `orphaned: true` (false for existing entries), with null `branch` and `head`. `path` and `exec` refuse orphaned names; `show` can inspect their records. `remove <name>` removes only the stale record, exits 0, and reports that the workspace was already gone; it runs no hooks and leaves branches alone. Its existing JSON result uses `workspace.orphaned: true` and `branchDeleted: false`. A live workspace matching the same name takes precedence; multiple orphaned records with the same resolved name are refused as ambiguous. Orphaned records do not reserve names for live workspaces. When a live workspace takes the same name, the orphan remains visible in `list`; use `gc --apply` to remove it, since `remove <name>` resolves to the live workspace.

## Plans before removal

From primary, with `feature/auth` already created:

```sh
rabi remove feature/auth --dry-run --json
rabi gc --json
rabi gc --apply --json
```

GC protects the primary checkout, default branch, current workspace, and Git-locked checkouts. There is no ownership or in-use detection, so coordinate with other agents before removing their workspace. Workspaces isolate files and branches, not processes or services.

GC JSON is `data: {plan, items:[…]}`; each item has `workspace`, `reasons`, `action`, and `detail`, and may include `preservationReason`. See [JSON](json.md) and [exit codes](exit-codes.md).

`rabi run` requires the same repository trust as hooks, without prompting. It runs a Codex parent in auto mode in the current linked workspace; the primary checkout is refused. The parent controls its own actions and subagents; rabi does not enforce task progress, check results, publication or completion. Trust is checked before inspecting worktree status, which can execute configured fsmonitor commands. Locks keyed by absolute checkout path prevent concurrent launches across branch changes and remain live while either the launcher or its Codex process group survives. SIGINT/SIGTERM and timeout terminate the parent process group and preserve records; commits and pushes are not rolled back. Missing sessions never silently start over. Skills are verbatim instructions, with no resolution or validation by rabi.

## Doctor findings

`rabi doctor` writes nothing: it only reads GitHub with GET requests and local Git configuration/committed objects. It never fetches, executes hooks, changes merge policy, or blocks other commands. Its findings are advisory and always exit 0. It never prints tokens, push URLs, raw API responses, or subprocess diagnostics. Missing access is not evidence of safe settings.

| Code | Meaning |
| --- | --- |
| `required_checks_not_enforced` | No required-status-checks ruleset rule protects the default branch. Classic branch protection may still enforce checks; it is not inspected. |
| `force_push_allowed` | No non-fast-forward ruleset rule protects the default branch. Classic branch protection is not inspected. |
| `deletion_allowed` | No deletion ruleset rule protects the default branch. Classic branch protection is not inspected. |
| `merge_queue` | A merge queue is enabled; rabi's merge flow does not support it yet. |
| `ruleset_bypass` | The current user may bypass a named active ruleset targeting the default branch. |
| `token_workflow_scope` | Warning: the OAuth token has `workflow` scope, so anything that can push with it can rewrite CI definitions. Info: no scopes header was returned, so fine-grained/app token permissions could not be determined. |
| `default_workflow_permissions` | Repository workflow tokens default to write permissions. |
| `remote_push_ssh` | An SSH push URL is configured for the default branch push remote, selected by `branch.<branch>.pushRemote`, then `remote.pushDefault`, then `branch.<branch>.remote`, then `origin`. The selected name must be a configured remote other than `.`; otherwise `could_not_check` reports that the push remote could not be determined. SSH keys cannot exclude workflow changes; prefer HTTPS with a token without workflow permission. |
| `pull_request_target_trigger` | A workflow may use the privileged `pull_request_target` event. |
| `pull_request_write_permissions` | A `pull_request` workflow grants write permissions at workflow or job level, lacks top-level permissions (inherits repository defaults), or uses an uncertain permissions form. File and job are named where known. |
| `pull_request_secrets` | A `pull_request` workflow references secrets beyond `secrets.GITHUB_TOKEN`. |
| `required_job_conditional` | A `pull_request` workflow uses path filters or a draft condition. A required check that is filtered or skipped never reports success; rabi's preflight treats a missing check as failure. |
| `could_not_check` | A named check lacked API access, authentication, a default branch, or readable local data. Its reason is sanitized; no credentials are included. |

Workflow detection is a conservative line/indent heuristic without a YAML parser. It recognizes the top-level `on:` key, including literal quoted `"on":` and `'on':` keys. Inline values must be a plain event name or a single-line flow sequence of plain names. Quoted/escaped inline values, inline flow mappings, indirection, expressions, and multiline forms are uncertain. Boolean `true:` keys are not decoded as `on`; they are uncertain.

For block triggers, the first non-blank, non-comment line determines the event indent. Only declarations at that exact indent are interpreted: an unquoted `[A-Za-z0-9_-]+:` key or a `- name` sequence entry with a plain name. Deeper lines are event configuration and never add events or affect trigger parsing, regardless of prose punctuation, quotes, aliases, or comments. Unsupported event keys (including quoted or escaped keys, merge keys, and complex keys), indicators at event key/value starts, event-level block scalars, and quotes or flow collections not closed on their event line make triggers uncertain. Partial dedents and tab indentation are also uncertain. Comments are stripped only from the `on:` header and event declarations using state local to that line; no quote or indicator state crosses lines. Findings identify uncertainty in both their message and detail. Unknown root keys that could supply `on`, missing triggers, and duplicate `on` keys are uncertain. These limits can produce conservative warnings for valid YAML outside this subset.

Permissions, secrets, and conditional checks remain separate heuristics. Permission scanning recognizes workflow/job permissions and block/inline maps. Raw text is scanned for case-insensitive `secrets` tokens, including comments and whole-context references; only the exact `secrets.GITHUB_TOKEN` reference is exempt. Whole `if:` values, including folded/literal block continuations, are checked for draft conditions. Path filters elsewhere in a mixed trigger block and draft conditions outside job scope can produce extra info findings. Secret bracket expressions are conservatively treated as uncertain secrets. The scanner does not evaluate expressions, expand YAML merges, or trace reusable workflows/actions; an empty report is not proof of CI safety.

Only rulesets are inspected through `rules/branches`; classic branch protection is not inspected. Rulesets are paginated, including inherited rulesets, and active details are inspected for default-branch conditions and bypass access. Unknown ref pattern forms are conservatively included and cannot establish an exclusion. Doctor inspects the locally available default-branch tip without fetching; rerun after updating refs yourself if current remote workflow definitions are needed.
