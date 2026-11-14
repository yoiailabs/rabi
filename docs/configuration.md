# Configuration

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

`rabi` stores user configuration in `$XDG_CONFIG_HOME/rabi/config.toml` (default `~/.config/rabi/config.toml`). `rabi config get`, `set`, `unset`, and `list` read and write that file. The first `set` creates the directory and file.

Config, state, metadata, and default workspace storage use only `rabi` paths. Project markers use `.rabi-common-dir`; discovery and stale clone staging cleanup recognize `.rabi-clone-`.


Run state is shared under `<git-common-dir>/rabi/runs/`, independent of workspace-root. Each workspace has one current resumable parent session and retained launch records. There are no user-config keys for runs; repository execution environment is declared separately below. See [commands](commands.md) and [records](json.md).

When `workspace-root` is unset, `rabi create` places the new workspace at `$XDG_DATA_HOME/rabi/workspaces/<checkout>-<hash>/<name>`. The fallback is `~/.local/share/rabi/workspaces/...` when `XDG_DATA_HOME` is unset, empty, or not absolute. `<checkout>` is the canonical primary checkout directory name, with characters outside letters, numbers, `.`, `_`, and `-` replaced by `-`. A symlink to that checkout uses the same directory. `<hash>` is the first 8 hexadecimal digits of the SHA-256 of the canonical Git common directory. Two clones of the same remote get different directories. Workspace names retain their `/` hierarchy. Deleting the last workspace keeps the repo directory and its `.rabi-common-dir` marker so concurrent and subsequent creates can reuse it safely. `remove` and `gc` also remove a linked workspace recorded in `rabi/workspaces/<id>.json`, including one adopted or created under a different workspace root. An unrecorded workspace outside the configured directory stays. Empty parent directories below the repo directory are cleaned up only for a workspace inside the resolved directory. A set `workspace-root` may be absolute, start with `~/`, or be relative to the repository root, and that directory is left in place when it becomes empty.

`clone-root` is where `rabi clone` places repositories. It is an absolute path or a path starting with `~/`. When `clone-root` is unset, `rabi clone` uses the first `git config ghq.root` value, then `$GHQ_ROOT`, then `~/ghq`. A configured `clone-root` is used without reading git config. If reading `ghq.root` fails, `rabi` warns and continues with `$GHQ_ROOT` or `~/ghq`. `repo list` scans that same directory.

`config get KEY` and `config list` return effective values, including defaults. Add `--sources` to see `value`, `source` (`config`, `env`, or `default`), and `detail`. JSON stores those values under `data.config[KEY]`; without `--sources`, each entry is the value itself. `workspace-root` defaults to null (the XDG layout), `discover-ghq` to true. The clone-root default chain stays `ghq.root` → `$GHQ_ROOT` → `~/ghq`, below user config. `sync.strategy` accepts `auto` (default), `rebase`, or `merge`; `sync.rewrite-published` is a boolean (default false). Sync flags override these user settings. Invalid policy values exit 2. Store them under `[sync]` in user config.toml; `.rabi.toml` rejects personal sync policy.

## Repository execution environment

Declare stable toolchain wiring in the **primary checkout's** `.rabi.toml`:

```toml
[env]
path-prepend = ["~/.cargo/bin"]

[env.set]
RUSTUP_TOOLCHAIN = "1.99.1"
```

For Rust projects, keep `rust-toolchain.toml` as the toolchain declaration and prepend `~/.cargo/bin` so Cargo is available. Set `RUSTUP_TOOLCHAIN` only when you want to override the toolchain file. Do not store secrets in committed configuration.

Task-launched providers do not apply repository `[env]`; they use the [task agent boundary](safety.md#task-agent-launch-boundary).

`rabi run` (Codex, Cursor, and Claude Code) and `rabi exec` read this environment from the primary checkout on **every launch**, including resume, with or without `-R`. A linked workspace's `.rabi.toml` cannot change its next parent's environment. Branch edits take effect only once they are present in the primary checkout. Both `path-prepend` and `set` are optional; an empty `[env]` does nothing. Unknown fields are rejected.

Execution environment needs the repository's common-directory trust (`rabi trust add` or `RABI_TRUST_ALL=1`). `trust add` lists declared `KEY=value` assignments and path-prepend entries alongside hooks, escaping control characters with the same visible-text rules as hook commands; its JSON data remains `{path,trustedAt}`. Run already refuses untrusted repositories. Exec never prompts: for nonempty untrusted declarations it warns with `env_untrusted`, then runs with the inherited environment and rabi's workspace variables. Use `--no-env` on run or exec to skip repository environment, its value validation, and this warning. TOML syntax and configuration structure are still checked.

Expansion accepts only a leading `~/` (using HOME), `${HOME}`, `${RABI_WORKSPACE_PATH}` (the selected checkout), and `${RABI_REPO_PATH}` (the primary checkout). Every other use of `$` is an error; there is no shell evaluation, command substitution, recursive expansion, or general environment-variable interpolation. Other text is literal. HOME must be set and nonempty when an applied declaration needs it. Referenced workspace and repository paths must be UTF-8; expansion fails clearly rather than replacing invalid bytes. rabi computes these paths itself, including with `run --no-handoff`.

Each `path-prepend` entry must be nonempty, contain neither NUL nor `:`, and be absolute after expansion. Variable keys must match `[A-Za-z_][A-Za-z0-9_]*`. Set values may be empty but cannot contain NUL. Validation errors exit 1 and identify the file and key. Normal config loading checks key grammar, reserved keys, expansion grammar and allowed variable names, literal NUL, and literal `:` in PATH entries without reading HOME or expanding checkout paths. HOME availability, nonempty/absolute expanded PATH entries, and `:` or NUL introduced by expansion are checked only when the environment is actually applied to a trusted run or exec. Inspection, setup, removal, and gc do not require HOME merely because `[env]` uses it. These variable keys are forbidden:

- `PATH` (use `path-prepend` instead), any key starting with `RABI`, `HOME`, `SHELL`, `CODEX_HOME`, and `CLAUDE_CONFIG_DIR`.
- Any key starting with `LD_` or `DYLD_`, plus `NODE_OPTIONS`, `BASH_ENV`, and `ENV`.
- XDG base-directory variables: `XDG_STATE_HOME`, `XDG_CONFIG_HOME`, `XDG_DATA_HOME`, `XDG_CACHE_HOME`, and `XDG_RUNTIME_DIR`.
- Git repository-selection, configuration, and execution variables: `GIT_DIR`, `GIT_WORK_TREE`, `GIT_INDEX_FILE`, `GIT_COMMON_DIR`, `GIT_OBJECT_DIRECTORY`, `GIT_ALTERNATE_OBJECT_DIRECTORIES`, `GIT_NAMESPACE`, `GIT_CEILING_DIRECTORIES`, `GIT_DISCOVERY_ACROSS_FILESYSTEM`, `GIT_CONFIG`, and `GIT_EXEC_PATH`; any key starting with `GIT_CONFIG_`, including `GIT_CONFIG_GLOBAL`, `GIT_CONFIG_SYSTEM`, `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_n`, `GIT_CONFIG_VALUE_n`, and `GIT_CONFIG_PARAMETERS`.

These reservations keep nested rabi commands, handoff storage, and the Git executor attached to the launcher's state, configuration, and repository.

Precedence is inherited environment, then `[env.set]`, then rabi's own `RABI_*` variables. PATH consists of prepend entries in declaration order followed by inherited PATH; an empty or unset inherited PATH adds no separator. Exec looks up its child command using this PATH. Run resolves the provider executable to an absolute path using the launcher's original PATH first, then applies eligible declarations only to that provider process. Text run output reports applied variable keys and the applied PATH-entry count, never set values.

For **run only**, a `path-prepend` entry is skipped if it references `${RABI_WORKSPACE_PATH}` or its expanded path is equal to or inside the target workspace after lexical normalization or canonicalization. Canonical checks also detect existing symlink aliases. A `[env.set]` value referencing `${RABI_WORKSPACE_PATH}` is also skipped. Each skip emits `env_workspace_path_skipped` on stderr, identifying the entry without printing its value: the provider process must not take executables from the workspace. The applied summary counts only retained entries. Exec continues to apply these declarations, including workspace-relative PATH entries, because its caller selects the command.

Declare only directories that agents cannot write. Avoid world-writable temporary directories. Declared PATH entries select interpreters used by provider launch scripts such as `#!/usr/bin/env node`, as well as tools. Resolving the provider script itself to an absolute path does not fix its interpreter lookup. The run skip rule protects the selected workspace; it does not establish that other declared directories are safe.

Eligible environment reaches the provider process. Provider settings and login shells can change the environment of subsequent tool shells; for example, Codex `shell_environment_policy` can filter inherited variables. Workspace-relative PATH entries still let workspace binaries shadow tools in exec. Review both declarations and directory write access before granting trust. Never commit secrets in `[env]`; trust output intentionally shows set values.

Setup and pre-remove hooks do not receive `[env]`. A hook can opt into it by running `rabi exec . -- <cmd>` in its target workspace. See [hooks and trust](hooks.md).

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

## Repository risk policy

`rabi task risk` reads `.rabi/policy.toml` from the resolved **base commit blob**, never the head or working tree. A missing file uses built-in defaults: unmatched files and all kinds are high, with no size limits. Invalid TOML, unknown fields/sections, wrong types, or invalid values fail with exit 1 (`failed`); there is no fallback for an invalid policy. `.rabi/policy.toml` must be a regular file in the base commit; a symlink, directory, or submodule there fails with exit 1 (`failed`). If `.rabi` itself is not a directory in the base commit, the policy is treated as missing. Other repository configuration remains in `.rabi.toml`.

```toml
schema = 1
# Optional; defaults to high for a path matching no rule.
default-tier = "high"

[[rule]]
paths = ["docs/**"]
tier = "low"

[[rule]]
paths = ["install.sh"]
floor = true

[limits]
max-changed-lines = 600
max-files = 40

[kinds]
binary = "high"
symlink = "high"
submodule = "high"
exec-bit = "high"

[ci]
required = ["Rust fmt", { name = "Rust test", app = "github-actions" }]
app = "github-actions"
```

`schema` is required and must be integer 1. `default-tier` and each kind accept only `low`, `medium`, or `high`, defaulting to high. Rules require a nonempty `paths` array of nonempty strings, and either a tier or `floor = true`; floor defaults to false and implies high. All matching rules contribute their maximum tier. Paths matching no rule use `default-tier`. Renames and copies evaluate old and new paths independently, taking the maximum and retaining either side's floor.

Policy globs are case-sensitive and anchored at the repository root, matching the entire path. Leading `/`, trailing `/`, and repeated `//` are rejected with an error naming `rule[i].paths`. `*` matches zero or more characters within one path segment, `?` matches one character within a segment, and `**` matches across segments. `**/` can match zero directories. A trailing `/**` matches everything below that directory (`docs/**` matches `docs/a/b.md`, not `docsx/a` or the file `docs`). Dotfiles match normally. All other characters are literal: no basename search, implicit leading-slash removal, negation, brackets, or backslash escapes. Use `**/*.md` for Markdown at any depth, or `*.md` only at the root.

Built-in floor paths `.rabi/policy.toml`, `.rabi.toml`, and the `.github/` prefix are compared ASCII case-insensitively and always yield high and `floor: true`, regardless of policy rules. This also protects spellings such as `.RABI.TOML`, `.rabi/Policy.toml`, and `.GitHub/workflows/check.yml` on case-insensitive filesystems. The `.github/` prefix check works on raw bytes even if the remaining path is non-UTF-8. A floor indicates that a human is always required; `task risk` itself is read-only; `task merge` requires explicit approval for floors. Policy floor rules add to these built-ins.

Binary files (numstat `-` counts) contribute zero changed lines. Committed `.gitattributes` are read from the base commit, so `.gitattributes` edits on the head branch, staged, or uncommitted cannot change binary detection or line totals. Symlink mode 120000 or submodule mode 160000 on either side raises the file to that kind's tier. A new executable file (100755), or a mode change to/from 100755 (including deletion), raises it to `exec-bit`; ordinary content edits of an existing executable alone do not. Kinds can raise, never lower, a path's tier. Optional limits must be positive integers: strictly exceeding total added+deleted lines or changed-file count raises the overall tier to high; equality is allowed. Limits do not set floor.

Non-UTF-8 paths, unrecognized statuses, or malformed diff data always classify high with an `unknown:*` reason. Every non-UTF-8 path also sets `floor: true` with reason `floor:unknown:path-encoding`: policy floor rules cannot be evaluated safely, so human review is always required. Built-in floors still add their canonical reasons when identifiable. Paths are matched as Git reports them, without lossy decoding; non-UTF-8 paths use replacement characters only for output. Reasons identify contributing rules (`rule:docs/**`), defaults (`default-tier`), floors (`floor:builtin:.github/**`, `floor:rule:install.sh`), kinds (`kind:binary`), limits (`limit:max-files`), or unknown data. An empty diff yields low and `no changes`.

Only committed changes are classified; the working tree and index are not read.

The optional `[classifier]` section selects the written policy, provider, and optional model for `task classify`:

```toml
[classifier]
doc = "risk-tiers.md"
provider = "claude" # Optional: claude is the default and only supported provider.
# model = "your-model" # Optional: omit to use the provider CLI default.
```

`doc` is a literal repository-root-relative path to a regular UTF-8 file in the **base commit**. Absolute paths, empty components, `.` and `..` components, NUL, and symlink traversal do not identify a document. No filesystem expansion or working-tree fallback occurs. Missing or unreadable documents and blank contents produce high with `classifier:no-policy-doc`, without launching a provider. Absent `doc` or policy does the same; an empty policy is accepted only by `task classify` for this fallback. Other malformed policy data still fails validation. Unknown classifier fields and providers (including `codex`) are invalid policy data and fail policy parsing. `--provider` overrides the policy provider; unsupported flag values exit 2. Only tool-less providers are supported. Claude disables all tools with `--tools ""`; Codex cannot currently guarantee a tool-less invocation without an injected model catalog and is unsupported. When `model` is absent, rabi leaves model selection to the CLI. When set, it must be non-empty and must not start with `-`; it is passed as a single `--model=<value>` argument to the selected provider. rabi does not otherwise validate model names or supply a model catalog.

`task classify` ignores path rule tiers and `default-tier`; it retains floors and high kind, limit, and unknown-data safeguards before requesting the written assessment. Kind tiers below high do not adjust the provider verdict. `task risk` continues to use path tiers as before. Merge uses a matching exact-head classification record only when its merge-base equals the locally computed PR merge-base and its policy/document blob IDs match the PR base SHA, retaining the deterministic safeguards; otherwise its existing path classification remains in use and `classification_record` is not evaluated. Records are local trusted Git state; see [safety](safety.md#task-classifier).

The optional `[ci]` section declares required check runs for `task merge` and `task merge --dry-run`. `required` is an array of nonempty check names or `{ name, app }` tables; tables require both fields and reject unknown keys. String entries use `[ci] app`, which defaults to `github-actions`. Names and app slugs cannot be empty or whitespace-only. Check names and app slugs match exactly. A missing or empty `required` array blocks preflight with `no_required_checks`; no checks are inferred from GitHub branch protection. Other keys and types are rejected. `task risk` accepts this section without changing its classification.

Merge preflight keeps using the policy at the PR’s **base SHA**, independently of `--base` (which selects the expected base branch), and requires its content to match `.rabi/policy.toml` at the current base-branch tip obtained through a GitHub GET. Comparison is byte-for-byte; absence at both is equal. If policy content differs, the approval item fails and asks to update the PR. If the tip cannot be determined or its policy cannot be read, approval also fails. This freshness check applies before auto-tier or status approval; all other preflight items are still evaluated. It reads local commit objects when available, otherwise uses read-only GitHub tree/blob endpoints. Neither path fetches or installs Git objects. Local task Git reads ignore replace refs and both default and inherited graft files. GitHub file paths, statuses, renames, additions and deletions drive classification. Local merge-base→head tree diffs supplement file kinds only when both commits and their merge-base are available locally, matching GitHub's PR comparison; their path set must match GitHub's. Policy and committed `.gitattributes` are read at the PR base SHA. Local Git state (objects, config and attribute files) is trusted input, not verified against GitHub; see [safety](safety.md#task-approval-and-merge). Missing local commits or an unavailable merge-base force high with `unknown:kinds`; truncated GitHub files prevent classification.

The optional merge and approval sections also live in this base-commit policy:

```toml
[merge]
auto = ["low", "medium"]
method = "squash"

[approval]
approvers = ["maintainer-login"]
```

`merge.auto` defaults to an empty array: every tier requires an explicit approval. It accepts only a subset of `low` and `medium`; `high` is rejected. A floor always requires explicit approval. `merge.method` accepts `squash` (default), `merge`, or `rebase`. `approval.approvers` defaults to an empty array of logins; empty or whitespace-only entries are invalid. Unknown keys, invalid tiers/methods and wrong types fail policy parsing with exit 1. These sections are accepted by `task risk` without changing classification.

Outside an auto tier, merge requires the latest `rabi/approval` commit status for the exact current head SHA to be `success`, with its creator login in `approval.approvers` (case-insensitive login comparison). An empty approvers list fails clearly; a newer failure, pending status, or unlisted creator supersedes an older success. Approval on an earlier head never counts. Both merge modes retain this configuration from the PR base SHA, independently of `--base`, and fail approval unless the current base-branch tip has identical policy content. The marker and token limitations are described in [safety](safety.md#task-approval-and-merge).

Every provider launched by `rabi run` receives `RABI_AGENT=1` unconditionally, including fresh sessions, resume, and `--no-handoff`. Repository `[env]` cannot override or remove it because all `RABI*` keys are reserved.
