# Exit codes

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

| Exit | Meaning | Example error codes |
|---|---|---|
| 0 | Success, including gc plans with protected candidates | |
| 1 | Operational failure | `failed`, `git_failed`, `io`, `command_failed`, `spawn`, `signaled` |
| 2 | Invalid invocation | `usage`, `ambiguous` |
| 3 | Not found or outside a repository | `not_found`, `branch_not_found`, `not_repository` |
| 4 | Safety refusal before publication or destructive changes | `dirty`, `unpreserved`, `published`, `exists`, `exists_remote`, `untrusted`, `primary`, `refused`, `precheck_changed`, `floor_changed`, `environment`, `diverged`, `destination_not_github` |
| 5 | Sync conflict, rolled back or kept with `--keep-conflicts` | `conflict` |
| 6 | Persistent changes followed by failure or partial success | `partial` |
| 10 | Task advance waiting for required CI | `waiting` |
| 124 | Agent launch wall-clock timeout | `failed` |
| 130 | Agent launch interrupted by SIGINT/SIGTERM | `failed` |

`refused` is the fallback safety error when no more specific code applies. Text and JSON use the same exit codes. Standalone `exec` preserves its child’s status, including 128+signal, and does not return an envelope.

Create may retain a workspace and exit 6 if metadata, copying, or setup fails. Read `data` before handling the error. A remove failure after persistent removal exits 6; use `error.details.retryCommand` after fixing the cause. Repeating the original remove command can exit 3 because completed targets have already gone. Sync uses exit 6 if another selected item changed before an error, conflict, or refusal.

GC plans and protected skips succeed with exit 0. A warning does not by itself mean failure. The complete error and warning code lists are in `rabi help --json` (`data.errorCodes` and `data.warningCodes`). See [JSON](json.md), [safety](safety.md), and [sync](sync.md) for per-command details.

`run` passes through the parent's exit status (0 does not imply task completion; signals use 128+signal). Launcher/protocol errors use 1, invalid options/timeout or a primary checkout use 2, untrusted or concurrent launches use 4. Timeout uses 124; SIGINT/SIGTERM received by rabi use 130. JSON retains run/launch data after launch failure and reports an error; the launch's exitCode records the parent's actual status separately. No rollback of parent changes or pushes occurs.

`task risk` returns 0 for every completed classification, including high and FLOOR. Invalid risk policies use 1 (`failed`); unresolved commits and failed Git inspection use 1 (`git_failed`). Unknown task subcommands and invalid arguments use 2 (`usage`); bare `rabi task` prints help and exits 0 like other command groups. Selector failures retain the shared not-found/ambiguity statuses. Only committed changes are classified; the working tree and index are not read.

`task classify` likewise exits 0 for every completed verdict, including high caused by provider launch/exit/timeout/answer failure, uncertainty, absent documents, or an oversized diff. These conditions appear in `reasons` as `classifier:*`; timeout is a high verdict, not exit 124. Git, invalid policy, selector, and invocation errors retain risk's codes; classification-record I/O failures exit 1.

`task status` succeeds with 0 when facts collection completes. `task merge --dry-run` succeeds with 0 for `blocked`, `incomplete`, and `ready`; item failures are evaluation results. Only collection, selection, policy or Git failures produce error envelopes. GitHub/API/auth failures use 1 (`failed`, or `spawn` if gh cannot start); missing resources use 3 (`not_found`). `task merge` without `--dry-run` refuses with 4 (`refused`) if preflight contains failed or unevaluated items. `--external-review` permits only the two missing task record items. Refusals retain the evaluation in `data` and write nothing. A failed gh merge or a PR not merged on reread exits 1 (`failed`) with reread facts in `data`; collection errors retain their existing codes. `task approve` uses 2 (`usage`) for a nonempty `RABI_AGENT`, invalid/abbreviated/mismatched SHA, or a PR that is not open, without writing. A completed status POST succeeds with 0; a failed POST uses 1 (`failed`).

`task new|list|show|abandon` reuse these codes: 0 for success (including list entries displayed as blocked with warnings), 1 (`failed`) for corrupt or unreadable task records/events, invalid or mismatched repository manifests, and other store/Git/remote-resolution failures, 2 (`usage`) for invalid intent/branch/base, branch reservations, short or ambiguous ID prefixes, or terminal transitions, and 3 (`not_found`) for an absent task ID. No exit codes are added.

Task record errors in show/abandon name the task ID. Creation fails closed with exit 1 and the unreadable task ID when it cannot verify branch reservations. Incomplete `.staging-<uuid>` directories are ignored. Invalid record data is an operational failure, not a usage error.

`task implement` passes through the provider status (signals use 128+signal); exit 0 does not declare task completion. Timeout returns 124 and SIGINT/SIGTERM received by rabi returns 130 after process-group cleanup. Launch/store/Git failures use 1, invalid arguments use 2, absent tasks use 3, and untrusted repositories, concurrent launches, invalid task states or unavailable linked workspaces use 4. Events and local edits/commits are retained after provider failure.

`task publish` succeeds with 0 after recording the verified PR head and entering `ci`. Precheck command failure, timeout or interruption uses 1 (`precheck_failed`), records the result, and returns directly to `implementing`, including from `publishing`. Missing tools block as `environment` with 4. Changed checks, floor paths, divergent remote branches and invalid GitHub push destinations block with 4 (`precheck_changed`, `floor_changed`, `diverged`, `destination_not_github`); dirty work, untrusted repositories, concurrent launches and invalid task states also use 4. Destination blocking requires an invalid recorded URL or a proven mismatch with gh's resolved repository. Failure to resolve that repository (including network, authentication, rate-limit, timeout or ambiguous-default failures), unambiguously resolve its base branch, obtain the exact base commit or verify common ancestry uses a retryable error (1), leaves task state unchanged and performs no push. Invocation/selector errors retain 2/3. Git, configuration and store failures use 1. A failure after a successful or reconciled push uses 6 (`partial`); the remote branch or PR may already exist. Retry a task still in `implementing`, `prechecking` or `publishing` after repairing the cause. A blocked task requires human intervention.

`task advance` returns 0 for progress, reaching `reviewing`, or an already merged task; 10 (`waiting`) for pending required CI without `--wait`; and 4 for blocked or abandoned tasks and unsupported states. Block reasons include `no_progress`, `no_ci`, `ci_timeout`, `budget`, and `repeated_failure` (the same failure fingerprint as the immediately previous fix attempt, even on a new HEAD). With `--wait`, pending CI is polled until a stopping condition. Ordinary provider failures block as `provider_failed` with 4; provider timeout/interruption also block as `provider_failed` while retaining 124/130. Cancellation during prechecks or log retrieval stops `--wait` with 130. Publication, usage, selector, Git and store errors retain their existing codes. The result remains in `data` for waiting and blocked outcomes. See [advance](commands.md#task-advance).
