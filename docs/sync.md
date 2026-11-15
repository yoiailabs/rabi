# Sync

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

## Selection

No arguments sync only the current workspace (including from a subdirectory), even with `-R`. If cwd is outside the selected repository, omitted SEL and explicit `.` both refuse with usage exit 2; pass a workspace name or path, or run from that checkout. In primary, no arguments behave as `--main`: fast-forward primary only when it is on the default branch, otherwise skip with `not-default-branch`. A workspace sync never advances primary implicitly. `--main` alone selects primary; `sync . --main` selects the current workspace and primary. `--all` selects every workspace in the selected repository and cannot combine with SELs (exit 2); add `--main` to include primary.

## Fetch and dry runs

Sync maps bases to remotes through configured fetch refspecs and fetches each involved remote once. It never pushes, changes upstream configuration, or runs hooks; every Git call, including status and rollback, disables Git hooks and fsmonitor. `--dry-run` also fetches, then returns `plan:true` without changing checkout contents, the index, local branches, or workspace records. Plans show the expected action in `status`/`detail`; a future merge/rebase commit ID is unknown (`after:null`). Plans never claim a published rewrite happened.

Both execution and dry runs use ordinary `git fetch` in the invoking repository. Before any fetch or integration, sync checks every configured fetch refspec of every involved remote and refuses with exit 4 `refused` if a positive refspec has a destination outside `refs/remotes/` (including explicit `refs/tags/` destinations and mirror mappings). Negative exclusions do not relax this conservative check; a missing or empty destination only writes `FETCH_HEAD` and is allowed. Use a tracking destination under `refs/remotes/` for custom namespaces. Sync also refuses any existing symbolic ref visible to `git for-each-ref` under `refs/remotes/` or `refs/tags/` that points outside `refs/remotes/`; repair or remove the alias before retrying. Dangling symbolic refs are omitted by Git's enumeration and are not detected: this is a known limit shared with plain `git fetch`. Git retains its configured pruning, automatic tag following, negative exclusions and `followRemoteHEAD` behavior; automatic tag following is not an explicit fetch refspec. Combined dry runs evaluate local bases against tips planned for primary and earlier workspaces, in selection order. Planned branch tips are keyed by full refs: the short branch name `refs/heads/a` maps to `refs/heads/refs/heads/a`, independently of branch `a` at `refs/heads/a`. When an earlier planned tip is unknown, dependent items report `updated`, `after:null`, `strategy:null` and a dependency explanation in `detail`, without deciding against the old base.

## Bases

`create` records the normalized base; records without a base fall back to the repository default unless explicitly cleared. `set --unset base` and adoption without a base report `base-unknown` until `set --base REF` supplies one. A removed base reports `base-gone` with a repair hint. Explicitly cleared bases do not cause a fetch by themselves.

## Strategies and published history

Strategy precedence is `--auto`/`--rebase`/`--merge`, user `sync.strategy`, then `auto`. Auto rebases when none of the workspace's own commits (`base..branch`) is reachable from an observed remote-tracking ref, configured fetch destination, or upstream; otherwise it merges. Independent local branch destinations count too, including those of remotes not fetched by this sync; only the current branch itself is excluded from publication evidence. An explicit rebase refuses published commits with exit 4 `published` and a `--rewrite-published` hint, unless `--rewrite-published` or user `sync.rewrite-published=true` allows it. Permission does not select a strategy: auto still merges published work. Items report `strategySource` (`flag`, `config`, `default`) separately from `rewriteAllowed` and `rewriteSource` (`flag`, `config`, or null). Explicit config false has source `config`.

When published history is actually rebased, `rewrotePublished:true` and a `published_rewritten` warning identify the workspace: “updating the existing remote branch may require a force push”. Sync leaves the remote branch and upstream settings untouched. Personal policy belongs in [user configuration](configuration.md), never in repository `.rabi.toml`.

## Integration guards

Rebase preserves upstream cherry-picks (including reverted ones), intentionally empty commits, and commits that become empty. It requires Git 2.29 or later; verifying that merge commits can be flattened requires Git 2.38 or later. Merges with their own changes or conflict resolutions, or whose automatic merge trees cannot be verified, fail untouched with a `--merge` hint. Dirty workspaces (including untracked non-ignored files), detached workspaces, and already integrated branches are skipped. Ignored paths that integration could overwrite cause a dirty skip, respecting `core.ignorecase`. Selected linked workspaces on the default branch retain the ff-only guard, regardless of strategy or rewrite permission. Local divergence is retained. Fetch failures affect items using that remote; other bases can proceed.

## Conflicts and partial completion

Conflicts list the files and roll back HEAD, index and contents (exit 5). `--keep-conflicts` leaves one workspace's operation in progress and requires exactly one workspace integration target; the optional primary update remains ff-only. Exit 6 takes precedence if another item changed before a failure, conflict or published refusal. Without changes, operational failures exit 1, conflicts 5, published refusals 4; skipped workspace outcomes exit 0. Primary-only dirty/diverged retains exit 1. Usage errors exit 2. There are no owners, internal locks, or in-use detection.

## Results

Human output has one `<name>  <status>  [detail]` line per item and a status count. JSON uses envelope schema 2 with `data: {plan, items:[…], main:item|null}`. Each item contains `workspace` (the shared observed workspace object), `status`, `strategy`, `strategySource`, `rewriteAllowed`, `rewriteSource`, `rewrotePublished`, `before`, `after`, `conflicts`, `detail`, and `code` (e.g. `published`). The observed workspace describes the checkout before integration; `before`/`after` report full HEAD IDs. Diagnostic aliases `name`, `path`, `branch`, and `base` are also included. `strategy` is `rebase`, `merge`, `ff-only`, or null when skipped before selection. Unavailable fields are null.

Statuses: `updated`, `up-to-date`, `dirty`, `diverged`, `integrated`, `detached`, `base-gone`, `base-unknown`, `conflict`, `refused`, `failed`, plus primary's `not-default-branch`.

## Example

From primary, with `feature/auth` and `feature/tests` already created with base `main`:

```sh
rabi sync feature/auth feature/tests --merge
rabi sync --all --main --dry-run --json
rabi sync feature/auth --rebase --rewrite-published
rabi sync feature/auth --keep-conflicts
rabi set feature/auth --base main
```

`--rewrite-published` is explicit permission to rewrite published commits. Use it only when that is intended. See [the JSON contract](json.md) for item fields and [exit codes](exit-codes.md) for partial completion.
