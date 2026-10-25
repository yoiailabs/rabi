# Hooks, trust, and setup

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

## Copying gitignored files

`rabi create` and `rabi setup` read `.worktreeinclude` and copy files from the **primary checkout**, even when run in a subdirectory or another workspace. The file uses gitignore syntax. A file is copied only when a pattern matches it and Git ignores it, so tracked files are never duplicated.

```text
.env
.env.local
config/secrets.json
```

Files that already exist in the new workspace are left unchanged. Symlinks are skipped and are not followed. A `.worktreeinclude` that is itself a symlink is not read.

`--copy-file` adds extra gitignore patterns for that command.

## Re-running setup

`setup [SEL]` defaults to `.` and uses the shared workspace selectors. The primary checkout is refused with exit 4 (`primary`), since it is the copy source. An untrusted setup hook refuses with exit 4 (`untrusted`) **before copying or updating records**. Missing targets exit 3. Copy or hook failures exit 1; a successful setup exits 0. Existing target files are preserved; a copying failure stops before hooks run.

After create exits 6 because setup failed, run the retry command from the error in bash or zsh: `cd '/absolute/source/checkout' && rabi setup 'SELECTOR'`. It preserves the original checkout's hook configuration and uses the workspace name or a unique PR/issue selector. If neither resolves uniquely, the hint uses `(cd WORKSPACE && rabi setup . --source-checkout SOURCE)` to retain the original hooks while selecting the target with dot. `--source-checkout` must belong to the same repository. The hint retains every `--copy-file` pattern, using ANSI-C quoting (`$'...'`) for control characters. `create NAME --reuse` only returns the matching workspace and never reruns setup. Setup hooks must be safe to run again.

Every shared workspace JSON object includes `setup: {state, at, error}`, or `null` for an unrecorded checkout (including after `forget`). New creations record `pending`, then `running`, then `ok` or `failed`; `--no-setup` records `skipped`. Records without setup information, including adopted records, report `unknown` with null time/error. Setup records the current attempt's timestamp and error, clearing the error on success. `data.setup` also includes `ran`, `ok`, `command`, `exitCode`, and `signal` for the invocation; `data.copy` contains `copied` paths and `error`.

A running attempt records its process ID and host internally. If that process no longer exists on the same host, inspection displays `interrupted` without changing the record. A different or unknown host keeps `running`. This is informational: it does not lock a workspace or block setup or any other operation. Concurrent updates of one record still use the last write.

## Repository hooks

`.rabi.toml` is read from the invoking checkout, or from the selected primary checkout when `-R/--repo` is supplied. `setup` and `remove` accept `--source-checkout PATH` to use another checkout in the same repository. `.worktreeinclude` and the files to copy always come from the primary checkout.

```toml
[hooks]
setup = ["git status --short"]
pre-remove = ["git diff --quiet"]
```

Replace these example hook commands with your project’s preparation and teardown steps.

`rabi create` and `rabi setup` run `setup` in the target workspace after copying. Write setup hooks to tolerate repeated execution. `rabi remove` and `rabi gc --apply` run `pre-remove` in the workspace before removing it. Commands run in order and stop at the first failure. Unknown keys are rejected.

Hooks run only when that repository is trusted. `.worktreeinclude` copies are not hooks and do not need trust. A repository with no hook commands never asks.

On a terminal, rabi prints the repo path and the hook commands, then asks `Trust this project and run its hooks? [y/N]`. `y` or `yes` saves trust and runs the hooks. rabi does not ask when `CI` is nonempty, when `--no-input` or `RABI_NO_INPUT` is enabled, when stdin or stderr is not a terminal, or when JSON output is selected. It then exits 4 before changing the repository, the same as a non-terminal. With `--json`, stdout contains one error envelope with `error.code` set to `untrusted`. The message tells you to run `rabi trust add` or pass `--no-setup` for create / `--no-hooks` for setup, remove, and gc.

`rabi trust add [REPO]` shows hooks on stderr and records trust without prompting. `rabi run` uses this same common-directory trust and refuses untrusted execution without prompting. `rabi trust remove [REPO]` removes the grant, and `rabi trust list` prints trusted common directories and grant times. REPO uses the same paths and names as `-R`; omission uses `-R` or the current repository. Trust is keyed by the canonical Git common directory, so the primary checkout and linked workspaces share a grant. Re-cloning at the same path keeps trust until `rabi trust remove`. Editing the hook commands does not ask again. The trust prompt is available during create, setup, and remove at a human TTY.

The record is `$XDG_STATE_HOME/rabi/trust.toml` when `XDG_STATE_HOME` is an absolute path, otherwise `~/.local/state/rabi/trust.toml`. Each entry stores `path` and `trustedAt`.

`RABI_TRUST_ALL=1` trusts every repository for that process and does not write the file. The value has to be the single character `1`. Use it in CI and containers. `create --no-setup` skips copying and hooks. `setup --no-hooks` copies files only; `remove --no-hooks` and `gc --no-hooks` skip removal hooks. These flags do not write trust. `--no-setup` cannot be combined with `--copy-file`.

`rabi gc` never asks, including on a terminal. An untrusted repository that has `pre-remove` hooks is skipped. The candidate detail is `untrusted hooks`. Other candidates still run. `--no-hooks` removes those workspaces without running hooks.

Hooks and `rabi exec` receive the same environment:

- `RABI` — `1`
- `RABI_WORKSPACE` — workspace name
- `RABI_WORKSPACE_ID` — recorded `ws_` ID, or empty when unrecorded
- `RABI_WORKSPACE_PATH` — absolute workspace path
- `RABI_REPO_PATH` — absolute primary checkout path, even when hooks are read from a linked workspace

## Try setup safely

In a repository with the `.rabi.toml` above, inspect its hooks and grant trust before creating the workspace:

```sh
rabi trust add
rabi trust list --json
rabi create feature/hooks --base main --no-setup
rabi setup feature/hooks
rabi setup feature/hooks --no-hooks --copy-file '*.env'
rabi remove feature/hooks
rabi trust remove
```

A skipped setup can be prepared later; `--no-hooks` still performs copying. See [configuration](configuration.md) for storage paths and [safety](safety.md) for removal retries.
