#[path = "common/git.rs"]
mod test_git;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rabi-trust-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let repo = root.join("repo");
        let home = root.join("home");
        fs::create_dir_all(home.join(".config/git")).expect("home");
        fs::create_dir_all(&repo).expect("repo");
        test_git::write_config(&home);
        let repo_ctx = Repo { root, repo, home };
        repo_ctx.git(&["init", "-b", "main"]);
        repo_ctx.git(&["config", "user.name", "Phantom Rust"]);
        repo_ctx.git(&["config", "user.email", "phantom-rust@example.com"]);
        fs::write(repo_ctx.repo.join("README.md"), "# phantom\n").expect("readme");
        repo_ctx.git(&["add", "README.md"]);
        repo_ctx.git(&["commit", "-m", "initial commit"]);
        repo_ctx
    }

    fn git(&self, args: &[&str]) {
        let output = self.command("git", args).output().expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            text(&output.stderr)
        );
    }

    fn command(&self, program: &str, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(&self.repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("xdg"))
            .env_remove("RABI_TRUST_ALL")
            .env_remove("CI")
            .env_remove("GHQ_ROOT")
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM");
        if program.ends_with("git") && args.first() == Some(&"init") {
            let template = self.root.join("git-template");
            fs::create_dir_all(&template).expect("git template");
            command.env("GIT_TEMPLATE_DIR", &template);
        }
        command
    }

    fn rabi(&self, args: &[&str]) -> std::process::Output {
        self.command(env!("CARGO_BIN_EXE_rabi"), args)
            .output()
            .expect("rabi")
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

fn hooks(repo: &Repo, post: &str, pre: &str) {
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\nsetup = [{post}]\npre-remove = [{pre}]\n"),
    )
    .expect("hooks");
}

fn worktree(repo: &Repo, name: &str) -> PathBuf {
    let common = rabi::git::canonical_common_dir(&repo.repo).expect("common dir");
    rabi::paths::default_worktrees_directory(&repo.repo, &common, &repo.root.join("xdg"))
        .path
        .join(name)
}

fn trust_file(repo: &Repo) -> PathBuf {
    repo.root.join("state/rabi/trust.toml")
}

fn common_dir(repo: &Repo) -> String {
    let output = repo
        .command("git", &["rev-parse", "--git-common-dir"])
        .output()
        .expect("common dir");
    assert!(output.status.success(), "{}", text(&output.stderr));
    let path = repo.repo.join(text(&output.stdout).trim());
    fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[test]
fn included_files_copy_without_trust() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".gitignore"), "*.env\n").expect("gitignore");
    fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").expect("include");
    fs::write(repo.repo.join("secret.env"), "secret\n").expect("secret");
    repo.git(&["add", ".gitignore", ".worktreeinclude"]);
    repo.git(&["commit", "-m", "include"]);
    let created = repo.rabi(&["create", "copied"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert_eq!(
        fs::read_to_string(worktree(&repo, "copied").join("secret.env")).expect("copied"),
        "secret\n"
    );
    assert!(!trust_file(&repo).exists());
}

#[test]
fn a_repository_with_no_hooks_never_prompts() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "plain"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(
        !text(&created.stderr).contains("Trust this project"),
        "{}",
        text(&created.stderr)
    );
    assert!(worktree(&repo, "plain").is_dir());
    assert!(!trust_file(&repo).exists());
}

#[test]
fn non_tty_refusal_creates_nothing() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"touch pre-remove.txt\"");
    let created = repo.rabi(&["create", "topic"]);
    assert_eq!(created.status.code(), Some(4));
    let stderr = text(&created.stderr);
    assert!(stderr.contains("rabi trust"), "{stderr}");
    assert!(stderr.contains("--no-hooks"), "{stderr}");
    assert!(stderr.contains("touch hook-ran.txt"), "{stderr}");
    assert!(
        !stderr.contains("Trust this project and run its hooks?"),
        "{stderr}"
    );
    assert!(
        text(&created.stdout).is_empty(),
        "{}",
        text(&created.stdout)
    );
    assert!(!worktree(&repo, "topic").exists());
    assert!(!trust_file(&repo).exists());
    let listed = repo
        .command("git", &["worktree", "list", "--porcelain"])
        .output()
        .expect("list");
    assert!(
        !text(&listed.stdout).contains(&worktree(&repo, "topic").to_string_lossy().to_string()),
        "{}",
        text(&listed.stdout)
    );
}

#[test]
fn json_refusal_keeps_stdout_to_one_document() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let created = repo.rabi(&["create", "topic", "--json"]);
    assert_eq!(created.status.code(), Some(4));
    assert!(
        text(&created.stderr).is_empty(),
        "{}",
        text(&created.stderr)
    );
    let document: serde_json::Value = serde_json::from_slice(&created.stdout).expect("json");
    assert_eq!(document["schema"], 2);
    assert_eq!(document["error"]["code"], "untrusted");
    assert!(document["error"].get("exitCode").is_none());
    let message = document["error"]["message"].as_str().expect("message");
    assert!(message.contains("rabi trust"), "{message}");
    assert!(message.contains("--no-hooks"), "{message}");
    assert!(!worktree(&repo, "topic").exists());
}

#[test]
fn no_hooks_creates_the_worktree_and_does_not_trust_it() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let created = repo.rabi(&["create", "topic", "--no-setup"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(worktree(&repo, "topic").is_dir());
    assert!(!worktree(&repo, "topic").join("hook-ran.txt").exists());
    assert!(!trust_file(&repo).exists());
}

#[test]
fn trust_untrust_and_list_round_trip() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let refused = repo.rabi(&["trust", "--yes"]);
    assert_eq!(refused.status.code(), Some(2), "{}", text(&refused.stderr));
    assert!(!trust_file(&repo).exists());

    let trusted = repo.rabi(&["trust", "add"]);
    assert!(trusted.status.success(), "{}", text(&trusted.stderr));
    let key = common_dir(&repo);
    assert!(
        text(&trusted.stdout).contains(&key),
        "{}",
        text(&trusted.stdout)
    );
    let stored = fs::read_to_string(trust_file(&repo)).expect("trust file");
    assert!(stored.contains(&key), "{stored}");
    assert!(stored.contains("trustedAt"), "{stored}");

    let listed = repo.rabi(&["trust", "list"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    assert!(
        text(&listed.stdout).contains(&key),
        "{}",
        text(&listed.stdout)
    );

    let json = repo.rabi(&["trust", "list", "--json"]);
    assert!(json.status.success(), "{}", text(&json.stderr));
    let document: serde_json::Value = serde_json::from_slice(&json.stdout).expect("json");
    assert_eq!(document["data"]["repos"][0]["path"], key);

    let created = repo.rabi(&["create", "topic"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(worktree(&repo, "topic").join("hook-ran.txt").is_file());

    hooks(&repo, "\"touch changed-hook.txt\"", "\"true\"");
    let again = repo.rabi(&["create", "other"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert!(
        !text(&again.stderr).contains("Trust this project and run its hooks?"),
        "{}",
        text(&again.stderr)
    );
    assert!(worktree(&repo, "other").join("changed-hook.txt").is_file());

    let removed = repo.rabi(&["trust", "remove"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    assert!(
        text(&removed.stdout).contains("Removed trust"),
        "{}",
        text(&removed.stdout)
    );
    let empty = repo.rabi(&["trust", "list"]);
    assert_eq!(text(&empty.stdout), "No trusted repos");
    let missing = repo.rabi(&["trust", "remove"]);
    assert!(missing.status.success(), "{}", text(&missing.stderr));
    assert!(
        text(&missing.stdout).contains("Not trusted"),
        "{}",
        text(&missing.stdout)
    );

    let blocked = repo.rabi(&["create", "third"]);
    assert_eq!(blocked.status.code(), Some(4));
    assert!(!worktree(&repo, "third").exists());
}

#[test]
fn trust_all_runs_hooks_without_writing_trust() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "topic"])
        .env("RABI_TRUST_ALL", "1")
        .output()
        .expect("rabi");
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(worktree(&repo, "topic").join("hook-ran.txt").is_file());
    assert!(!trust_file(&repo).exists());
}

#[test]
fn delete_without_a_terminal_keeps_the_worktree() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "topic"]).status.success());
    hooks(&repo, "\"true\"", "\"touch pre-remove.txt\"");
    let deleted = repo.rabi(&["remove", "topic"]);
    assert_eq!(deleted.status.code(), Some(4));
    assert!(
        text(&deleted.stderr).contains("--no-hooks"),
        "{}",
        text(&deleted.stderr)
    );
    assert!(worktree(&repo, "topic").is_dir());
    assert!(!repo.repo.join("pre-remove.txt").exists());
    let skipped = repo.rabi(&["remove", "topic", "--no-hooks"]);
    assert!(skipped.status.success(), "{}", text(&skipped.stderr));
    assert!(!worktree(&repo, "topic").exists());
    assert!(!repo.repo.join("pre-remove.txt").exists());
}

#[test]
fn gc_skips_an_untrusted_candidate_and_does_not_ask() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    let dir = worktree(&repo, "feature");
    fs::write(dir.join("feature.txt"), "feature\n").expect("note");
    for args in [vec!["add", "feature.txt"], vec!["commit", "-m", "feature"]] {
        let output = repo
            .command("git", &args)
            .current_dir(&dir)
            .output()
            .expect("commit");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    repo.git(&["merge", "--ff-only", "feature"]);
    hooks(&repo, "\"true\"", "\"touch pre-remove.txt\"");

    let plan = repo.rabi(&["gc", "--json"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let document: serde_json::Value = serde_json::from_slice(&plan.stdout).expect("json");
    assert_eq!(document["data"]["items"][0]["action"], "skipped");
    assert_eq!(document["data"]["items"][0]["detail"], "untrusted hooks");
    assert!(
        !text(&plan.stdout).contains("Trust this project and run its hooks?"),
        "{}",
        text(&plan.stdout)
    );
    assert!(
        !text(&plan.stderr).contains("Trust this project and run its hooks?"),
        "{}",
        text(&plan.stderr)
    );

    let applied = repo.rabi(&["gc", "--apply", "--json"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let applied_doc: serde_json::Value = serde_json::from_slice(&applied.stdout).expect("json");
    assert_eq!(applied_doc["data"]["items"][0]["action"], "skipped");
    assert_eq!(applied_doc["data"]["items"][0]["detail"], "untrusted hooks");
    assert!(dir.is_dir());
    assert!(!repo.repo.join("pre-remove.txt").exists());
    assert!(!dir.join("pre-remove.txt").exists());

    let removed = repo.rabi(&["gc", "--apply", "--no-hooks"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    assert!(
        text(&removed.stdout).contains("removed feature"),
        "{}",
        text(&removed.stdout)
    );
    assert!(!dir.exists());
    assert!(!repo.repo.join("pre-remove.txt").exists());
}

fn an_agent_environment_does_not_prompt_on_a_terminal(name: &str, value: &str, json: bool) {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let args: &[&str] = if json {
        &["create", "topic", "--json"]
    } else {
        &["create", "topic"]
    };
    let output = pty_env(&repo, args, Some("y\n"), &[(name, value)]);
    assert_eq!(
        output.status.code(),
        Some(4),
        "stdout {}\nstderr {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let shown = text(&output.stdout);
    assert!(
        !shown.contains("Trust this project and run its hooks?"),
        "{shown}"
    );
    if json {
        let document: serde_json::Value = serde_json::from_str(&shown).expect(&shown);
        assert_eq!(document["error"]["code"], "untrusted");
        assert!(document["error"].get("exitCode").is_none());
    } else {
        assert!(shown.contains("Untrusted project"), "{shown}");
        assert!(shown.contains("--no-hooks"), "{shown}");
    }
    assert!(!worktree(&repo, "topic").exists());
    assert!(!trust_file(&repo).exists());
}

#[test]
fn ci_on_a_terminal_does_not_prompt_or_create() {
    an_agent_environment_does_not_prompt_on_a_terminal("CI", "true", false);
}

#[test]
fn ci_still_honors_no_hooks_and_trust_all() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let skipped = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "skipped", "--no-setup"],
        )
        .env("CI", "1")
        .output()
        .expect("rabi");
    assert!(skipped.status.success(), "{}", text(&skipped.stderr));
    assert!(worktree(&repo, "skipped").is_dir());
    assert!(!worktree(&repo, "skipped").join("hook-ran.txt").exists());

    let trusted = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "all"])
        .env("CI", "1")
        .env("RABI_TRUST_ALL", "1")
        .output()
        .expect("rabi");
    assert!(trusted.status.success(), "{}", text(&trusted.stderr));
    assert!(worktree(&repo, "all").join("hook-ran.txt").is_file());
    assert!(!trust_file(&repo).exists());
}

#[test]
fn ci_trust_add_records_the_grant_without_prompting() {
    let repo = Repo::new();
    hooks(&repo, "\"echo shown-hook\"", "\"true\"");
    let output = pty_env(&repo, &["trust", "add"], None, &[("CI", "1")]);
    assert_eq!(output.status.code(), Some(0), "{}", text(&output.stdout));
    assert!(
        !text(&output.stdout).contains("[y/N]"),
        "{}",
        text(&output.stdout)
    );
    assert!(text(&output.stdout).contains("echo shown-hook"));
    assert!(trust_file(&repo).exists());
}

#[test]
fn a_terminal_yes_trusts_and_runs_the_hook() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let output = pty(&repo, &["create", "topic"], Some("y\n"));
    assert!(
        output.status.success(),
        "stdout {}\nstderr {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let shown = text(&output.stdout);
    assert!(
        shown.contains("Trust this project and run its hooks? [y/N]"),
        "{shown}"
    );
    assert!(shown.contains("Project:"), "{shown}");
    assert!(shown.contains("touch hook-ran.txt"), "{shown}");
    assert!(worktree(&repo, "topic").join("hook-ran.txt").is_file());
    let stored = fs::read_to_string(trust_file(&repo)).expect("trust file");
    assert!(stored.contains(&common_dir(&repo)), "{stored}");
}

#[test]
fn a_terminal_no_creates_nothing() {
    let repo = Repo::new();
    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let output = pty(&repo, &["create", "topic"], Some("n\n"));
    assert_eq!(output.status.code(), Some(4), "{}", text(&output.stdout));
    assert!(
        text(&output.stdout).contains("Trust this project and run its hooks? [y/N]"),
        "{}",
        text(&output.stdout)
    );
    assert!(!worktree(&repo, "topic").exists());
    assert!(!trust_file(&repo).exists());
}

#[test]
fn gc_on_a_terminal_does_not_ask() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    let dir = worktree(&repo, "feature");
    fs::write(dir.join("feature.txt"), "feature\n").expect("note");
    for args in [vec!["add", "feature.txt"], vec!["commit", "-m", "feature"]] {
        let output = repo
            .command("git", &args)
            .current_dir(&dir)
            .output()
            .expect("commit");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    repo.git(&["merge", "--ff-only", "feature"]);
    hooks(&repo, "\"true\"", "\"touch pre-remove.txt\"");
    let output = pty(&repo, &["gc", "--apply"], None);
    assert!(
        output.status.success(),
        "stdout {}\nstderr {}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let shown = text(&output.stdout);
    assert!(!shown.contains("Trust this project"), "{shown}");
    assert!(shown.contains("untrusted hooks"), "{shown}");
    assert!(dir.is_dir());
}

#[test]
fn a_broken_trust_file_still_allows_skip_paths() {
    let repo = Repo::new();
    fs::create_dir_all(repo.root.join("state/rabi")).expect("state");
    fs::write(trust_file(&repo), "[[project]]\npath = 1\n").expect("broken");

    let plain = repo.rabi(&["create", "plain"]);
    assert!(plain.status.success(), "{}", text(&plain.stderr));

    hooks(&repo, "\"touch hook-ran.txt\"", "\"true\"");
    let skipped = repo.rabi(&["create", "skipped", "--no-setup"]);
    assert!(skipped.status.success(), "{}", text(&skipped.stderr));
    assert!(!worktree(&repo, "skipped").join("hook-ran.txt").exists());

    let trusted = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "ci"])
        .env("RABI_TRUST_ALL", "1")
        .output()
        .expect("rabi");
    assert!(trusted.status.success(), "{}", text(&trusted.stderr));
    assert!(worktree(&repo, "ci").join("hook-ran.txt").is_file());

    let blocked = repo.rabi(&["create", "needs-trust"]);
    assert_eq!(blocked.status.code(), Some(1));
    assert!(
        text(&blocked.stderr).contains("trust.toml"),
        "{}",
        text(&blocked.stderr)
    );
    assert!(!worktree(&repo, "needs-trust").exists());
}

fn pty(repo: &Repo, args: &[&str], answer: Option<&str>) -> std::process::Output {
    pty_env(repo, args, answer, &[])
}

fn pty_env(
    repo: &Repo,
    args: &[&str],
    answer: Option<&str>,
    env: &[(&str, &str)],
) -> std::process::Output {
    let script = r#"
import os, pty, select, subprocess, sys, time
argv = sys.argv[1:]
master, slave = pty.openpty()
proc = subprocess.Popen(argv, stdin=slave, stdout=slave, stderr=slave, close_fds=True)
os.close(slave)
answer = os.environ.get("RABI_TEST_ANSWER", "").encode()
buf = b""
sent = False
deadline = time.time() + 15
while time.time() < deadline:
    if proc.poll() is not None:
        while select.select([master], [], [], 0)[0]:
            try:
                chunk = os.read(master, 4096)
            except OSError:
                chunk = b""
            if not chunk:
                break
            buf += chunk
        break
    ready, _, _ = select.select([master], [], [], 0.2)
    if not ready:
        continue
    try:
        chunk = os.read(master, 4096)
    except OSError:
        break
    if not chunk:
        break
    buf += chunk
    if answer and not sent and b"Trust this project and run its hooks? [y/N]" in buf:
        os.write(master, answer)
        sent = True
code = proc.wait(timeout=5)
sys.stdout.buffer.write(buf)
raise SystemExit(code if code is not None else 1)
"#;
    let mut command = repo.command("python3", &[]);
    command.arg("-c").arg(script);
    command.arg(env!("CARGO_BIN_EXE_rabi"));
    command.args(args);
    if let Some(answer) = answer {
        command.env("RABI_TEST_ANSWER", answer);
    } else {
        command.env_remove("RABI_TEST_ANSWER");
    }
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().expect("python pty")
}

#[test]
fn explicit_no_input_and_environment_no_input_refuse_at_a_tty() {
    an_agent_environment_does_not_prompt_on_a_terminal("RABI_NO_INPUT", "1", false);
    an_agent_environment_does_not_prompt_on_a_terminal("RABI_NO_INPUT", "yes", false);
    let repo = Repo::new();
    hooks(&repo, "\"true\"", "\"true\"");
    let out = pty(&repo, &["create", "topic", "--no-input"], Some("y\n"));
    assert_eq!(out.status.code(), Some(4));
    assert!(!text(&out.stdout).contains("Trust this project and run its hooks?"));
    assert!(!worktree(&repo, "topic").exists());
    let out = pty_env(
        &repo,
        &["create", "topic"],
        Some("y\n"),
        &[("RABI_NO_INPUT", "0")],
    );
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert!(text(&out.stdout).contains("Trust this project and run its hooks?"));
}

#[test]
fn branch_and_detached_sources_preserve_trust_and_reuse_skips_authorization() {
    for (name, source) in [
        ("existing", vec!["--branch", "existing"]),
        ("snapshot", vec!["snapshot", "--detach"]),
    ] {
        let repo = Repo::new();
        repo.git(&["branch", "existing"]);
        hooks(&repo, "\"touch setup-ran\"", "");
        let args = [vec!["create"], source, vec!["--json"]].concat();
        let refused = repo.rabi(&args);
        assert_eq!(refused.status.code(), Some(4));
        let doc: serde_json::Value = serde_json::from_slice(&refused.stdout).unwrap();
        assert_eq!(doc["error"]["code"], "untrusted");
        let created = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_TRUST_ALL", "1")
            .output()
            .unwrap();
        assert!(created.status.success(), "{}", text(&created.stdout));
        let doc: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
        let path = PathBuf::from(doc["data"]["workspace"]["path"].as_str().unwrap());
        assert!(path.join("setup-ran").exists());
        fs::remove_file(path.join("setup-ran")).unwrap();
        let reuse = [args, vec!["--reuse"]].concat();
        let reused = repo.rabi(&reuse);
        assert!(reused.status.success(), "{name}: {}", text(&reused.stdout));
        assert!(!path.join("setup-ran").exists());
    }
}

#[test]
fn trust_add_records_bare_repositories_and_ignores_retired_workflow_files() {
    let repo = Repo::new();
    let bare = repo.root.join("bare.git");
    repo.git(&["init", "--bare", bare.to_str().unwrap()]);
    // Discovery would fail if trust tried to treat the bare Git directory as
    // a checkout. It must still be possible to grant common-directory trust.
    fs::create_dir(bare.join(".rabi")).unwrap();
    fs::write(bare.join(".rabi/workflows"), "not a workflow directory").unwrap();
    let common = fs::canonicalize(&bare).unwrap();
    for json in [false, true] {
        let args = if json {
            vec!["trust", "add", "--json"]
        } else {
            vec!["trust", "add"]
        };
        let output = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&bare)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            text(&output.stdout),
            text(&output.stderr)
        );
        assert!(!text(&output.stderr).contains("workflows:"));
        if json {
            let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(document["schema"], 2);
            assert_eq!(document["command"], "trust add");
            assert_eq!(document["ok"], true);
            assert_eq!(document["data"]["path"], common.to_str().unwrap());
            assert!(document["data"]["trustedAt"].is_string());
            assert_eq!(document["data"].as_object().unwrap().len(), 2);
        } else {
            assert_eq!(
                text(&output.stdout),
                format!("Trusted {}", common.display())
            );
        }
        assert!(fs::read_to_string(trust_file(&repo))
            .unwrap()
            .contains(common.to_str().unwrap()));
    }
}
