# Exit codes

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

| Exit | Meaning | Example error codes |
|---|---|---|
| 0 | Success, including gc plans with protected candidates | |
| 1 | Operational failure | `failed`, `git_failed`, `io`, `command_failed`, `spawn`, `signaled` |
| 2 | Invalid invocation | `usage`, `ambiguous` |
| 3 | Not found or outside a repository | `not_found`, `branch_not_found`, `not_repository` |
| 4 | Safety refusal before changes | `dirty`, `unpreserved`, `published`, `exists`, `exists_remote`, `untrusted`, `primary`, `refused` |
| 5 | Sync conflict, rolled back or kept with `--keep-conflicts` | `conflict` |
| 6 | Persistent changes followed by failure or partial success | `partial` |
| 124 | Run wall-clock timeout | `failed` |
| 130 | Run interrupted by SIGINT/SIGTERM | `failed` |

`refused` is the fallback safety error when no more specific code applies. Text and JSON use the same exit codes. Standalone `exec` preserves its child’s status, including 128+signal, and does not return an envelope.

Create may retain a workspace and exit 6 if metadata, copying, or setup fails. Read `data` before handling the error. A remove failure after persistent removal exits 6; use `error.details.retryCommand` after fixing the cause. Repeating the original remove command can exit 3 because completed targets have already gone. Sync uses exit 6 if another selected item changed before an error, conflict, or refusal.

GC plans and protected skips succeed with exit 0. A warning does not by itself mean failure. The complete error and warning code lists are in `rabi help --json` (`data.errorCodes` and `data.warningCodes`). See [JSON](json.md), [safety](safety.md), and [sync](sync.md) for per-command details.

`run` passes through the parent's exit status (0 does not imply task completion; signals use 128+signal). Launcher/protocol errors use 1, invalid options/timeout or a primary checkout use 2, untrusted or concurrent launches use 4. Timeout uses 124; SIGINT/SIGTERM received by rabi use 130. JSON retains run/launch data after launch failure and reports an error; the launch's exitCode records the parent's actual status separately. No rollback of parent changes or pushes occurs.
