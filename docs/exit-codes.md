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

`refused` is the fallback safety error when no more specific code applies. Text and JSON use the same exit codes. Standalone `exec` preserves its child’s status, including 128+signal, and does not return an envelope.

Create may retain a workspace and exit 6 if metadata, copying, or setup fails. Read `data` before handling the error. A remove failure after persistent removal exits 6; use `error.details.retryCommand` after fixing the cause. Repeating the original remove command can exit 3 because completed targets have already gone. Sync uses exit 6 if another selected item changed before an error, conflict, or refusal.

GC plans and protected skips succeed with exit 0. A warning does not by itself mean failure. The complete error and warning code lists are in `rabi help --json` (`data.errorCodes` and `data.warningCodes`). See [JSON](json.md), [safety](safety.md), and [sync](sync.md) for per-command details.


`workflow list/show/check` use the existing exit table. Unknown workflow names exit 3 (`not_found`). Invalid definitions exit 2 (`usage`); check keeps all items and diagnostics in data. Reading and validation require no trust and run no workflow commands.

`run start` uses 2 for invalid definitions/options, 4 (`untrusted`) for untrusted execution before allocation, and 6 (`partial`) when execution stops after persistent changes. Operational preflight errors use their ordinary status. `run show` exits 0 for any readable run, including stopped runs; unknown IDs exit 3 and malformed IDs exit 2.
