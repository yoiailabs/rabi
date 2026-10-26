# Configuration

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

`rabi` stores user configuration in `$XDG_CONFIG_HOME/rabi/config.toml` (default `~/.config/rabi/config.toml`). `rabi config get`, `set`, `unset`, and `list` read and write that file. The first `set` creates the directory and file.

Config, state, metadata, and default workspace storage use only `rabi` paths. Project markers use `.rabi-common-dir`; discovery and stale clone staging cleanup recognize `.rabi-clone-`.


Run state is shared under `<git-common-dir>/rabi/runs/`, independent of workspace-root. Each workspace has one current resumable parent session and retained launch records. There are no run configuration keys. See [commands](commands.md) and [records](json.md).

When `workspace-root` is unset, `rabi create` places the new workspace at `$XDG_DATA_HOME/rabi/workspaces/<checkout>-<hash>/<name>`. The fallback is `~/.local/share/rabi/workspaces/...` when `XDG_DATA_HOME` is unset, empty, or not absolute. `<checkout>` is the canonical primary checkout directory name, with characters outside letters, numbers, `.`, `_`, and `-` replaced by `-`. A symlink to that checkout uses the same directory. `<hash>` is the first 8 hexadecimal digits of the SHA-256 of the canonical Git common directory. Two clones of the same remote get different directories. Workspace names retain their `/` hierarchy. Deleting the last workspace keeps the repo directory and its `.rabi-common-dir` marker so concurrent and subsequent creates can reuse it safely. `remove` and `gc` also remove a linked workspace recorded in `rabi/workspaces/<id>.json`, including one adopted or created under a different workspace root. An unrecorded workspace outside the configured directory stays. Empty parent directories below the repo directory are cleaned up only for a workspace inside the resolved directory. A set `workspace-root` may be absolute, start with `~/`, or be relative to the repository root, and that directory is left in place when it becomes empty.

`clone-root` is where `rabi clone` places repositories. It is an absolute path or a path starting with `~/`. When `clone-root` is unset, `rabi clone` uses the first `git config ghq.root` value, then `$GHQ_ROOT`, then `~/ghq`. A configured `clone-root` is used without reading git config. If reading `ghq.root` fails, `rabi` warns and continues with `$GHQ_ROOT` or `~/ghq`. `repo list` scans that same directory.

`config get KEY` and `config list` return effective values, including defaults. Add `--sources` to see `value`, `source` (`config`, `env`, or `default`), and `detail`. JSON stores those values under `data.config[KEY]`; without `--sources`, each entry is the value itself. `workspace-root` defaults to null (the XDG layout), `discover-ghq` to true. The clone-root default chain stays `ghq.root` → `$GHQ_ROOT` → `~/ghq`, below user config. `sync.strategy` accepts `auto` (default), `rebase`, or `merge`; `sync.rewrite-published` is a boolean (default false). Sync flags override these user settings. Invalid policy values exit 2. Store them under `[sync]` in user config.toml; `.rabi.toml` rejects personal sync policy.

## Cloning repositories

`rabi clone` clones a Git repository into `<root>/<host>/<owner>/<repo>` and registers it. `owner/repo` uses `github.com`. The command accepts `https` and `http` URLs, `ssh://` URLs, scp-style `git@host:owner/repo` paths, `host/owner/repo`, and `owner/repo`. The host directory is lowercased, and a URL port is part of it (`example.net:9443`). `file://` URLs are rejected. A local path has no host, owner, and repository to place under that layout. A clone that exits early can leave a hidden `.rabi-clone-<pid>-<uuid>` directory beside the destination. A later clone in that directory removes it when the recorded process is no longer running.

If that path is already the same repository, `rabi` registers it and prints the path instead of cloning again. `--update` fetches `origin` in that case. If the path exists but is a different repository, the command exits with a validation error. `--branch` and `--depth` apply only to a new clone.

Authentication is left to Git. `rabi` does not call `gh`. Git's progress is written to stderr. New workspaces for the cloned repository are created under `$XDG_DATA_HOME/rabi/workspaces/<checkout>-<hash>` (`~/.local/share/rabi/workspaces/...` when `XDG_DATA_HOME` is unset, empty, or relative). `<checkout>` is the canonical primary checkout directory name, so a symlink or a non-canonical path uses the same directory. `<hash>` is the first 8 hexadecimal digits of the SHA-256 of the canonical Git common directory, the same path `rabi trust` records. The `workspace-root` config key overrides that location; relative values are joined to the repository root.

## Repository registrations

`repo add` defaults to the selected repository or current checkout and registers its primary checkout. `repo list` combines registrations and discovered repositories; registered roots take precedence. `--registered` disables discovery; `--discovered` shows unregistered discovered roots. `discover-ghq=false` disables discovery. `repo show` reports `repo.root`, `name`, `defaultBranch` (null if unknown), `configPresent` for `.rabi.toml`, `trusted` (including `RABI_TRUST_ALL=1`), and `workspaceRoot`. REF accepts a registration ID, path, canonical name, or unambiguous suffix; omission uses `-R` or the current repository. `repo forget` removes only the registration, leaving files and workspaces intact. A forgotten repository may remain discoverable.

## Targeting a repository

The global `-R/--repo` option selects a repository. `skill list` uses the current checkout and does not accept it; `run -R` requires an explicit `SEL --`. Workspace commands (`create`, `adopt`, `forget`, `list`, `show`, `path`, `remove`, `sync`, `gc`, `exec`, `set`, `setup`, `handoff show|path|add`) accept `--repo <ref>` (`-R`) so they can run against a repository other than the current directory. It may precede the command or appear among its options. `rabi exec` is the exception: put rabi options before SEL and separate the child command with the required `--`. Everything after `--` is passed to the command.

A ref that starts with `/`, `.`, or `~` is a directory. It can be anywhere inside the repository, including a linked workspace, and resolves to the primary checkout. The selected primary checkout supplies `.worktreeinclude` and the copied files. With `-R`, hooks use the selected primary checkout; otherwise they use the invoking checkout; `setup` and `remove` can override the hook source with `--source-checkout`.

Any other ref is matched against registered repos and discovered checkouts (the same set as `repo list`, honoring `discover-ghq`). The canonical name is `host/owner/repo`: from the `origin` URL when it is an http(s), ssh, or scp-style remote, otherwise from a checkout stored at `<root>/<host>/<owner>/<repo>`. A local or relative origin, including `backups/widget.git`, is not a host-based name. `owner/repo` and `repo` match when only one repo has that suffix. The directory name shown by `repo list` matches too, including when it differs from the canonical name. A location name that differs from the remote also matches. Several matches is a usage error (exit code 2) that lists the candidates. No match is not found (exit code 3). A path that does not exist is not found and says the path does not exist. A `--repo` value that starts with `-` is a validation error.

## Effective keys

| Key | Default | Accepted values |
|---|---|---|
| `workspace-root` | null (XDG workspace layout) | Absolute path, `~/…`, or repository-relative path |
| `clone-root` | `ghq.root` → `GHQ_ROOT` → `~/ghq` | Absolute path or `~/…` |
| `discover-ghq` | true | Boolean |
| `sync.strategy` | auto | auto, rebase, merge |
| `sync.rewrite-published` | false | Boolean |

Unknown keys are rejected without rewriting the file. Unset, empty, or relative XDG config/state paths use their HOME defaults too.

For example, user `config.toml` can contain:

```toml
workspace-root = "../rabi-workspaces"
discover-ghq = true
clone-root = "~/ghq"

[sync]
strategy = "auto"
rewrite-published = false
```

Read and change configuration:

```sh
rabi config list --sources
rabi config get clone-root --sources --json
rabi config set workspace-root ~/workspaces
rabi config unset workspace-root
rabi config set sync.strategy rebase
rabi config set sync.rewrite-published true
rabi config unset sync.rewrite-published
rabi config unset sync.strategy
```

Register and inspect the current repository, then remove only its registration:

```sh
rabi repo add .
rabi repo list --registered --json
rabi repo show --json
rabi -R . list --json
rabi repo forget .
```

Clone examples below follow the command help; they require network access. Run them independently, using a repository you can access:

```sh
rabi clone yoiailabs/rabi
rabi clone git@github.com:yoiailabs/rabi.git --branch develop
rabi clone github.com/yoiailabs/rabi --depth 1
rabi clone yoiailabs/rabi --update
```
