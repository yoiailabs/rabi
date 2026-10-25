# Workflow definitions

[README](../README.md) · [Commands](commands.md) · [JSON](json.md)

`rabi workflow list`, `show NAME`, and `check [NAME]` inspect definitions without running steps, check commands, or agent tools. They need no trust.

The built-in `default` is embedded from [default.yaml](../src/workflow/default.yaml). Repository definitions are `.rabi/workflows/<name>.yaml` in the invoking checkout, or the selected primary checkout with `-R/--repo`. Selection uses the filename stem; the embedded `name` need not match it. A repository `default.yaml` cannot shadow the built-in and is an invalid item in an all-definition check. Listing includes definitions even when their contents are invalid.

```sh
rabi workflow list
rabi workflow show default --json
rabi workflow check
rabi -R owner/repo workflow check default --json
```

A definition has `schema_version: 1`, `name`, numeric `version`, `workspace: new`, `start`, required `limits: {max_steps: N}`, optional `loops: {name: N}`, and a `steps` map. The optional top-level `agent` supplies a provider (`codex`, `claude`, or `cursor`) and optional model/effort. Agent steps can override those fields individually; omitted model/effort inherit the workflow defaults, including when the provider changes. An agent step must resolve a provider. Model and effort strings are passed through without tool-specific validation.

```yaml
schema_version: 1
name: verify
version: 1
workspace: new
start: check
limits: {max_steps: 1}
steps:
  check:
    type: check
    commands: ["cargo test --locked"]
    on: {pass: succeeded, fail: stopped}
```

Step types:

- `agent`: `mode: edit|read-only`, `output: report-v1|review-v1`, `instruction`, optional `agent` overrides, and optional `context`. Context defaults to `fresh`, or uses `{continue: STEP}`.
- `check`: `commands: cli` or a list of command strings. `commands: cli` requires one or more `run start --check CMD` options.
- `system`: `action: prepare`.

Every step accepts `allow_dirty: true|false`, default false. Show includes it in `definition.steps[ID].allow_dirty`, a `steps[ID].allowDirty` summary, and the text step summary. The executor stops after a step ending with uncommitted non-ignored Git changes unless allow_dirty is true.

Every step has an `on` map. Its keys are `ok` for ordinary agent/system steps, `pass`/`revise` for review-v1, or `pass`/`fail` for checks. Each value names a step or the terminal `succeeded`/`stopped`, or is `{to: STEP, loop: NAME}`. Unspecified results are not inferred as success.

Validation checks YAML syntax/types, schema version, an existing start step, existing transition targets, valid result keys, positive max_steps/loop limits, defined loop references, and absence of cycles after loop edges are removed. Review-v1 requires fresh context. Continue targets must exist, be agent steps, and resolve to the same provider; their model and effort can differ. Read-only agent steps require a resolved provider with read-only enforcement capability: Codex is supported, Cursor is unsupported, and Claude is unverified and therefore unsupported. Step-level provider overrides are resolved before checking this capability. Validation imposes no reachability requirement, name regex, filename/name match, fixed step cap, or execution timeout.

`check` without NAME reports the built-in and every repository file. Invalid definitions exit 2 (`usage`), keeping all JSON items and diagnostics. `show` returns diagnostics for invalid definitions in `error.details.diagnostics`. Unknown names exit 3 (`not_found`). Diagnostics include a message, an available step path, and nullable one-based YAML line/column values.

`rabi trust add` displays repository workflow names and paths together with hooks. Workflow agent steps and check commands, including built-in workflows, use that same Git common-directory trust. No per-workflow approval or new trust store is added. Trust records do not validate definitions or enable execution through these inspection commands.

Internal run storage is shared through the Git common directory. A new run saves its manifest and copies of task.md/workflow.yaml, then stores attempts in distinct `steps/NNN-<step>/result.json` directories. Numbers are run-wide and monotonically increase; revisiting a step allocates a new number. Step IDs with path separators are encoded for directory names without restricting YAML names. Progress updates use the shared atomic writer; HEAD values are observations taken after execution. Existing schema-1 records remain readable. Before prepare, the workspace object describes the requested destination with a null ID; allocation fills in observed workspace metadata.

## Execution

The executor requires repository trust, validates command bindings and provider support before workspace creation, and runs from `start`. Only Codex is implemented. Prepare uses the existing allocation and setup hooks. Publication is an ordinary agent edit step defined in YAML. rabi advances on process/session success and the existing allow_dirty rule, without pushing, creating or inspecting PRs, checking remote refs, or extracting PR URLs. Checks run through `sh -c` in order and stop at the first nonzero exit. Agent steps use their exit status, while review-v1 reads the final non-empty verdict line. Missing transitions, invalid verdicts and process failures stop execution without retries or automatic resume.

Codex edit steps run in auto mode (`--approve-for-me`), routing approval requests through automatic review so agents can commit in linked workspaces. They receive neither `--sandbox` nor `--add-dir`. Read-only steps use `--sandbox read-only` without `--approve-for-me`. These settings also apply when continuing a session.

The step counter is saved before execution. A step beyond max_steps never starts; loop edges consume their named budget before proceeding. Each attempt stores stdout/stderr, command exit codes, observed HEAD and agent prompt/answer/session details. Run progress and results use atomic writes, and worklog.md gets one line per completed step. Fresh agents start a session; continue uses the latest successful session of its named step. Review receives task, diff and latest checks; edit steps receive latest checks/review answers. Round 1 compares merge-base..HEAD, and later rounds compare the same step's previously reviewed HEAD..HEAD plus prior review answers and fix responses. Every review is fresh. Prompts give round/limit, comparison SHAs, and existing workspace REVIEW.md/AGENTS.md/CLAUDE.md paths without parsing their contents. Findings are count-first, grouped by bug class, tagged Important/Nit/Pre-existing with file:line evidence; unresolved Important means `判定: 要修正`, otherwise `判定: 問題なし`. Every review prompt ends by requiring its final line to be exactly `判定: 問題なし` or `判定: 要修正`, even with no findings. Only the last non-empty verdict line is parsed. After every step with a workspace, Git status checks tracked changes and non-ignored untracked paths, including submodule changes.

```sh
rabi trust add
rabi run start task.md --name fix/example --base origin/main --check 'cargo test --locked'
rabi run show run_<uuid>
```

Start takes a Markdown file and requires `--name` and `--base`; there is no `--workspace`. `--workflow NAME` selects a definition; `--provider P` and `--model M` override all agent steps after step defaults. A CLI provider change replaces settings rather than inheriting another tool's model/effort; model-only keeps each provider. Only Codex runs today. Validation of effective read-only capabilities and continue providers happens before allocation. `-R` and common JSON options work as usual. Untrusted execution refuses with exit 4 before creating a workspace. JSON uses schema 2 with `data.run`, progress goes to stderr, and child output is saved in attempt logs. A stopped run after persistent changes exits 6. Show requires no trust and returns 0 for readable stopped runs.

The default workflow (version 8) ends with review → draft-pull-request. Implement/fix prompts say not to push or create PRs; this is an instruction, without enforcement. The fresh `draft-pull-request` agent uses Codex auto mode and is instructed to push only the run branch to origin (no force, tags, or other refs), then open a DRAFT PR against the run's base branch. The agent writes the title and body using the repository template sections Summary / Testing / Risk / How to revert, including checks, unverified items, review verdicts, reviewed HEADs and rounds, and that risk is not assessed by rabi unless the task says otherwise. Only on success does the agent report the PR URL in its answer.

If pushing or creating the draft PR fails (for example, missing `gh` authentication), the default and security-review prompts tell the agent to stop without endless retries or workarounds through other tools, write the error to `DRAFT_PULL_REQUEST_FAILED.txt` at the workspace root, leave it uncommitted and untracked, and report failure. The agent must not commit the file or add it to `.gitignore`, and must confirm that it appears in `git status --porcelain`. If existing repository or global excludes hide the marker, the agent must instead leave the error as an uncommitted modification of an existing tracked file (for example, append it), then verify that a non-ignored, uncommitted change remains in `git status --porcelain`. With `allow_dirty: false` (the default), the remaining change makes the step fail through the existing dirty-workspace rule: the run stops with `dirty_workspace`, and `rabi run show RUN_ID` names the changed path. This relies on the agent following the prompt; rabi itself does not check whether a PR exists.

Agent prompts include the run branch, normalized base branch when available, requested base ref and remote (`origin`). Edit prompts receive the latest agent report, checks, review outcomes and prior feedback. Use a local branch or origin tracking branch as `--base` for an unambiguous base branch name; otherwise the prompt identifies it as unknown and supplies the requested ref for the agent to resolve. rabi does not generate a PR body file, extract the reported URL, or update run/workspace PR links. Read the draft-pull-request attempt's `answer.md` for the agent's report. Existing schema-1 runs with PR links remain readable.

Only `prepare` remains a system action. Removed `push` and `draft-pr` actions fail normal YAML validation; there is no compatibility shim. There is no publication verification field or action. rabi does not verify draft status or other publication claims. No retries, automatic resume, CI waiting, merge, or risk classification are added.

Review revise after exhausting repair stops with a review-limit message asking for reconsideration; exhausted checks, malformed verdicts, dirty workspaces and external failures retain specific reasons in run.json and show. `repair: 2` is shared between checks and reviews; the default has at most three review rounds and max_steps 15. Instructions ask reviewers to stop for uninterpretable comparisons, without adding history gates to the executor.

Text output preserves stop-reason line breaks with indented continuation lines and escapes other control characters. JSON retains the original message.

For an optional independent security pass, copy [security-review.yaml](workflows/security-review.yaml) into `.rabi/workflows/security-review.yaml` and run `rabi workflow check security-review`. It uses review → security-review → draft-pull-request, Codex/fresh/read-only/review-v1, the same repair budget, and max_steps 18. It is not part of the built-in default. Risk classification, CI waiting and merging remain outside execution.
