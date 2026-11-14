#![cfg(unix)]
#[path = "common/scripts.rs"]
mod scripts;
#[path = "common/git.rs"]
mod test_git;
#[path = "common/wait.rs"]
mod wait;

use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GH: &str = r#"#!/bin/sh
set -eu
printf '%s\000' "$@" > "$RABI_TUNE_FIXTURE/gh-calls/args.$$"
case "$1 $2" in
  'repo view')
    printf '{"url":"https://github.com/fixture/project","owner":{"login":"fixture"},"name":"project"}\n'
    ;;
  'api user')
    printf '{"login":"%s"}\n' "$(cat "$RABI_TUNE_FIXTURE/login")"
    ;;
  'pr list')
    if [ -f "$RABI_TUNE_FIXTURE/pr-created" ]; then
      printf '[%s]\n' "$(cat "$RABI_TUNE_FIXTURE/pr.json")"
    else printf '[]\n'; fi
    ;;
  'pr create')
    printf '%s\000' "$@" > "$RABI_TUNE_FIXTURE/pr-create-args"
    head=''
    previous=''
    for arg in "$@"; do
      if [ "$previous" = --body-file ]; then cat "$arg" > "$RABI_TUNE_FIXTURE/pr-body"; fi
      if [ "$previous" = --head ]; then head=$arg; fi
      previous=$arg
    done
    oid=$(git --git-dir="$RABI_TUNE_FIXTURE/remote.git" rev-parse "refs/heads/$head")
    printf '{"number":7,"headRefName":"%s","headRefOid":"%s","isCrossRepository":false,"state":"OPEN"}\n' "$head" "$oid" > "$RABI_TUNE_FIXTURE/pr.json"
    touch "$RABI_TUNE_FIXTURE/pr-created"
    printf 'https://github.com/fixture/project/pull/7\n'
    ;;
  'pr view') cat "$RABI_TUNE_FIXTURE/pr.json" ;;
  *) touch "$RABI_TUNE_FIXTURE/forbidden-gh"; exit 91 ;;
esac
"#;

const CLAUDE: &str = r#"#!/bin/sh
set -eu
printf '%s\000' "$@" > "$RABI_TUNE_FIXTURE/claude-args.$$"
printf 'called\n' >> "$RABI_TUNE_FIXTURE/provider-calls"
cat > "$RABI_TUNE_FIXTURE/prompt.$$"
printf '%s\n' "$PWD" > "$RABI_TUNE_FIXTURE/provider-cwd"
printf '%s\n' "${RABI_AGENT-}" > "$RABI_TUNE_FIXTURE/provider-agent"
if [ -f "$RABI_TUNE_FIXTURE/first-answer" ]; then
  cat "$RABI_TUNE_FIXTURE/first-answer"
  rm "$RABI_TUNE_FIXTURE/first-answer"
else cat "$RABI_TUNE_FIXTURE/answer"; fi
"#;

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    head: String,
}
impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-tune-test-{}", uuid::Uuid::new_v4()));
        let repo = root.join("repo");
        for path in [
            &repo,
            &root.join("home"),
            &root.join("bin"),
            &root.join("gh-calls"),
        ] {
            fs::create_dir_all(path).unwrap();
        }
        test_git::write_config(&root.join("home"));
        scripts::write_executable(root.join("bin/gh"), GH).unwrap();
        scripts::write_executable(root.join("bin/claude"), CLAUDE).unwrap();
        scripts::write_executable(root.join("bin/codex"), "#!/bin/sh\nexit 99\n").unwrap();
        fs::write(root.join("login"), "maintainer").unwrap();
        let mut fixture = Self {
            root,
            repo,
            head: String::new(),
        };
        fixture.git(&["init", "--template=", "-b", "main"]);
        fixture.git(&["config", "user.name", "fixture"]);
        fixture.git(&["config", "user.email", "fixture"]);
        fs::create_dir(fixture.repo.join(".rabi")).unwrap();
        fs::write(fixture.repo.join(".rabi/policy.toml"), "schema=1\ndefault-tier='low'\n[approval]\napprovers=['maintainer']\n[classifier]\ndoc='risk-tiers.md'\nmodel='fixture-model'\n").unwrap();
        fs::write(
            fixture.repo.join("AGENTS.md"),
            "Existing instructions.\nRepeated\nRepeated\naaaa\n",
        )
        .unwrap();
        fs::write(fixture.repo.join("risk-tiers.md"), "Existing policy.\n").unwrap();
        fs::write(
            fixture.repo.join(".rabi.toml"),
            "[precheck]\ncommands=['true']\n[hooks]\nsetup=['exit 99']\n",
        )
        .unwrap();
        fixture.git(&["add", "."]);
        fixture.git(&["commit", "-m", "fixture base"]);
        fixture.head = fixture.git(&["rev-parse", "HEAD"]);
        fixture.git(&[
            "init",
            "--bare",
            "--template=",
            fixture.root.join("remote.git").to_str().unwrap(),
        ]);
        test_git::configure_repo(&fixture.root.join("remote.git"));
        fixture.git(&[
            "push",
            fixture.root.join("remote.git").to_str().unwrap(),
            "main",
        ]);
        fixture.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/fixture/project.git",
        ]);
        fixture.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        fixture.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        fs::write(
            fixture.root.join("push.gitconfig"),
            format!(
                "[url \"{}\"]\n insteadOf = https://github.com/fixture/project.git\n",
                fixture.root.join("remote.git").display()
            ),
        )
        .unwrap();
        let real = Self::run(Command::new("sh").args(["-c", "command -v git"]));
        assert!(real.status.success());
        let wrapper = format!(
            r#"#!/bin/sh
set -eu
previous=''
for arg in "$@"; do
  if [ "$previous" = worktree ] && [ "$arg" = add ] && [ -f "$RABI_TUNE_FIXTURE/pause-worktree" ]; then
    touch "$RABI_TUNE_FIXTURE/worktree-paused"
    while [ -f "$RABI_TUNE_FIXTURE/pause-worktree" ]; do sleep 0.01; done
  fi
  previous=$arg
  case "$arg" in push|fetch|ls-remote) export GIT_CONFIG_GLOBAL="$RABI_TUNE_FIXTURE/push.gitconfig" ;; esac
done
exec '{}' "$@"
"#,
            String::from_utf8(real.stdout)
                .unwrap()
                .trim()
                .replace('\'', "'\\''")
        );
        scripts::write_executable(fixture.root.join("bin/git"), wrapper).unwrap();
        fixture
    }
    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = test_git::command(program);
        let path = std::env::join_paths(std::iter::once(self.root.join("bin")).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        command
            .args(args)
            .current_dir(&self.repo)
            .env("HOME", self.root.join("home"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_DATA_HOME", self.root.join("data"))
            .env("PATH", path)
            .env("RABI_TUNE_FIXTURE", &self.root)
            .env("RABI_TRUST_ALL", "1")
            .env_remove("RABI_AGENT")
            .env_remove("RABI_OUTPUT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
        command
    }
    fn run(command: &mut Command) -> Output {
        let mut capture = wait::OutputCapture::new();
        let mut child = capture.apply(command).spawn().unwrap();
        capture.finish(&mut child)
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Self::run(&mut self.command("git", args));
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }
    fn rabi(&self, args: &[&str]) -> Output {
        Self::run(&mut self.command(env!("CARGO_BIN_EXE_rabi"), args))
    }
    fn json(&self, args: &[&str], exit: i32) -> Value {
        decode(self.rabi(args), exit)
    }
    fn classification(&self) -> String {
        let directory = self.repo.join(".git/rabi/classifications");
        fs::create_dir_all(&directory).unwrap();
        let record = json!({"schema":1,"headSha":self.head,"baseSha":self.head,"mergeBaseSha":self.head,"policyBlobSha":null,"docBlobSha":null,"tier":"high","floor":false,"reasons":["Repeated issue requires written guidance"],"uncertainty":"none","provider":"claude","createdAt":"2026-01-02T00:00:00Z"});
        fs::write(
            directory.join(format!("{}.json", self.head)),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        format!("classification:{}", self.head)
    }
    fn proposal(&self, evidence: &str) -> Value {
        json!({"edits":[{"path":"risk-tiers.md","find":"Existing policy.","replace":"Clarified policy.","rationale":"Repeated classification evidence calls for a clearer rule.","evidence":[evidence]}],"summary":"Clarify written policy"})
    }
    fn answer(&self, value: &Value) {
        fs::write(self.root.join("answer"), serde_json::to_vec(value).unwrap()).unwrap();
    }
    fn calls(&self) -> usize {
        fs::read_to_string(self.root.join("provider-calls"))
            .unwrap_or_default()
            .lines()
            .count()
    }
    fn tasks(&self) -> PathBuf {
        let repositories = self.root.join("state/rabi/repositories");
        fs::read_dir(repositories)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("tasks")
    }
    fn new_task(&self) -> String {
        self.json(
            &[
                "task",
                "new",
                "Fixture change",
                "--branch",
                "topic",
                "--base",
                "main",
                "--json",
            ],
            0,
        )["data"]["task"]["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    fn assert_no_gate_calls(&self) {
        assert!(!self.root.join("forbidden-gh").exists());
        for entry in fs::read_dir(self.root.join("gh-calls")).unwrap() {
            let args = args(&entry.unwrap().path());
            assert!(
                !args
                    .iter()
                    .any(|arg| matches!(arg.as_str(), "merge" | "approve" | "review")),
                "{args:?}"
            );
            assert!(
                !args.iter().any(|arg| arg.contains("statuses/")),
                "{args:?}"
            );
        }
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn args(path: &Path) -> Vec<String> {
    fs::read(path)
        .unwrap()
        .split(|byte| *byte == 0)
        .map(|bytes| String::from_utf8(bytes.to_vec()).unwrap())
        .collect()
}
fn decode(output: Output, exit: i32) -> Value {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], exit == 0);
    value
}
fn inventory(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    if path.exists() {
        visit(path, path, &mut result);
    }
    result
}

#[test]
fn override_agent_tripwire_and_non_approver_refuse_without_writing() {
    let repo = Repo::new();
    let id = repo.new_task();
    repo.classification();
    let before = inventory(&repo.tasks());
    let command_args = [
        "task",
        "override",
        &id,
        "--tier",
        "low",
        "--note",
        "Safe wording",
        "--sha",
        &repo.head,
        "--json",
    ];
    let rejected = decode(
        Repo::run(
            repo.command(env!("CARGO_BIN_EXE_rabi"), &command_args)
                .env("RABI_AGENT", "1"),
        ),
        2,
    );
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("RABI_AGENT"));
    assert_eq!(fs::read_dir(repo.root.join("gh-calls")).unwrap().count(), 0);
    assert_eq!(inventory(&repo.tasks()), before);
    fs::write(repo.root.join("login"), "contributor").unwrap();
    let rejected = repo.json(&command_args, 4);
    assert!(rejected["error"]["message"]
        .as_str()
        .unwrap()
        .contains("approval.approvers"));
    assert_eq!(inventory(&repo.tasks()), before);
    repo.assert_no_gate_calls();
}

#[test]
fn override_is_one_sanitized_append_only_observation() {
    let repo = Repo::new();
    let id = repo.new_task();
    repo.classification();
    let directory = repo.tasks().join(&id);
    let before_task = fs::read(directory.join("task.json")).unwrap();
    let before_events = fs::read(directory.join("events.jsonl")).unwrap();
    fs::write(repo.root.join("login"), "MAINTAINER").unwrap();
    let note = format!(
        "Safe wording token=fixture-hidden {}\u{1b}[31m",
        repo.repo.display()
    );
    let event = repo.json(
        &[
            "task",
            "override",
            &id,
            "--tier",
            "low",
            "--note",
            &note,
            "--sha",
            &repo.head.to_uppercase(),
            "--json",
        ],
        0,
    )["data"]
        .clone();
    assert_eq!(event["reason"], "override");
    assert_eq!(event["from"], event["to"]);
    assert_eq!(event["override"]["tier"], "low");
    assert_eq!(event["override"]["classifiedTier"], "high");
    assert_eq!(event["override"]["sha"], repo.head);
    assert!(event["override"]["id"]
        .as_str()
        .unwrap()
        .starts_with("override_"));
    assert!(chrono::DateTime::parse_from_rfc3339(event["at"].as_str().unwrap()).is_ok());
    let sanitized = event["override"]["note"].as_str().unwrap();
    assert!(!sanitized.contains("fixture-hidden"));
    assert!(!sanitized.contains(&repo.repo.to_string_lossy().to_string()));
    assert!(!sanitized.contains('\u{1b}'));
    assert_eq!(fs::read(directory.join("task.json")).unwrap(), before_task);
    let after = fs::read(directory.join("events.jsonl")).unwrap();
    assert!(after.starts_with(&before_events));
    assert_eq!(
        after
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .count(),
        2
    );
    let history = repo.rabi(&["task", "events", &id, "--json"]);
    assert!(history.status.success());
    assert_eq!(
        String::from_utf8(history.stdout).unwrap().lines().count(),
        2
    );
    repo.assert_no_gate_calls();
}

#[test]
fn tune_without_evidence_is_successful_and_creates_nothing() {
    let repo = Repo::new();
    let before = inventory(&repo.repo);
    let output = repo.rabi(&["task", "tune"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        "nothing to propose"
    );
    let report = repo.json(&["task", "tune", "--json"], 0);
    assert_eq!(report["data"]["proposal"], Value::Null);
    assert_eq!(repo.calls(), 0);
    assert_eq!(inventory(&repo.repo), before);
    assert!(!repo.root.join("state").exists());
    assert_eq!(fs::read_dir(repo.root.join("gh-calls")).unwrap().count(), 0);
}

#[test]
fn tune_dry_run_uses_standalone_classification_and_toolless_nonce_framed_provider() {
    let repo = Repo::new();
    let evidence = repo.classification();
    let proposal = repo.proposal(&evidence);
    repo.answer(&proposal);
    let before = inventory(&repo.repo);
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    assert_eq!(report["data"]["plan"], true);
    assert_eq!(report["data"]["proposal"], proposal);
    assert!(report["data"]["diff"]
        .as_str()
        .unwrap()
        .contains("-Existing policy."));
    assert!(report["data"]["diff"]
        .as_str()
        .unwrap()
        .contains("+Clarified policy."));
    assert_eq!(report["data"]["taskId"], Value::Null);
    assert_eq!(repo.calls(), 1);
    assert_eq!(inventory(&repo.repo), before);
    assert!(!repo.root.join("state").exists());
    let provider_args = fs::read_dir(&repo.root)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("claude-args.")
                .then_some(path)
        })
        .collect::<Vec<_>>();
    assert_eq!(provider_args.len(), 1);
    let provider_args = args(&provider_args[0]);
    for pair in [
        ["--tools", ""],
        ["--setting-sources", ""],
        ["--mcp-config", "{\"mcpServers\":{}}"],
    ] {
        assert!(
            provider_args.windows(2).any(|args| args == pair),
            "{provider_args:?}"
        );
    }
    for flag in [
        "--safe-mode",
        "--strict-mcp-config",
        "--no-session-persistence",
        "--disable-slash-commands",
        "--model=fixture-model",
    ] {
        assert!(
            provider_args.iter().any(|arg| arg == flag),
            "{provider_args:?}"
        );
    }
    assert!(!provider_args
        .iter()
        .any(|arg| matches!(arg.as_str(), "--resume" | "--continue")));
    let cwd = fs::read_to_string(repo.root.join("provider-cwd")).unwrap();
    assert!(!cwd.trim().starts_with(repo.repo.to_str().unwrap()));
    assert_eq!(
        fs::read_to_string(repo.root.join("provider-agent"))
            .unwrap()
            .trim(),
        "1"
    );
    let prompt_path = fs::read_dir(&repo.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("prompt.")
        })
        .unwrap();
    let prompt = fs::read_to_string(prompt_path).unwrap();
    let start = prompt.find("<rabi-untrusted-tune-").unwrap();
    let framed = &prompt[start..];
    let (opening, body) = framed.split_once('>').unwrap();
    assert!(body.contains(&format!("</{}>", &opening[1..])));
    assert!(body.contains(&evidence));
    repo.assert_no_gate_calls();
}

#[test]
fn tune_rejects_disallowed_nonunique_oversized_and_malformed_output_after_one_retry() {
    for case in [
        "path",
        "nonunique",
        "overlap",
        "missing",
        "oversize",
        "malformed",
        "unknown-field",
        "unknown-evidence",
    ] {
        let repo = Repo::new();
        let evidence = repo.classification();
        let mut proposal = repo.proposal(&evidence);
        match case {
            "path" => proposal["edits"][0]["path"] = json!(".rabi/policy.toml"),
            "nonunique" => {
                proposal["edits"][0]["path"] = json!("AGENTS.md");
                proposal["edits"][0]["find"] = json!("Repeated");
            }
            "overlap" => {
                proposal["edits"][0]["path"] = json!("AGENTS.md");
                proposal["edits"][0]["find"] = json!("aaa");
            }
            "missing" => proposal["edits"][0]["find"] = json!("Not in base"),
            "oversize" => proposal["edits"][0]["replace"] = json!("x".repeat(8193)),
            "unknown-field" => proposal["extra"] = json!(true),
            "unknown-evidence" => proposal["edits"][0]["evidence"] = json!(["pr:999"]),
            "malformed" => {}
            _ => unreachable!(),
        }
        repo.answer(&proposal);
        if case == "malformed" {
            fs::write(repo.root.join("answer"), "{invalid").unwrap();
        }
        let before = inventory(&repo.repo);
        let rejected = repo.json(&["task", "tune", "--dry-run", "--json"], 1);
        assert!(
            rejected["error"]["message"]
                .as_str()
                .unwrap()
                .contains("after one retry"),
            "{case}: {rejected}"
        );
        assert_eq!(repo.calls(), 2, "{case}");
        assert_eq!(inventory(&repo.repo), before, "{case}");
        assert!(!repo.root.join("state").exists(), "{case}");
        repo.assert_no_gate_calls();
    }
}

#[test]
fn tune_retries_malformed_output_once_and_accepts_replacement() {
    let repo = Repo::new();
    let evidence = repo.classification();
    let proposal = repo.proposal(&evidence);
    repo.answer(&proposal);
    fs::write(repo.root.join("first-answer"), "not JSON").unwrap();
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    assert_eq!(report["data"]["proposal"], proposal);
    assert_eq!(repo.calls(), 2);
    repo.assert_no_gate_calls();
}

#[test]
fn tune_since_excludes_old_evidence_and_empty_model_proposal_stops() {
    let repo = Repo::new();
    repo.classification();
    let report = repo.json(
        &[
            "task",
            "tune",
            "--since",
            "2026-01-03",
            "--dry-run",
            "--json",
        ],
        0,
    );
    assert_eq!(report["data"]["proposal"], Value::Null);
    assert_eq!(repo.calls(), 0);
    repo.answer(&json!({"edits":[],"summary":"No justified edit"}));
    let report = repo.json(&["task", "tune", "--json"], 0);
    assert_eq!(report["data"]["proposal"], Value::Null);
    assert_eq!(repo.calls(), 1);
    assert!(!repo.root.join("state").exists());
}

#[test]
fn tune_creates_normal_task_linked_workspace_commit_and_draft_pr_only() {
    let repo = Repo::new();
    let evidence = repo.classification();
    let proposal = repo.proposal(&evidence);
    repo.answer(&proposal);
    let before_refs = repo.git(&[
        "for-each-ref",
        "--format=%(refname):%(objectname)",
        "refs/heads/main",
        "refs/remotes/origin/main",
    ]);
    let report = repo.json(&["task", "tune", "--json"], 0);
    assert_eq!(report["data"]["plan"], false);
    assert_eq!(report["data"]["prNumber"], 7);
    let task_id = report["data"]["taskId"].as_str().unwrap();
    let task = repo.json(&["task", "show", task_id, "--json"], 0)["data"].clone();
    assert_eq!(task["task"]["intent"], "tune: Clarify written policy");
    assert_eq!(task["task"]["state"]["name"], "ci");
    assert_eq!(task["task"]["prNumber"], 7);
    let workspace = task["task"]["workspacePath"].as_str().unwrap();
    assert_ne!(workspace, repo.repo.to_str().unwrap());
    assert_eq!(
        fs::read_to_string(Path::new(workspace).join("risk-tiers.md")).unwrap(),
        "Clarified policy.\n"
    );
    assert_eq!(
        fs::read_to_string(repo.repo.join("risk-tiers.md")).unwrap(),
        "Existing policy.\n"
    );
    assert_eq!(
        repo.git(&[
            "-C",
            workspace,
            "rev-list",
            "--count",
            &format!("{}..HEAD", repo.head)
        ]),
        "1"
    );
    assert!(repo
        .git(&["-C", workspace, "log", "-1", "--format=%B"])
        .contains(&serde_json::to_string(&evidence).unwrap()));
    assert!(repo
        .git(&["-C", workspace, "status", "--porcelain"])
        .is_empty());
    assert_eq!(
        repo.git(&[
            "for-each-ref",
            "--format=%(refname):%(objectname)",
            "refs/heads/main",
            "refs/remotes/origin/main"
        ]),
        before_refs
    );
    let draft_args = args(&repo.root.join("pr-create-args"));
    assert!(draft_args.iter().any(|arg| arg == "--draft"));
    let body = fs::read_to_string(repo.root.join("pr-body")).unwrap();
    assert!(body.contains("Repeated classification evidence"));
    assert!(body.contains(&evidence));
    assert!(body.contains("Proposal only"));
    assert_eq!(repo.calls(), 1);
    let events = task["events"].as_array().unwrap();
    assert!(!events.iter().any(|event| matches!(
        event["reason"].as_str(),
        Some("review_passed" | "approval_received" | "auto_tier" | "merged")
    )));
    repo.assert_no_gate_calls();
}

#[test]
fn tune_collects_override_classification_events_and_report_fingerprints() {
    let repo = Repo::new();
    let id = repo.new_task();
    let event_path = repo.tasks().join(&id).join("events.jsonl");
    let classification = json!({"at":"2026-01-02T00:00:00Z","from":{"name":"classifying"},"to":{"name":"awaiting_approval"},"reason":"approval_required","classification":{"headSha":repo.head,"tier":"high","reasons":["Prior evidence needs clearer wording"]}});
    let mut events = fs::OpenOptions::new()
        .append(true)
        .open(&event_path)
        .unwrap();
    writeln!(events, "{classification}").unwrap();
    let observation = repo.json(
        &[
            "task",
            "override",
            &id,
            "--tier",
            "medium",
            "--note",
            "Safe documentation wording",
            "--json",
        ],
        0,
    )["data"]
        .clone();
    assert_eq!(observation["override"]["sha"], repo.head);
    for _ in 0..2 {
        let review = json!({"at":"2026-01-03T00:00:00Z","from":{"name":"reviewing"},"to":{"name":"ready"},"reason":"review_passed","review":{"findings":[{"path":"src/example.rs","body":"Explain the validation requirement"}]}});
        writeln!(events, "{review}").unwrap();
    }
    drop(events);
    let report = repo.json(&["task", "report", "--json"], 0);
    let finding = &report["data"]["topFindings"][0];
    assert_eq!(finding["count"], 2);
    let finding_id = format!(
        "finding:{}:{}",
        finding["path"].as_str().unwrap(),
        finding["bodyHash"].as_str().unwrap()
    );
    let override_id = observation["override"]["id"].as_str().unwrap();
    let mut proposal = repo.proposal(override_id);
    proposal["edits"][0]["path"] = json!("AGENTS.md");
    proposal["edits"][0]["find"] = json!("Existing instructions.");
    proposal["edits"][0]["replace"] = json!("Clarified instructions.");
    proposal["edits"][0]["evidence"] = json!([override_id, format!("{id}:event:2"), finding_id]);
    repo.answer(&proposal);
    let tuned = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    assert_eq!(tuned["data"]["proposal"], proposal);
    let prompt_path = fs::read_dir(&repo.root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("prompt.")
        })
        .unwrap();
    let prompt = fs::read_to_string(prompt_path).unwrap();
    for text in [
        override_id,
        "classifiedTier",
        "Safe documentation wording",
        "Prior evidence needs clearer wording",
        "Explain the validation requirement",
        "repeatedFindings",
    ] {
        assert!(prompt.contains(text), "Missing {text}: {prompt}");
    }
    assert!(!repo.repo.join(".git/rabi/classifications").exists());
    repo.assert_no_gate_calls();
}

#[test]
fn tune_uses_override_only_evidence_after_classification_record_is_unavailable() {
    let repo = Repo::new();
    let id = repo.new_task();
    repo.classification();
    let observation = repo.json(
        &[
            "task",
            "override",
            &id,
            "--tier",
            "low",
            "--note",
            "Safe wording",
            "--sha",
            &repo.head,
            "--json",
        ],
        0,
    )["data"]
        .clone();
    fs::remove_dir_all(repo.repo.join(".git/rabi/classifications")).unwrap();
    let override_id = observation["override"]["id"].as_str().unwrap();
    let proposal = repo.proposal(override_id);
    repo.answer(&proposal);
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    assert_eq!(report["data"]["proposal"], proposal);
    assert_eq!(repo.calls(), 1);
    repo.assert_no_gate_calls();
}

#[test]
fn tune_uses_report_finding_fingerprints_without_classifications_or_overrides() {
    let repo = Repo::new();
    let id = repo.new_task();
    let directory = repo.tasks().join(&id);
    let mut events = fs::OpenOptions::new()
        .append(true)
        .open(directory.join("events.jsonl"))
        .unwrap();
    for _ in 0..2 {
        let event = json!({"at":"2026-01-03T00:00:00Z","from":{"name":"reviewing"},"to":{"name":"ready"},"reason":"review_passed","review":{"findings":[{"path":"src/example.rs","body":"Explain the validation requirement"}]}});
        writeln!(events, "{event}").unwrap();
    }
    drop(events);
    let report = repo.json(&["task", "report", "--json"], 0);
    let finding = &report["data"]["topFindings"][0];
    assert_eq!(finding["count"], 2);
    let evidence = format!(
        "finding:{}:{}",
        finding["path"].as_str().unwrap(),
        finding["bodyHash"].as_str().unwrap()
    );
    let proposal = repo.proposal(&evidence);
    repo.answer(&proposal);
    let tuned = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    assert_eq!(tuned["data"]["proposal"], proposal);
    assert_eq!(repo.calls(), 1);
    assert!(!repo.repo.join(".git/rabi/classifications").exists());
    repo.assert_no_gate_calls();
}

#[test]
fn tune_empty_document_replacement_diff_can_be_applied() {
    let repo = Repo::new();
    let evidence = repo.classification();
    let mut proposal = repo.proposal(&evidence);
    proposal["edits"][0]["find"] = json!("Existing policy.\n");
    proposal["edits"][0]["replace"] = json!("");
    repo.answer(&proposal);
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    let patch = repo.root.join("proposal.patch");
    fs::write(&patch, report["data"]["diff"].as_str().unwrap()).unwrap();
    repo.git(&["apply", "--check", patch.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(repo.repo.join("risk-tiers.md")).unwrap(),
        "Existing policy.\n"
    );
    repo.git(&["apply", patch.to_str().unwrap()]);
    assert!(fs::read(repo.repo.join("risk-tiers.md"))
        .unwrap()
        .is_empty());
    assert_eq!(repo.git(&["diff", "--name-only"]), "risk-tiers.md");
    assert_eq!(repo.calls(), 1);
    repo.assert_no_gate_calls();
}

#[test]
fn tune_one_line_edit_has_a_small_deterministic_context_hunk() {
    let mut repo = Repo::new();
    let original: String = (1..=80)
        .map(|line| format!("Policy line {line}\n"))
        .collect();
    fs::write(repo.repo.join("risk-tiers.md"), &original).unwrap();
    repo.git(&["add", "risk-tiers.md"]);
    repo.git(&["commit", "-m", "fixture policy lines"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.head = repo.git(&["rev-parse", "HEAD"]);
    let evidence = repo.classification();
    let mut proposal = repo.proposal(&evidence);
    proposal["edits"][0]["find"] = json!("Policy line 40\n");
    proposal["edits"][0]["replace"] = json!("Clarified line 40\n");
    repo.answer(&proposal);
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    let diff = report["data"]["diff"].as_str().unwrap();
    assert!(diff.contains("@@ -37,7 +37,7 @@"), "{diff}");
    assert_eq!(diff.lines().count(), 12, "{diff}");
    assert!(!diff.contains("Policy line 1\n"));
    assert!(!diff.contains("Policy line 80\n"));
    let global = repo.root.join("home/.gitconfig");
    let mut config = fs::OpenOptions::new().append(true).open(&global).unwrap();
    writeln!(config, "[diff]\n algorithm = histogram\n indentHeuristic = true\n interHunkContext = 200\n context = 200\n mnemonicPrefix = true\n suppressBlankEmpty = true\n wordRegex = .\n[core]\n quotePath = true").unwrap();
    drop(config);
    repo.git(&["config", "diff.suppressBlankEmpty", "true"]);
    repo.git(&["config", "diff.interHunkContext", "200"]);
    repo.git(&["config", "diff.context", "200"]);
    let temporary = repo.repo.join("temporary");
    fs::create_dir(&temporary).unwrap();
    let repeated = decode(
        Repo::run(
            repo.command(
                env!("CARGO_BIN_EXE_rabi"),
                &["task", "tune", "--dry-run", "--json"],
            )
            .env("TMPDIR", &temporary),
        ),
        0,
    );
    assert_eq!(repeated["data"]["diff"], diff);
    let patch = repo.root.join("proposal.patch");
    fs::write(&patch, diff).unwrap();
    repo.git(&["apply", "--check", patch.to_str().unwrap()]);
    repo.git(&["apply", patch.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(repo.repo.join("risk-tiers.md")).unwrap(),
        original.replace("Policy line 40\n", "Clarified line 40\n")
    );
    repo.assert_no_gate_calls();
}

#[test]
fn tune_separates_distant_hunks_and_preserves_missing_final_newlines() {
    let mut repo = Repo::new();
    let original = (1..=80)
        .map(|line| format!("Policy line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(repo.repo.join("risk-tiers.md"), &original).unwrap();
    repo.git(&["add", "risk-tiers.md"]);
    repo.git(&["commit", "-m", "fixture policy without final newline"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.head = repo.git(&["rev-parse", "HEAD"]);
    let evidence = repo.classification();
    let mut proposal = repo.proposal(&evidence);
    proposal["edits"] = json!([10, 80].map(|line| json!({
        "path": "risk-tiers.md", "find": format!("Policy line {line}"),
        "replace": format!("Clarified line {line}"), "rationale": "Clarify this rule",
        "evidence": [&evidence]
    })));
    repo.answer(&proposal);
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    let diff = report["data"]["diff"].as_str().unwrap();
    assert_eq!(
        diff.lines().filter(|line| line.starts_with("@@ ")).count(),
        2,
        "{diff}"
    );
    assert_eq!(
        diff.matches("\\ No newline at end of file").count(),
        2,
        "{diff}"
    );
    let patch = repo.root.join("proposal.patch");
    fs::write(&patch, diff).unwrap();
    repo.git(&["apply", "--check", patch.to_str().unwrap()]);
    repo.git(&["apply", patch.to_str().unwrap()]);
    assert_eq!(
        fs::read_to_string(repo.repo.join("risk-tiers.md")).unwrap(),
        original
            .replace("Policy line 10", "Clarified line 10")
            .replace("Policy line 80", "Clarified line 80")
    );
    repo.assert_no_gate_calls();
}

#[test]
fn tune_and_override_load_whitespace_policy_as_defaults() {
    let mut repo = Repo::new();
    fs::write(repo.repo.join(".rabi/policy.toml"), " \t\r\n").unwrap();
    repo.git(&["add", ".rabi/policy.toml"]);
    repo.git(&["commit", "-m", "fixture whitespace policy"]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.head = repo.git(&["rev-parse", "HEAD"]);
    let evidence = repo.classification();
    let mut proposal = repo.proposal(&evidence);
    proposal["edits"][0]["path"] = json!("AGENTS.md");
    proposal["edits"][0]["find"] = json!("Existing instructions.");
    proposal["edits"][0]["replace"] = json!("Clarified instructions.");
    repo.answer(&proposal);
    let report = repo.json(&["task", "tune", "--dry-run", "--json"], 0);
    assert_eq!(report["data"]["proposal"], proposal);
    assert_eq!(repo.calls(), 1);
    let task = repo.new_task();
    let before = inventory(&repo.tasks());
    let refused = repo.json(
        &[
            "task",
            "override",
            &task,
            "--tier",
            "low",
            "--note",
            "Safe wording",
            "--sha",
            &repo.head,
            "--json",
        ],
        4,
    );
    assert!(refused["error"]["message"]
        .as_str()
        .unwrap()
        .contains("approval.approvers"));
    assert_eq!(inventory(&repo.tasks()), before);
    repo.assert_no_gate_calls();
}

#[test]
fn tune_holds_operation_locks_before_its_task_becomes_discoverable() {
    let repo = Repo::new();
    let evidence = repo.classification();
    repo.answer(&repo.proposal(&evidence));
    let barrier = repo.root.join("pause-worktree");
    fs::write(&barrier, "pause").unwrap();
    struct ReleaseBarrier(PathBuf);
    impl Drop for ReleaseBarrier {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let release = ReleaseBarrier(barrier);
    let mut capture = wait::OutputCapture::new();
    let mut tune = capture
        .apply(&mut repo.command(env!("CARGO_BIN_EXE_rabi"), &["task", "tune", "--json"]))
        .spawn()
        .unwrap();
    let guard = wait::hang_guard();
    while !repo.root.join("worktree-paused").exists() {
        if let Some(status) = tune.try_wait().unwrap() {
            let output = capture.output(status);
            panic!(
                "tune exited before worktree barrier: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(
            std::time::Instant::now() < guard,
            "tune did not reach worktree barrier before hang guard"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let listed = repo.json(&["task", "list", "--json"], 0);
    let tasks = listed["data"]["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 1);
    let task = tasks[0]["id"].as_str().unwrap();
    let mut refused = Vec::new();
    for operation in ["advance", "implement", "publish", "abandon"] {
        let output = repo.rabi(&["task", operation, task, "--json"]);
        refused.push((operation, output));
    }
    let all = repo.rabi(&["task", "advance", "--all", "--json"]);
    let calls_while_paused = repo.calls();
    let gh_calls_while_paused = fs::read_dir(repo.root.join("gh-calls")).unwrap().count();
    drop(release);
    let completed = decode(capture.finish(&mut tune), 0);
    assert_eq!(completed["data"]["taskId"], task);
    assert_eq!(completed["data"]["prNumber"], 7);
    for (operation, output) in refused {
        let report = decode(output, 4);
        let message = report["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("already running") || message.contains("publication is running"),
            "{operation}: {report}"
        );
    }
    let batch = decode(all, 4);
    assert_eq!(batch["data"]["items"][0]["taskId"], task);
    assert!(batch["data"]["items"][0]["error"]["message"]
        .as_str()
        .unwrap()
        .contains("already running"));
    assert_eq!(calls_while_paused, 1);
    assert_eq!(gh_calls_while_paused, 0);
    assert_eq!(repo.calls(), 1);
    repo.assert_no_gate_calls();
}

#[test]
fn override_corrupt_or_mismatched_classification_is_operational_error_without_mutation() {
    for case in ["malformed", "mismatched-sha", "invalid-schema"] {
        let repo = Repo::new();
        let id = repo.new_task();
        repo.classification();
        let path = repo
            .repo
            .join(".git/rabi/classifications")
            .join(format!("{}.json", repo.head));
        if case == "malformed" {
            fs::write(&path, "{invalid").unwrap();
        } else {
            let mut record: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match case {
                "mismatched-sha" => record["headSha"] = json!("0".repeat(40)),
                "invalid-schema" => record["schema"] = json!(99),
                _ => unreachable!(),
            }
            fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        let before = inventory(&repo.tasks());
        let rejected = repo.json(
            &[
                "task",
                "override",
                &id,
                "--tier",
                "low",
                "--note",
                "Safe wording",
                "--sha",
                &repo.head,
                "--json",
            ],
            1,
        );
        assert!(
            rejected["error"]["message"]
                .as_str()
                .unwrap()
                .to_lowercase()
                .contains("classification"),
            "{case}: {rejected}"
        );
        assert_eq!(inventory(&repo.tasks()), before, "{case}");
        repo.assert_no_gate_calls();
    }
}
