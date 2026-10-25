#[path = "common/records.rs"]
mod records;
#[path = "common/git.rs"]
mod test_git;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo {
    root: PathBuf,
    repo: PathBuf,
    home: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("phantom-rust-{}", uuid()));
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
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn rabi(&self, args: &[&str]) -> std::process::Output {
        self.command(env!("CARGO_BIN_EXE_rabi"), args)
            .output()
            .expect("rabi")
    }

    fn create_recorded(&self, args: &[&str]) -> std::process::Output {
        let mut create = vec!["create"];
        let mut intent = vec!["set", args[1]];
        let mut index = 1;
        while index < args.len() {
            if matches!(args[index], "--pr" | "--issue") {
                intent.extend_from_slice(&args[index..index + 2]);
                index += 2;
            } else {
                create.push(args[index]);
                index += 1;
            }
        }
        let created = self.rabi(&create);
        assert!(created.status.success(), "{}", text(&created.stderr));
        let set = self.rabi(&intent);
        assert!(set.status.success(), "{}", text(&set.stderr));
        self.rabi(&["show", args[1], "--json"])
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
            .env_remove("GHQ_ROOT")
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .env_remove("RABI_NAME_CANDIDATES")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        if std::path::Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            == Some("rabi")
        {
            command.env("RABI_TRUST_ALL", "1");
        }
        // Parallel `git init` hardlinks the shared template and can fail with
        // "File exists" on the hook samples. Give each repo an empty template.
        if program.ends_with("git") && args.first() == Some(&"init") {
            let template = self.root.join("git-template");
            fs::create_dir_all(&template).expect("git template");
            command.env("GIT_TEMPLATE_DIR", &template);
        }
        command
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn managed_directory(git_root: &Path, xdg: &Path) -> PathBuf {
    let common = rabi::git::canonical_common_dir(git_root).expect("common dir");
    rabi::paths::default_worktrees_directory(git_root, &common, xdg).path
}

fn managed(repo: &Repo, relative: &str) -> PathBuf {
    managed_directory(&repo.repo, &repo.root.join("xdg")).join(relative)
}

fn managed_root(repo: &Repo) -> PathBuf {
    managed_directory(&repo.repo, &repo.root.join("xdg"))
}

fn uuid() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{nanos}-{}", std::process::id())
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_end().to_string()
}

// Close the reader before spawning, so even a short help page deterministically
// hits EPIPE instead of depending on scheduling or the platform's pipe capacity.
fn closed_pipe() -> std::io::PipeWriter {
    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader);
    writer
}

fn assert_closed_stdout(repo: &Repo, args: &[&str], expected: i32) {
    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), args)
        .stdout(closed_pipe())
        .output()
        .expect("rabi with closed stdout");
    let stderr = text(&output.stderr);
    assert!(!stderr.contains("panicked"), "{args:?}: {stderr}");
    assert_eq!(output.status.code(), Some(expected), "{args:?}: {stderr}");
}

#[test]
fn broken_pipe_help_stdout_preserves_success() {
    let repo = Repo::new();
    for args in [vec!["help", "--json"], vec!["help"], vec!["--help"]] {
        assert_closed_stdout(&repo, &args, 0);
    }
}

#[test]
fn broken_pipe_large_list_stdout_preserves_success() {
    let repo = Repo::new();
    for index in 0..16 {
        let path = repo.root.join(format!("workspace-{index}"));
        repo.git(&[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap(),
            "HEAD",
        ]);
        rabi::metadata::write(
            &repo.repo,
            &path,
            &rabi::metadata::MetadataInput {
                purpose: Some("x".repeat(1024)),
                ..Default::default()
            },
        )
        .expect("workspace metadata");
    }
    for args in [vec!["list"], vec!["list", "--json"]] {
        let normal = repo.rabi(&args);
        assert!(normal.status.success(), "{}", text(&normal.stderr));
        assert!(normal.stdout.len() > 16 * 1024, "large output fixture");
        assert_closed_stdout(&repo, &args, 0);
    }
}

#[test]
fn broken_pipe_stderr_preserves_success_and_usage_status() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"true\"]\n",
    )
    .unwrap();
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    // Setup emits progress and succeeds; usage writes diagnostics and help.
    for (args, expected) in [
        (vec!["setup", "topic", "--json"], 0),
        (vec!["not-a-command"], 2),
    ] {
        let normal = repo.rabi(&args);
        assert_eq!(normal.status.code(), Some(expected));
        assert!(!normal.stderr.is_empty());
        let output = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .stderr(closed_pipe())
            .output()
            .unwrap();
        assert!(!text(&output.stdout).contains("panicked"));
        assert_eq!(output.status.code(), Some(expected), "{args:?}");
        if args[0] == "setup" {
            // Each setup records a new timestamp; stdout must remain valid JSON.
            let doc = assert_envelope(&output, "setup");
            assert_eq!(doc["data"]["setup"]["state"], "ok");
        } else {
            assert_eq!(output.stdout, normal.stdout);
        }
    }
    assert_closed_stdout(&repo, &["not-a-command", "--json"], 2);
}

#[test]
fn broken_pipe_progress_does_not_interrupt_create_or_setup() {
    for json in [false, true] {
        for (last_hook, expected, state) in [("true", 0, "ok"), ("exit 7", 6, "failed")] {
            let repo = Repo::new();
            fs::write(
                repo.repo.join(".rabi.toml"),
                format!("[hooks]\nsetup = [\"touch setup-completed\", \"{last_hook}\"]\n"),
            )
            .unwrap();
            let args = if json {
                vec!["create", "topic", "--json"]
            } else {
                vec!["create", "topic"]
            };
            let mut command = repo.command(env!("CARGO_BIN_EXE_rabi"), &args);
            if json {
                command.stderr(closed_pipe());
            } else {
                command.stdout(closed_pipe());
            }
            let output = command.output().unwrap();
            assert!(!text(&output.stderr).contains("panicked"));
            assert_eq!(
                output.status.code(),
                Some(expected),
                "{}",
                text(&output.stderr)
            );
            let shown = repo.rabi(&["show", "topic", "--json"]);
            assert!(shown.status.success(), "{}", text(&shown.stderr));
            let document: serde_json::Value = serde_json::from_slice(&shown.stdout).unwrap();
            let path = document["data"]["workspace"]["path"].as_str().unwrap();
            assert!(Path::new(path).join("setup-completed").is_file());
            assert_eq!(document["data"]["workspace"]["setup"]["state"], state);
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn json_stderr_errors_are_included_before_emitting_the_envelope() {
    let repo = Repo::new();
    let args = ["config", "list", "--json"];
    let normal = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &args)
        .env("GIT_CONFIG_COUNT", "broken")
        .output()
        .unwrap();
    assert!(normal.status.success(), "{}", text(&normal.stderr));
    assert!(!normal.stderr.is_empty());
    let expected = assert_envelope(&normal, "config list");
    assert!(!expected["warnings"].as_array().unwrap().is_empty());

    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &args)
        .env("GIT_CONFIG_COUNT", "broken")
        .stderr(fs::File::options().write(true).open("/dev/full").unwrap())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let document = assert_envelope(&output, "config list");
    assert_eq!(document["ok"], false);
    assert_eq!(document["error"]["code"], "failed");
    assert!(document["error"]["message"]
        .as_str()
        .unwrap()
        .starts_with("Failed to write stderr: "));
    assert_eq!(document["data"], expected["data"]);
    assert_eq!(document["warnings"], expected["warnings"]);
}

#[cfg(target_os = "linux")]
#[test]
fn json_stderr_errors_preserve_mutations_and_override_partial_status() {
    for last_hook in ["true", "exit 7"] {
        let repo = Repo::new();
        fs::write(
            repo.repo.join(".rabi.toml"),
            format!("[hooks]\nsetup = [\"touch setup-completed\", \"{last_hook}\"]\n"),
        )
        .unwrap();
        let output = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["create", "topic", "--json"])
            .stderr(fs::File::options().write(true).open("/dev/full").unwrap())
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        let document = assert_envelope(&output, "create");
        assert_eq!(document["ok"], false);
        assert_eq!(document["error"]["code"], "failed");
        assert!(document["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Failed to write stderr: "));
        let workspace = &document["data"]["workspace"];
        let path = workspace["path"].as_str().unwrap();
        assert!(Path::new(path).join("setup-completed").is_file());
        assert_eq!(
            workspace["setup"]["state"],
            if last_hook == "true" { "ok" } else { "failed" }
        );
    }
}

#[cfg(target_os = "linux")]
#[test]
fn output_errors_other_than_broken_pipe_fail_without_panicking() {
    let repo = Repo::new();
    for args in [vec!["help"], vec!["help", "--json"], vec!["--help"]] {
        let output = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .stdout(fs::File::options().write(true).open("/dev/full").unwrap())
            .output()
            .unwrap();
        let stderr = text(&output.stderr);
        assert!(!stderr.contains("panicked"), "{stderr}");
        assert!(stderr.contains("Failed to write stdout:"), "{stderr}");
        assert_eq!(output.status.code(), Some(1));
    }
    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["help"])
        .stdout(fs::File::options().write(true).open("/dev/full").unwrap())
        .stderr(closed_pipe())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["not-a-command"])
        .stderr(fs::File::options().write(true).open("/dev/full").unwrap())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
}

fn rabi_color(
    repo: &Repo,
    args: &[&str],
    no_color: Option<&str>,
    force_color: Option<&str>,
) -> std::process::Output {
    let mut command = repo.command(env!("CARGO_BIN_EXE_rabi"), args);
    command.env_remove("NO_COLOR").env_remove("FORCE_COLOR");
    if let Some(value) = no_color {
        command.env("NO_COLOR", value);
    }
    if let Some(value) = force_color {
        command.env("FORCE_COLOR", value);
    }
    command.output().expect("rabi")
}

#[test]
fn help_is_plain_when_stdout_is_not_a_tty() {
    let repo = Repo::new();
    let help = rabi_color(&repo, &["--help"], None, None);
    assert!(help.status.success());
    assert!(
        !text(&help.stdout).contains('\u{1b}'),
        "piped help should not include ANSI color: {}",
        text(&help.stdout)
    );
    let list = rabi_color(&repo, &["list", "--help"], None, None);
    assert!(list.status.success());
    assert!(!text(&list.stdout).contains('\u{1b}'));

    let forced = rabi_color(&repo, &["--help"], None, Some("1"));
    assert!(
        text(&forced.stdout).contains('\u{1b}'),
        "FORCE_COLOR=1 should color piped help"
    );

    let no_color = rabi_color(&repo, &["--help"], Some("1"), Some("1"));
    assert!(
        !text(&no_color.stdout).contains('\u{1b}'),
        "NO_COLOR disables color even when FORCE_COLOR is set"
    );

    let disabled = rabi_color(&repo, &["create", "--pr", "--help"], None, Some("0"));
    assert!(!text(&disabled.stdout).contains('\u{1b}'));
}

#[test]
fn outside_a_git_repository_is_a_phantom_error() {
    let repo = Repo::new();
    let outside = repo.root.join("outside");
    fs::create_dir_all(&outside).expect("outside");
    for args in [
        &["list"][..],
        &["create", "name"],
        &["path", "name"],
        &["remove", "name"],
        &["exec", "name", "--", "true"],
        &["gc"],
    ] {
        let output = repo
            .command(env!("CARGO_BIN_EXE_rabi"), args)
            .current_dir(&outside)
            .output()
            .expect("rabi");
        let stderr = text(&output.stderr);
        assert!(
            !stderr.to_ascii_lowercase().contains("fatal:"),
            "{args:?} leaked git output: {stderr}"
        );
        assert!(
            stderr.contains("Not a git repository. Run rabi from inside a Git repository."),
            "{args:?} stderr: {stderr}"
        );
        assert_eq!(output.status.code(), Some(3), "{args:?} stderr: {stderr}");
    }
    let help = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["--help"])
        .current_dir(&outside)
        .output()
        .expect("help");
    assert!(help.status.success());
}

#[test]
fn version_help_and_unknown_command() {
    let repo = Repo::new();
    let version = repo.rabi(&["--version"]);
    assert!(version.status.success());
    let version_line = text(&version.stdout);
    assert!(
        version_line.starts_with("rabi v7.0.0+rust"),
        "{version_line}"
    );
    assert!(!version_line.contains("ptm"), "{version_line}");
    assert_ne!(version_line, "rabi v7.0.0");
    let subcommand = repo.rabi(&["-v"]);
    assert_eq!(text(&subcommand.stdout), version_line);

    let help = repo.rabi(&["--help"]);
    assert!(help.status.success());
    let help_text = text(&help.stdout);
    assert!(help_text.contains("rabi - Isolated Git workspaces for parallel coding agents"));
    assert!(!help_text.contains("Phantom"), "{help_text}");
    assert!(help_text.contains("rabi <command> [options]"));
    assert!(help_text.contains("Run 'rabi <command> --help'"));
    assert!(!help_text.contains("phantom <command>"));

    for args in [["--version", "--help"], ["repo", "--help"]] {
        let command_help = repo.rabi(&args);
        assert!(command_help.status.success(), "{args:?}");
        let command_text = text(&command_help.stdout);
        assert!(
            !command_text.contains("Phantom"),
            "{args:?} still names Phantom:\n{command_text}"
        );
    }

    let where_help = repo.rabi(&["path", "--help"]);
    assert!(where_help.status.success());
    let where_text = text(&where_help.stdout);
    assert!(where_text.contains("rabi path"));
    assert!(where_text.contains("rabi path [--repo <ref>] [SEL] [options]"));
    assert!(!where_text.contains("phantom path"));

    let missing_where = repo.rabi(&["path"]);
    assert_eq!(missing_where.status.code(), Some(0));
    assert!(same_dir(&text(&missing_where.stdout), &repo.repo));

    let unknown = repo.rabi(&["nope"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(text(&unknown.stderr).contains("Unknown command 'nope'"));
}

#[test]
fn linked_worktree_git_file_and_insteadof_canonical_name() {
    let repo = Repo::new();
    let worktree = repo.root.join("linked");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "feature",
        worktree.to_str().expect("worktree"),
        "HEAD",
    ]);
    let gitfile = worktree.join(".git");
    assert!(gitfile.is_file(), "{}", gitfile.display());
    let from_worktree = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["list", "-R", worktree.to_str().expect("worktree"), "--json"],
        )
        .output()
        .expect("list workspace");
    assert!(
        from_worktree.status.success(),
        "{}",
        text(&from_worktree.stderr)
    );
    let from_gitfile = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["list", "-R", gitfile.to_str().expect("git file"), "--json"],
        )
        .output()
        .expect("list git file");
    assert!(
        from_gitfile.status.success(),
        "stderr: {}",
        text(&from_gitfile.stderr)
    );
    assert_eq!(
        default_worktree_path(&json_value(&from_gitfile)),
        default_worktree_path(&json_value(&from_worktree))
    );
    assert!(same_dir(
        &default_worktree_path(&json_value(&from_gitfile)),
        &repo.repo
    ));

    let project = repo.root.join("aliased");
    fs::create_dir_all(&project).expect("repo");
    git_at(&repo, &project, &["init", "-b", "main"]);
    git_at(
        &repo,
        &project,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(&repo, &project, &["config", "user.name", "Phantom Rust"]);
    fs::write(project.join("README.md"), "hi\n").expect("readme");
    git_at(&repo, &project, &["add", "README.md"]);
    git_at(&repo, &project, &["commit", "-m", "initial"]);
    git_at(
        &repo,
        &project,
        &["remote", "add", "origin", "gh:zed/aliased"],
    );
    git_at(
        &repo,
        &project,
        &["config", "url.https://github.com/.insteadOf", "gh:"],
    );
    let added = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["repo", "add", project.to_str().expect("repo"), "--json"],
        )
        .output()
        .expect("repo add");
    assert!(added.status.success(), "{}", text(&added.stderr));
    let catalog = repo.rabi(&["repo", "list", "--json"]);
    assert!(catalog.status.success(), "{}", text(&catalog.stderr));
    let catalog = json_value(&catalog);
    let entry = catalog["data"]["repos"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project_entry| {
            project_entry["rootPath"]
                .as_str()
                .is_some_and(|path| same_dir(path, &project))
        })
        .expect("registered project");
    assert_eq!(entry["source"], "registry");
    assert_eq!(entry["canonicalName"], "github.com/zed/aliased");
}

fn default_worktree_path(value: &serde_json::Value) -> String {
    value["data"]["workspaces"]
        .as_array()
        .expect("worktrees")
        .iter()
        .find(|item| item["kind"] == "primary")
        .expect("primary checkout")["path"]
        .as_str()
        .expect("path")
        .to_string()
}

#[test]
fn create_list_where_delete_and_attach() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature/add-tests"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(text(&created.stdout).contains("Created workspace 'feature/add-tests' at "));
    let worktree = managed(&repo, "feature/add-tests");
    assert!(worktree.join("README.md").is_file());
    assert!(!repo.repo.join(".git/phantom").exists());
    assert!(!repo.repo.join(".git/ptm").exists());
    assert!(repo.repo.join(".git/rabi/workspaces").is_dir());

    let duplicate = repo.rabi(&["create", "feature/add-tests"]);
    assert_eq!(duplicate.status.code(), Some(4));
    assert!(text(&duplicate.stderr).contains("already exists"));

    let names = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    assert!(names.status.success(), "{}", text(&names.stderr));
    assert_eq!(text(&names.stdout), "feature/add-tests");

    let empty = repo.rabi(&["list", "--kind", "workspace"]);
    let _ = empty;
    let where_path = repo.rabi(&["path", "feature/add-tests"]);
    assert_eq!(where_path.status.code(), Some(0));
    assert_eq!(text(&where_path.stdout), worktree.to_string_lossy());

    let missing = repo.rabi(&["path", "missing-worktree"]);
    assert_eq!(missing.status.code(), Some(3));
    assert!(text(&missing.stderr).contains("not found"));

    repo.git(&["branch", "existing-feature"]);
    let attached = repo.rabi(&["create", "--branch", "existing-feature"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    assert!(text(&attached.stdout).contains("Created workspace 'existing-feature'"));
    let missing_branch = repo.rabi(&["create", "--branch", "missing-branch"]);
    assert_eq!(missing_branch.status.code(), Some(3));

    let kept = repo.rabi(&["remove", "existing-feature", "--keep-branch"]);
    assert!(kept.status.success(), "{}", text(&kept.stderr));
    assert!(text(&kept.stdout).contains("kept its branch 'existing-feature'"));
    let branch = repo
        .command("git", &["branch", "--list", "existing-feature"])
        .output()
        .expect("branch");
    assert!(text(&branch.stdout).contains("existing-feature"));

    let deleted = repo.rabi(&["remove", "feature/add-tests"]);
    assert!(deleted.status.success(), "{}", text(&deleted.stderr));
    assert!(text(&deleted.stdout).contains("and its branch 'feature/add-tests'"));
    assert!(!worktree.exists());
    let missing_delete = repo.rabi(&["remove", "missing-worktree"]);
    assert_eq!(missing_delete.status.code(), Some(3));
}

#[test]
fn invalid_config_names_the_file_and_the_problem() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".rabi.toml"), "[\n").expect("config");
    let created = repo.rabi(&["create", "feature/bad-toml"]);
    let stderr = text(&created.stderr);
    assert!(
        !created.status.success(),
        "invalid TOML should fail, stdout: {}",
        text(&created.stdout)
    );
    assert!(
        stderr.contains(".rabi.toml"),
        "stderr should name the file: {stderr}"
    );
    assert!(
        stderr.contains("parse") || stderr.contains("expected") || stderr.contains("invalid"),
        "stderr should describe the TOML problem: {stderr}"
    );
    assert!(!managed(&repo, "feature/bad-toml").exists());

    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = \"pnpm install\"\n",
    )
    .expect("schema");
    let created = repo.rabi(&["path", "feature/bad-toml"]);
    let stderr = text(&created.stderr);
    assert!(!created.status.success(), "{}", text(&created.stdout));
    assert!(stderr.contains(".rabi.toml"), "{stderr}");
    assert!(
        stderr.contains("setup") || stderr.contains("invalid type"),
        "{stderr}"
    );

    fs::write(
        repo.repo.join(".rabi.toml"),
        "worktreesDirectory = \".phantom\"\n",
    )
    .expect("unknown");
    let created = repo.rabi(&["path", "feature/bad-toml"]);
    let stderr = text(&created.stderr);
    assert!(!created.status.success(), "{}", text(&created.stdout));
    assert!(stderr.contains(".rabi.toml"), "{stderr}");
    assert!(stderr.contains("worktreesDirectory"), "{stderr}");
}

#[test]
fn github_checkout_logs_the_post_create_hook() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"true\"]\n",
    )
    .expect("config");
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = \"repo\" ]; then\n  printf '%s\\n' '{\"url\":\"https://github.com/acme/demo\",\"owner\":{\"login\":\"acme\"},\"name\":\"demo\"}'\n  exit 0\nfi\nif [ \"$1\" = \"api\" ]; then\n  printf '%s\\n' '{\"number\":7}'\n  exit 0\nfi\nprintf '%s\\n' \"unexpected gh: $*\" >&2\nexit 1\n",
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let cli = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "--issue", "7"])
        .env("PATH", &path)
        .output()
        .expect("cli");
    assert!(
        cli.status.success(),
        "{}\n{}",
        text(&cli.stdout),
        text(&cli.stderr)
    );
    assert!(
        text(&cli.stdout).contains("Executing: true"),
        "CLI checkout should log the hook:\n{}",
        text(&cli.stdout)
    );
}

#[test]
fn post_create_failure_reports_the_created_worktree() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"phantom-missing-setup-bin\"]\n",
    )
    .expect("config");
    let created = repo.rabi(&["create", "feature/hooks"]);
    let worktree = managed(&repo, "feature/hooks");
    assert!(
        worktree.join("README.md").is_file(),
        "workspace should exist after a setup failure"
    );
    let stdout = text(&created.stdout);
    let stderr = text(&created.stderr);
    assert!(
        stdout.contains("Created workspace 'feature/hooks' at "),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains(&worktree.to_string_lossy().to_string()),
        "stdout: {stdout}"
    );
    assert!(
        stdout.contains("Executing: phantom-missing-setup-bin"),
        "stdout: {stdout}"
    );
    assert!(
        stderr.contains("Warning:"),
        "stderr should warn, not look like creation failed: {stderr}"
    );
    assert!(stderr.contains("phantom-missing-setup-bin"), "{stderr}");
    assert!(stderr.contains("127"), "{stderr}");
    assert_eq!(created.status.code(), Some(6), "{stderr}");

    repo.git(&["branch", "existing-hook"]);
    let attached = repo.rabi(&["create", "--branch", "existing-hook"]);
    let attached_path = managed(&repo, "existing-hook");
    assert!(attached_path.join("README.md").is_file());
    assert!(
        text(&attached.stdout).contains("Created workspace 'existing-hook'"),
        "{}",
        text(&attached.stdout)
    );
    assert!(
        text(&attached.stderr).contains("Warning:"),
        "{}",
        text(&attached.stderr)
    );
    assert_eq!(attached.status.code(), Some(6));
}

#[test]
fn copies_worktreeinclude_files_and_runs_post_create() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".gitignore"), ".env.local\n").expect("gitignore");
    fs::write(repo.repo.join(".env.local"), "API_KEY=test-value\n").expect("env");
    fs::write(repo.repo.join(".worktreeinclude"), ".env.local\n").expect("include");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch setup.txt\"]\n",
    )
    .expect("config");
    let created = repo.rabi(&["create", "feature/config"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let worktree = managed(&repo, "feature/config");
    assert_eq!(
        fs::read_to_string(worktree.join(".env.local")).expect("copied"),
        "API_KEY=test-value\n"
    );
    assert!(text(&created.stdout).contains("Running setup commands"));
    assert!(text(&created.stdout).contains("Executing: touch setup.txt"));
    assert!(worktree.join("setup.txt").is_file());
}

#[test]
fn hooks_run_in_order_stop_on_failure_and_export_root_path() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        r#"[hooks]
setup = [
  "printf '%s' \"$RABI_REPO_PATH\" > root-path.txt",
  "printf '%s' \"$RABI\" > rabi-flag.txt",
  "printf '%s' \"$RABI_WORKSPACE\" > rabi-name.txt",
  "printf '%s' \"$RABI_WORKSPACE_PATH\" > rabi-path.txt",
  "pwd > hook-cwd.txt",
  "touch first.txt",
  "false",
  "touch skipped.txt",
]
pre-remove = [
  "touch pre-first.txt",
  "exit 9",
  "touch pre-skipped.txt",
]
"#,
    )
    .expect("toml");

    let created = repo.rabi(&["create", "feature/hooks-order"]);
    let worktree = managed(&repo, "feature/hooks-order");
    assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
    assert_eq!(
        fs::read_to_string(worktree.join("root-path.txt")).expect("root path"),
        repo.repo.display().to_string()
    );
    assert_eq!(
        fs::read_to_string(worktree.join("rabi-flag.txt")).expect("flag"),
        "1"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("rabi-name.txt")).expect("name"),
        "feature/hooks-order"
    );
    assert_eq!(
        fs::read_to_string(worktree.join("rabi-path.txt")).expect("path"),
        worktree.display().to_string()
    );
    assert_eq!(
        text(&fs::read(worktree.join("hook-cwd.txt")).expect("cwd")),
        worktree.display().to_string()
    );
    assert!(worktree.join("first.txt").is_file());
    assert!(!worktree.join("skipped.txt").exists());

    let deleted = repo.rabi(&[
        "remove",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "feature/hooks-order",
    ]);
    let stderr = text(&deleted.stderr);
    assert!(!deleted.status.success(), "{}", text(&deleted.stdout));
    assert!(stderr.contains("exit code 9"), "{stderr}");
    assert!(
        worktree.exists(),
        "pre-remove failure should keep the workspace"
    );
    assert!(worktree.join("pre-first.txt").is_file());
    assert!(!worktree.join("pre-skipped.txt").exists());
}

#[test]
fn hooks_follow_the_source_checkout_when_run_from_a_worktree() {
    let repo = Repo::new();
    let source_created = repo.rabi(&["create", "feature/source"]);
    assert!(
        source_created.status.success(),
        "{}\n{}",
        text(&source_created.stdout),
        text(&source_created.stderr)
    );
    let source = managed(&repo, "feature/source");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch from-main.txt\"]\npre-remove = [\"touch pre-from-main.txt\"]\n",
    )
    .expect("main hooks");
    fs::write(
        source.join(".rabi.toml"),
        r#"[hooks]
setup = ["printf '%s' \"$RABI_REPO_PATH\" > root-path.txt"]
pre-remove = ["printf '%s' \"$RABI_REPO_PATH\" > \"$RABI_REPO_PATH/pre-root.txt\""]
"#,
    )
    .expect("source hooks");
    let toplevel = repo
        .command("git", &["rev-parse", "--show-toplevel"])
        .current_dir(&source)
        .output()
        .expect("toplevel");
    assert!(toplevel.status.success(), "{}", text(&toplevel.stderr));
    let source_path = text(&toplevel.stdout);
    assert_ne!(source_path, repo.repo.display().to_string());

    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "feature/child"])
        .current_dir(&source)
        .output()
        .expect("create");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let child = managed(&repo, "feature/child");
    assert_eq!(
        fs::read_to_string(child.join("root-path.txt")).expect("root path"),
        repo.repo.display().to_string()
    );
    assert!(!child.join("from-main.txt").exists());

    let deleted = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "remove",
                "--discard-changes",
                "--delete-unpreserved-branch",
                "feature/child",
            ],
        )
        .current_dir(&source)
        .output()
        .expect("remove");
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.repo.join("pre-root.txt")).expect("pre-remove"),
        repo.repo.display().to_string()
    );
    assert!(!child.join("pre-from-main.txt").exists());
}

#[test]
fn phantom_config_json_is_not_read() {
    let repo = Repo::new();
    fs::write(repo.repo.join("phantom.config.json"), "{").expect("json");
    let created = repo.rabi(&["create", "feature/no-json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(managed(&repo, "feature/no-json/README.md").is_file());
}

#[test]
fn preferences_and_project_registry() {
    let repo = Repo::new();
    let set = repo.rabi(&["config", "set", "discover-ghq", "true"]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let set_stdout = text(&set.stdout);
    assert!(set_stdout.contains("discover-ghq"), "{set_stdout}");
    assert!(set_stdout.contains("true"), "{set_stdout}");
    assert!(
        set_stdout.contains(
            repo.home
                .join(".config/rabi/config.toml")
                .to_str()
                .expect("path")
        ),
        "{set_stdout}"
    );
    let get = repo.rabi(&["config", "get", "discover-ghq"]);
    assert_eq!(text(&get.stdout), "true");

    let added = repo.rabi(&["repo", "add", "--json"]);
    assert!(added.status.success(), "{}", text(&added.stderr));
    let value: serde_json::Value = serde_json::from_str(&text(&added.stdout)).expect("json");
    assert_eq!(value["data"]["status"], "added");
    assert_eq!(value["data"]["repo"]["name"], "repo");
    assert_eq!(
        value["data"]["repo"]["rootPath"],
        repo.repo.to_string_lossy().as_ref()
    );

    repo.rabi(&["config", "set", "discover-ghq", "false"]);
    let listed = repo.rabi(&["repo", "list", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let catalog: serde_json::Value = serde_json::from_str(&text(&listed.stdout)).expect("catalog");
    assert_eq!(catalog["schema"], 2);
    assert_eq!(catalog["data"]["repos"][0]["source"], "registry");
    assert_eq!(
        catalog["data"]["repos"][0]["id"],
        value["data"]["repo"]["id"]
    );
    assert_eq!(catalog["data"]["repos"][0]["name"], "repo");
    assert_eq!(catalog["data"]["repos"][0]["canonicalName"], "repo");

    let removed = repo.rabi(&[
        "repo",
        "forget",
        value["data"]["repo"]["id"].as_str().expect("id"),
        "--json",
    ]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    let empty = repo.rabi(&["repo", "list", "--json"]);
    let empty_catalog: serde_json::Value =
        serde_json::from_str(&text(&empty.stdout)).expect("empty");
    assert_eq!(
        empty_catalog["data"]["repos"].as_array().map(Vec::len),
        Some(0)
    );
}

#[test]
fn project_registry_is_stored_under_xdg_state_home_rabi() {
    let repo = Repo::new();
    let added = repo.rabi(&["repo", "add", "--json"]);
    assert!(added.status.success(), "{}", text(&added.stderr));
    let registry = fs::read_to_string(
        fs::read_dir(repo.root.join("state/rabi/repos"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .expect("registry");
    assert!(registry.contains("\"name\": \"repo\""));

    let disabled = repo.rabi(&["config", "set", "discover-ghq", "false"]);
    assert!(disabled.status.success(), "{}", text(&disabled.stderr));
    let listed = repo.rabi(&["repo", "list", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let catalog: serde_json::Value = serde_json::from_str(&text(&listed.stdout)).expect("catalog");
    assert_eq!(catalog["data"]["repos"].as_array().map(Vec::len), Some(1));
    assert_eq!(catalog["data"]["repos"][0]["name"], "repo");
    assert_eq!(catalog["data"]["repos"][0]["source"], "registry");

    let help = repo.rabi(&["repo", "--help"]);
    assert!(help.status.success(), "{}", text(&help.stderr));
    let help_text = text(&help.stdout);
    assert!(help_text.contains("$XDG_STATE_HOME/rabi"));
    assert!(help_text.contains("~/.local/state/rabi"));
    assert!(!help_text.contains("XDG_STATE_HOME/phantom"));
    assert!(!help_text.contains(".local/state/phantom"));
    assert!(!help_text.contains("XDG_STATE_HOME/ptm"));
    assert!(!help_text.contains(".local/state/ptm"));
}

#[test]
fn project_registry_defaults_to_home_local_state_rabi() {
    let repo = Repo::new();
    let disabled = repo.rabi(&["config", "set", "discover-ghq", "false"]);
    assert!(disabled.status.success(), "{}", text(&disabled.stderr));

    let added = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "add", "--json"])
        .env_remove("XDG_STATE_HOME")
        .output()
        .expect("add");
    assert!(added.status.success(), "{}", text(&added.stderr));
    let registry = fs::read_to_string(
        fs::read_dir(repo.home.join(".local/state/rabi/repos"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path(),
    )
    .expect("registry");
    assert!(registry.contains("\"name\": \"repo\""));

    let listed = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "list", "--names"])
        .env("XDG_STATE_HOME", "relative/state")
        .output()
        .expect("list");
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    assert_eq!(text(&listed.stdout), "repo");
    assert!(!repo.repo.join("relative").exists());
}

#[test]
fn preferences_file_uses_xdg_config_home() {
    let repo = Repo::new();
    let path = repo.home.join(".config/rabi/config.toml");
    let gitconfig = fs::read_to_string(repo.home.join(".gitconfig")).expect("gitconfig");

    let missing = repo.rabi(&["config", "get", "discover-ghq"]);
    assert!(missing.status.success(), "{}", text(&missing.stderr));
    let missing_stdout = text(&missing.stdout);
    assert_eq!(missing_stdout, "true");
    assert!(!path.exists());

    let invalid_bool = repo.rabi(&["config", "set", "discover-ghq", "yes"]);
    assert_eq!(invalid_bool.status.code(), Some(2));
    assert!(
        text(&invalid_bool.stderr).contains("true"),
        "{}",
        text(&invalid_bool.stderr)
    );
    assert!(!path.exists());

    let set = repo.rabi(&["config", "set", "workspace-root", "../spaces"]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let written = fs::read_to_string(&path).expect("config");
    assert!(
        written.contains("workspace-root = \"../spaces\""),
        "{written}"
    );
    assert_eq!(
        fs::read_to_string(repo.home.join(".gitconfig")).expect("gitconfig"),
        gitconfig
    );

    fs::write(
        &path,
        "# header\ndiscover-ghq = false # inline\n\n# separator\nworkspace-root = \"../spaces\"\n",
    )
    .expect("seed");
    let updated = repo.rabi(&["config", "set", "discover-ghq", "true"]);
    assert!(updated.status.success(), "{}", text(&updated.stderr));
    let written = fs::read_to_string(&path).expect("updated");
    assert!(written.contains("# header"), "{written}");
    assert!(written.contains("# inline"), "{written}");
    assert!(written.contains("# separator"), "{written}");
    assert!(written.contains("discover-ghq = true"), "{written}");
    assert!(
        written.contains("workspace-root = \"../spaces\""),
        "{written}"
    );

    let removed = repo.rabi(&["config", "unset", "discover-ghq"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    let written = fs::read_to_string(&path).expect("removed");
    assert!(!written.contains("discover-ghq"), "{written}");
    assert!(written.contains("# separator"), "{written}");

    fs::write(&path, "{\n").expect("invalid");
    let invalid = repo.rabi(&["config", "get", "discover-ghq"]);
    assert_eq!(invalid.status.code(), Some(1));
    let stderr = text(&invalid.stderr);
    assert!(stderr.contains(path.to_str().expect("path")), "{stderr}");

    fs::write(&path, "editor = \"code\"\n").expect("unknown");
    let unknown = repo.rabi(&["config", "get", "discover-ghq"]);
    assert_eq!(unknown.status.code(), Some(1));
    let stderr = text(&unknown.stderr);
    assert!(stderr.contains("editor"), "{stderr}");
    assert!(stderr.contains(path.to_str().expect("path")), "{stderr}");

    fs::remove_file(&path).expect("cleanup");
    repo.git(&["config", "--global", "phantom.keepBranch", "true"]);
    repo.git(&["config", "--global", "ptm.keepBranch", "true"]);
    let ignored = repo.rabi(&["config", "get", "discover-ghq"]);
    assert!(ignored.status.success(), "{}", text(&ignored.stderr));
    assert!(text(&ignored.stdout) == "true", "{}", text(&ignored.stdout));
    assert!(!path.exists());
}

#[test]
fn reports_when_only_the_default_worktree_exists() {
    let repo = Repo::new();
    let result = repo.rabi(&["list", "--kind", "workspace"]);
    assert_eq!(result.status.code(), Some(0), "{}", text(&result.stderr));
    assert!(text(&result.stdout).contains("No linked workspaces found"));
}

#[test]
fn project_add_from_a_linked_worktree_is_idempotent() {
    let repo = Repo::new();
    let added = repo.rabi(&["repo", "add", "--json"]);
    assert!(added.status.success(), "{}", text(&added.stderr));
    let value: serde_json::Value = serde_json::from_str(&text(&added.stdout)).expect("json");
    let linked = repo.root.join("linked-worktree");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "project-linked",
        linked.to_str().expect("path"),
    ]);
    let again = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["repo", "add", linked.to_str().expect("path"), "--json"],
        )
        .current_dir(&linked)
        .output()
        .expect("add");
    assert!(again.status.success(), "{}", text(&again.stderr));
    let again_value: serde_json::Value = serde_json::from_str(&text(&again.stdout)).expect("json");
    assert_eq!(again_value["data"]["status"], "existing");
    assert_eq!(again_value["data"]["repo"], value["data"]["repo"]);
}

#[test]
fn project_list_discovers_ghq_and_warns_when_discovery_fails() {
    let repo = Repo::new();
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let ghq = bin.join("ghq");
    fs::write(
        &ghq,
        "#!/bin/sh\nif [ \"$1\" != \"list\" ] || [ \"$2\" != \"--full-path\" ]; then\n  exit 2\nfi\nprintf \"%s\\n\" \"$RABI_TEST_GHQ_PROJECT\"\n",
    )
    .expect("ghq");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&ghq).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&ghq, perms.clone()).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let discovered = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "list", "--json"])
        .env("PATH", &path)
        .env("RABI_TEST_GHQ_PROJECT", &repo.repo)
        .output()
        .expect("list");
    assert!(discovered.status.success(), "{}", text(&discovered.stderr));
    let catalog: serde_json::Value = serde_json::from_str(&text(&discovered.stdout)).expect("json");
    assert_eq!(catalog["schema"], 2);
    assert_eq!(catalog["data"]["repos"][0]["source"], "ghq");
    assert_eq!(
        catalog["data"]["repos"][0]["rootPath"],
        repo.repo.to_string_lossy().as_ref()
    );

    fs::write(
        &ghq,
        "#!/bin/sh\nprintf '%s\\n' 'simulated ghq failure' >&2\nexit 17\n",
    )
    .expect("fail");
    fs::set_permissions(&ghq, perms).expect("chmod");
    let added = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "add", "--json"])
        .env("PATH", &path)
        .output()
        .expect("add");
    assert!(added.status.success(), "{}", text(&added.stderr));
    let warned = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "list", "--json"])
        .env("PATH", &path)
        .output()
        .expect("warn");
    assert!(warned.status.success(), "{}", text(&warned.stderr));
    assert!(text(&warned.stdout).contains("\"source\": \"registry\""));
    assert!(json_value(&warned)["warnings"]
        .as_array()
        .is_some_and(|warnings| !warnings.is_empty()));
    assert!(text(&warned.stderr).contains("Warning: Failed to discover ghq repositories:"));
    assert!(text(&warned.stderr).contains("simulated ghq failure"));
}

#[test]
fn worktreeinclude_copies_only_matching_gitignored_files() {
    let repo = Repo::new();
    fs::write(repo.repo.join("tracked.env"), "committed\n").expect("tracked");
    repo.git(&["add", "tracked.env"]);
    repo.git(&["commit", "-m", "track env"]);
    fs::write(
        repo.repo.join(".gitignore"),
        "*.env\n*.log\nfrom-config.local\n",
    )
    .expect("gitignore");
    fs::write(
        repo.repo.join(".worktreeinclude"),
        "*.env\nnested/**\n!nested/skip.env\n",
    )
    .expect("include");
    fs::write(
        repo.repo.join("phantom.config.json"),
        r#"{"postCreate":{"copyFiles":["from-config.local"]}}"#,
    )
    .expect("legacy config");
    fs::create_dir_all(repo.repo.join("nested/deep")).expect("nested");
    fs::write(repo.repo.join("secret.env"), "secret\n").expect("secret");
    fs::write(repo.repo.join("nested/deep/inner.env"), "inner\n").expect("inner");
    fs::write(repo.repo.join("nested/skip.env"), "skip\n").expect("skip");
    fs::write(repo.repo.join("nested/deep/note.txt"), "note\n").expect("note");
    fs::write(repo.repo.join("notes.log"), "log\n").expect("log");
    fs::write(repo.repo.join("other.log"), "other\n").expect("other");
    fs::write(repo.repo.join("from-config.local"), "config\n").expect("legacy");
    fs::write(repo.repo.join("tracked.env"), "dirty\n").expect("dirty");

    let created = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "copied", "--copy-file", "notes.log"],
        )
        .current_dir(repo.repo.join("nested"))
        .output()
        .expect("rabi");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let worktree = managed(&repo, "copied");
    assert_eq!(read_copy(&worktree.join("secret.env")), "secret\n");
    assert_eq!(
        read_copy(&worktree.join("nested/deep/inner.env")),
        "inner\n"
    );
    assert_eq!(read_copy(&worktree.join("notes.log")), "log\n");
    assert_eq!(read_copy(&worktree.join("tracked.env")), "committed\n");
    assert!(!worktree.join("nested/skip.env").exists());
    assert!(!worktree.join("nested/deep/note.txt").exists());
    assert!(!worktree.join("other.log").exists());
    assert!(!worktree.join("from-config.local").exists());

    repo.git(&["branch", "attached"]);
    let attached = repo.rabi(&["create", "--branch", "attached"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    let attached_path = managed(&repo, "attached");
    assert_eq!(read_copy(&attached_path.join("secret.env")), "secret\n");
    assert!(!attached_path.join("nested/skip.env").exists());
    assert!(!attached_path.join("other.log").exists());
}

#[test]
fn copies_gitignored_files_from_primary_even_from_another_checkout() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".gitignore"), "*.env\n").expect("gitignore");
    fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").expect("include");
    fs::write(repo.repo.join("secret.env"), "from-main\n").expect("main env");
    repo.git(&["add", ".gitignore", ".worktreeinclude"]);
    repo.git(&["commit", "-m", "include"]);
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "source",
        "custom/workspaces/source",
    ]);
    let source = repo.repo.join("custom/workspaces/source");
    fs::write(source.join("secret.env"), "from-source\n").expect("source env");

    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "copied"])
        .current_dir(&source)
        .output()
        .expect("rabi");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert_eq!(
        read_copy(&managed(&repo, "copied/secret.env")),
        "from-main\n"
    );
}

fn read_copy(path: &std::path::Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

#[test]
fn copy_file_globs_ignore_brackets_in_the_repository_path() {
    let root = std::env::temp_dir().join(format!(
        "phantom-bracket-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let repo_dir = root.join("repo[tmp]");
    let home = root.join("home");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    fs::create_dir_all(&repo_dir).expect("repo");
    test_git::write_config(&home);
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&repo_dir)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .output()
            .expect("git");
        assert!(output.status.success(), "{}", text(&output.stderr));
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.name", "Phantom Rust"]);
    git(&["config", "user.email", "phantom-rust@example.com"]);
    fs::write(repo_dir.join("README.md"), "#\n").expect("readme");
    git(&["add", "README.md"]);
    git(&["commit", "-m", "initial"]);
    fs::write(repo_dir.join(".gitignore"), "*.env\n").expect("gitignore");
    fs::write(repo_dir.join("kept.env"), "kept\n").expect("kept");
    let output = Command::new(env!("CARGO_BIN_EXE_rabi"))
        .args(["create", "copied", "--copy-file", "*.env"])
        .current_dir(&repo_dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("xdg"))
        .output()
        .expect("rabi");
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(
        fs::read_to_string(
            managed_directory(&repo_dir, &root.join("xdg")).join("copied/kept.env"),
        )
        .expect("copied"),
        "kept\n"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn concurrent_project_add_processes_keep_every_record() {
    let repo = Repo::new();
    repo.rabi(&["config", "set", "discover-ghq", "false"]);
    let mut children = Vec::new();
    for index in 0..6 {
        let path = repo.root.join(format!("project-{index}"));
        fs::create_dir_all(&path).expect("repo");
        let init = repo
            .command("git", &["init", "-b", "main"])
            .current_dir(&path)
            .output()
            .expect("init");
        assert!(init.status.success(), "{}", text(&init.stderr));
        let child = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["repo", "add", path.to_str().expect("path"), "--json"],
            )
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn");
        children.push(child);
    }
    for child in children {
        let output = child.wait_with_output().expect("wait");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    let listed = repo.rabi(&["repo", "list", "--names"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let names = text(&listed.stdout);
    for index in 0..6 {
        assert!(names.contains(&format!("project-{index}")), "{names}");
    }
}

#[test]
fn create_and_attach_exec_export_rabi_env() {
    let repo = Repo::new();
    for (name, branch) in [("new-env", false), ("existing-env", true)] {
        if branch {
            repo.git(&["branch", name]);
        }
        let args = if branch {
            vec!["create", "--branch", name]
        } else {
            vec!["create", name]
        };
        assert!(repo.rabi(&args).status.success());
        let output = repo.rabi(&[
            "exec",
            name,
            "--",
            "sh",
            "-c",
            "printf '%s\n' \"$RABI\" \"$RABI_WORKSPACE\" \"$RABI_WORKSPACE_PATH\"",
        ]);
        assert!(output.status.success());
        assert_eq!(
            text(&output.stdout),
            format!("1\n{name}\n{}", managed(&repo, name).display())
        );
    }
}

#[test]
fn parent_cleanup_failure_still_deletes_the_branch() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "cursor/foo"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let root = managed_root(&repo);
    use std::os::unix::fs::PermissionsExt;
    let _reset = ModeReset(&root);
    let mut perms = fs::metadata(&root).expect("meta").permissions();
    perms.set_mode(0o555);
    fs::set_permissions(&root, perms).expect("chmod");

    let deleted = repo.rabi(&["remove", "cursor/foo"]);
    assert!(
        deleted.status.success(),
        "cleanup must not fail the delete\n{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    let combined = format!("{}\n{}", text(&deleted.stdout), text(&deleted.stderr));
    assert!(
        combined.contains("Warning:"),
        "cleanup error should be a warning: {combined}"
    );
    let branch = repo
        .command("git", &["branch", "--list", "cursor/foo"])
        .output()
        .expect("branch");
    assert!(
        text(&branch.stdout).is_empty(),
        "branch should still be deleted: {}",
        text(&branch.stdout)
    );
}

struct ModeReset<'a>(&'a std::path::Path);

impl Drop for ModeReset<'_> {
    fn drop(&mut self) {
        let Ok(metadata) = fs::metadata(self.0) else {
            return;
        };
        let mut perms = metadata.permissions();
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o755);
        let _ = fs::set_permissions(self.0, perms);
    }
}

#[test]
fn delete_removes_empty_parents_but_keeps_the_project_directory_and_marker() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "cursor/foo"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let project = managed_root(&repo);
    let marker_path = project.join(rabi::paths::PROJECT_MARKER);
    let marker = fs::read(&marker_path).expect("marker");
    let ceiling = project.parent().expect("rabi/workspaces");
    let parent = project.join("cursor");
    assert!(parent.join("foo").is_dir());
    let deleted = repo.rabi(&["remove", "cursor/foo"]);
    assert!(deleted.status.success(), "{}", text(&deleted.stderr));
    assert!(!parent.join("foo").exists());
    assert!(!parent.exists(), "empty parent directory should be removed");
    assert!(project.is_dir(), "empty project directory should stay");
    assert_eq!(fs::read(&marker_path).expect("retained marker"), marker);
    assert!(ceiling.is_dir(), "rabi/workspaces should stay");

    assert!(repo.rabi(&["create", "cursor/foo"]).status.success());
    assert!(repo.rabi(&["create", "cursor/bar"]).status.success());
    assert!(repo.rabi(&["remove", "cursor/foo"]).status.success());
    assert!(parent.join("bar").is_dir());
    assert!(parent.is_dir(), "parent still holds another workspace");
    assert!(
        project.is_dir(),
        "project directory still holds a workspace"
    );
    assert!(repo.rabi(&["remove", "cursor/bar"]).status.success());
    assert!(!parent.exists());
    assert!(project.is_dir());
    assert_eq!(fs::read(&marker_path).expect("retained marker"), marker);
    assert!(ceiling.is_dir());

    assert!(repo.rabi(&["create", "a/b/c"]).status.success());
    assert!(repo.rabi(&["remove", "a/b/c"]).status.success());
    assert!(!managed(&repo, "a").exists());
    assert!(ceiling.is_dir());
}

#[test]
fn detached_worktree_keeps_its_directory_name() {
    let repo = Repo::new();
    let worktree = managed(&repo, "next");
    repo.git(&[
        "worktree",
        "add",
        "--detach",
        worktree.to_str().expect("path"),
        "HEAD",
    ]);
    let head = repo
        .command("git", &["rev-parse", "--short=7", "HEAD"])
        .output()
        .expect("rev-parse");
    let sha = text(&head.stdout);
    let names = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    assert!(names.status.success(), "{}", text(&names.stderr));
    assert_eq!(text(&names.stdout), "next");
    let listed = repo.rabi(&["list", "--kind", "workspace"]);
    assert!(
        text(&listed.stdout)
            .lines()
            .any(|line| line.trim_start().starts_with("next  (detached)")),
        "{}",
        text(&listed.stdout)
    );
    assert!(
        !text(&listed.stdout).starts_with(&sha),
        "{}",
        text(&listed.stdout)
    );

    let found = repo.rabi(&["path", "next"]);
    assert_eq!(found.status.code(), Some(0), "{}", text(&found.stderr));
    assert_eq!(text(&found.stdout), worktree.to_string_lossy());

    let moved = repo
        .command(
            "git",
            &["commit", "--allow-empty", "-m", "move detached head"],
        )
        .current_dir(&worktree)
        .output()
        .expect("commit");
    assert!(moved.status.success(), "{}", text(&moved.stderr));
    let names_after = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    assert_eq!(text(&names_after.stdout), "next");

    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        managed(&repo, "alias").to_str().expect("path"),
        "HEAD",
    ]);
    let by_directory = repo.rabi(&["path", "alias"]);
    let by_branch = repo.rabi(&["path", "topic"]);
    let alias = managed(&repo, "alias");
    assert_eq!(text(&by_directory.stdout), alias.to_string_lossy());
    assert_eq!(text(&by_branch.stdout), alias.to_string_lossy());
    let both = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    let listed_names = text(&both.stdout);
    assert!(listed_names.contains("next"), "{listed_names}");
    assert!(listed_names.contains("alias"), "{listed_names}");
    assert!(!listed_names.lines().any(|line| line == "topic"));
}

#[test]
fn directory_name_is_preferred_over_another_worktrees_branch() {
    let repo = Repo::new();
    let branch_topic = managed(&repo, "a");
    let directory_topic = managed(&repo, "topic");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        branch_topic.to_str().expect("path"),
        "HEAD",
    ]);
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "other",
        directory_topic.to_str().expect("path"),
        "HEAD",
    ]);

    let found = repo.rabi(&["path", "topic"]);
    assert_eq!(found.status.code(), Some(0), "{}", text(&found.stderr));
    assert_eq!(
        text(&found.stdout),
        directory_topic.to_string_lossy(),
        "directory name should win over branch topic checked out at a"
    );

    let deleted = repo.rabi(&["remove", "topic", "--keep-branch"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(!directory_topic.exists());
    assert!(
        branch_topic.exists(),
        "delete topic must not remove the workspace whose branch is topic"
    );
    let kept = repo
        .command("git", &["branch", "--list", "other"])
        .output()
        .expect("branch");
    assert!(
        text(&kept.stdout).contains("other"),
        "kept the branch of the directory named topic"
    );
}

fn fake_gh_for_topic_pr(repo: &Repo) -> String {
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
if [ "$1" = "repo" ]; then
  printf '%s\n' '{"url":"https://github.com/acme/demo","owner":{"login":"acme"},"name":"demo"}'
  exit 0
fi
case "$2" in
  *pulls*)
    printf '%s\n' '{"head":{"ref":"topic","repo":{"full_name":"acme/demo"}},"base":{"repo":{"full_name":"acme/demo"}}}'
    ;;
  *)
    printf '%s\n' '{"number":7,"pull_request":{}}'
    ;;
esac
"#,
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn publish_pr_topic(repo: &Repo) {
    let origin = repo.root.join("origin.git");
    let init = repo
        .command(
            "git",
            &[
                "init",
                "--bare",
                "-b",
                "main",
                origin.to_str().expect("origin"),
            ],
        )
        .output()
        .expect("bare");
    assert!(init.status.success(), "{}", text(&init.stderr));
    repo.git(&["remote", "add", "origin", origin.to_str().expect("origin")]);
    repo.git(&["checkout", "-b", "topic"]);
    fs::write(repo.repo.join("topic.txt"), "topic\n").expect("topic");
    repo.git(&["add", "topic.txt"]);
    repo.git(&["commit", "-m", "topic"]);
    repo.git(&["push", "origin", "main", "topic"]);
    repo.git(&["push", "origin", "topic:refs/pull/7/head"]);
    repo.git(&["checkout", "main"]);
    repo.git(&["branch", "-D", "topic"]);
}

fn local_branch(repo: &Repo, name: &str) -> String {
    let listed = repo
        .command("git", &["branch", "--list", name])
        .output()
        .expect("branch");
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    text(&listed.stdout)
}

#[test]
fn pr_checkout_requires_the_branch_not_only_the_directory() {
    let repo = Repo::new();
    let path = fake_gh_for_topic_pr(&repo);
    let directory = managed(&repo, "topic");
    reserve_project_dir(&repo.repo, &repo.root.join("xdg"));
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "other",
        directory.to_str().expect("path"),
        "HEAD",
    ]);
    let output = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse"],
        )
        .env("PATH", &path)
        .output()
        .expect("checkout");
    let combined = format!("{}\n{}", text(&output.stdout), text(&output.stderr));
    assert!(
        !combined.contains("Reused workspace"),
        "directory topic on branch other is not PR branch topic:\n{combined}"
    );
    let current = repo
        .command("git", &["branch", "--show-current"])
        .current_dir(&directory)
        .output()
        .expect("branch");
    assert_eq!(text(&current.stdout), "other");

    let matching = Repo::new();
    let matching_path = fake_gh_for_topic_pr(&matching);
    let checked_out = managed(&matching, "topic");
    matching.git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        checked_out.to_str().expect("path"),
        "HEAD",
    ]);
    let again = matching
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse"],
        )
        .env("PATH", &matching_path)
        .output()
        .expect("again");
    assert!(
        again.status.success(),
        "{}\n{}",
        text(&again.stdout),
        text(&again.stderr)
    );
    assert!(
        text(&again.stdout).contains("Reused workspace"),
        "{}",
        text(&again.stdout)
    );
}

fn json_value(output: &std::process::Output) -> serde_json::Value {
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not JSON ({error})\n{}\n{}",
            text(&output.stdout),
            text(&output.stderr)
        )
    });
    let command = value["command"].as_str().expect("command");
    assert!(!command.is_empty());
    assert_envelope(output, command)
}

fn paths_match(json_path: &str, expected: &std::path::Path) -> bool {
    let left = std::path::Path::new(json_path);
    left == expected || fs::canonicalize(left).ok() == fs::canonicalize(expected).ok()
}

fn full_head(repo: &Repo) -> String {
    let output = repo
        .command("git", &["rev-parse", "HEAD"])
        .output()
        .expect("head");
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

fn assert_worktree_object(value: &serde_json::Value, name: &str, path: &std::path::Path) {
    assert_eq!(value["name"], name);
    assert!(
        paths_match(value["path"].as_str().unwrap_or(""), path),
        "path {} != {}",
        value["path"],
        path.display()
    );
    assert!(value["path"].as_str().unwrap_or("").starts_with('/'));
    for field in [
        "id",
        "name",
        "kind",
        "path",
        "current",
        "repo",
        "managed",
        "orphaned",
        "branch",
        "head",
        "detached",
        "dirty",
        "base",
        "createdFrom",
        "upstream",
        "purpose",
        "links",
        "createdAt",
        "updatedAt",
    ] {
        assert!(value.get(field).is_some(), "missing {field}: {value}");
    }
    assert!(value["repo"]["root"].as_str().unwrap().starts_with('/'));
    assert!(value["repo"]["name"].is_string());
    assert_eq!(value["kind"], "workspace");
    assert!(value.get("rootPath").is_none());
    assert!(value.get("is_clean").is_none());
    for field in ["owner", "ownerRelation", "lastUsedAt"] {
        assert!(value.get(field).is_none(), "removed field {field}: {value}");
    }
}

#[test]
fn worktree_json_covers_list_where_create_attach_and_delete() {
    let repo = Repo::new();
    let head = full_head(&repo);
    let empty = repo.rabi(&["list", "--json", "--kind", "workspace"]);
    assert!(empty.status.success(), "{}", text(&empty.stderr));
    let empty_json = json_value(&empty);
    assert_eq!(empty_json["schema"], 2);
    assert_eq!(
        empty_json["data"]["workspaces"].as_array().map(Vec::len),
        Some(0)
    );
    assert!(text(&empty.stderr).is_empty(), "{}", text(&empty.stderr));

    let created = repo.rabi(&["create", "feature/json", "--json", "--base", "HEAD"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created_json = json_value(&created);
    assert_eq!(created_json["schema"], 2);
    assert_worktree_object(
        &created_json["data"]["workspace"],
        "feature/json",
        &managed(&repo, "feature/json"),
    );
    assert_eq!(created_json["data"]["workspace"]["branch"], "feature/json");
    assert_eq!(created_json["data"]["workspace"]["head"], head);
    assert_eq!(created_json["data"]["workspace"]["base"], "refs/heads/main");
    assert_eq!(created_json["data"]["workspace"]["dirty"], false);
    assert_eq!(created_json["data"]["workspace"]["detached"], false);
    assert!(created_json["data"]["copy"]["error"].is_null());
    assert!(created_json["data"]["copy"]["error"].is_null());
    assert_eq!(created_json["data"]["setup"]["ran"], false);
    assert_eq!(created_json["data"]["setup"]["ok"], true);
    assert!(created_json["data"]["setup"]["command"].is_null());
    assert!(created_json["data"]["setup"]["exitCode"].is_null());
    assert!(created_json["data"]["exec"].is_null());
    assert!(!text(&created.stdout).contains("Created workspace"));

    let duplicate = repo.rabi(&["create", "feature/json", "--json"]);
    assert_eq!(duplicate.status.code(), Some(4));
    let duplicate_json = json_value(&duplicate);
    assert_eq!(duplicate_json["error"]["code"], "exists");
    assert!(duplicate_json["error"].get("exitCode").is_none());
    assert!(
        text(&duplicate.stderr).is_empty(),
        "{}",
        text(&duplicate.stderr)
    );

    let both = repo.rabi(&["list", "--json", "--names"]);
    assert_eq!(both.status.code(), Some(2));
    assert_eq!(json_value(&both)["error"]["code"], "usage");

    let listed = repo.rabi(&["list", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let listed_json = json_value(&listed);
    let worktrees = listed_json["data"]["workspaces"]
        .as_array()
        .expect("worktrees");
    assert!(worktrees
        .iter()
        .any(|entry| entry["kind"] == "primary" && entry["branch"] == "main"));
    let feature = worktrees
        .iter()
        .find(|entry| entry["name"] == "feature/json")
        .expect("feature");
    assert_eq!(feature["head"], head);
    assert_eq!(feature["dirty"], false);
    assert_eq!(feature["base"], "refs/heads/main");

    let worktree = managed(&repo, "feature/json");
    fs::write(worktree.join("README.md"), "dirty\n").expect("dirty");
    let dirty = repo.rabi(&["list", "--json", "--kind", "workspace"]);
    let dirty_json = json_value(&dirty);
    assert_eq!(dirty_json["data"]["workspaces"][0]["dirty"], true);
    assert_eq!(
        dirty_json["data"]["workspaces"].as_array().map(Vec::len),
        Some(1)
    );

    let located = repo.rabi(&["show", "feature/json", "--json"]);
    assert!(located.status.success(), "{}", text(&located.stderr));
    let located_json = json_value(&located);
    assert_eq!(located_json["schema"], 2);
    assert_worktree_object(
        &located_json["data"]["workspace"],
        "feature/json",
        &worktree,
    );
    assert!(!text(&located.stdout).contains("feature/json ("));

    let missing = repo.rabi(&["show", "missing-worktree", "--json"]);
    assert_eq!(missing.status.code(), Some(3));
    let missing_json = json_value(&missing);
    assert_eq!(missing_json["schema"], 2);
    assert_eq!(missing_json["error"]["code"], "not_found");
    assert!(missing_json["error"].get("exitCode").is_none());
    assert!(missing_json["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("missing-worktree"));
    assert!(
        text(&missing.stderr).is_empty(),
        "{}",
        text(&missing.stderr)
    );

    repo.git(&["branch", "existing-json"]);
    let attached = repo.rabi(&["create", "--branch", "existing-json", "--json"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    let attached_json = json_value(&attached);
    assert_worktree_object(
        &attached_json["data"]["workspace"],
        "existing-json",
        &managed(&repo, "existing-json"),
    );
    assert!(attached_json["data"]["workspace"]["base"].is_null());
    assert_eq!(
        attached_json["data"]["workspace"]["branch"],
        "existing-json"
    );
    assert!(attached_json["data"]["copy"]["error"].is_null());
    assert!(attached_json["data"]["exec"].is_null());

    let kept = repo.rabi(&["remove", "existing-json", "--keep-branch", "--json"]);
    assert!(kept.status.success(), "{}", text(&kept.stderr));
    let kept_json = json_value(&kept);
    assert!(kept_json["error"].is_null());
    assert_eq!(kept_json["data"]["items"][0]["branchDeleted"], false);
    assert!(kept_json["data"]["items"][0]["branchError"].is_null());
    assert!(kept_json["data"]["items"][0]["error"].is_null());
    assert_eq!(
        kept_json["data"]["items"][0]["workspace"]["name"],
        "existing-json"
    );
    let branch = repo
        .command("git", &["branch", "--list", "existing-json"])
        .output()
        .expect("branch");
    assert!(text(&branch.stdout).contains("existing-json"));

    fs::write(worktree.join("README.md"), "# phantom\n").expect("restore");
    repo.git(&["branch", "second-json"]);
    assert!(repo
        .rabi(&["create", "--branch", "second-json"])
        .status
        .success());
    let partial = repo.rabi(&["remove", "feature/json", "missing-json", "--json"]);
    assert_eq!(partial.status.code(), Some(3));
    let document = json_value(&partial);
    assert_eq!(document["data"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(document["data"]["items"][0]["removed"], false);
    assert_eq!(document["data"]["items"][1]["error"]["code"], "not_found");
    assert!(worktree.exists());

    let outside = repo.root.join("outside");
    fs::create_dir_all(&outside).expect("outside");
    let not_repo = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["list", "--json"])
        .current_dir(&outside)
        .output()
        .expect("list");
    assert_eq!(not_repo.status.code(), Some(3));
    let not_repo_json = json_value(&not_repo);
    assert_eq!(not_repo_json["error"]["code"], "not_repository");
    assert!(not_repo_json["error"].get("exitCode").is_none());
    assert!(
        text(&not_repo.stderr).is_empty(),
        "{}",
        text(&not_repo.stderr)
    );

    let project_modes = repo.rabi(&["repo", "list", "--json", "--names"]);
    assert_eq!(project_modes.status.code(), Some(2));
    assert_eq!(json_value(&project_modes)["error"]["code"], "usage");
    assert_eq!(json_value(&project_modes)["schema"], 2);
}

#[test]
fn json_mode_keeps_hook_and_exec_output_off_stdout() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"echo HOOK_STDOUT_MARKER\"]\npre-remove = [\"echo PRE_DELETE_MARKER\"]\n",
    )
    .expect("hooks");
    let created = repo.rabi(&["create", "feature/hooks-json", "--json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let created_json = json_value(&created);
    assert!(!text(&created.stdout).contains("HOOK_STDOUT_MARKER"));
    assert!(text(&created.stderr)
        .lines()
        .any(|line| line == "HOOK_STDOUT_MARKER"));
    let executed = repo.rabi(&["exec", "feature/hooks-json", "--", "echo", "EXEC_MARKER"]);
    assert!(executed.status.success());
    assert_eq!(text(&executed.stdout), "EXEC_MARKER");
    assert_eq!(created_json["data"]["setup"]["ran"], true);
    assert_eq!(created_json["data"]["setup"]["ok"], true);
    assert_eq!(created_json["data"]["setup"]["exitCode"], 0);

    let deleted = repo.rabi(&[
        "remove",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "feature/hooks-json",
        "--json",
    ]);
    assert!(deleted.status.success(), "{}", text(&deleted.stderr));
    assert!(!text(&deleted.stdout).contains("PRE_DELETE_MARKER"));
    assert!(text(&deleted.stderr).contains("PRE_DELETE_MARKER"));
    assert_eq!(
        json_value(&deleted)["data"]["items"][0]["branchDeleted"],
        true
    );

    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"echo HOOK_FAIL_MARKER\", \"exit 7\"]\n",
    )
    .expect("failing hook");
    let failed = repo.rabi(&["create", "feature/hook-fail", "--json"]);
    assert_eq!(failed.status.code(), Some(6), "{}", text(&failed.stderr));
    let failed_json = json_value(&failed);
    assert_eq!(failed_json["data"]["setup"]["ok"], false);
    assert_eq!(failed_json["data"]["setup"]["exitCode"], 7);
    assert_eq!(failed_json["data"]["setup"]["command"], "exit 7");
    assert_eq!(
        failed_json["data"]["workspace"]["name"],
        "feature/hook-fail"
    );
    assert!(!text(&failed.stdout).contains("HOOK_FAIL_MARKER"));
    assert!(text(&failed.stderr).contains("HOOK_FAIL_MARKER"));
    assert!(managed(&repo, "feature/hook-fail/README.md").is_file());

    fs::write(repo.repo.join(".rabi.toml"), "").expect("clear hooks");
    assert!(repo
        .rabi(&["create", "feature/exec-fail", "--json"])
        .status
        .success());
    let exec_failed = repo.rabi(&[
        "exec",
        "feature/exec-fail",
        "--",
        "sh",
        "-c",
        "echo EXEC_FAIL; exit 3",
    ]);
    assert_eq!(exec_failed.status.code(), Some(3));
    assert_eq!(text(&exec_failed.stdout), "EXEC_FAIL");
    assert!(managed(&repo, "feature/exec-fail/README.md").is_file());
}

#[test]
fn json_reports_detached_head_and_copy_errors() {
    let repo = Repo::new();
    let detached = managed(&repo, "next");
    reserve_project_dir(&repo.repo, &repo.root.join("xdg"));
    repo.git(&[
        "worktree",
        "add",
        "--detach",
        detached.to_str().expect("path"),
        "HEAD",
    ]);
    let listed = repo.rabi(&["list", "--json", "--kind", "workspace"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let entry = &json_value(&listed)["data"]["workspaces"][0];
    assert_eq!(entry["name"], "next");
    assert!(entry["branch"].is_null());
    assert_eq!(entry["detached"], true);
    assert_eq!(entry["head"], full_head(&repo));
    assert_eq!(entry["kind"], "workspace");

    std::os::unix::fs::symlink("elsewhere", repo.repo.join(".worktreeinclude")).expect("symlink");
    let created = repo.rabi(&["create", "feature/copy", "--json"]);
    assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
    let created_json = json_value(&created);
    assert!(!created_json["data"]["copy"]["error"].is_null());
    assert!(created_json["data"]["copy"]["error"]
        .as_str()
        .unwrap_or("")
        .contains("symlink"));
    assert_eq!(created_json["data"]["workspace"]["name"], "feature/copy");
}

#[test]
fn project_parse_errors_keep_the_validation_exit_code() {
    let repo = Repo::new();
    let cases: &[&[&str]] = &[
        &["repo", "add", "--unknown"],
        &["repo", "list", "--unknown"],
        &["repo", "forget", "--unknown"],
        &["repo", "add", "--unknown", "--json"],
        &["repo", "list", "--unknown", "--json"],
        &["repo", "forget", "--unknown", "--json"],
    ];
    for args in cases {
        let output = repo.rabi(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}\nstdout:\n{}\nstderr:\n{}",
            text(&output.stdout),
            text(&output.stderr)
        );
        if args.contains(&"--json") {
            let value = json_value(&output);
            assert_eq!(value["schema"], 2, "{args:?} {value}");
            assert_eq!(value["error"]["code"], "usage", "{args:?} {value}");
            assert!(value["error"].get("exitCode").is_none(), "{args:?} {value}");
            assert!(
                text(&output.stderr).is_empty(),
                "{args:?} stderr: {}",
                text(&output.stderr)
            );
        } else {
            assert!(
                text(&output.stderr).contains("Unknown option"),
                "{args:?} {}",
                text(&output.stderr)
            );
        }
    }
}

#[test]
fn json_unborn_head_is_null() {
    let repo = Repo::new();
    repo.git(&["update-ref", "-d", "HEAD"]);
    let listed = repo.rabi(&["list", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let listed_json = json_value(&listed);
    let worktrees = listed_json["data"]["workspaces"]
        .as_array()
        .expect("worktrees");
    assert_eq!(worktrees.len(), 1, "{listed_json}");
    assert_eq!(worktrees[0]["name"], "main");
    assert_eq!(worktrees[0]["branch"], "main");
    assert_eq!(worktrees[0]["kind"], "primary");
    assert!(
        worktrees[0]["head"].is_null(),
        "unborn HEAD should be null, got {}",
        worktrees[0]["head"]
    );

    let located = repo.rabi(&["show", "main", "--json"]);
    assert!(located.status.success(), "{}", text(&located.stderr));
    let located_json = json_value(&located);
    assert!(
        located_json["data"]["workspace"]["head"].is_null(),
        "{located_json}"
    );
    assert_eq!(located_json["data"]["workspace"]["branch"], "main");
}

#[test]
fn argument_errors_are_validation_for_every_command() {
    let repo = Repo::new();
    let cases: &[&[&str]] = &[
        &["create", "topic", "--base"],
        &["create", "topic", "--json", "--base"],
        &["list", "--unknown"],
        &["list", "--json", "--unknown"],
        &["path", "--bogus"],
        &["show", "--json", "--bogus"],
        &["remove", "--bogus", "x"],
        &["remove", "--json", "--bogus", "x"],
        &["create", "--branch", "--zzz", "b"],
        &["create", "--json", "--branch", "--zzz", "b"],
        &["create", "--pr", "--bogus", "1"],
        &["create", "--pr", "1", "--json", "--base"],
        &["exec", "--bogus", "name", "true"],
        &["config", "get", "--bogus"],
        &["--version", "extra"],
        &["clone", "--bogus", "owner/repo"],
        &["clone", "owner/repo", "--json", "--depth"],
    ];
    for args in cases {
        let output = repo.rabi(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}\nstdout:\n{}\nstderr:\n{}",
            text(&output.stdout),
            text(&output.stderr)
        );
        let detail = format!("{}\n{}", text(&output.stdout), text(&output.stderr));
        assert!(
            detail.contains("looks like an option")
                || detail.contains("Unknown option")
                || detail.contains("argument missing")
                || detail.contains("Unexpected argument"),
            "{args:?} {detail}"
        );
        if args.contains(&"--json") {
            let value = json_value(&output);
            assert_eq!(value["error"]["code"], "usage", "{args:?} {value}");
            assert!(value["error"].get("exitCode").is_none(), "{args:?} {value}");
            assert!(
                text(&output.stderr).is_empty(),
                "{args:?} stderr: {}",
                text(&output.stderr)
            );
        }
    }
}

#[test]
fn invalid_names_are_validation_and_missing_worktrees_exit_not_found() {
    let repo = Repo::new();
    for args in [
        &["create", "a/../x"][..],
        &["create", "a/../x", "--json"],
        &["create", "--branch", "a/../x"],
        &["create", "--branch", "a/../x", "--json"],
    ] {
        let output = repo.rabi(args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}\n{}\n{}",
            text(&output.stdout),
            text(&output.stderr)
        );
        if args.contains(&"--json") {
            let value = json_value(&output);
            assert_eq!(value["error"]["code"], "usage", "{args:?} {value}");
            assert!(value["error"].get("exitCode").is_none(), "{args:?} {value}");
            assert!(value.get("worktree").is_none(), "{args:?} {value}");
            assert!(text(&output.stderr).is_empty(), "{args:?}");
        } else {
            assert!(
                text(&output.stderr).contains("consecutive dots"),
                "{args:?} {}",
                text(&output.stderr)
            );
        }
    }
    assert!(!managed(&repo, "a/../x").exists());

    let missing_delete = repo.rabi(&["remove", "nope"]);
    assert_eq!(missing_delete.status.code(), Some(3));
    let missing_json = repo.rabi(&["remove", "nope", "--json"]);
    assert_eq!(missing_json.status.code(), Some(3));
    let missing_value = json_value(&missing_json);
    assert_eq!(missing_value["error"]["code"], "not_found");
    assert!(missing_value["error"].get("exitCode").is_none());
    assert_eq!(repo.rabi(&["path", "nope"]).status.code(), Some(3));
    assert_eq!(
        repo.rabi(&["exec", "nope", "--", "true"]).status.code(),
        Some(3)
    );
}

#[test]
fn json_exec_signal_and_spawn_failure_return_the_worktree() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "child", "--json"]).status.success());
    let signal = repo.rabi(&["exec", "child", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(signal.status.code(), Some(143));
    let missing = repo.rabi(&["exec", "child", "--", "missing-rabi-child-command"]);
    assert!(!missing.status.success());
    assert!(managed(&repo, "child").exists());
}

#[test]
fn json_post_create_signal_and_spawn_return_the_worktree() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"kill -TERM $$\"]\n",
    )
    .expect("hooks");
    let created = repo.rabi(&["create", "h", "--json"]);
    assert_eq!(
        created.status.code(),
        Some(6),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let created_json = json_value(&created);
    assert!(created_json["error"]["code"] == "partial", "{created_json}");
    assert_eq!(created_json["data"]["workspace"]["name"], "h");
    assert_eq!(created_json["data"]["setup"]["ok"], false);
    assert_eq!(created_json["data"]["setup"]["command"], "kill -TERM $$");
    assert_eq!(created_json["data"]["setup"]["exitCode"], 143);
    assert_eq!(created_json["data"]["setup"]["signal"], "SIGTERM");
    assert!(created_json["data"]["setup"]["error"].is_string());
    assert!(managed(&repo, "h/README.md").is_file());

    repo.git(&["branch", "attach-hook"]);
    let attached = repo.rabi(&["create", "--branch", "attach-hook", "--json"]);
    assert_eq!(
        attached.status.code(),
        Some(6),
        "{}",
        text(&attached.stderr)
    );
    let attached_json = json_value(&attached);
    assert!(
        attached_json["error"]["code"] == "partial",
        "{attached_json}"
    );
    assert_eq!(attached_json["data"]["workspace"]["name"], "attach-hook");
    assert_eq!(attached_json["data"]["setup"]["ok"], false);
    assert_eq!(attached_json["data"]["setup"]["signal"], "SIGTERM");
    assert_eq!(attached_json["data"]["setup"]["exitCode"], 143);

    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"true\"]\n",
    )
    .expect("spawn hook");
    let spawned = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "hook-spawn", "--json"],
        )
        .env("SHELL", "/nonexistent")
        .output()
        .expect("hook spawn");
    assert_eq!(
        spawned.status.code(),
        Some(6),
        "{}\n{}",
        text(&spawned.stdout),
        text(&spawned.stderr)
    );
    let spawned_json = json_value(&spawned);
    assert!(spawned_json["error"]["code"] == "partial", "{spawned_json}");
    assert_eq!(spawned_json["data"]["workspace"]["name"], "hook-spawn");
    assert_eq!(spawned_json["data"]["setup"]["ok"], false);
    assert_eq!(spawned_json["data"]["setup"]["exitCode"], 1);
    assert!(spawned_json["data"]["setup"]["signal"].is_null());
    let spawn_error = spawned_json["data"]["setup"]["error"]
        .as_str()
        .unwrap_or("");
    assert!(
        spawn_error.contains("ENOENT") && spawn_error.contains("/nonexistent"),
        "{spawn_error}"
    );
    assert!(managed(&repo, "hook-spawn/README.md").is_file());

    repo.git(&["branch", "attach-hook-spawn"]);
    let attach_spawn = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--branch", "attach-hook-spawn", "--json"],
        )
        .env("SHELL", "/nonexistent")
        .output()
        .expect("attach hook spawn");
    assert_eq!(attach_spawn.status.code(), Some(6));
    let attach_json = json_value(&attach_spawn);
    assert_eq!(attach_json["data"]["setup"]["ok"], false);
    assert!(attach_json["data"]["setup"]["error"]
        .as_str()
        .unwrap_or("")
        .contains("ENOENT"));
    assert!(attach_json["error"]["code"] == "partial", "{attach_json}");
    assert!(managed(&repo, "attach-hook-spawn/README.md").is_file());

    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"kill -TERM $$\"]\n",
    )
    .expect("pre-remove signal");
    let deleted = repo.rabi(&[
        "remove",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "h",
        "--json",
    ]);
    assert_eq!(
        deleted.status.code(),
        Some(1),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    let deleted_json = json_value(&deleted);
    assert_eq!(deleted_json["error"]["code"], "signaled");
    assert!(deleted_json["error"].get("exitCode").is_none());
    assert_eq!(deleted_json["data"]["items"][0]["workspace"]["name"], "h");
    assert!(paths_match(
        deleted_json["data"]["items"][0]["workspace"]["path"]
            .as_str()
            .unwrap_or(""),
        &managed(&repo, "h")
    ));
    assert!(managed(&repo, "h").is_dir());

    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"true\"]\n",
    )
    .expect("pre-remove spawn");
    let pre_spawn = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "remove",
                "--discard-changes",
                "--delete-unpreserved-branch",
                "attach-hook",
                "--json",
            ],
        )
        .env("SHELL", "/nonexistent")
        .output()
        .expect("pre-remove spawn");
    assert_eq!(pre_spawn.status.code(), Some(1));
    let pre_json = json_value(&pre_spawn);
    assert_eq!(pre_json["error"]["code"], "spawn");
    assert!(pre_json["error"].get("exitCode").is_none());
    assert_eq!(
        pre_json["data"]["items"][0]["workspace"]["name"],
        "attach-hook"
    );
    assert!(paths_match(
        pre_json["data"]["items"][0]["workspace"]["path"]
            .as_str()
            .unwrap_or(""),
        &managed(&repo, "attach-hook")
    ));
    assert!(managed(&repo, "attach-hook").is_dir());
}

#[test]
fn detached_exec_resolves_the_created_path_with_a_configured_root() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["config", "set", "workspace-root", "custom"])
        .status
        .success());
    let output = repo.rabi(&["create", "detached", "--detach", "--json"]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let path = repo.repo.join("custom/detached");
    let ran = repo.rabi(&["exec", "detached", "--", "pwd"]);
    assert!(ran.status.success());
    assert!(paths_match(&text(&ran.stdout), &path));
}

#[test]
fn create_issue_refreshes_the_workspace_after_a_hook_detaches_head() {
    let repo = Repo::new();
    let set = repo.rabi(&["config", "set", "workspace-root", "../spaces"]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"test \\\"$(git branch --show-current)\\\" = issues/7 && git checkout -q --detach\"]\n",
    )
    .expect("detach hook");
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
set -eu
if [ "$1" = "repo" ]; then
  printf '%s\n' '{"url":"https://github.com/acme/demo","owner":{"login":"acme"},"name":"demo"}'
elif [ "$1" = "api" ] && [ "$2" = "repos/acme/demo/issues/7" ]; then
  printf '%s\n' '{"number":7,"title":"Issue with a detached checkout"}'
else
  echo "unexpected gh invocation: $*" >&2
  exit 1
fi
"#,
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH"));
    let checkout = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--issue", "7", "--json"],
        )
        .env("PATH", path)
        .output()
        .expect("create issue");
    assert!(
        checkout.status.success(),
        "{}\n{}",
        text(&checkout.stdout),
        text(&checkout.stderr)
    );
    let document = json_value(&checkout);
    assert!(document["error"].is_null(), "{document}");
    assert_eq!(document["data"]["source"]["kind"], "issue");
    assert_eq!(document["data"]["source"]["number"], 7);
    assert_eq!(document["data"]["setup"]["ran"], true);
    assert_eq!(document["data"]["setup"]["ok"], true);
    let directory = repo.root.join("spaces/issues/7");
    let workspace = &document["data"]["workspace"];
    assert_worktree_object(workspace, "issues/7", &directory);
    assert_eq!(workspace["detached"], true);
    assert!(workspace["branch"].is_null());
    assert_eq!(workspace["links"]["issue"]["number"], 7);
    assert!(directory.join("README.md").is_file());
    let head = repo
        .command("git", &["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(&directory)
        .output()
        .expect("detached HEAD");
    assert!(head.status.success(), "{}", text(&head.stderr));
    assert_eq!(text(&head.stdout), "HEAD");
    let ran = repo.rabi(&["exec", "issues/7", "--", "pwd"]);
    assert!(ran.status.success(), "{}", text(&ran.stderr));
    assert!(paths_match(&text(&ran.stdout), &directory));
}

#[test]
fn github_checkout_json_reports_issue_and_existing_pull_request() {
    let repo = Repo::new();
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = \"repo\" ]; then\n  printf '%s\\n' '{\"url\":\"https://github.com/acme/demo\",\"owner\":{\"login\":\"acme\"},\"name\":\"demo\"}'\n  exit 0\nfi\nprintf '%s\\n' '{\"number\":7}'\nexit 0\n",
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let checkout = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--issue", "7", "--json"],
        )
        .env("PATH", &path)
        .output()
        .expect("checkout");
    assert!(
        checkout.status.success(),
        "{}\n{}",
        text(&checkout.stdout),
        text(&checkout.stderr)
    );
    let checkout_json = json_value(&checkout);
    assert_eq!(checkout_json["schema"], 2);
    assert_eq!(checkout_json["data"]["source"]["kind"], "issue");
    assert_eq!(checkout_json["data"]["source"]["number"], 7);
    assert!(checkout_json["data"]["source"]["headRef"].is_null());
    assert_eq!(checkout_json["data"]["source"]["reused"], false);
    assert_worktree_object(
        &checkout_json["data"]["workspace"],
        "issues/7",
        &managed(&repo, "issues/7"),
    );
    assert_eq!(checkout_json["data"]["workspace"]["branch"], "issues/7");
    assert_eq!(
        checkout_json["data"]["workspace"]["base"],
        "refs/heads/main"
    );
    assert_eq!(
        checkout_json["data"]["workspace"]["links"]["issue"]["number"],
        7
    );
    assert_eq!(
        checkout_json["data"]["workspace"]["links"]["issue"]["url"],
        "https://github.com/acme/demo/issues/7"
    );
    assert!(checkout_json["data"]["workspace"]["links"]["pr"].is_null());
    assert!(checkout_json["data"]["workspace"]["createdAt"].is_string());
    assert!(!text(&checkout.stdout).contains("Created workspace"));

    let missing = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "--json", "--output=json"],
        )
        .env("PATH", &path)
        .output()
        .expect("missing");
    assert_eq!(missing.status.code(), Some(2));
    assert_eq!(json_value(&missing)["error"]["code"], "usage");

    let matching = Repo::new();
    let matching_path = fake_gh_for_topic_pr(&matching);
    let checked_out = managed(&matching, "topic");
    matching.git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        checked_out.to_str().expect("path"),
        "HEAD",
    ]);
    let again = matching
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &matching_path)
        .output()
        .expect("again");
    assert!(
        again.status.success(),
        "{}\n{}",
        text(&again.stdout),
        text(&again.stderr)
    );
    let again_json = json_value(&again);
    assert_eq!(again_json["data"]["source"]["kind"], "pr");
    assert_eq!(again_json["data"]["source"]["number"], 7);
    assert_eq!(again_json["data"]["source"]["headRef"], "topic");
    assert_eq!(again_json["data"]["source"]["reused"], true);
    assert!(again_json["data"]["workspace"]["base"].is_null());
    assert_eq!(again_json["data"]["setup"]["ran"], false);
    assert_worktree_object(&again_json["data"]["workspace"], "topic", &checked_out);
    assert_eq!(again_json["data"]["workspace"]["links"]["pr"]["number"], 7);
    assert_eq!(
        again_json["data"]["workspace"]["links"]["pr"]["url"],
        "https://github.com/acme/demo/pull/7"
    );
    assert!(again_json["data"]["workspace"]["links"]["issue"].is_null());
    assert!(!text(&again.stdout).contains("already checked out"));
}

#[test]
fn exec_passes_command_options_through() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature/exec-flags"]);
    assert!(created.status.success(), "{}", text(&created.stderr));

    let direct = repo.rabi(&[
        "exec",
        "feature/exec-flags",
        "--",
        "git",
        "branch",
        "--show-current",
    ]);
    assert!(
        direct.status.success(),
        "{}\n{}",
        text(&direct.stdout),
        text(&direct.stderr)
    );
    assert_eq!(text(&direct.stdout), "feature/exec-flags");

    let separated = repo.rabi(&[
        "exec",
        "feature/exec-flags",
        "--",
        "git",
        "branch",
        "--show-current",
    ]);
    assert!(separated.status.success(), "{}", text(&separated.stderr));
    assert_eq!(text(&separated.stdout), "feature/exec-flags");

    let nested = repo.rabi(&[
        "exec",
        "feature/exec-flags",
        "--",
        "sh",
        "-c",
        "printf '%s\\n' \"$@\"",
        "sh",
        "--show-current",
        "-n",
        "--",
        "--keep",
        "--help",
    ]);
    assert!(
        nested.status.success(),
        "{}\n{}",
        text(&nested.stdout),
        text(&nested.stderr)
    );
    assert_eq!(
        text(&nested.stdout),
        "--show-current\n-n\n--\n--keep\n--help"
    );
    assert!(!text(&nested.stdout).contains("rabi exec <workspace-name>"));

    let help = repo.rabi(&["exec", "--help"]);
    assert!(help.status.success(), "{}", text(&help.stderr));
    assert!(text(&help.stdout).contains("rabi exec [--repo <ref>] [SEL] -- COMMAND [ARG...]"));

    let early = repo.rabi(&["exec", "--show-current", "feature/exec-flags", "true"]);
    assert_eq!(early.status.code(), Some(2));
    assert!(text(&early.stderr).contains("Unknown option '--show-current'"));
}

#[test]
fn exec_help_skips_output_values_and_preserves_child_help() {
    let repo = Repo::new();
    for args in [
        vec!["--output", "text", "exec", "--help"],
        vec!["exec", "--output", "text", "--help"],
        vec!["--output=text", "exec", "--help"],
        vec!["exec", "--output=text", "--help"],
    ] {
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
        assert!(text(&out.stdout).contains("rabi exec [--repo <ref>] [SEL] -- COMMAND [ARG...]"));
        assert!(out.stderr.is_empty());
    }
    let value = repo.rabi(&["exec", "--output", "--help"]);
    assert_eq!(value.status.code(), Some(2));
    assert!(text(&value.stderr).contains("Invalid output mode"));

    for prefix in [
        vec!["exec"],
        vec!["--output", "text", "exec"],
        vec!["exec", "--output", "text"],
    ] {
        {
            let mut args = prefix.clone();
            args.push("main");
            args.push("--");
            args.extend(["printf", "%s", "--help"]);
            let out = repo.rabi(&args);
            assert!(out.status.success(), "{args:?}: {}", text(&out.stderr));
            assert_eq!(text(&out.stdout), "--help", "{args:?}");
            assert!(out.stderr.is_empty());
        }
    }
}

#[test]
fn create_and_attach_exec_pass_command_options_through() {
    let repo = Repo::new();
    repo.git(&["branch", "existing"]);
    for args in [
        vec!["create", "new"],
        vec!["create", "--branch", "existing"],
    ] {
        assert!(repo.rabi(&args).status.success());
        let name = args.last().unwrap();
        let output = repo.rabi(&["exec", name, "--", "printf", "%s", "--json --repo --help"]);
        assert!(output.status.success());
        assert_eq!(text(&output.stdout), "--json --repo --help");
    }
}

#[test]
fn exec_preserves_the_child_exit_code() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature/exec"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let output = repo.rabi(&["exec", "feature/exec", "--", "sh", "-c", "exit 4"]);
    assert_eq!(output.status.code(), Some(4));
    assert!(text(&output.stderr).contains("failed with exit code 4"));
}

#[test]
fn clone_registration_failure_preserves_completed_clone_and_fetch_results() {
    for json in [false, true] {
        let repo = Repo::new();
        let bare = repo.root.join("bare.git");
        repo.git(&[
            "clone",
            "--bare",
            repo.repo.to_str().expect("repo"),
            bare.to_str().expect("bare"),
        ]);
        let root = repo.root.join("clones");
        assert!(repo
            .rabi(&["config", "set", "clone-root", root.to_str().expect("root")])
            .status
            .success());
        append_instead_of(&repo, &bare, "https://github.com/example/widget.git");

        // A file at the records-directory path prevents registration writes,
        // including when tests run as a user that can bypass Unix permissions.
        let registry = repo.root.join("state/rabi/repos");
        fs::create_dir_all(registry.parent().expect("state dir")).expect("state dir");
        fs::write(&registry, "blocked registry\n").expect("block registry");
        let destination = root.join("github.com/example/widget");
        for (update, cloned, updated, status) in [
            (false, true, false, 6),
            (true, false, true, 6),
            (false, false, false, 1),
        ] {
            if update {
                repo.git(&["commit", "--allow-empty", "-m", "advance origin"]);
                repo.git(&["push", bare.to_str().expect("bare"), "main"]);
            }
            let mut args = vec!["clone", "example/widget"];
            if update {
                args.push("--update");
            }
            if json {
                args.push("--json");
            }
            let output = repo.rabi(&args);
            assert_eq!(
                output.status.code(),
                Some(status),
                "{} {}",
                text(&output.stdout),
                text(&output.stderr)
            );
            assert!(destination.join(".git").is_dir());
            assert_eq!(
                fs::read_to_string(&registry).expect("registry"),
                "blocked registry\n"
            );
            if json {
                let value = json_value(&output);
                assert_eq!(value["schema"], 2);
                assert_eq!(value["command"], "clone");
                assert_eq!(value["ok"], false);
                assert_eq!(
                    value["error"]["code"],
                    if status == 6 { "partial" } else { "failed" }
                );
                assert!(value["error"]["message"]
                    .as_str()
                    .expect("message")
                    .contains("register"));
                assert_eq!(
                    value["data"]["repo"]["root"],
                    canonical(&destination).to_string_lossy().as_ref()
                );
                assert_eq!(value["data"]["repo"]["name"], "github.com/example/widget");
                assert_eq!(
                    value["data"]["repo"]["remoteUrl"],
                    "https://github.com/example/widget.git"
                );
                assert_eq!(value["data"]["cloned"], cloned);
                assert_eq!(value["data"]["updated"], updated);
            } else {
                assert!(text(&output.stdout).contains(destination.to_str().expect("path")));
                assert!(text(&output.stderr).contains("register"));
            }
            if update {
                let fetched = repo
                    .command("git", &["rev-parse", "refs/remotes/origin/main"])
                    .current_dir(&destination)
                    .output()
                    .expect("fetched ref");
                let origin = repo
                    .command("git", &["rev-parse", "HEAD"])
                    .output()
                    .expect("origin ref");
                assert!(fetched.status.success());
                assert_eq!(text(&fetched.stdout), text(&origin.stdout));
            }
        }
        fs::remove_file(&registry).expect("unblock registry");
        let retried = repo.rabi(&["clone", "example/widget", "--json"]);
        assert!(retried.status.success(), "{}", text(&retried.stderr));
        let value = json_value(&retried);
        assert_eq!(value["data"]["cloned"], false);
        assert_eq!(value["data"]["updated"], false);
        assert_eq!(fs::read_dir(&registry).expect("registered").count(), 1);
    }
}

#[test]
fn clone_from_a_local_bare_repo_is_idempotent_and_json() {
    let repo = Repo::new();
    repo.git(&["branch", "feature"]);
    let bare = repo.root.join("bare.git");
    repo.git(&[
        "clone",
        "--bare",
        repo.repo.to_str().expect("repo"),
        bare.to_str().expect("bare"),
    ]);
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");

    let help = repo.rabi(&["clone", "--help"]);
    assert!(help.status.success(), "{}", text(&help.stderr));
    let help_text = text(&help.stdout);
    assert!(help_text.contains("rabi clone"));
    assert!(help_text.contains("--update"));
    assert!(help_text.contains("--branch <value>"));
    assert!(help_text.contains("~/ghq"));

    let missing = repo.rabi(&["clone", "--json"]);
    assert_eq!(missing.status.code(), Some(2));
    let missing_json: serde_json::Value =
        serde_json::from_str(&text(&missing.stdout)).expect("usage json");
    assert_eq!(missing_json["schema"], 2);
    assert_eq!(missing_json["error"]["code"], "usage");
    assert!(missing_json["error"].get("exitCode").is_none());

    let bad_depth = repo.rabi(&["clone", "example/widget", "--depth", "0", "--json"]);
    assert_eq!(
        bad_depth.status.code(),
        Some(2),
        "{}",
        text(&bad_depth.stdout)
    );
    let bad_depth_json: serde_json::Value =
        serde_json::from_str(&text(&bad_depth.stdout)).expect("depth json");
    assert_eq!(bad_depth_json["error"]["code"], "usage");

    let bad_url = repo.rabi(&["clone", "not a repo", "--json"]);
    assert_eq!(bad_url.status.code(), Some(2));
    let bad_url_json: serde_json::Value =
        serde_json::from_str(&text(&bad_url.stdout)).expect("url json");
    assert_eq!(bad_url_json["error"]["code"], "usage");
    assert!(bad_url_json["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("not a repo"));

    let cloned = repo.rabi(&[
        "clone",
        "example/widget",
        "--branch",
        "feature",
        "--depth",
        "1",
        "--json",
    ]);
    assert!(
        cloned.status.success(),
        "{}\n{}",
        text(&cloned.stdout),
        text(&cloned.stderr)
    );
    assert!(
        text(&cloned.stderr).contains("Cloning into"),
        "git progress should stay on stderr: {}",
        text(&cloned.stderr)
    );
    assert!(!text(&cloned.stdout).contains("Cloning into"));
    let value: serde_json::Value = serde_json::from_str(&text(&cloned.stdout)).expect("clone json");
    let destination = canonical(&root.join("github.com/example/widget"));
    assert_eq!(value["schema"], 2);
    assert_eq!(
        value["data"]["repo"]["root"],
        destination.to_string_lossy().as_ref()
    );
    assert_eq!(value["data"]["repo"]["name"], "github.com/example/widget");
    assert_eq!(
        value["data"]["repo"]["remoteUrl"],
        "https://github.com/example/widget.git"
    );
    assert_eq!(value["data"]["cloned"], true);
    assert_eq!(value["data"]["updated"], false);
    assert!(value.get("repo").is_none());
    let branch = repo
        .command("git", &["branch", "--show-current"])
        .current_dir(&destination)
        .output()
        .expect("branch");
    assert_eq!(text(&branch.stdout), "feature");
    let shallow = repo
        .command("git", &["rev-parse", "--is-shallow-repository"])
        .current_dir(&destination)
        .output()
        .expect("shallow");
    assert_eq!(text(&shallow.stdout), "true");

    fs::write(destination.join("local.txt"), "keep\n").expect("marker");
    let again = repo.rabi(&["clone", "git@github.com:example/widget.git"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    let again_text = text(&again.stdout);
    assert!(
        again_text.contains(&destination.to_string_lossy().to_string()),
        "{again_text}"
    );
    assert!(again_text.contains("Already cloned"), "{again_text}");
    assert_eq!(
        fs::read_to_string(destination.join("local.txt")).expect("marker"),
        "keep\n"
    );
    let listed = repo.rabi(&["repo", "list", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let catalog: serde_json::Value = serde_json::from_str(&text(&listed.stdout)).expect("catalog");
    let registered = catalog["data"]["repos"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project| project["rootPath"] == value["data"]["repo"]["root"])
        .expect("registered project");
    assert_eq!(registered["source"], "registry");
    assert_eq!(registered["name"], "widget");
    assert_eq!(registered["canonicalName"], "github.com/example/widget");

    let updated = repo.rabi(&[
        "clone",
        "https://github.com/example/widget.git",
        "--update",
        "--json",
    ]);
    assert!(
        updated.status.success(),
        "{}\n{}",
        text(&updated.stdout),
        text(&updated.stderr)
    );
    let updated_json: serde_json::Value =
        serde_json::from_str(&text(&updated.stdout)).expect("update json");
    assert_eq!(updated_json["data"]["cloned"], false);
    assert_eq!(updated_json["data"]["updated"], true);
    assert_eq!(
        updated_json["data"]["repo"]["root"],
        value["data"]["repo"]["root"]
    );
    assert_eq!(
        fs::read_to_string(destination.join("local.txt")).expect("marker"),
        "keep\n"
    );

    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "topic"])
        .current_dir(&destination)
        .output()
        .expect("worktree");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(managed_directory(&destination, &repo.root.join("xdg"))
        .join("topic")
        .is_dir());

    let mismatch_path = root.join("github.com/example/other");
    fs::create_dir_all(&mismatch_path).expect("mismatch dir");
    let init = repo
        .command("git", &["init", "-b", "main"])
        .current_dir(&mismatch_path)
        .output()
        .expect("init");
    assert!(init.status.success(), "{}", text(&init.stderr));
    let remote = repo
        .command(
            "git",
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/example/different.git",
            ],
        )
        .current_dir(&mismatch_path)
        .output()
        .expect("remote");
    assert!(remote.status.success(), "{}", text(&remote.stderr));
    let mismatch = repo.rabi(&["clone", "example/other", "--json"]);
    assert_eq!(
        mismatch.status.code(),
        Some(2),
        "{}\n{}",
        text(&mismatch.stdout),
        text(&mismatch.stderr)
    );
    let mismatch_json: serde_json::Value =
        serde_json::from_str(&text(&mismatch.stdout)).expect("mismatch json");
    assert_eq!(mismatch_json["schema"], 2);
    assert_eq!(mismatch_json["error"]["code"], "usage");
    assert!(mismatch_json["error"].get("exitCode").is_none());
    let message = mismatch_json["error"]["message"]
        .as_str()
        .unwrap_or_default();
    assert!(message.contains("does not match"), "{message}");
    assert!(message.contains("different.git"), "{message}");

    let empty = root.join("github.com/example/empty");
    fs::create_dir_all(&empty).expect("empty");
    let not_repo = repo.rabi(&["clone", "example/empty"]);
    assert_eq!(
        not_repo.status.code(),
        Some(2),
        "{}",
        text(&not_repo.stderr)
    );
    assert!(text(&not_repo.stderr).contains("not a Git repository"));

    let removed = repo.rabi(&["repo", "forget", "widget"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    let discovered = repo.rabi(&["repo", "list", "--json"]);
    assert!(discovered.status.success(), "{}", text(&discovered.stderr));
    let discovered_json: serde_json::Value =
        serde_json::from_str(&text(&discovered.stdout)).expect("discovered");
    let found = discovered_json["data"]["repos"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project| project["rootPath"] == value["data"]["repo"]["root"])
        .expect("scanned project");
    assert_eq!(found["source"], "ghq");
}

#[test]
fn clone_root_resolution_order() {
    let repo = Repo::new();
    let bare = repo.root.join("bare.git");
    repo.git(&[
        "clone",
        "--bare",
        repo.repo.to_str().expect("repo"),
        bare.to_str().expect("bare"),
    ]);
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");

    let config_root = repo.root.join("from-config");
    let git_root = repo.home.join("from-git");
    let second_root = repo.root.join("second-root");
    let env_root = repo.root.join("from-env");
    let default_root = repo.home.join("ghq");
    let invalid = repo.rabi(&["config", "set", "clone-root", "relative/path"]);
    assert_eq!(invalid.status.code(), Some(2), "{}", text(&invalid.stderr));
    assert!(text(&invalid.stderr).contains("absolute"));
    let set = repo.rabi(&[
        "config",
        "set",
        "clone-root",
        config_root.to_str().expect("config root"),
    ]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    repo.git(&["config", "--global", "--add", "ghq.root", "~/from-git"]);
    repo.git(&[
        "config",
        "--global",
        "--add",
        "ghq.root",
        second_root.to_str().expect("second"),
    ]);

    let first = clone_widget(&repo, Some(&env_root));
    assert_clone_landed(&first, &config_root);
    assert!(!git_root.join("github.com/example/widget").exists());
    assert!(!env_root.join("github.com/example/widget").exists());
    assert!(!second_root.join("github.com/example/widget").exists());

    let removed = repo.rabi(&["config", "unset", "clone-root"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    let from_git = clone_widget(&repo, Some(&env_root));
    assert_clone_landed(&from_git, &git_root);
    assert!(!second_root.join("github.com/example/widget").exists());
    assert!(!env_root.join("github.com/example/widget").exists());

    repo.git(&["config", "--global", "--unset-all", "ghq.root"]);
    let from_env = clone_widget(&repo, Some(&env_root));
    assert_clone_landed(&from_env, &env_root);

    let from_home = clone_widget(&repo, None);
    assert_clone_landed(&from_home, &default_root);
}

#[test]
fn clone_names_the_remote_origin_when_default_remote_name_is_upstream() {
    let repo = Repo::new();
    let bare = repo.root.join("bare.git");
    repo.git(&[
        "clone",
        "--bare",
        repo.repo.to_str().expect("repo"),
        bare.to_str().expect("bare"),
    ]);
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");
    repo.git(&["config", "--global", "clone.defaultRemoteName", "upstream"]);
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));

    let cloned = repo.rabi(&["clone", "example/widget", "--json"]);
    assert!(
        cloned.status.success(),
        "{}\n{}",
        text(&cloned.stdout),
        text(&cloned.stderr)
    );
    let destination = canonical(&root.join("github.com/example/widget"));
    let remotes = repo
        .command("git", &["remote"])
        .current_dir(&destination)
        .output()
        .expect("remote");
    assert!(remotes.status.success(), "{}", text(&remotes.stderr));
    assert_eq!(text(&remotes.stdout), "origin");

    let again = repo.rabi(&["clone", "example/widget", "--update", "--json"]);
    assert!(
        again.status.success(),
        "{}\n{}",
        text(&again.stdout),
        text(&again.stderr)
    );
    let again_json: serde_json::Value = serde_json::from_str(&text(&again.stdout)).expect("json");
    assert_eq!(again_json["data"]["cloned"], false);
    assert_eq!(again_json["data"]["updated"], true);
    assert_eq!(
        again_json["data"]["repo"]["root"],
        destination.to_string_lossy().as_ref()
    );
}

#[test]
fn clone_lowercases_remote_url_host() {
    let repo = Repo::new();
    let bare = repo.root.join("bare.git");
    repo.git(&[
        "clone",
        "--bare",
        repo.repo.to_str().expect("repo"),
        bare.to_str().expect("bare"),
    ]);
    append_instead_of(&repo, &bare, "https://GitHub.com/example/widget.git");
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));

    let cloned = repo.rabi(&["clone", "https://GitHub.com/example/widget.git", "--json"]);
    assert!(
        cloned.status.success(),
        "{}\n{}",
        text(&cloned.stdout),
        text(&cloned.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&text(&cloned.stdout)).expect("json");
    assert_eq!(
        value["data"]["repo"]["remoteUrl"],
        "https://github.com/example/widget.git"
    );
    assert_eq!(
        value["data"]["repo"]["root"],
        canonical(&root.join("github.com/example/widget"))
            .to_string_lossy()
            .as_ref()
    );
}

#[test]
fn clone_overlong_owner_is_validation() {
    let repo = Repo::new();
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let owner = "o".repeat(256);
    let source = format!("example.com/{owner}/widget");
    let output = repo.rabi(&["clone", &source, "--json"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&text(&output.stdout)).expect("json");
    assert_eq!(value["schema"], 2);
    assert_eq!(value["error"]["code"], "usage");
    assert!(value["error"].get("exitCode").is_none());
    let message = value["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("Failed to create"), "{message}");
    assert!(text(&output.stderr).is_empty(), "{}", text(&output.stderr));
    assert!(!root.join("example.com").exists());
}

#[test]
fn clone_overlong_name_with_existing_parent_is_validation() {
    let repo = Repo::new();
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));

    let host = root.join("github.com");
    fs::create_dir_all(&host).expect("host");
    let owner = "w".repeat(300);
    assert_clone_validation(&repo, &format!("github.com/{owner}/widget"));
    assert!(host.is_dir());
    assert!(!host.join(&owner).exists());

    let owner_dir = root.join("example.test").join("acme");
    fs::create_dir_all(&owner_dir).expect("owner");
    let repo_name = "w".repeat(300);
    assert_clone_validation(&repo, &format!("example.test/acme/{repo_name}"));
    assert!(owner_dir.is_dir());
    assert!(!owner_dir.join(&repo_name).exists());
}

fn assert_clone_validation(repo: &Repo, source: &str) {
    let output = repo.rabi(&["clone", source, "--json"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{source}\n{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&text(&output.stdout)).expect("json");
    assert_eq!(value["schema"], 2);
    assert_eq!(value["error"]["code"], "usage");
    assert!(value["error"].get("exitCode").is_none());
    let message = value["error"]["message"].as_str().unwrap_or_default();
    // The OS error text comes from the C library: glibc and macOS say
    // "File name too long", musl says "Filename too long".
    assert!(
        message.contains("File name too long") || message.contains("Filename too long"),
        "{source}: {message}"
    );
    assert!(text(&output.stderr).is_empty(), "{}", text(&output.stderr));
}

#[test]
fn clone_surfaces_unreadable_git_config_for_an_existing_checkout() {
    let repo = Repo::new();
    let bare = repo.root.join("bare.git");
    repo.git(&[
        "clone",
        "--bare",
        repo.repo.to_str().expect("repo"),
        bare.to_str().expect("bare"),
    ]);
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let cloned = repo.rabi(&["clone", "example/widget", "--json"]);
    assert!(
        cloned.status.success(),
        "{}\n{}",
        text(&cloned.stdout),
        text(&cloned.stderr)
    );

    let broken = repo.root.join("broken.gitconfig");
    fs::write(&broken, "[[[[ not valid\n").expect("config");
    let again = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["clone", "example/widget", "--json"],
        )
        .env("GIT_CONFIG_GLOBAL", &broken)
        .output()
        .expect("clone");
    assert_eq!(
        again.status.code(),
        Some(1),
        "{}\n{}",
        text(&again.stdout),
        text(&again.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&text(&again.stdout)).expect("json");
    assert_eq!(value["schema"], 2);
    assert_eq!(value["error"]["code"], "failed");
    assert!(value["error"].get("exitCode").is_none());
    let message = value["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("bad config line"),
        "git's error should be in the message: {message}"
    );
    assert!(
        !message.contains("not a Git repository"),
        "config failure was reported as a missing repository: {message}"
    );
    assert!(text(&again.stderr).is_empty(), "{}", text(&again.stderr));
}

#[test]
fn clone_removes_stale_staging_directories() {
    let repo = Repo::new();
    let bare = repo.root.join("bare.git");
    repo.git(&[
        "clone",
        "--bare",
        repo.repo.to_str().expect("repo"),
        bare.to_str().expect("bare"),
    ]);
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");
    let root = repo.root.join("clones");
    let set = repo.rabi(&["config", "set", "clone-root", root.to_str().expect("root")]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let owner = root.join("github.com").join("example");
    fs::create_dir_all(&owner).expect("owner");
    let dead_pid = unused_pid();
    let dead = owner.join(format!(".rabi-clone-{dead_pid}-{}", uuid_token()));
    fs::create_dir_all(dead.join(".git")).expect("dead");
    fs::write(dead.join("payload.txt"), "stale").expect("payload");
    let live = owner.join(format!(
        ".rabi-clone-{}-{}",
        std::process::id(),
        uuid_token()
    ));
    fs::create_dir_all(&live).expect("live");
    fs::write(live.join("payload.txt"), "live").expect("live");

    let cloned = repo.rabi(&["clone", "example/widget", "--json"]);
    assert!(
        cloned.status.success(),
        "{}\n{}",
        text(&cloned.stdout),
        text(&cloned.stderr)
    );
    assert!(!dead.exists(), "stale staging directory remained");
    assert_eq!(
        fs::read_to_string(live.join("payload.txt")).expect("live"),
        "live"
    );
    assert!(owner.join("widget").join(".git").exists());
}

#[test]
fn clone_rejects_file_urls() {
    let repo = Repo::new();
    let output = repo.rabi(&["clone", "file:///tmp/repo.git", "--json"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output.stdout));
    let value: serde_json::Value = serde_json::from_str(&text(&output.stdout)).expect("json");
    assert_eq!(value["error"]["code"], "usage");
    assert!(value["error"].get("exitCode").is_none());
    let message = value["error"]["message"].as_str().unwrap_or_default();
    assert!(message.contains("file"), "{message}");
    assert!(message.contains("https"), "{message}");
}

fn unused_pid() -> u32 {
    for pid in 1_000_000..1_001_000 {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(true);
        if !alive {
            return pid;
        }
    }
    panic!("no unused pid");
}

fn uuid_token() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!(
        "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
        (nanos & 0xffff_ffff) as u32,
        ((nanos >> 32) & 0xffff) as u32,
        ((nanos >> 48) & 0xfff) as u32,
        std::process::id() & 0xfff,
        nanos & 0xffff_ffff_ffff
    )
}

fn append_instead_of(repo: &Repo, bare: &std::path::Path, url: &str) {
    let path = repo.home.join(".gitconfig");
    let mut body = fs::read_to_string(&path).unwrap_or_default();
    // file:// makes `--depth` apply. A plain path is a local clone and Git ignores depth.
    body.push_str(&format!(
        "\n[url \"file://{}\"]\n\tinsteadOf = {url}\n",
        bare.display()
    ));
    fs::write(path, body).expect("gitconfig");
}

fn clone_widget(repo: &Repo, ghq_root: Option<&std::path::Path>) -> std::process::Output {
    let mut command = repo.command(
        env!("CARGO_BIN_EXE_rabi"),
        &["clone", "example/widget", "--json"],
    );
    if let Some(root) = ghq_root {
        command.env("GHQ_ROOT", root);
    }
    command.output().expect("clone")
}

fn assert_clone_landed(output: &std::process::Output, root: &std::path::Path) {
    assert!(
        output.status.success(),
        "{}\n{}",
        text(&output.stdout),
        text(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_str(&text(&output.stdout)).expect("json");
    let expected = canonical(&root.join("github.com/example/widget"));
    assert_eq!(
        value["data"]["repo"]["root"],
        expected.to_string_lossy().as_ref()
    );
    assert_eq!(value["data"]["cloned"], true);
    assert_eq!(value["data"]["repo"]["name"], "github.com/example/widget");
}

fn canonical(path: &std::path::Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn same_dir(left: &str, right: &std::path::Path) -> bool {
    fs::canonicalize(left).ok() == fs::canonicalize(right).ok()
}

fn silent_ghq_path(repo: &Repo) -> String {
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let ghq = bin.join("ghq");
    fs::write(&ghq, "#!/bin/sh\nexit 0\n").expect("ghq");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&ghq).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&ghq, perms).expect("chmod");
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

fn git_at(repo: &Repo, dir: &std::path::Path, args: &[&str]) {
    let output = repo
        .command("git", args)
        .current_dir(dir)
        .output()
        .expect("git");
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        text(&output.stderr)
    );
}

fn init_named_repo(repo: &Repo, relative: &str, origin: &str) -> PathBuf {
    let path = repo.root.join("clones").join(relative);
    fs::create_dir_all(&path).expect("project dir");
    git_at(repo, &path, &["init", "-b", "main"]);
    git_at(
        repo,
        &path,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(repo, &path, &["config", "user.name", "Phantom Rust"]);
    fs::write(path.join("README.md"), "hi\n").expect("readme");
    git_at(repo, &path, &["add", "README.md"]);
    git_at(repo, &path, &["commit", "-m", "initial"]);
    git_at(repo, &path, &["remote", "add", "origin", origin]);
    path
}

#[test]
fn project_option_resolves_paths_suffixes_and_errors() {
    let repo = Repo::new();
    let path_env = silent_ghq_path(&repo);
    let clones = repo.root.join("clones");
    let set = repo.rabi(&[
        "config",
        "set",
        "clone-root",
        clones.to_str().expect("root"),
    ]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let only = init_named_repo(
        &repo,
        "github.com/acme/ptm805only",
        "https://github.com/acme/ptm805only.git",
    );
    let acme_widget = init_named_repo(
        &repo,
        "github.com/acme/ptm805widget",
        "https://github.com/acme/ptm805widget.git",
    );
    let other_widget = init_named_repo(
        &repo,
        "github.com/other/ptm805widget",
        "https://github.com/other/ptm805widget.git",
    );
    let placed = init_named_repo(
        &repo,
        "github.com/acme/placed",
        "https://gitlab.com/other/renamed.git",
    );
    let run = |args: &[&str], cwd: &std::path::Path| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("PATH", &path_env)
            .current_dir(cwd)
            .output()
            .expect("rabi")
    };

    let catalog = run(&["repo", "list", "--json"], &repo.home);
    assert!(catalog.status.success(), "{}", text(&catalog.stderr));
    let catalog_json = json_value(&catalog);
    let only_entry = catalog_json["data"]["repos"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project| project["canonicalName"] == "github.com/acme/ptm805only")
        .expect("only project");
    assert_eq!(only_entry["name"], "ptm805only");
    assert_eq!(only_entry["canonicalName"], "github.com/acme/ptm805only");
    let placed_entry = catalog_json["data"]["repos"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project| project["canonicalName"] == "gitlab.com/other/renamed")
        .expect("placed project");
    assert_eq!(placed_entry["name"], "placed");

    for reference in [
        "ptm805only",
        "acme/ptm805only",
        "github.com/acme/ptm805only",
    ] {
        let located = run(&["path", "--repo", reference, "main"], &repo.home);
        assert!(
            located.status.success(),
            "{reference}: {}\n{}",
            text(&located.stdout),
            text(&located.stderr)
        );
        assert!(
            same_dir(&text(&located.stdout), &only),
            "{reference} -> {}",
            text(&located.stdout)
        );
    }
    let by_remote = run(&["path", "--repo", "other/renamed", "main"], &repo.home);
    assert!(by_remote.status.success(), "{}", text(&by_remote.stderr));
    assert!(same_dir(&text(&by_remote.stdout), &placed));
    let by_location = run(&["path", "--repo", "acme/placed", "main"], &repo.home);
    assert!(
        by_location.status.success(),
        "{}",
        text(&by_location.stderr)
    );
    assert!(same_dir(&text(&by_location.stdout), &placed));

    let ambiguous = run(&["list", "--repo", "ptm805widget", "--json"], &repo.home);
    assert_eq!(
        ambiguous.status.code(),
        Some(2),
        "{}",
        text(&ambiguous.stdout)
    );
    let ambiguous_json = json_value(&ambiguous);
    assert_eq!(ambiguous_json["schema"], 2);
    assert_eq!(ambiguous_json["error"]["code"], "ambiguous");
    assert!(ambiguous_json["error"].get("exitCode").is_none());
    let message = ambiguous_json["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains("github.com/acme/ptm805widget"),
        "{message}"
    );
    assert!(
        message.contains("github.com/other/ptm805widget"),
        "{message}"
    );
    assert!(
        message.contains(&acme_widget.to_string_lossy().to_string())
            || message.contains(&canonical(&acme_widget).to_string_lossy().to_string()),
        "{message}"
    );
    assert!(
        message.contains(&other_widget.to_string_lossy().to_string())
            || message.contains(&canonical(&other_widget).to_string_lossy().to_string()),
        "{message}"
    );
    let unique = run(&["path", "--repo", "acme/ptm805widget", "main"], &repo.home);
    assert!(unique.status.success(), "{}", text(&unique.stderr));
    assert!(same_dir(&text(&unique.stdout), &acme_widget));

    let missing = run(&["list", "--repo", "no-such-ptm805", "--json"], &repo.home);
    assert_eq!(missing.status.code(), Some(3), "{}", text(&missing.stdout));
    let missing_json = json_value(&missing);
    assert_eq!(missing_json["error"]["code"], "not_found");
    assert!(missing_json["error"].get("exitCode").is_none());
    assert_eq!(
        missing_json["error"]["message"],
        "Repository 'no-such-ptm805' not found"
    );
    let prefix = run(
        &["show", "--repo", "github.com/acme", "main", "--json"],
        &repo.home,
    );
    assert_eq!(prefix.status.code(), Some(3), "{}", text(&prefix.stdout));
    assert_eq!(json_value(&prefix)["error"]["code"], "not_found");

    let created = repo.rabi(&["create", "feature"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let worktree = managed(&repo, "feature");
    let nested = worktree.join("nested");
    fs::create_dir_all(&nested).expect("nested");
    let sub = repo.repo.join("sub");
    fs::create_dir_all(&sub).expect("sub");

    let absolute = run(
        &["path", "-R", repo.repo.to_str().expect("repo"), "main"],
        &repo.home,
    );
    assert!(absolute.status.success(), "{}", text(&absolute.stderr));
    assert!(same_dir(&text(&absolute.stdout), &repo.repo));
    let relative = run(&["path", "--repo", "./repo", "main"], &repo.root);
    assert!(relative.status.success(), "{}", text(&relative.stderr));
    assert!(same_dir(&text(&relative.stdout), &repo.repo));
    let dotted = run(&["path", "--repo", ".", "main"], &sub);
    assert!(dotted.status.success(), "{}", text(&dotted.stderr));
    assert!(same_dir(&text(&dotted.stdout), &repo.repo));
    let tilde = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["path", "--repo", "~/repo", "main"],
        )
        .env("PATH", &path_env)
        .env("HOME", &repo.root)
        .current_dir(&repo.home)
        .output()
        .expect("tilde");
    assert!(tilde.status.success(), "{}", text(&tilde.stderr));
    assert!(same_dir(&text(&tilde.stdout), &repo.repo));

    let inside = run(
        &["list", "--json", "--repo", nested.to_str().expect("nested")],
        &repo.home,
    );
    assert!(
        inside.status.success(),
        "{}\n{}",
        text(&inside.stdout),
        text(&inside.stderr)
    );
    let inside_json = json_value(&inside);
    let worktrees = inside_json["data"]["workspaces"]
        .as_array()
        .expect("worktrees");
    let default_tree = worktrees
        .iter()
        .find(|worktree| worktree["kind"].as_str() == Some("primary"))
        .expect("primary checkout");
    assert!(same_dir(
        default_tree["path"].as_str().unwrap_or(""),
        &repo.repo
    ));
    let feature_tree = worktrees
        .iter()
        .find(|worktree| worktree["name"] == "feature")
        .expect("feature");
    assert_eq!(feature_tree["kind"], "workspace");
    let from_link = run(&["path", "--repo", ".", "main"], &worktree);
    assert!(from_link.status.success(), "{}", text(&from_link.stderr));
    assert!(same_dir(&text(&from_link.stdout), &repo.repo));
    let created_from_link = run(
        &[
            "create",
            "--repo",
            nested.to_str().expect("nested"),
            "from-link",
        ],
        &repo.home,
    );
    assert!(
        created_from_link.status.success(),
        "{}\n{}",
        text(&created_from_link.stdout),
        text(&created_from_link.stderr)
    );
    assert!(managed(&repo, "from-link").is_dir());
    assert!(!worktree.join("custom/workspaces/from-link").exists());

    let file = run(
        &[
            "list",
            "--repo",
            repo.repo.join("README.md").to_str().expect("readme"),
            "--json",
        ],
        &repo.home,
    );
    assert_eq!(file.status.code(), Some(2), "{}", text(&file.stdout));
    let file_json = json_value(&file);
    assert_eq!(file_json["error"]["code"], "usage");
    assert!(file_json["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("not a directory"));
    let outside = run(
        &[
            "list",
            "--repo",
            repo.home.to_str().expect("home"),
            "--json",
        ],
        &repo.home,
    );
    assert_eq!(outside.status.code(), Some(3), "{}", text(&outside.stdout));
    assert_eq!(json_value(&outside)["error"]["code"], "not_repository");
    let absent = run(
        &["show", "--repo", "/no/such/ptm805-path", "main", "--json"],
        &repo.home,
    );
    assert_eq!(absent.status.code(), Some(3), "{}", text(&absent.stdout));
    let absent_json = json_value(&absent);
    assert_eq!(absent_json["error"]["code"], "not_found");
    assert!(absent_json["error"].get("exitCode").is_none());
    assert_eq!(
        absent_json["error"]["message"],
        "Path '/no/such/ptm805-path' does not exist"
    );

    let repeated = run(
        &[
            "list",
            "--repo",
            "acme/ptm805only",
            "--repo",
            "acme/ptm805only",
            "--json",
        ],
        &repo.home,
    );
    assert_eq!(
        repeated.status.code(),
        Some(2),
        "{}",
        text(&repeated.stdout)
    );
    assert!(json_value(&repeated)["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("only be specified once"));

    let checkout_bin = repo.root.join("bin/gh");
    fs::write(
        &checkout_bin,
        "#!/bin/sh\nif [ \"$1\" = \"repo\" ]; then\n  printf '%s\\n' '{\"url\":\"https://github.com/acme/demo\",\"owner\":{\"login\":\"acme\"},\"name\":\"demo\"}'\n  exit 0\nfi\nprintf '%s\\n' '{\"number\":9}'\nexit 0\n",
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&checkout_bin).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&checkout_bin, perms).expect("chmod");
    let checkout = run(
        &[
            "create",
            "--repo",
            repo.repo.to_str().expect("repo"),
            "--issue",
            "9",
            "--json",
        ],
        &repo.home,
    );
    assert!(
        checkout.status.success(),
        "{}\n{}",
        text(&checkout.stdout),
        text(&checkout.stderr)
    );
    assert!(managed(&repo, "issues/9").is_dir());
    let missing_checkout = run(
        &[
            "create",
            "--repo",
            "no-such-ptm805",
            "--issue",
            "9",
            "--json",
        ],
        &repo.home,
    );
    assert_eq!(missing_checkout.status.code(), Some(3));
    assert_eq!(json_value(&missing_checkout)["error"]["code"], "not_found");
}

#[test]
fn exec_project_option_passes_later_options_to_the_command() {
    let repo = Repo::new();
    let path_env = silent_ghq_path(&repo);
    let clones = repo.root.join("clones");
    let set = repo.rabi(&[
        "config",
        "set",
        "clone-root",
        clones.to_str().expect("root"),
    ]);
    assert!(set.status.success(), "{}", text(&set.stderr));
    let project = init_named_repo(
        &repo,
        "github.com/acme/ptm805exec",
        "https://github.com/acme/ptm805exec.git",
    );
    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("PATH", &path_env)
            .current_dir(&repo.home)
            .output()
            .expect("rabi")
    };
    let created = run(&["create", "--repo", "acme/ptm805exec", "foo"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(managed_directory(&project, &repo.root.join("xdg"))
        .join("foo")
        .is_dir());

    let passed = run(&[
        "exec",
        "--repo",
        "acme/ptm805exec",
        "foo",
        "--",
        "pwd",
        "--repo",
        "y",
    ]);
    assert_eq!(passed.status.code(), Some(1), "{}", text(&passed.stderr));
    let stderr = text(&passed.stderr);
    assert!(
        stderr.contains("unrecognized option '--repo'"),
        "pwd should receive --repo y: {stderr}"
    );
    assert!(!stderr.contains("Unknown option"), "{stderr}");
    assert!(!stderr.contains("matches multiple"), "{stderr}");

    let printed = run(&[
        "exec",
        "--repo",
        "acme/ptm805exec",
        "foo",
        "--",
        "printf",
        "%s\n",
        "--repo",
        "y",
    ]);
    assert!(
        printed.status.success(),
        "{}\n{}",
        text(&printed.stdout),
        text(&printed.stderr)
    );
    assert_eq!(text(&printed.stdout), "--repo\ny");

    let missing = run(&[
        "exec",
        "--repo",
        "no-such-ptm805",
        "foo",
        "--",
        "pwd",
        "--repo",
        "y",
    ]);
    assert_eq!(missing.status.code(), Some(3), "{}", text(&missing.stderr));
    assert!(text(&missing.stderr).contains("Repository 'no-such-ptm805' not found"));
    assert!(!text(&missing.stderr).contains("unrecognized option"));
}

#[test]
fn project_option_ignores_the_callers_config_and_matches_directory_names() {
    let repo = Repo::new();
    let path_env = silent_ghq_path(&repo);
    let tool = repo.root.join("tool");
    fs::create_dir_all(&tool).expect("tool");
    git_at(&repo, &tool, &["init", "-b", "main"]);
    git_at(
        &repo,
        &tool,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(&repo, &tool, &["config", "user.name", "Phantom Rust"]);
    fs::write(tool.join("README.md"), "tool\n").expect("readme");
    git_at(&repo, &tool, &["add", "README.md"]);
    git_at(&repo, &tool, &["commit", "-m", "initial"]);
    let feature = tool.join("custom/workspaces/h1");
    git_at(
        &repo,
        &tool,
        &[
            "worktree",
            "add",
            "-b",
            "h1",
            feature.to_str().expect("h1"),
            "HEAD",
        ],
    );
    let localname = repo.root.join("localname");
    fs::create_dir_all(&localname).expect("localname");
    git_at(&repo, &localname, &["init", "-b", "main"]);
    git_at(
        &repo,
        &localname,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(&repo, &localname, &["config", "user.name", "Phantom Rust"]);
    fs::write(localname.join("README.md"), "local\n").expect("readme");
    git_at(&repo, &localname, &["add", "README.md"]);
    git_at(&repo, &localname, &["commit", "-m", "initial"]);
    git_at(
        &repo,
        &localname,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/ptm805renamed.git",
        ],
    );
    for path in [&tool, &localname] {
        let added = repo.rabi(&["repo", "add", path.to_str().expect("path"), "--json"]);
        assert!(added.status.success(), "{}", text(&added.stderr));
    }
    fs::write(repo.repo.join(".rabi.toml"), "not = [\n").expect("invalid config");

    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("PATH", &path_env)
            .output()
            .expect("rabi")
    };
    let listed = run(&["list", "-R", "tool"]);
    assert!(
        listed.status.success(),
        "{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    assert!(!text(&listed.stderr).contains("Invalid .rabi.toml"));
    assert!(
        text(&listed.stdout).contains("h1"),
        "{}",
        text(&listed.stdout)
    );
    let located = run(&["path", "-R", "tool", "h1"]);
    assert!(located.status.success(), "{}", text(&located.stderr));
    assert!(same_dir(&text(&located.stdout), &feature));
    let created = run(&["create", "-R", "tool", "z1"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(managed_directory(&tool, &repo.root.join("xdg"))
        .join("z1")
        .is_dir());
    assert!(!text(&created.stderr).contains("Invalid .rabi.toml"));
    assert!(!managed(&repo, "z1").exists());

    let by_directory = run(&["path", "-R", "localname", "main"]);
    assert!(
        by_directory.status.success(),
        "{}",
        text(&by_directory.stderr)
    );
    assert!(same_dir(&text(&by_directory.stdout), &localname));
    let by_suffix = run(&["path", "--repo", "ptm805renamed", "main"]);
    assert!(by_suffix.status.success(), "{}", text(&by_suffix.stderr));
    assert!(same_dir(&text(&by_suffix.stdout), &localname));

    let option_value = run(&["list", "-R", "--json"]);
    assert_eq!(option_value.status.code(), Some(2));
    assert!(option_value.stdout.is_empty());
    assert!(text(&option_value.stderr).contains("looks like an option"));
    let option_json = run(&["list", "--json", "-R", "--json"]);
    assert_eq!(
        option_json.status.code(),
        Some(2),
        "{}",
        text(&option_json.stdout)
    );
    let option_body = json_value(&option_json);
    assert_eq!(option_body["error"]["code"], "usage");
    assert!(option_body["error"].get("exitCode").is_none());
    assert!(option_body["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("looks like an option"));
    let option_later = run(&["show", "--json", "-R", "--bogus", "main"]);
    assert_eq!(option_later.status.code(), Some(2));
    assert_eq!(json_value(&option_later)["error"]["code"], "usage");
    let option_human = run(&["path", "-R", "--nope", "main"]);
    assert_eq!(
        option_human.status.code(),
        Some(2),
        "{}",
        text(&option_human.stderr)
    );
    assert!(text(&option_human.stderr).contains("looks like an option"));
    assert!(!text(&option_human.stdout).contains("\"version\""));

    let help = run(&["exec", "--repo", "tool", "--help"]);
    assert!(help.status.success(), "{}", text(&help.stderr));
    assert!(text(&help.stdout).contains("rabi exec"));
    assert!(!text(&help.stderr).contains("Unknown option"));
}

#[test]
fn github_checkout_project_passes_the_selected_host() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".rabi.toml"), "not = [\n").expect("invalid config");
    let tool = repo.root.join("ghe-tool");
    fs::create_dir_all(&tool).expect("tool");
    git_at(&repo, &tool, &["init", "-b", "main"]);
    git_at(
        &repo,
        &tool,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(&repo, &tool, &["config", "user.name", "Phantom Rust"]);
    fs::write(tool.join("README.md"), "ghe\n").expect("readme");
    git_at(&repo, &tool, &["add", "README.md"]);
    git_at(&repo, &tool, &["commit", "-m", "initial"]);
    git_at(
        &repo,
        &tool,
        &[
            "remote",
            "add",
            "origin",
            "ssh://git@ghe.corp.example:2222/acme/tool.git",
        ],
    );
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let log = repo.root.join("gh.log");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"pwd=$(pwd)\" >> \"$RABI_GH_LOG\"\nprintf '%s\\n' \"$*\" >> \"$RABI_GH_LOG\"\nif [ \"$1\" = \"repo\" ]; then\n  printf '%s\\n' \"$RABI_GH_REPO_JSON\"\n  exit 0\nfi\nprintf '%s\\n' '{\"number\":3}'\nexit 0\n",
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    let path_env = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let checkout = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "create", "-R",
                tool.to_str().expect("tool"),
                "--issue", "3",
                "--json",
            ],
        )
        .env("PATH", &path_env)
        .env("RABI_GH_LOG", &log)
        .env(
            "RABI_GH_REPO_JSON",
            r#"{"url":"https://ghe.corp.example/acme/tool","owner":{"login":"acme"},"name":"tool"}"#,
        )
        .output()
        .expect("checkout");
    assert!(
        checkout.status.success(),
        "{}\n{}",
        text(&checkout.stdout),
        text(&checkout.stderr)
    );
    assert!(!text(&checkout.stderr).contains("Invalid .rabi.toml"));
    assert!(managed_directory(&tool, &repo.root.join("xdg"))
        .join("issues/3")
        .is_dir());
    let recorded = fs::read_to_string(&log).expect("gh log");
    assert!(
        recorded.contains("--hostname ghe.corp.example"),
        "{recorded}"
    );
    assert!(!recorded.contains(":2222"), "{recorded}");
    assert!(recorded.contains("repos/acme/tool/issues/3"), "{recorded}");
    let tool_pwd = canonical(&tool).to_string_lossy().to_string();
    assert!(recorded.contains(&format!("pwd={tool_pwd}")), "{recorded}");
    assert!(!recorded.contains(&format!("pwd={}", repo.repo.display())));

    fs::write(&log, "").expect("truncate");
    let local = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "--issue", "4"])
        .current_dir(&tool)
        .env("PATH", &path_env)
        .env("RABI_GH_LOG", &log)
        .env(
            "RABI_GH_REPO_JSON",
            r#"{"url":"https://ghe.corp.example/acme/tool","owner":{"login":"acme"},"name":"tool"}"#,
        )
        .output()
        .expect("local checkout");
    assert!(
        local.status.success(),
        "{}\n{}",
        text(&local.stdout),
        text(&local.stderr)
    );
    let local_log = fs::read_to_string(&log).expect("local log");
    assert!(
        local_log.contains("--hostname ghe.corp.example"),
        "{local_log}"
    );
    assert!(!local_log.contains(":2222"), "{local_log}");

    let alias = repo.root.join("alias-tool");
    fs::create_dir_all(&alias).expect("alias");
    git_at(&repo, &alias, &["init", "-b", "main"]);
    git_at(
        &repo,
        &alias,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(&repo, &alias, &["config", "user.name", "Phantom Rust"]);
    fs::write(alias.join("README.md"), "alias\n").expect("readme");
    git_at(&repo, &alias, &["add", "README.md"]);
    git_at(&repo, &alias, &["commit", "-m", "initial"]);
    git_at(&repo, &alias, &["remote", "add", "origin", "gh:acme/tool"]);
    fs::write(&log, "").expect("truncate");
    let aliased = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "create",
                "-R",
                alias.to_str().expect("alias"),
                "--issue",
                "5",
            ],
        )
        .env("PATH", &path_env)
        .env("RABI_GH_LOG", &log)
        .env(
            "RABI_GH_REPO_JSON",
            r#"{"url":"https://github.com/acme/tool","owner":{"login":"acme"},"name":"tool"}"#,
        )
        .output()
        .expect("alias checkout");
    assert!(
        aliased.status.success(),
        "{}\n{}",
        text(&aliased.stdout),
        text(&aliased.stderr)
    );
    let alias_log = fs::read_to_string(&log).expect("alias log");
    assert!(alias_log.contains("--hostname github.com"), "{alias_log}");
    assert!(
        !alias_log.contains("--hostname gh\n") && !alias_log.contains("--hostname gh "),
        "{alias_log}"
    );
}

#[test]
fn relative_origin_is_not_a_github_project_name() {
    let repo = Repo::new();
    let path_env = silent_ghq_path(&repo);
    let project = repo.root.join("ptm805local");
    fs::create_dir_all(&project).expect("repo");
    git_at(&repo, &project, &["init", "-b", "main"]);
    git_at(
        &repo,
        &project,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    git_at(&repo, &project, &["config", "user.name", "Phantom Rust"]);
    fs::write(project.join("README.md"), "local\n").expect("readme");
    git_at(&repo, &project, &["add", "README.md"]);
    git_at(&repo, &project, &["commit", "-m", "initial"]);
    git_at(
        &repo,
        &project,
        &["remote", "add", "origin", "backups/widget.git"],
    );
    let added = repo.rabi(&["repo", "add", project.to_str().expect("path"), "--json"]);
    assert!(added.status.success(), "{}", text(&added.stderr));
    let listed = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "list", "--json"])
        .env("PATH", &path_env)
        .output()
        .expect("list");
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let catalog = json_value(&listed);
    let entry = catalog["data"]["repos"]
        .as_array()
        .expect("projects")
        .iter()
        .find(|project| project["name"] == "ptm805local")
        .expect("repo");
    assert_eq!(entry["canonicalName"], "ptm805local");
    assert!(!entry["canonicalName"]
        .as_str()
        .unwrap_or("")
        .contains("github.com"));
    let matched = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["show", "-R", "github.com/backups/widget", "main", "--json"],
        )
        .env("PATH", &path_env)
        .output()
        .expect("show");
    assert_eq!(matched.status.code(), Some(3), "{}", text(&matched.stdout));
    assert_eq!(json_value(&matched)["error"]["code"], "not_found");
    let by_name = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["path", "-R", "ptm805local", "main"],
        )
        .env("PATH", &path_env)
        .output()
        .expect("name");
    assert!(by_name.status.success(), "{}", text(&by_name.stderr));
    assert!(same_dir(&text(&by_name.stdout), &project));

    let git_dir = repo.rabi(&[
        "path",
        "-R",
        repo.repo.join(".git").to_str().expect("git"),
        "main",
    ]);
    assert!(
        git_dir.status.success(),
        "{}\n{}",
        text(&git_dir.stdout),
        text(&git_dir.stderr)
    );
    assert!(same_dir(&text(&git_dir.stdout), &repo.repo));
    let objects = repo.rabi(&[
        "path",
        "-R",
        repo.repo.join(".git/objects").to_str().expect("objects"),
        "main",
    ]);
    assert!(objects.status.success(), "{}", text(&objects.stderr));
    assert!(same_dir(&text(&objects.stdout), &repo.repo));
    assert!(!text(&objects.stderr).contains("must be run in a work tree"));
}

#[test]
fn workspace_intent_is_recorded_updated_and_removed() {
    let repo = Repo::new();
    let created = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "feature", "--purpose", "fix the flake", "--json"],
        )
        .output()
        .unwrap();
    assert!(created.status.success(), "{}", text(&created.stderr));
    assert!(repo
        .rabi(&["set", "feature", "--pr", "774"])
        .status
        .success());
    let created = json_value(&repo.rabi(&["show", "feature", "--json"]));
    let workspace = &created["data"]["workspace"];
    assert!(workspace.get("owner").is_none());
    assert!(workspace.get("ownerRelation").is_none());
    assert!(workspace.get("lastUsedAt").is_none());
    assert_eq!(workspace["purpose"], "fix the flake");
    assert_eq!(workspace["links"]["pr"]["number"], 774);
    assert_eq!(workspace["createdFrom"], workspace["head"]);
    assert!(text(&repo.rabi(&["list"]).stdout).contains("fix the flake"));
    let updated = repo.rabi(&[
        "set",
        "feature",
        "--purpose",
        "land the fix",
        "--issue",
        "12",
        "--json",
    ]);
    assert!(updated.status.success(), "{}", text(&updated.stderr));
    let updated = json_value(&updated);
    assert_eq!(updated["data"]["workspace"]["purpose"], "land the fix");
    assert_eq!(updated["data"]["workspace"]["links"]["pr"]["number"], 774);
    assert_eq!(updated["data"]["workspace"]["links"]["issue"]["number"], 12);
    assert_eq!(
        updated["data"]["workspace"]["createdAt"],
        workspace["createdAt"]
    );
    assert_eq!(
        updated["data"]["workspace"]["createdFrom"],
        workspace["createdFrom"]
    );
    assert_eq!(repo.rabi(&["set", "feature"]).status.code(), Some(2));
    assert_eq!(
        repo.rabi(&["set", "missing", "--purpose", "task"])
            .status
            .code(),
        Some(3)
    );
    assert_eq!(
        repo.rabi(&["set", "feature", "--pr", "invalid"])
            .status
            .code(),
        Some(2)
    );
    repo.git(&["branch", "side"]);
    let attached = repo.rabi(&[
        "create",
        "--branch",
        "side",
        "--purpose",
        "review",
        "--issue",
        "12",
        "--json",
    ]);
    assert!(attached.status.success());
    let attached = json_value(&attached);
    assert_eq!(attached["data"]["workspace"]["purpose"], "review");
    assert!(attached["data"]["workspace"].get("owner").is_none());
    assert!(attached["data"]["workspace"].get("lastUsedAt").is_none());
    let deleted = repo.rabi(&["remove", "feature", "--keep-branch", "--json"]);
    assert!(deleted.status.success());
    let deleted = json_value(&deleted);
    assert_eq!(
        deleted["data"]["items"][0]["workspace"]["purpose"],
        "land the fix"
    );
    assert!(deleted["data"]["items"][0]["workspace"]
        .get("owner")
        .is_none());
    assert!(deleted["data"]["items"][0]["workspace"]
        .get("lastUsedAt")
        .is_none());
    assert!(!text(&repo.rabi(&["list", "--names"]).stdout).contains("feature"));
}

#[test]
fn github_recheckout_keeps_the_original_intent() {
    let repo = Repo::new();
    // Bare links must belong to the repository returned by the gh fixture.
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/demo.git",
    ]);
    let created = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "topic", "--purpose", "original task", "--json"],
        )
        .output()
        .expect("create");
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created_at = json_value(&created)["data"]["workspace"]["createdAt"]
        .as_str()
        .expect("created")
        .to_string();
    let file = repo.repo.join(".git/rabi/workspaces");
    let mut document: serde_json::Value =
        serde_json::from_str(&records::read_to_string(&file).expect("store")).expect("json");
    document["worktrees"][0]["lastUsedAt"] = serde_json::json!("2000-01-01T00:00:00.000Z");
    records::write(
        &file,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&document).expect("encode")
        ),
    )
    .expect("rewrite");

    assert!(repo.rabi(&["set", "topic", "--pr", "7"]).status.success());
    let path = fake_gh_for_topic_pr(&repo);
    let again = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "create",
                "--pr",
                "7",
                "--reuse",
                "--json",
                "--purpose",
                "stolen",
            ],
        )
        .env("PATH", &path)
        .output()
        .expect("checkout");
    assert!(again.status.success(), "{}", text(&again.stderr));
    let again_json = json_value(&again);
    assert_eq!(again_json["data"]["source"]["reused"], true);

    assert_eq!(again_json["data"]["workspace"]["purpose"], "original task");
    assert_eq!(again_json["data"]["workspace"]["createdAt"], created_at);
    assert!(again_json["data"]["workspace"].get("lastUsedAt").is_none());
    assert_eq!(again_json["data"]["workspace"]["links"]["pr"]["number"], 7);
    let listed = repo.rabi(&["list", "--json", "--kind", "workspace"]);
    assert_eq!(
        json_value(&listed)["data"]["workspaces"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn github_checkout_refuses_before_fetch_when_the_metadata_store_is_unusable() {
    let corrupt = Repo::new();
    publish_pr_topic(&corrupt);
    let corrupt_path = fake_gh_for_topic_pr(&corrupt);
    let file = corrupt.repo.join(".git/rabi/workspaces");
    fs::create_dir_all(file.parent().expect("dir")).expect("dir");
    records::write(&file, "{not json").expect("corrupt");
    let refused = corrupt
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "--pr", "7"])
        .env("PATH", &corrupt_path)
        .output()
        .expect("checkout");
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused.stdout));
    assert!(
        text(&refused.stdout).is_empty(),
        "{}",
        text(&refused.stdout)
    );
    let stderr = text(&refused.stderr);
    assert!(stderr.contains("workspaces"), "{stderr}");
    assert!(stderr.contains("invalid JSON"), "{stderr}");
    assert_eq!(records::read_to_string(&file).expect("kept"), "{not json");
    assert!(
        local_branch(&corrupt, "topic").is_empty(),
        "corrupt store left {}",
        local_branch(&corrupt, "topic")
    );
    assert!(!managed(&corrupt, "topic").exists());

    let locked = Repo::new();
    publish_pr_topic(&locked);
    let locked_path = fake_gh_for_topic_pr(&locked);
    let metadata_dir = locked.repo.join(".git/rabi/workspaces");
    fs::create_dir_all(&metadata_dir).expect("dir");
    let store = r#"{"worktrees": []}"#;
    records::write(&metadata_dir, store).expect("store");
    let before = records::read_to_string(&metadata_dir).expect("snapshot");
    let mut perms = fs::metadata(&metadata_dir).expect("meta").permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o555);
    fs::set_permissions(&metadata_dir, perms).expect("chmod");
    let _restore = ModeReset(&metadata_dir);
    let unwritable = locked
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &locked_path)
        .output()
        .expect("checkout");
    assert_eq!(
        unwritable.status.code(),
        Some(1),
        "{}",
        text(&unwritable.stdout)
    );
    let unwritable_json = json_value(&unwritable);
    assert_eq!(unwritable_json["error"]["code"], "failed");
    assert!(unwritable_json["error"].get("exitCode").is_none());
    assert!(unwritable_json["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("not writable"));
    assert!(
        text(&unwritable.stderr).is_empty(),
        "{}",
        text(&unwritable.stderr)
    );
    assert!(
        local_branch(&locked, "topic").is_empty(),
        "unwritable store left {}",
        local_branch(&locked, "topic")
    );
    assert!(!managed(&locked, "topic").exists());
    drop(_restore);
    assert_eq!(
        records::read_to_string(&metadata_dir).expect("kept"),
        before
    );

    let ready = Repo::new();
    publish_pr_topic(&ready);
    let ready_path = fake_gh_for_topic_pr(&ready);
    let checked_out = ready
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &ready_path)
        .output()
        .expect("checkout");
    assert!(
        checked_out.status.success(),
        "{}\n{}",
        text(&checked_out.stdout),
        text(&checked_out.stderr)
    );
    let checked_out_json = json_value(&checked_out);
    assert!(checked_out_json["error"].is_null(), "{checked_out_json}");
    assert_eq!(checked_out_json["data"]["source"]["kind"], "pr");
    assert_eq!(checked_out_json["data"]["source"]["headRef"], "topic");
    assert_eq!(checked_out_json["data"]["source"]["reused"], false);
    assert_eq!(checked_out_json["data"]["metadata"]["ok"], true);
    assert!(managed(&ready, "topic").is_dir());
    assert!(
        local_branch(&ready, "topic").contains("topic"),
        "{}",
        local_branch(&ready, "topic")
    );
    let upstream = ready
        .command("git", &["rev-parse", "--abbrev-ref", "topic@{upstream}"])
        .output()
        .expect("upstream");
    assert!(upstream.status.success(), "{}", text(&upstream.stderr));
    assert_eq!(text(&upstream.stdout), "origin/topic");
}

#[test]
fn delete_refuses_when_metadata_is_not_writable() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "doomed", "--purpose", "creator", "--json"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let metadata_dir = repo.repo.join(".git/rabi/workspaces");
    let store = records::read_to_string(&metadata_dir).expect("store");
    use std::os::unix::fs::PermissionsExt;
    let mut locked = fs::metadata(&metadata_dir).expect("meta").permissions();
    locked.set_mode(0o555);
    fs::set_permissions(&metadata_dir, locked).expect("chmod");
    let _restore = ModeReset(&metadata_dir);
    let deleted = repo.rabi(&["remove", "doomed", "--keep-branch", "--json"]);
    assert_eq!(deleted.status.code(), Some(1), "{}", text(&deleted.stdout));
    let deleted_json = json_value(&deleted);
    assert!(deleted_json["error"]["message"]
        .as_str()
        .unwrap_or("")
        .contains("workspaces"));
    assert!(deleted_json["error"].get("exitCode").is_none());
    assert!(managed(&repo, "doomed").is_dir());
    drop(_restore);
    assert_eq!(records::read_to_string(&metadata_dir).expect("kept"), store);
}

#[test]
fn create_replaces_a_record_left_by_git_worktree_remove() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature", "--purpose", "old task", "--json"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let created_at = json_value(&created)["data"]["workspace"]["createdAt"]
        .as_str()
        .expect("created")
        .to_string();
    let path = managed(&repo, "feature");
    repo.git(&["worktree", "remove", path.to_str().expect("path")]);
    repo.git(&["branch", "-D", "feature"]);
    let again = repo.rabi(&["create", "feature", "--purpose", "new task", "--json"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    let again_json = json_value(&again);

    assert_eq!(again_json["data"]["workspace"]["purpose"], "new task");
    assert_ne!(again_json["data"]["workspace"]["createdAt"], created_at);
    assert_eq!(again_json["data"]["metadata"]["ok"], true);
    let listed = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    assert_eq!(text(&listed.stdout), "feature");
    assert_eq!(
        json_value(&repo.rabi(&["list", "--json", "--kind", "workspace"]))["data"]["workspaces"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

// Metadata save failure injection is only supported in debug builds.
#[cfg(debug_assertions)]
#[test]
fn metadata_save_failure_after_create_still_runs_hooks() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch hook-ran\"]\n",
    )
    .expect("hooks");
    let created = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "feature", "--purpose", "creator", "--json"],
        )
        .env("RABI_TEST_FAIL_METADATA_SAVE", "1")
        .output()
        .expect("create");
    assert_eq!(created.status.code(), Some(6), "{}", text(&created.stdout));
    let created_json = json_value(&created);
    assert!(created_json["error"]["code"] == "partial", "{created_json}");
    assert_eq!(created_json["data"]["workspace"]["name"], "feature");
    assert!(created_json["data"]["workspace"]["purpose"].is_null());
    assert_eq!(created_json["data"]["metadata"]["ok"], false);
    assert!(created_json["data"]["metadata"]["error"]
        .as_str()
        .unwrap_or("")
        .contains("injected"));
    assert_eq!(created_json["data"]["setup"]["ok"], true);
    assert!(created_json["data"]["copy"]["error"].is_null());
    assert!(managed(&repo, "feature/hook-ran").is_file());
    assert!(text(&created.stderr).contains("Warning"));
    assert!(text(&created.stderr).contains("injected"));
    assert!(!created_json["warnings"]
        .as_array()
        .expect("warnings")
        .is_empty());
}

#[test]
fn readers_warn_on_corrupt_metadata_and_still_run() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature", "--purpose", "creator", "--json"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let id = json_value(&created)["data"]["workspace"]["id"]
        .as_str()
        .expect("workspace id")
        .to_string();
    assert!(repo
        .rabi(&["create", "healthy", "--purpose", "reader"])
        .status
        .success());
    let file = repo
        .repo
        .join(".git/rabi/workspaces")
        .join(format!("{id}.json"));
    fs::write(&file, "{not json").expect("corrupt");

    let listed = repo.rabi(&["list", "--json", "--kind", "workspace"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let listed_json = json_value(&listed);
    assert_eq!(listed_json["data"]["workspaces"][0]["name"], "feature");
    assert!(listed_json["data"]["workspaces"][0]["purpose"].is_null());
    assert!(text(&listed.stderr).contains("workspaces"));
    let healthy = listed_json["data"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tree| tree["name"] == "healthy")
        .unwrap();
    assert_eq!(healthy["purpose"], "reader");
    assert!(healthy["id"].as_str().unwrap().starts_with("ws_"));
    assert!(!listed_json["warnings"]
        .as_array()
        .expect("warnings")
        .is_empty());

    let located = repo.rabi(&["show", "feature", "--json"]);
    assert!(located.status.success(), "{}", text(&located.stderr));
    assert!(json_value(&located)["data"]["workspace"]["purpose"].is_null());
    assert!(text(&located.stderr).contains("invalid JSON"));
    let human = repo.rabi(&["path", "feature"]);
    assert!(human.status.success());
    assert!(text(&human.stdout).starts_with('/'));

    let ran = repo.rabi(&["exec", "feature", "--", "true"]);
    assert!(ran.status.success(), "{}", text(&ran.stderr));
    assert!(text(&ran.stderr).contains("invalid JSON"));

    assert_eq!(fs::read_to_string(&file).expect("kept"), "{not json");
}

#[test]
fn exec_command_json_flag_does_not_switch_rabi_to_json() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "child"]).status.success());
    let out = repo.rabi(&["exec", "child", "--", "printf", "%s", "--json"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "--json");
}

#[test]
fn separate_git_dir_creates_the_default_worktree_under_xdg() {
    let root = std::env::temp_dir().join(format!(
        "rabi-sep-default-{}-{}",
        std::process::id(),
        uuid()
    ));
    let home = root.join("home");
    let checkout = root.join("checkout");
    let sep = root.join("sep.git");
    let template = root.join("git-template");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    fs::create_dir_all(&template).expect("git template");
    test_git::write_config(&home);
    fs::create_dir_all(&checkout).expect("checkout");
    let init = std::process::Command::new("git")
        .args([
            "init",
            "--separate-git-dir",
            sep.to_str().expect("sep"),
            "-b",
            "main",
        ])
        .current_dir(&checkout)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TEMPLATE_DIR", &template)
        .env("HOME", &home)
        .output()
        .expect("init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&checkout)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["config", "user.name", "Phantom Rust"]);
    git(&["config", "user.email", "phantom-rust@example.com"]);
    fs::write(checkout.join("README.md"), "# sep\n").expect("readme");
    git(&["add", "README.md"]);
    git(&["commit", "-m", "initial"]);
    assert!(checkout.join(".git").is_file(), ".git is a gitdir file");

    let created = std::process::Command::new(env!("CARGO_BIN_EXE_rabi"))
        .args(["create", "feature", "--json"])
        .current_dir(&checkout)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("xdg"))
        .env_remove("RABI_OUTPUT")
        .env_remove("RABI_NO_INPUT")
        .output()
        .expect("rabi");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let created_json = json_value(&created);
    assert_eq!(created_json["schema"], 2);
    let worktree = managed_directory(&checkout, &root.join("xdg")).join("feature");
    assert_worktree_object(&created_json["data"]["workspace"], "feature", &worktree);
    assert!(worktree.is_dir());
    assert!(!sep.join("rabi").join("workspaces").join("feature").exists());
    assert!(sep.join("rabi").join("workspaces").is_dir());
    assert!(checkout.join(".git").is_file(), ".git stayed a gitdir file");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn separate_git_dir_writes_metadata_in_the_common_dir() {
    let root = std::env::temp_dir().join(format!("rabi-sep-{}-{}", std::process::id(), uuid()));
    let home = root.join("home");
    let checkout = root.join("checkout");
    let sep = root.join("sep.git");
    let worktrees = root.join("worktrees");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    test_git::write_config(&home);
    fs::create_dir_all(&checkout).expect("checkout");
    let init = std::process::Command::new("git")
        .args([
            "init",
            "--separate-git-dir",
            sep.to_str().expect("sep"),
            "-b",
            "main",
        ])
        .current_dir(&checkout)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .output()
        .expect("init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(&checkout)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&["config", "user.name", "Phantom Rust"]);
    git(&["config", "user.email", "phantom-rust@example.com"]);
    fs::write(checkout.join("README.md"), "# sep\n").expect("readme");
    git(&["add", "README.md"]);
    git(&["commit", "-m", "initial"]);
    assert!(checkout.join(".git").is_file(), ".git is a gitdir file");

    let rabi = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_rabi"))
            .args(args)
            .current_dir(&checkout)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("xdg"))
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .output()
            .expect("rabi")
    };
    let pref = rabi(&[
        "config",
        "set",
        "workspace-root",
        worktrees.to_str().expect("worktrees"),
    ]);
    assert!(pref.status.success(), "{}", text(&pref.stderr));
    let created = rabi(&["create", "feature", "--purpose", "sep", "--json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(worktrees.join("feature").is_dir());
    let store = sep.join("rabi").join("workspaces");
    let document: serde_json::Value =
        serde_json::from_str(&records::read_to_string(&store).expect("common store"))
            .expect("json");
    assert!(document["worktrees"]
        .as_array()
        .expect("records")
        .iter()
        .any(|record| record["purpose"] == "sep"));
    let _ = fs::remove_dir_all(&root);
}

struct SeparateCheckout {
    root: PathBuf,
    checkout: PathBuf,
    git_dir: PathBuf,
    home: PathBuf,
}

impl SeparateCheckout {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("rabi-sep-root-{}", uuid()));
        let home = root.join("home");
        let checkout = root.join("deep").join("checkout");
        let git_dir = root.join("var").join("sep.git");
        fs::create_dir_all(home.join(".config/git")).expect("home");
        test_git::write_config(&home);
        fs::create_dir_all(&checkout).expect("checkout");
        fs::create_dir_all(git_dir.parent().expect("git dir parent")).expect("git dir parent");
        let repo = Self {
            root,
            checkout,
            git_dir,
            home,
        };
        repo.git(&[
            "init",
            "--separate-git-dir",
            repo.git_dir.to_str().expect("git dir"),
            "-b",
            "main",
        ]);
        repo.git(&["config", "user.name", "Phantom Rust"]);
        repo.git(&["config", "user.email", "phantom-rust@example.com"]);
        fs::write(repo.checkout.join("README.md"), "# sep\n").expect("readme");
        repo.git(&["add", "README.md"]);
        repo.git(&["commit", "-m", "initial"]);
        assert!(
            repo.checkout.join(".git").is_file(),
            ".git is a gitdir file"
        );
        repo
    }

    fn git(&self, args: &[&str]) {
        let output = self
            .command("git", args, &self.checkout)
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn rabi(&self, cwd: &Path, args: &[&str]) -> std::process::Output {
        self.command(env!("CARGO_BIN_EXE_rabi"), args, cwd)
            .output()
            .expect("rabi")
    }

    fn command(&self, program: &str, args: &[&str], cwd: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_DATA_HOME", self.root.join("xdg"))
            .env_remove("GHQ_ROOT")
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS");
        if std::path::Path::new(program)
            .file_name()
            .and_then(|name| name.to_str())
            == Some("rabi")
        {
            command.env("RABI_TRUST_ALL", "1");
        }
        if program.ends_with("git") && args.first() == Some(&"init") {
            let template = self.root.join("git-template");
            fs::create_dir_all(&template).expect("git template");
            command.env("GIT_TEMPLATE_DIR", &template);
        }
        command
    }
}

impl Drop for SeparateCheckout {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn separate_git_dir_delete_current_removes_the_linked_worktree() {
    let repo = SeparateCheckout::new();
    let created = repo.rabi(&repo.checkout, &["create", "linked"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let linked = managed_directory(&repo.checkout, &repo.root.join("xdg")).join("linked");
    assert!(
        linked.is_dir(),
        "primary checkout lives under XDG data home"
    );
    assert!(!repo
        .git_dir
        .join("rabi")
        .join("workspaces")
        .join("linked")
        .exists());
    let deleted = repo.rabi(&linked, &["remove", "."]);
    let stdout = text(&deleted.stdout);
    let stderr = text(&deleted.stderr);
    assert!(
        deleted.status.success(),
        "delete --current from the linked workspace should remove it\n{stdout}\n{stderr}"
    );
    assert!(
        stdout.contains("Deleted workspace 'linked'"),
        "{stdout}\n{stderr}"
    );
    assert!(!linked.exists(), "linked workspace should be removed");
    assert!(
        repo.checkout.join("README.md").is_file(),
        "main checkout stays"
    );
    assert!(
        repo.checkout.join(".git").is_file(),
        "main checkout stays a gitfile, not the deleted workspace"
    );
    let branch = repo
        .command("git", &["branch", "--list", "linked"], &repo.checkout)
        .output()
        .expect("branch");
    assert!(branch.status.success(), "{}", text(&branch.stderr));
    assert!(
        text(&branch.stdout).is_empty(),
        "delete --current removes the workspace branch: {}",
        text(&branch.stdout)
    );
    let main = repo
        .command("git", &["branch", "--list", "main"], &repo.checkout)
        .output()
        .expect("main");
    assert!(
        text(&main.stdout).contains("main"),
        "{}",
        text(&main.stdout)
    );
}

#[test]
fn separate_git_dir_relative_worktrees_directory_joins_the_main_checkout() {
    let repo = SeparateCheckout::new();
    let pref = repo.rabi(
        &repo.checkout,
        &["config", "set", "workspace-root", "../wt"],
    );
    assert!(pref.status.success(), "{}", text(&pref.stderr));
    let first = repo.rabi(&repo.checkout, &["create", "r1"]);
    assert!(
        first.status.success(),
        "{}\n{}",
        text(&first.stdout),
        text(&first.stderr)
    );
    let first_path = repo.root.join("deep").join("wt").join("r1");
    assert!(
        first_path.is_dir(),
        "from the main checkout, ../wt is deep/wt"
    );
    let second = repo.rabi(&first_path, &["create", "r2"]);
    let stdout = text(&second.stdout);
    let stderr = text(&second.stderr);
    assert!(second.status.success(), "{stdout}\n{stderr}");
    let expected = repo.root.join("deep").join("wt").join("r2");
    assert!(
        expected.is_dir(),
        "relative workspace-root stays joined to the main checkout\n{stdout}\n{stderr}"
    );
    assert!(
        !repo
            .root
            .join("deep")
            .join("wt")
            .join("wt")
            .join("r2")
            .exists(),
        "must not nest under the linked workspace"
    );
    assert!(
        !repo.root.join("var").join("wt").join("r2").exists(),
        "must not join the relative directory to the separate git dir"
    );
    assert!(repo.checkout.join("README.md").is_file());
}

#[test]
fn submodule_delete_current_and_relative_directory_use_the_submodule_checkout() {
    let root = std::env::temp_dir().join(format!("rabi-sub-root-{}", uuid()));
    let home = root.join("home");
    let remote = root.join("remote");
    let superproject = root.join("super");
    let checkout = superproject.join("sub");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    test_git::write_config(&home);
    fs::create_dir_all(&remote).expect("remote");
    fs::create_dir_all(&superproject).expect("super");
    let git = |cwd: &Path, args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&remote, &["init", "-b", "main"]);
    git(&remote, &["config", "user.name", "Phantom Rust"]);
    git(
        &remote,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    fs::write(remote.join("f"), "hi\n").expect("file");
    git(&remote, &["add", "f"]);
    git(&remote, &["commit", "-m", "initial"]);
    git(&superproject, &["init", "-b", "main"]);
    git(&superproject, &["config", "user.name", "Phantom Rust"]);
    git(
        &superproject,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    let added = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            remote.to_str().expect("remote"),
            "sub",
        ])
        .current_dir(&superproject)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env_remove("GIT_CONFIG_GLOBAL")
        .output()
        .expect("submodule add");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    git(&superproject, &["commit", "-m", "add submodule"]);
    let rabi = |cwd: &Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rabi"))
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("xdg"))
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .output()
            .expect("rabi")
    };
    let created = rabi(&checkout, &["create", "linked"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let linked = managed_directory(&checkout, &root.join("xdg")).join("linked");
    assert!(!superproject
        .join(".git/modules/sub/custom/workspaces/linked")
        .exists());
    assert!(linked.is_dir());
    let deleted = rabi(&linked, &["remove", "."]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(!linked.exists());
    assert!(checkout.join(".git").is_file(), "submodule checkout stays");
    let pref = rabi(&checkout, &["config", "set", "workspace-root", "../wt"]);
    assert!(pref.status.success(), "{}", text(&pref.stderr));
    let first = rabi(&checkout, &["create", "r1"]);
    assert!(first.status.success(), "{}", text(&first.stderr));
    let first_path = superproject.join("wt").join("r1");
    assert!(first_path.is_dir());
    let second = rabi(&first_path, &["create", "r2"]);
    assert!(
        second.status.success(),
        "{}\n{}",
        text(&second.stdout),
        text(&second.stderr)
    );
    assert!(superproject.join("wt").join("r2").is_dir());
    assert!(!superproject.join("wt").join("wt").join("r2").exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn ambiguous_gitfiles_fail_until_core_worktree_is_set() {
    let root = std::env::temp_dir().join(format!("rabi-ambiguous-{}", uuid()));
    let home = root.join("home");
    let real = root.join("zz-real");
    let copy = root.join("aa-copy");
    let git_dir = root.join("var").join("sep.git");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    test_git::write_config(&home);
    fs::create_dir_all(&real).expect("checkout");
    fs::create_dir_all(git_dir.parent().expect("parent")).expect("var");
    let git = |cwd: &Path, args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env("GIT_TEMPLATE_DIR", {
                let template = root.join("git-template");
                fs::create_dir_all(&template).expect("template");
                template
            })
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(
        &real,
        &[
            "init",
            "--separate-git-dir",
            git_dir.to_str().expect("git dir"),
            "-b",
            "main",
        ],
    );
    git(&real, &["config", "user.name", "Phantom Rust"]);
    git(&real, &["config", "user.email", "phantom-rust@example.com"]);
    git(&real, &["commit", "--allow-empty", "-m", "initial"]);
    copy_dir(&real, &copy);
    let linked = git_dir.join("custom").join("workspaces").join("q");
    git(
        &real,
        &[
            "worktree",
            "add",
            "-b",
            "q",
            linked.to_str().expect("linked"),
        ],
    );
    let rabi = |cwd: &Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rabi"))
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("xdg"))
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .output()
            .expect("rabi")
    };

    for checkout in [&copy, &real] {
        let here = rabi(checkout, &["list", "--json"]);
        assert!(
            here.status.success(),
            "running inside {} should use that checkout\n{}\n{}",
            checkout.display(),
            text(&here.stdout),
            text(&here.stderr)
        );
        let main = default_worktree(&json_value(&here));
        assert!(
            paths_match(main["path"].as_str().unwrap_or(""), checkout),
            "got {}",
            main["path"]
        );
        assert_eq!(main["kind"], "primary");
        let ran = rabi(checkout, &["exec", "main", "--", "pwd"]);
        assert!(ran.status.success(), "{}", text(&ran.stderr));
        assert!(
            paths_match(text(&ran.stdout).trim(), checkout),
            "exec main ran in {}",
            text(&ran.stdout)
        );
    }

    let listed = rabi(&linked, &["list"]);
    assert_eq!(
        listed.status.code(),
        Some(1),
        "two gitfiles must not resolve to the first sorted checkout\n{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    let stderr = text(&listed.stderr);
    assert!(stderr.contains(real.to_str().expect("real")), "{stderr}");
    assert!(stderr.contains(copy.to_str().expect("copy")), "{stderr}");
    assert!(stderr.contains("core.worktree"), "{stderr}");
    assert!(!stderr.to_ascii_lowercase().contains("fatal:"), "{stderr}");
    assert!(text(&listed.stdout).is_empty(), "{}", text(&listed.stdout));

    let json_error = rabi(&linked, &["list", "--json"]);
    assert_eq!(json_error.status.code(), Some(1));
    assert!(
        text(&json_error.stderr).is_empty(),
        "{}",
        text(&json_error.stderr)
    );
    let error = json_value(&json_error);
    assert_eq!(error["schema"], 2);
    assert_eq!(error["error"]["code"], "failed");
    assert!(error["error"].get("exitCode").is_none());
    let message = error["error"]["message"].as_str().unwrap_or("");
    assert!(message.contains(real.to_str().expect("real")), "{message}");
    assert!(message.contains(copy.to_str().expect("copy")), "{message}");
    assert!(message.contains("core.worktree"), "{message}");
    assert!(
        !message.to_ascii_lowercase().contains("fatal:"),
        "{message}"
    );

    git(
        &real,
        &["config", "core.worktree", real.to_str().expect("real")],
    );
    let resolved = rabi(&linked, &["list", "--json"]);
    assert!(
        resolved.status.success(),
        "{}\n{}",
        text(&resolved.stdout),
        text(&resolved.stderr)
    );
    let main = default_worktree(&json_value(&resolved));
    assert!(
        paths_match(main["path"].as_str().unwrap_or(""), &real),
        "core.worktree selects zz-real, got {}",
        main["path"]
    );
    assert_eq!(main["kind"], "primary");
    let ran = rabi(&linked, &["exec", "main", "--", "pwd"]);
    assert!(
        ran.status.success(),
        "{}\n{}",
        text(&ran.stdout),
        text(&ran.stderr)
    );
    assert!(
        paths_match(text(&ran.stdout).trim(), &real),
        "exec main ran in {}",
        text(&ran.stdout)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn missing_separate_checkout_names_the_git_dir() {
    let root = std::env::temp_dir().join(format!("rabi-missing-checkout-{}", uuid()));
    let home = root.join("home");
    let checkout = root.join("deep").join("checkout");
    let git_dir = root.join("var").join("sep.git");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    test_git::write_config(&home);
    fs::create_dir_all(&checkout).expect("checkout");
    fs::create_dir_all(git_dir.parent().expect("parent")).expect("var");
    let init = Command::new("git")
        .args([
            "init",
            "--separate-git-dir",
            git_dir.to_str().expect("git dir"),
            "-b",
            "main",
        ])
        .current_dir(&checkout)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env_remove("GIT_CONFIG_GLOBAL")
        .output()
        .expect("init");
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    fs::remove_dir_all(&checkout).expect("remove checkout");
    let listed = Command::new(env!("CARGO_BIN_EXE_rabi"))
        .args(["list", "--json"])
        .current_dir(&git_dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_DATA_HOME", root.join("xdg"))
        .env_remove("RABI_OUTPUT")
        .env_remove("RABI_NO_INPUT")
        .output()
        .expect("rabi");
    assert_eq!(
        listed.status.code(),
        Some(1),
        "{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    assert!(text(&listed.stderr).is_empty(), "{}", text(&listed.stderr));
    let error = json_value(&listed);
    let message = error["error"]["message"].as_str().unwrap_or("");
    assert!(
        message.contains(git_dir.to_str().expect("git dir")),
        "{message}"
    );
    assert!(message.contains("core.worktree"), "{message}");
    assert!(
        !message.to_ascii_lowercase().contains("fatal:"),
        "{message}"
    );
    assert_eq!(error["error"]["code"], "failed");
    assert!(error["error"].get("exitCode").is_none());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn separate_git_dir_outside_the_search_lists_from_the_checkout() {
    let root = std::env::temp_dir().join(format!("rabi-far-checkout-{}", uuid()));
    let home = root.join("home");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    test_git::write_config(&home);
    let rabi = |cwd: &Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rabi"))
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("xdg"))
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .output()
            .expect("rabi")
    };
    let init_checkout = |checkout: &Path, git_dir: &Path| {
        fs::create_dir_all(checkout).expect("checkout");
        fs::create_dir_all(git_dir.parent().expect("git parent")).expect("git parent");
        let init = Command::new("git")
            .args([
                "init",
                "--separate-git-dir",
                git_dir.to_str().expect("git dir"),
                "-b",
                "main",
            ])
            .current_dir(checkout)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env_remove("GIT_CONFIG_GLOBAL")
            .output()
            .expect("init");
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(checkout)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", &home)
                .output()
                .expect("git");
            assert!(
                output.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["config", "user.name", "Phantom Rust"]);
        git(&["config", "user.email", "phantom-rust@example.com"]);
        git(&["commit", "--allow-empty", "-m", "initial"]);
    };

    for index in 1..=30 {
        let wide = root.join(format!("dir{index:02}"));
        for name in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
            fs::create_dir_all(wide.join(name)).expect("wide child");
        }
    }
    let wide_checkout = root.join("src").join("proj");
    init_checkout(&wide_checkout, &root.join(".gitdirs").join("proj.git"));
    let listed = rabi(&wide_checkout, &["list", "--json"]);
    assert!(
        listed.status.success(),
        "list from a checkout beside wide siblings\n{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    let main = default_worktree(&json_value(&listed));
    assert!(
        paths_match(main["path"].as_str().unwrap_or(""), &wide_checkout),
        "{}",
        main["path"]
    );
    assert_eq!(main["kind"], "primary");

    let deep_checkout = root
        .join("1")
        .join("2")
        .join("3")
        .join("4")
        .join("5")
        .join("6");
    init_checkout(&deep_checkout, &root.join("deep.git"));
    let deep = rabi(&deep_checkout, &["list", "--json"]);
    assert!(
        deep.status.success(),
        "list from a checkout six levels down\n{}\n{}",
        text(&deep.stdout),
        text(&deep.stderr)
    );
    let deep_main = default_worktree(&json_value(&deep));
    assert!(
        paths_match(deep_main["path"].as_str().unwrap_or(""), &deep_checkout),
        "{}",
        deep_main["path"]
    );
    assert_eq!(deep_main["kind"], "primary");
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn core_worktree_without_a_gitfile_names_that_path() {
    let repo = SeparateCheckout::new();
    let created = repo.rabi(&repo.checkout, &["create", "linked"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let empty = repo.root.join("empty");
    fs::create_dir_all(&empty).expect("empty");
    repo.git(&["config", "core.worktree", empty.to_str().expect("empty")]);
    let linked = managed_directory(&repo.checkout, &repo.root.join("xdg")).join("linked");
    let listed = repo.rabi(&linked, &["list"]);
    assert_eq!(
        listed.status.code(),
        Some(1),
        "{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    let stderr = text(&listed.stderr);
    assert!(stderr.contains(empty.to_str().expect("empty")), "{stderr}");
    assert!(stderr.contains("gitfile"), "{stderr}");
    assert!(!stderr.contains("Set the main checkout"), "{stderr}");
    assert!(!stderr.to_ascii_lowercase().contains("fatal:"), "{stderr}");
    let json = repo.rabi(&linked, &["list", "--json"]);
    assert_eq!(json.status.code(), Some(1));
    assert!(text(&json.stderr).is_empty(), "{}", text(&json.stderr));
    let message = json_value(&json)["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert!(message.contains("gitfile"), "{message}");
    assert!(
        message.contains(empty.to_str().expect("empty")),
        "{message}"
    );
    assert!(!message.contains("Set the main checkout"), "{message}");
}

#[test]
fn separate_git_dir_lists_and_execs_the_main_checkout() {
    let repo = SeparateCheckout::new();
    let created = repo.rabi(&repo.checkout, &["create", "linked"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let linked = managed_directory(&repo.checkout, &repo.root.join("xdg")).join("linked");
    assert_main_checkout(&repo, &linked, &repo.checkout, &repo.git_dir);
}

#[test]
fn submodule_lists_and_execs_the_main_checkout() {
    let root = std::env::temp_dir().join(format!("rabi-sub-list-{}", uuid()));
    let home = root.join("home");
    let remote = root.join("remote");
    let superproject = root.join("super");
    let checkout = superproject.join("sub");
    fs::create_dir_all(home.join(".config/git")).expect("home");
    test_git::write_config(&home);
    fs::create_dir_all(&remote).expect("remote");
    fs::create_dir_all(&superproject).expect("super");
    let git = |cwd: &Path, args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env_remove("GIT_CONFIG_GLOBAL")
            .env_remove("GIT_CONFIG_SYSTEM")
            .output()
            .expect("git");
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    };
    git(&remote, &["init", "-b", "main"]);
    git(&remote, &["config", "user.name", "Phantom Rust"]);
    git(
        &remote,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    fs::write(remote.join("f"), "hi\n").expect("file");
    git(&remote, &["add", "f"]);
    git(&remote, &["commit", "-m", "initial"]);
    git(&superproject, &["init", "-b", "main"]);
    git(&superproject, &["config", "user.name", "Phantom Rust"]);
    git(
        &superproject,
        &["config", "user.email", "phantom-rust@example.com"],
    );
    let added = Command::new("git")
        .args([
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            remote.to_str().expect("remote"),
            "sub",
        ])
        .current_dir(&superproject)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env_remove("GIT_CONFIG_GLOBAL")
        .output()
        .expect("submodule add");
    assert!(
        added.status.success(),
        "{}",
        String::from_utf8_lossy(&added.stderr)
    );
    git(&superproject, &["commit", "-m", "add submodule"]);
    let rabi = |cwd: &Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_rabi"))
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("xdg"))
            .env_remove("RABI_OUTPUT")
            .env_remove("RABI_NO_INPUT")
            .output()
            .expect("rabi")
    };
    let created = rabi(&checkout, &["create", "linked"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let git_dir = superproject.join(".git").join("modules").join("sub");
    let linked = managed_directory(&checkout, &root.join("xdg")).join("linked");
    assert!(git_dir.join("rabi").join("workspaces").is_dir());
    assert!(!git_dir
        .join("rabi")
        .join("workspaces")
        .join("linked")
        .exists());
    let listed = rabi(&linked, &["list"]);
    assert!(
        listed.status.success(),
        "{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    let stdout = text(&listed.stdout);
    assert!(
        stdout
            .lines()
            .any(|line| line.trim_start().starts_with("main  main")),
        "main entry should be the submodule checkout\n{stdout}"
    );
    let json = rabi(&linked, &["list", "--json"]);
    assert!(json.status.success(), "{}", text(&json.stderr));
    assert!(text(&json.stderr).is_empty(), "{}", text(&json.stderr));
    let main = default_worktree(&json_value(&json));
    assert!(
        paths_match(main["path"].as_str().unwrap_or(""), &checkout),
        "{}",
        main["path"]
    );
    assert_eq!(main["kind"], "primary");
    let hidden = rabi(&linked, &["list", "--kind", "workspace", "--json"]);
    assert!(hidden.status.success(), "{}", text(&hidden.stderr));
    let hidden_rows = json_value(&hidden)["data"]["workspaces"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        hidden_rows.iter().all(|row| row["kind"] != "primary"
            && !paths_match(row["path"].as_str().unwrap_or(""), &checkout)),
        "{hidden_rows:?}"
    );
    assert!(
        hidden_rows.iter().any(|row| row["name"] == "linked"),
        "{hidden_rows:?}"
    );
    let ran = rabi(&linked, &["exec", "main", "--", "pwd"]);
    assert!(
        ran.status.success(),
        "{}\n{}",
        text(&ran.stdout),
        text(&ran.stderr)
    );
    assert!(
        paths_match(text(&ran.stdout).trim(), &checkout),
        "exec main ran in {}",
        text(&ran.stdout)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn running_inside_dot_git_returns_a_rabi_error() {
    let repo = Repo::new();
    for cwd in [repo.repo.join(".git"), repo.repo.join(".git").join("refs")] {
        let listed = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["list"])
            .current_dir(&cwd)
            .output()
            .expect("rabi");
        assert_eq!(
            listed.status.code(),
            Some(1),
            "{} should not be a work tree\n{}\n{}",
            cwd.display(),
            text(&listed.stdout),
            text(&listed.stderr)
        );
        let stderr = text(&listed.stderr);
        assert!(stderr.contains("work tree"), "{stderr}");
        assert!(!stderr.to_ascii_lowercase().contains("fatal:"), "{stderr}");
        assert!(text(&listed.stdout).is_empty(), "{}", text(&listed.stdout));

        let json = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["list", "--json"])
            .current_dir(&cwd)
            .output()
            .expect("rabi");
        assert_eq!(json.status.code(), Some(1), "{}", cwd.display());
        assert!(text(&json.stderr).is_empty(), "{}", text(&json.stderr));
        let error = json_value(&json);
        assert_eq!(error["schema"], 2);
        assert_eq!(error["error"]["code"], "failed");
        assert!(error["error"].get("exitCode").is_none());
        let message = error["error"]["message"].as_str().unwrap_or("");
        assert!(message.contains("work tree"), "{message}");
        assert!(
            !message.to_ascii_lowercase().contains("fatal:"),
            "{message}"
        );
        assert!(error["data"]["workspaces"].is_null());
    }
}

fn assert_main_checkout(repo: &SeparateCheckout, linked: &Path, checkout: &Path, git_dir: &Path) {
    let listed = repo.rabi(linked, &["list"]);
    assert!(
        listed.status.success(),
        "{}\n{}",
        text(&listed.stdout),
        text(&listed.stderr)
    );
    let stdout = text(&listed.stdout);
    assert!(
        stdout.contains("deep/checkout"),
        "human list should name the checkout\n{stdout}"
    );
    assert!(
        !stdout.contains("sep.git"),
        "human list should not show the git dir\n{stdout}"
    );
    let json = repo.rabi(linked, &["list", "--json"]);
    assert!(json.status.success(), "{}", text(&json.stderr));
    assert!(text(&json.stderr).is_empty(), "{}", text(&json.stderr));
    let document = json_value(&json);
    assert_eq!(document["schema"], 2);
    let rows = document["data"]["workspaces"]
        .as_array()
        .expect("worktrees");
    assert!(
        rows.iter()
            .all(|row| !paths_match(row["path"].as_str().unwrap_or(""), git_dir)),
        "git dir is still a workspace path: {rows:?}"
    );
    let main = default_worktree(&document);
    assert!(
        paths_match(main["path"].as_str().unwrap_or(""), checkout),
        "{}",
        main["path"]
    );
    assert_eq!(main["kind"], "primary");
    let hidden = repo.rabi(linked, &["list", "--kind", "workspace"]);
    assert!(hidden.status.success(), "{}", text(&hidden.stderr));
    let hidden_text = text(&hidden.stdout);
    assert!(
        !hidden_text.lines().any(|line| line.starts_with("main ")),
        "workspace-only list still shows main\n{hidden_text}"
    );
    assert!(hidden_text.contains("linked"), "{hidden_text}");
    let ran = repo.rabi(linked, &["exec", "main", "--", "pwd"]);
    assert!(
        ran.status.success(),
        "{}\n{}",
        text(&ran.stdout),
        text(&ran.stderr)
    );
    assert!(
        paths_match(text(&ran.stdout).trim(), checkout),
        "exec main ran in {}",
        text(&ran.stdout)
    );
}

fn default_worktree(document: &serde_json::Value) -> serde_json::Value {
    document["data"]["workspaces"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["kind"] == "primary"))
        .cloned()
        .unwrap_or_else(|| panic!("no primary checkout in {document}"))
}

fn copy_dir(from: &Path, to: &Path) {
    let status = Command::new("cp")
        .args(["-a", from.to_str().expect("from"), to.to_str().expect("to")])
        .status()
        .expect("cp");
    assert!(status.success(), "cp failed");
}

fn worktree_dir(repo: &Repo, name: &str) -> PathBuf {
    managed_root(repo).join(name)
}

fn branch_listed(repo: &Repo, name: &str) -> bool {
    let output = repo
        .command("git", &["branch", "--list", name])
        .output()
        .expect("branch");
    !text(&output.stdout).is_empty()
}

fn worktree_registered(repo: &Repo, name: &str) -> bool {
    let output = repo
        .command("git", &["worktree", "list", "--porcelain"])
        .output()
        .expect("worktrees");
    let path = worktree_dir(repo, name);
    text(&output.stdout).contains(&path.to_string_lossy().to_string())
}

fn reserve_project_dir(git_root: &Path, xdg: &Path) {
    let common = rabi::git::canonical_common_dir(git_root).expect("common dir");
    let directory = rabi::paths::default_worktrees_directory(git_root, &common, xdg).path;
    rabi::paths::ensure_worktrees_directory(&directory, Some(&common)).expect("project dir");
}

fn land_branch(repo: &Repo, name: &str) {
    commit_in_worktree(repo, name);
    repo.git(&["merge", "--ff-only", name]);
}

fn commit_in_worktree(repo: &Repo, name: &str) {
    let file = format!("{name}.txt");
    commit_file(repo, name, &file, &format!("{name}\n"));
}

fn commit_file(repo: &Repo, name: &str, file: &str, body: &str) {
    let dir = worktree_dir(repo, name);
    fs::write(dir.join(file), body).expect("note");
    for args in [vec!["add", file], vec!["commit", "-m", file]] {
        let output = repo
            .command("git", &args)
            .current_dir(&dir)
            .output()
            .expect("commit");
        assert!(
            output.status.success(),
            "git {args:?}: stderr={} stdout={}",
            text(&output.stderr),
            text(&output.stdout)
        );
    }
}

fn publish_branch(repo: &Repo, name: &str) {
    let bare = repo.root.join("origin.git");
    let spec = bare.to_str().expect("bare path");
    if !bare.exists() {
        repo.git(&["init", "--bare", spec]);
        repo.git(&["remote", "add", "origin", spec]);
        repo.git(&["push", "-u", "origin", "main"]);
    }
    repo.git(&["push", "-u", "origin", name]);
}

fn squash_onto_main(repo: &Repo, name: &str) {
    repo.git(&["merge", "--squash", name]);
    let message = format!("squash {name}");
    repo.git(&["commit", "-m", &message]);
    if repo.root.join("origin.git").exists() {
        repo.git(&["push", "origin", "main"]);
        repo.git(&["push", "origin", "--delete", name]);
    }
}

fn clear_created_from(repo: &Repo, name: &str) {
    let file = repo.repo.join(".git/rabi/workspaces");
    let mut document: serde_json::Value =
        serde_json::from_str(&records::read_to_string(&file).expect("store")).expect("json");
    let records = document["worktrees"].as_array_mut().expect("records");
    let record = records
        .iter_mut()
        .find(|record| {
            record["path"].as_str().is_some_and(|path| {
                Path::new(path).file_name().and_then(|part| part.to_str()) == Some(name)
            })
        })
        .expect(name);
    record
        .as_object_mut()
        .expect("record")
        .remove("createdFrom");
    records::write(
        &file,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&document).expect("encode")
        ),
    )
    .expect("rewrite");
}

fn age_metadata(repo: &Repo, name: &str) {
    let file = repo.repo.join(".git/rabi/workspaces");
    let mut document: serde_json::Value =
        serde_json::from_str(&records::read_to_string(&file).expect("store")).expect("json");
    let records = document["worktrees"].as_array_mut().expect("records");
    let record = records
        .iter_mut()
        .find(|record| {
            record["path"].as_str().is_some_and(|path| {
                Path::new(path).file_name().and_then(|part| part.to_str()) == Some(name)
            })
        })
        .expect(name);
    record["createdAt"] = serde_json::json!("2000-01-01T00:00:00.000Z");
    records::write(
        &file,
        serde_json::to_string_pretty(&document).expect("encode"),
    )
    .expect("rewrite");
}

#[test]
fn gc_dry_run_keeps_a_merged_worktree_until_apply() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    land_branch(&repo, "feature");
    let plan = repo.rabi(&["gc"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let planned = text(&plan.stdout);
    assert!(
        planned.contains("would remove feature [merged]"),
        "{planned}"
    );
    assert!(planned.contains("Re-run with --apply"), "{planned}");
    assert!(worktree_dir(&repo, "feature").is_dir());
    assert!(branch_listed(&repo, "feature"));

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let removed = text(&applied.stdout);
    assert!(removed.contains("removed feature [merged]"), "{removed}");
    assert!(!worktree_dir(&repo, "feature").is_dir());
    assert!(!branch_listed(&repo, "feature"));
    assert!(!worktree_registered(&repo, "feature"));

    let again = repo.rabi(&["gc", "--apply"]);
    assert!(again.status.success(), "{}", text(&again.stderr));
    assert!(text(&again.stdout).contains("Nothing to clean up."));
}

#[test]
fn gc_json_reports_the_plan_and_rejects_a_bad_duration() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "feature", "--purpose", "grok"])
        .status
        .success());
    land_branch(&repo, "feature");
    let plan = repo.rabi(&["gc", "--json"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    assert!(text(&plan.stderr).is_empty(), "{}", text(&plan.stderr));
    let document = json_value(&plan);
    assert_eq!(document["schema"], 2);
    assert_eq!(document["data"]["applied"], false);
    assert!(document["error"].is_null());
    assert_eq!(document["data"]["items"][0]["name"], "feature");
    assert!(document["data"]["items"][0].get("owner").is_none());
    assert_eq!(
        document["data"]["items"][0]["reasons"],
        serde_json::json!(["merged"])
    );
    assert_eq!(document["data"]["items"][0]["action"], "wouldRemove");
    assert!(document["data"]["items"][0]["detail"].is_null());
    assert!(document["data"]["items"][0]["branchDeleted"].is_null());
    assert!(document["data"]["items"][0]["path"]
        .as_str()
        .unwrap()
        .ends_with("/feature"));

    let bad = repo.rabi(&["gc", "--older-than", "14", "--json"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(text(&bad.stderr).is_empty(), "{}", text(&bad.stderr));
    let error = json_value(&bad);
    assert_eq!(error["error"]["code"], "usage");
    assert!(error["error"].get("exitCode").is_none());
    assert!(worktree_dir(&repo, "feature").is_dir());
}

#[test]
fn gc_skips_dirty_unpushed_and_current_workspaces() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "dirty"]).status.success());
    land_branch(&repo, "dirty");
    fs::write(worktree_dir(&repo, "dirty").join("local.txt"), "x\n").expect("dirty");
    assert!(repo.rabi(&["create", "wip"]).status.success());
    commit_in_worktree(&repo, "wip");
    assert!(repo.rabi(&["create", "here"]).status.success());
    land_branch(&repo, "here");
    assert!(repo
        .rabi(&["create", "bob", "--purpose", "bob"])
        .status
        .success());
    commit_in_worktree(&repo, "bob");
    age_metadata(&repo, "bob");

    let plan = repo.rabi(&["gc"]);
    assert!(
        plan.status.success(),
        "{}\n{}",
        text(&plan.stdout),
        text(&plan.stderr)
    );
    let planned = text(&plan.stdout);
    assert!(planned.contains("skip dirty [merged] dirty"), "{planned}");
    assert!(planned.contains("would remove here [merged]"), "{planned}");
    assert!(!planned.contains("wip"), "{planned}");
    assert!(!planned.contains("bob"), "{planned}");
    assert!(worktree_dir(&repo, "dirty").is_dir());

    let aged = repo.rabi(&["gc", "--older-than", "14d"]);
    assert!(aged.status.success(), "{}", text(&aged.stderr));
    let aged_text = text(&aged.stdout);
    assert!(aged_text.contains("Nothing to clean up."), "{aged_text}");
    assert!(!aged_text.contains("would remove dirty"), "{aged_text}");

    let current = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["gc", "--apply"])
        .current_dir(worktree_dir(&repo, "here"))
        .output()
        .expect("gc");
    assert!(current.status.success(), "{}", text(&current.stderr));
    let current_text = text(&current.stdout);
    assert!(
        current_text.contains("skip here [merged] current workspace"),
        "{current_text}"
    );
    assert!(
        current_text.contains("skip dirty [merged] dirty"),
        "{current_text}"
    );
    assert!(worktree_dir(&repo, "here").is_dir());
    assert!(worktree_dir(&repo, "dirty").is_dir());

    let forced = repo.rabi(&["gc", "--apply"]);
    assert!(forced.status.success(), "{}", text(&forced.stderr));
    let forced_text = text(&forced.stdout);
    assert!(
        forced_text.contains("skip dirty [merged] dirty"),
        "{forced_text}"
    );
    assert!(
        forced_text.contains("removed here [merged]"),
        "{forced_text}"
    );
    assert!(worktree_dir(&repo, "dirty").is_dir());
    assert!(!worktree_dir(&repo, "here").is_dir());
    assert!(worktree_dir(&repo, "wip").is_dir());
    assert!(worktree_dir(&repo, "bob").is_dir());
}

#[test]
fn gc_drops_a_missing_directory_and_keeps_an_unpushed_branch() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "gone"]).status.success());
    land_branch(&repo, "gone");
    assert!(repo.rabi(&["create", "local"]).status.success());
    commit_in_worktree(&repo, "local");
    fs::remove_dir_all(worktree_dir(&repo, "gone")).expect("rm gone");
    fs::remove_dir_all(worktree_dir(&repo, "local")).expect("rm local");

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let text = text(&applied.stdout);
    assert!(text.contains("removed gone [merged, orphaned]"), "{text}");
    assert!(text.contains("removed local [orphaned]"), "{text}");
    assert!(!worktree_registered(&repo, "gone"));
    assert!(!worktree_registered(&repo, "local"));
    assert!(branch_listed(&repo, "gone"));
    assert!(branch_listed(&repo, "local"));
}

#[test]
fn gc_skips_missing_registration_with_changed_branch_or_head() {
    for change in ["branch", "head"] {
        let repo = Repo::new();
        assert!(repo.rabi(&["create", "a-trigger"]).status.success());
        land_branch(&repo, "a-trigger");
        assert!(repo.rabi(&["create", "z-local"]).status.success());
        commit_in_worktree(&repo, "z-local");
        repo.git(&["branch", "replacement", "main"]);
        let original = text(
            &repo
                .command("git", &["rev-parse", "z-local"])
                .output()
                .unwrap()
                .stdout,
        );
        fs::remove_dir_all(managed(&repo, "z-local")).unwrap();
        let command = if change == "branch" {
            format!(
                "printf 'ref: refs/heads/replacement\\n' > '{}'",
                repo.repo.join(".git/worktrees/z-local/HEAD").display()
            )
        } else {
            "git update-ref refs/heads/z-local main".to_string()
        };
        fs::write(
            repo.repo.join(".rabi.toml"),
            format!("[hooks]\npre-remove = [\"{command}\"]\n"),
        )
        .unwrap();
        let out = repo.rabi(&["gc", "--apply", "--json"]);
        let doc = json_value(&out);
        assert!(branch_listed(&repo, "z-local"), "{change}: {doc}");
        assert!(worktree_registered(&repo, "z-local"), "{change}: {doc}");
        let item = doc["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["name"] == "z-local")
            .unwrap();
        assert_eq!(item["action"], "skipped", "{change}: {doc}");
        assert!(item["detail"].as_str().unwrap().contains("changed"));
        assert!(out.status.success(), "{doc}");
        if change == "branch" {
            let after = repo
                .command("git", &["rev-parse", "z-local"])
                .output()
                .unwrap();
            assert_eq!(text(&after.stdout), original);
        }
    }
}

#[test]
fn gc_apply_caches_each_target_pull_request() {
    use std::os::unix::fs::PermissionsExt;
    for kind in ["live", "missing", "orphan"] {
        let repo = Repo::new();
        let count = 4;
        for number in 1..=count {
            let name = format!("topic-{number}");
            let url = format!("https://github.com/acme/demo/pull/{number}");
            let created = repo.create_recorded(&["create", &name, "--pr", &url]);
            assert!(created.status.success(), "{}", text(&created.stderr));
            if kind == "missing" {
                fs::remove_dir_all(managed(&repo, &name)).unwrap();
            } else if kind == "orphan" {
                repo.git(&[
                    "worktree",
                    "remove",
                    "--force",
                    managed(&repo, &name).to_str().unwrap(),
                ]);
            }
        }
        let bin = repo.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let gh = bin.join("gh");
        let calls = repo.root.join("gh-calls");
        fs::write(
            &gh,
            format!(
                r#"#!/bin/sh
test "$1 $2" = 'pr view' || exit 1
printf '%s\n' "$3" >> '{}'
printf '%s\n' '{{"state":"MERGED"}}'
"#,
                calls.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["gc", "--closed-pr", "--apply", "--apply", "--json"],
            )
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let doc = json_value(&out);
        let candidates = doc["data"]["items"].as_array().unwrap();
        assert_eq!(candidates.len(), count);
        assert!(candidates.iter().all(|item| item["action"] == "removed"));
        let calls = fs::read_to_string(calls).unwrap();
        assert_eq!(calls.lines().count(), count, "{kind}: {calls}");
        for number in 1..=count {
            let url = format!("https://github.com/acme/demo/pull/{number}");
            assert_eq!(calls.lines().filter(|line| *line == url).count(), 1);
        }
    }
}

#[test]
fn gc_rechecks_pull_request_link_after_pre_remove_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    assert!(repo
        .create_recorded(&[
            "create",
            "topic",
            "--pr",
            "https://github.com/acme/demo/pull/7"
        ])
        .status
        .success());
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let state = repo.root.join("pr-state");
    fs::write(&state, "{\"state\":\"MERGED\"}\n").unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, format!("#!/bin/sh\ncat '{}'\n", state.display())).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    // Remote responses are command snapshots; metadata and local Git state are
    // still rechecked after hooks, so removing the PR link revokes eligibility.
    let hook = format!(
        "'{}' set topic --unset pr",
        env!("CARGO_BIN_EXE_rabi").replace('\'', "'\"'\"'")
    );
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!(
            "[hooks]\npre-remove = [{}]\n",
            serde_json::to_string(&hook).unwrap()
        ),
    )
    .unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["gc", "--closed-pr", "--apply", "--json"],
        )
        .env("PATH", path)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let doc = json_value(&out);
    assert_eq!(doc["data"]["items"][0]["action"], "failed");
    assert!(doc["data"]["items"][0]["detail"]
        .as_str()
        .unwrap()
        .contains("no longer qualifies"));
    assert!(managed(&repo, "topic").exists());
    assert!(branch_listed(&repo, "topic"));
}

#[test]
fn gc_closed_pull_request_with_unpreserved_history_is_skipped() {
    let repo = Repo::new();
    assert!(repo
        .create_recorded(&[
            "create",
            "topic",
            "--pr",
            "https://github.com/acme/demo/pull/7",
        ])
        .status
        .success());
    commit_in_worktree(&repo, "topic");
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(&gh, "#!/bin/sh\nprintf '%s\\n' '{\"state\":\"MERGED\"}'\n").expect("gh");
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let plan = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["gc", "--closed-pr", "--json"])
        .env("PATH", &path)
        .output()
        .expect("gc");
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let document = json_value(&plan);
    assert_eq!(
        document["data"]["items"][0]["reasons"],
        serde_json::json!(["pullRequest"])
    );
    assert_eq!(document["data"]["items"][0]["action"], "skipped");
    assert!(document["data"]["items"][0]["detail"]
        .as_str()
        .unwrap()
        .contains(" remove --delete-unpreserved-branch -- 'topic'"));
    assert!(worktree_dir(&repo, "topic").is_dir());

    let forced = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["gc", "--closed-pr", "--apply", "--json"],
        )
        .env("PATH", &path)
        .output()
        .expect("force");
    assert!(forced.status.success(), "{}", text(&forced.stderr));
    let removed = json_value(&forced);
    assert_eq!(removed["data"]["applied"], true);
    assert_eq!(removed["data"]["items"][0]["action"], "skipped");
    assert!(removed["data"]["items"][0]["branchDeleted"].is_null());
    assert!(worktree_dir(&repo, "topic").is_dir());
    assert!(branch_listed(&repo, "topic"));
}

#[test]
fn gc_metadata_io_failure_after_removal_exits_partial_for_a_single_candidate() {
    use std::os::unix::fs::PermissionsExt;
    let real_git = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("git"))
        .find(|file| file.is_file())
        .expect("git");
    for missing in [false, true] {
        for json in [false, true] {
            let repo = Repo::new();
            assert!(repo.rabi(&["create", "feature"]).status.success());
            land_branch(&repo, "feature");
            if missing {
                fs::remove_dir_all(worktree_dir(&repo, "feature")).expect("missing checkout");
            }
            let bin = repo.root.join("bin");
            fs::create_dir(&bin).expect("bin");
            let wrapper = bin.join("git");
            // Let Git really remove the checkout/registration, then replace
            // the records directory with a file to cause a real I/O failure.
            fs::write(
                &wrapper,
                r#"#!/bin/sh
case " $* " in
  *" worktree remove "*)
    "$REAL_GIT" "$@" || exit $?
    mv "$METADATA_DIR" "$METADATA_DIR.saved" || exit $?
    printf 'not a directory' > "$METADATA_DIR"
    ;;
  *) exec "$REAL_GIT" "$@" ;;
esac
"#,
            )
            .expect("wrapper");
            fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("chmod");
            let mut paths = vec![bin];
            paths.extend(std::env::split_paths(
                &std::env::var_os("PATH").expect("PATH"),
            ));
            let mut args = vec![
                "gc",
                "--apply",
                if missing { "--orphaned" } else { "--merged" },
            ];
            if json {
                args.push("--json");
            }
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("PATH", std::env::join_paths(paths).expect("paths"))
                .env("REAL_GIT", &real_git)
                .env("METADATA_DIR", repo.repo.join(".git/rabi/workspaces"))
                .output()
                .expect("gc");
            assert_eq!(
                out.status.code(),
                Some(6),
                "{} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            assert!(!worktree_dir(&repo, "feature").exists());
            assert!(!worktree_registered(&repo, "feature"));
            if json {
                let value = assert_envelope(&out, "gc");
                assert_eq!(value["error"]["code"], "partial");
                let candidates = value["data"]["items"].as_array().expect("candidates");
                assert_eq!(candidates.len(), 1);
                assert_eq!(candidates[0]["action"], "failed");
                let detail = candidates[0]["detail"].as_str().expect("detail");
                assert!(
                    detail.contains("metadata") && detail.contains("not writable"),
                    "{detail}"
                );
            } else {
                assert!(text(&out.stdout).contains("Failed 1"));
            }
        }
    }
}

#[test]
fn gc_failure_before_removal_exits_failure() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    land_branch(&repo, "feature");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"exit 42\"]\n",
    )
    .expect("hook");
    let out = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(assert_envelope(&out, "gc")["error"]["code"], "failed");
    assert!(worktree_dir(&repo, "feature").is_dir());
    assert!(worktree_registered(&repo, "feature"));
}

#[test]
fn gc_runs_pre_delete_hooks_only_when_applying() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    land_branch(&repo, "feature");
    assert!(repo.rabi(&["create", "other"]).status.success());
    land_branch(&repo, "other");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"test \\\"$RABI_WORKSPACE\\\" != feature && touch \\\"$RABI_REPO_PATH/hooked\\\"\"]\n",
    )
    .expect("hooks");

    let plan = repo.rabi(&["gc"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    assert!(!repo.repo.join("hooked").exists());
    assert!(worktree_dir(&repo, "feature").is_dir());

    let applied = repo.rabi(&["gc", "--apply", "--json"]);
    assert_eq!(applied.status.code(), Some(6), "{}", text(&applied.stdout));
    assert!(
        text(&applied.stderr).is_empty()
            || text(&applied.stderr).contains("pre-remove")
            || text(&applied.stderr).contains("Running")
    );
    let document = json_value(&applied);
    assert_eq!(document["error"]["code"], "partial");
    assert!(document["error"].get("exitCode").is_none());
    let failed = document["data"]["items"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|entry| entry["name"] == "feature")
        .expect("feature");
    assert_eq!(failed["action"], "failed");
    let removed = document["data"]["items"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|entry| entry["name"] == "other")
        .expect("other");
    assert_eq!(removed["action"], "removed");
    assert!(worktree_dir(&repo, "feature").is_dir());
    assert!(!worktree_dir(&repo, "other").is_dir());
    assert!(repo.repo.join("hooked").exists());
}

fn gh_path(repo: &Repo, body: &str) -> String {
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(&gh, body).expect("gh");
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[test]
fn gc_apply_removes_a_merged_worktree_when_another_remote_branch_is_ahead() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    land_branch(&repo, "feature");
    let bare = repo.root.join("origin.git");
    repo.git(&["init", "--bare", bare.to_str().expect("bare")]);
    repo.git(&["remote", "add", "origin", bare.to_str().expect("bare")]);
    repo.git(&["push", "origin", "main"]);
    repo.git(&["checkout", "-b", "ahead"]);
    fs::write(repo.repo.join("ahead.txt"), "ahead\n").expect("ahead");
    repo.git(&["add", "ahead.txt"]);
    repo.git(&["commit", "-m", "ahead"]);
    repo.git(&["push", "origin", "ahead"]);
    repo.git(&["checkout", "main"]);

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("removed feature [merged]"), "{output}");
    assert!(!output.contains("unpushed"), "{output}");
    assert!(!worktree_dir(&repo, "feature").is_dir());
    assert!(!branch_listed(&repo, "feature"));
    assert!(branch_listed(&repo, "main"));
    assert!(branch_listed(&repo, "ahead"));
}

#[test]
fn gc_does_not_remove_a_worktree_checked_out_on_the_default_branch() {
    let repo = Repo::new();
    fs::write(repo.repo.join("second.txt"), "second\n").expect("second");
    repo.git(&["add", "second.txt"]);
    repo.git(&["commit", "-m", "second"]);
    let bare = repo.root.join("origin.git");
    repo.git(&["init", "--bare", bare.to_str().expect("bare")]);
    repo.git(&["remote", "add", "origin", bare.to_str().expect("bare")]);
    repo.git(&["push", "origin", "main"]);
    repo.git(&["remote", "set-head", "origin", "main"]);
    repo.git(&["checkout", "-b", "dev"]);
    let attached = repo.rabi(&["create", "--branch", "main"]);
    assert!(
        attached.status.success(),
        "{}\n{}",
        text(&attached.stdout),
        text(&attached.stderr)
    );

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(
        output.contains("skip main [merged] default branch"),
        "{output}"
    );
    assert!(worktree_dir(&repo, "main").is_dir());
    assert!(branch_listed(&repo, "main"));
    assert!(branch_listed(&repo, "dev"));
}

#[test]
fn gc_keeps_a_fresh_worktree_when_the_branch_reflog_is_missing() {
    let repo = Repo::new();
    repo.git(&["config", "core.logAllRefUpdates", "false"]);
    let created = repo.rabi(&["create", "fresh", "--json"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let head = json_value(&created)["data"]["workspace"]["head"]
        .as_str()
        .expect("head")
        .to_string();
    assert!(
        !repo.repo.join(".git/logs/refs/heads/fresh").exists(),
        "the fresh branch reflog must be absent"
    );
    repo.git(&["commit", "--allow-empty", "-m", "advance"]);

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("Nothing to clean up."), "{output}");
    assert!(worktree_dir(&repo, "fresh").is_dir());
    assert!(branch_listed(&repo, "fresh"));
    assert_eq!(
        json_value(&created)["data"]["workspace"]["createdFrom"],
        head,
        "create records the base commit"
    );

    age_metadata(&repo, "fresh");
    let aged = repo.rabi(&["gc", "--older-than", "14d", "--apply"]);
    assert!(aged.status.success(), "{}", text(&aged.stderr));
    let aged_text = text(&aged.stdout);
    assert!(aged_text.contains("Nothing to clean up."), "{aged_text}");
    assert!(worktree_dir(&repo, "fresh").is_dir());
    assert!(branch_listed(&repo, "fresh"));
}

#[test]
fn gc_removes_a_merged_worktree_when_the_branch_reflog_is_missing() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    land_branch(&repo, "feature");
    let reflog = repo.repo.join(".git/logs/refs/heads/feature");
    assert!(reflog.is_file(), "feature reflog");
    fs::remove_file(&reflog).expect("remove reflog");

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("removed feature [merged]"), "{output}");
    assert!(!worktree_dir(&repo, "feature").is_dir());
    assert!(!branch_listed(&repo, "feature"));
}

#[test]
fn gc_removes_an_attached_branch_that_already_moved_past_its_creation_commit() {
    let repo = Repo::new();
    repo.git(&["checkout", "-b", "landed"]);
    fs::write(repo.repo.join("landed.txt"), "landed\n").expect("landed");
    repo.git(&["add", "landed.txt"]);
    repo.git(&["commit", "-m", "landed"]);
    repo.git(&["checkout", "main"]);
    repo.git(&["merge", "--ff-only", "landed"]);
    let attached = repo.rabi(&["create", "--branch", "landed"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("removed landed [merged]"), "{output}");
    assert!(!worktree_dir(&repo, "landed").is_dir());
    assert!(!branch_listed(&repo, "landed"));
}

#[test]
fn gc_removes_an_attached_merged_branch_when_its_reflog_is_missing() {
    let repo = Repo::new();
    repo.git(&["checkout", "-b", "landed"]);
    fs::write(repo.repo.join("landed.txt"), "landed\n").expect("landed");
    repo.git(&["add", "landed.txt"]);
    repo.git(&["commit", "-m", "landed"]);
    repo.git(&["checkout", "main"]);
    repo.git(&["merge", "--ff-only", "landed"]);
    let reflog = repo.repo.join(".git/logs/refs/heads/landed");
    assert!(reflog.is_file(), "landed reflog");
    fs::remove_file(&reflog).expect("remove reflog");
    let attached = repo.rabi(&["create", "--branch", "landed"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("removed landed [merged]"), "{output}");
    assert!(!worktree_dir(&repo, "landed").is_dir());
    assert!(!branch_listed(&repo, "landed"));
}

#[test]
fn gc_keeps_an_older_record_by_its_reflog_when_created_from_is_absent() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "fresh"]).status.success());
    clear_created_from(&repo, "fresh");

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("Nothing to clean up."), "{output}");
    assert!(worktree_dir(&repo, "fresh").is_dir());
    assert!(branch_listed(&repo, "fresh"));
}

#[test]
fn gc_keeps_a_worktree_still_at_its_creation_point() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "fresh"]).status.success());
    assert!(repo
        .rabi(&["create", "theirs", "--purpose", "other"])
        .status
        .success());

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let output = text(&applied.stdout);
    assert!(output.contains("Nothing to clean up."), "{output}");
    assert!(worktree_dir(&repo, "fresh").is_dir());
    assert!(worktree_dir(&repo, "theirs").is_dir());
    assert!(branch_listed(&repo, "fresh"));
    assert!(branch_listed(&repo, "theirs"));

    age_metadata(&repo, "fresh");
    let aged = repo.rabi(&["gc", "--older-than", "14d", "--apply"]);
    assert!(aged.status.success(), "{}", text(&aged.stderr));
    let aged_text = text(&aged.stdout);
    assert!(aged_text.contains("Nothing to clean up."), "{aged_text}");
    assert!(worktree_dir(&repo, "fresh").is_dir());
    assert!(worktree_dir(&repo, "theirs").is_dir());
    assert!(branch_listed(&repo, "theirs"));
}

#[test]
fn gc_views_pull_requests_by_url_and_reports_an_unconfirmed_number() {
    let repo = Repo::new();
    assert!(repo
        .create_recorded(&[
            "create",
            "alpha",
            "--pr",
            "https://github.com/acme/alpha/pull/7",
        ])
        .status
        .success());
    assert!(repo
        .create_recorded(&[
            "create",
            "beta",
            "--pr",
            "https://github.com/acme/beta/pull/7",
        ])
        .status
        .success());
    assert!(repo
        .create_recorded(&["create", "local", "--pr", "4"])
        .status
        .success());
    let log = repo.root.join("gh.log");
    let path = gh_path(
        &repo,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = repo ]; then\nexit 1\nfi\nprintf '%s\\n' '{{\"state\":\"MERGED\"}}'\nexit 0\n",
            log.display()
        ),
    );

    let applied = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["gc", "--closed-pr", "--apply", "--json"],
        )
        .env("PATH", &path)
        .output()
        .expect("gc");
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let document = json_value(&applied);
    let candidates = document["data"]["items"].as_array().expect("candidates");
    let alpha = candidates
        .iter()
        .find(|entry| entry["name"] == "alpha")
        .expect("alpha");
    assert_eq!(alpha["reasons"], serde_json::json!(["pullRequest"]));
    assert_eq!(alpha["action"], "removed");
    let beta = candidates
        .iter()
        .find(|entry| entry["name"] == "beta")
        .expect("beta");
    assert_eq!(beta["action"], "removed");
    let local = candidates
        .iter()
        .find(|entry| entry["name"] == "local")
        .expect("local");
    assert_eq!(local["action"], "skipped");
    assert_eq!(local["detail"], "pr status unavailable");
    assert!(worktree_dir(&repo, "local").is_dir());
    assert!(!worktree_dir(&repo, "alpha").is_dir());
    assert!(text(&applied.stderr).contains("pull request status unavailable for local"));
    let logged = fs::read_to_string(&log).expect("gh log");
    assert!(
        logged.contains("https://github.com/acme/alpha/pull/7"),
        "{logged}"
    );
    assert!(
        logged.contains("https://github.com/acme/beta/pull/7"),
        "{logged}"
    );
    assert!(!logged.contains("pr view 4"), "{logged}");
}

#[test]
fn removing_the_last_worktree_keeps_the_project_marker_for_the_next_create() {
    for gc in [false, true] {
        let repo = Repo::new();
        assert!(repo.rabi(&["create", "feature/last"]).status.success());
        let project = managed_root(&repo);
        let marker_path = project.join(rabi::paths::PROJECT_MARKER);
        let marker = fs::read(&marker_path).expect("marker");
        let removed = if gc {
            commit_file(&repo, "feature/last", "landed.txt", "landed\n");
            repo.git(&["merge", "--ff-only", "feature/last"]);
            repo.rabi(&["gc", "--apply", "--merged", "--json"])
        } else {
            repo.rabi(&["remove", "feature/last", "--json"])
        };
        assert!(
            removed.status.success(),
            "{} {}",
            text(&removed.stdout),
            text(&removed.stderr)
        );
        assert!(!managed(&repo, "feature/last").exists());
        assert!(
            !project.join("feature").exists(),
            "empty name parents are still removed"
        );
        assert!(project.is_dir());
        assert_eq!(fs::read(&marker_path).expect("retained marker"), marker);
        assert_eq!(fs::read_dir(&project).expect("project dir").count(), 1);
        let created = repo.rabi(&["create", "next", "--json"]);
        assert!(
            created.status.success(),
            "{} {}",
            text(&created.stdout),
            text(&created.stderr)
        );
        assert!(managed(&repo, "next").join("README.md").is_file());
        assert_eq!(fs::read(&marker_path).expect("reused marker"), marker);
    }
}

#[test]
fn deleting_the_last_worktree_between_directory_claim_and_git_add_keeps_the_marker() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "last"]).status.success());
    let marker_path = managed_root(&repo).join(rabi::paths::PROJECT_MARKER);
    let marker = fs::read(&marker_path).expect("marker");
    let real_git = std::env::split_paths(&std::env::var_os("PATH").expect("PATH"))
        .map(|dir| dir.join("git"))
        .find(|file| file.is_file())
        .expect("git");
    let bin = repo.root.join("bin");
    fs::create_dir(&bin).expect("bin");
    let wrapper = bin.join("git");
    // The outer create has claimed the project directory but has not added
    // its checkout yet. Finish deletion in that exact window, without sleeps.
    fs::write(
        &wrapper,
        r#"#!/bin/sh
case " $* " in
  *" worktree add "*)
    "$RABI_BIN" remove last --json > "$DELETE_RESULT" || exit $?
    ;;
esac
exec "$REAL_GIT" "$@"
"#,
    )
    .expect("wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("chmod");
    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").expect("PATH"),
    ));
    let delete_result = repo.root.join("delete.json");
    let created = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "replacement", "--json"],
        )
        .env("PATH", std::env::join_paths(paths).expect("paths"))
        .env("REAL_GIT", real_git)
        .env("RABI_BIN", env!("CARGO_BIN_EXE_rabi"))
        .env("DELETE_RESULT", &delete_result)
        .output()
        .expect("create");
    assert!(
        created.status.success(),
        "{} {}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let deleted: serde_json::Value =
        serde_json::from_slice(&fs::read(delete_result).expect("delete ran")).expect("delete json");
    assert_eq!(deleted["ok"], true);
    assert!(!managed(&repo, "last").exists());
    assert!(managed(&repo, "replacement").join("README.md").is_file());
    assert_eq!(fs::read(&marker_path).expect("retained marker"), marker);
    let next = repo.rabi(&["create", "next", "--json"]);
    assert!(
        next.status.success(),
        "{} {}",
        text(&next.stdout),
        text(&next.stderr)
    );
}

#[test]
fn new_worktrees_land_under_xdg_and_a_preference_still_wins() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature", "--json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let path = managed(&repo, "feature");
    assert!(path.join("README.md").is_file());
    assert!(paths_match(
        json_value(&created)["data"]["workspace"]["path"]
            .as_str()
            .unwrap_or(""),
        &path
    ));
    assert!(!repo.repo.join("custom/workspaces/feature").exists());
    assert!(repo.repo.join(".git/rabi/workspaces").is_dir());
    let common = rabi::git::canonical_common_dir(&repo.repo).expect("common dir");
    let marker =
        fs::read_to_string(managed_root(&repo).join(rabi::paths::PROJECT_MARKER)).expect("marker");
    assert_eq!(marker.trim_end(), common.to_string_lossy());

    let preference = repo.rabi(&["config", "set", "workspace-root", "../custom-wt"]);
    assert!(preference.status.success(), "{}", text(&preference.stderr));
    let custom = repo.rabi(&["create", "custom"]);
    assert!(
        custom.status.success(),
        "{}\n{}",
        text(&custom.stdout),
        text(&custom.stderr)
    );
    let custom_path = repo.root.join("custom-wt").join("custom");
    assert!(custom_path.join("README.md").is_file());
    assert!(!managed(&repo, "custom").exists());
    let removed = repo.rabi(&["remove", "custom"]);
    assert!(removed.status.success(), "{}", text(&removed.stderr));
    assert!(!custom_path.exists());
    assert!(
        custom_path.parent().expect("custom dir").is_dir(),
        "a preference directory stays when its last workspace is gone"
    );

    let unmanaged = repo.repo.join("custom/workspaces/unmanaged");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "unmanaged",
        unmanaged.to_str().expect("unmanaged"),
        "HEAD",
    ]);
    let names = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    assert!(names.status.success(), "{}", text(&names.stderr));
    let listed = text(&names.stdout);
    assert!(listed.lines().any(|line| line == "feature"), "{listed}");
    assert!(listed.lines().any(|line| line == "unmanaged"), "{listed}");
    let found = repo.rabi(&["path", "unmanaged"]);
    assert!(found.status.success(), "{}", text(&found.stderr));
    assert!(paths_match(text(&found.stdout).trim(), &unmanaged));
    let refused = repo.rabi(&["remove", "unmanaged", "--keep-branch"]);
    assert!(!refused.status.success(), "{}", text(&refused.stdout));
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused.stderr));
    let refused_text = text(&refused.stderr);
    assert!(
        refused_text.contains("not created by rabi"),
        "{refused_text}"
    );
    assert!(
        refused_text.contains("git worktree remove"),
        "{refused_text}"
    );
    assert!(unmanaged.is_dir());
}

#[test]
fn relative_xdg_data_home_uses_the_home_fallback() {
    let repo = Repo::new();
    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "feature"])
        .env("XDG_DATA_HOME", "relative/data")
        .output()
        .expect("create");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let data_home = rabi::paths::data_home(Some(&repo.home), Some(Path::new("relative/data")))
        .expect("fallback");
    assert_eq!(data_home, repo.home.join(".local/share"));
    assert!(managed_directory(&repo.repo, &data_home)
        .join("feature")
        .is_dir());
    assert!(!repo.root.join("relative/data").exists());
    assert!(!managed(&repo, "feature").exists());
}

#[test]
fn a_project_directory_owned_by_another_repository_is_refused() {
    let repo = Repo::new();
    let directory = managed_root(&repo);
    fs::create_dir_all(&directory).expect("project dir");
    fs::write(
        directory.join(rabi::paths::PROJECT_MARKER),
        "/other/repo/.git\n",
    )
    .expect("foreign marker");
    let created = repo.rabi(&["create", "feature"]);
    assert!(!created.status.success(), "{}", text(&created.stdout));
    let message = format!("{}\n{}", text(&created.stdout), text(&created.stderr));
    assert!(message.contains("belongs to"), "{message}");
    assert!(message.contains("/other/repo/.git"), "{message}");
    assert!(!directory.join("feature").exists());
}

#[test]
fn two_clones_do_not_share_a_worktree_directory() {
    let first = Repo::new();
    let second = Repo::new();
    assert!(first.rabi(&["create", "feature"]).status.success());
    assert!(second.rabi(&["create", "feature"]).status.success());
    let left = managed_root(&first);
    let right = managed_root(&second);
    assert_ne!(left, right);
    assert!(left.join("feature").is_dir());
    assert!(right.join("feature").is_dir());
    assert!(left
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("repo-")));
}

#[test]
fn symlink_checkout_shares_the_project_directory() {
    let repo = Repo::new();
    let link = repo.root.join("link-name");
    std::os::unix::fs::symlink(&repo.repo, &link).expect("symlink");
    let created = repo.rabi(&["create", "from-real"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let via_link = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "create",
                "--repo",
                link.to_str().expect("link"),
                "from-link",
            ],
        )
        .output()
        .expect("create");
    assert!(
        via_link.status.success(),
        "{}\n{}",
        text(&via_link.stdout),
        text(&via_link.stderr)
    );
    let project = managed_root(&repo);
    assert!(project.join("from-real").is_dir());
    assert!(project.join("from-link").is_dir());
    let projects = fs::read_dir(project.parent().expect("ceiling"))
        .expect("ceiling")
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
        .count();
    assert_eq!(
        projects, 1,
        "symlink spelling created a second project directory"
    );
    let deleted = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "remove",
                "--repo",
                link.to_str().expect("link"),
                "from-real",
            ],
        )
        .output()
        .expect("remove");
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(!project.join("from-real").exists());
    assert!(project.join("from-link").is_dir());
}

#[test]
fn recorded_worktrees_outside_the_resolved_directory_can_be_deleted() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["config", "set", "workspace-root", "custom/workspaces",])
        .status
        .success());
    let created = repo.rabi(&["create", "feature/topic"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let topic = repo.repo.join("custom/workspaces/feature/topic");
    let parent = repo.repo.join("custom/workspaces/feature");
    assert!(topic.is_dir());
    assert!(repo
        .rabi(&["config", "unset", "workspace-root"])
        .status
        .success());
    let deleted = repo.rabi(&["remove", "feature/topic"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(!topic.exists());
    assert!(parent.is_dir(), "parents of a recorded workspace stay");
    assert!(repo.repo.join("custom/workspaces").is_dir());

    let other = repo.root.join("other-xdg");
    let elsewhere_created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "elsewhere"])
        .env("XDG_DATA_HOME", &other)
        .output()
        .expect("elsewhere");
    assert!(
        elsewhere_created.status.success(),
        "{}\n{}",
        text(&elsewhere_created.stdout),
        text(&elsewhere_created.stderr)
    );
    let elsewhere = managed_directory(&repo.repo, &other).join("elsewhere");
    assert!(elsewhere.is_dir());
    fs::write(elsewhere.join("dirty.txt"), "dirty\n").expect("dirty");
    let blocked = repo.rabi(&["remove", "elsewhere"]);
    assert!(!blocked.status.success(), "{}", text(&blocked.stdout));
    assert!(
        text(&blocked.stderr).contains("uncommitted"),
        "{}",
        text(&blocked.stderr)
    );
    assert!(elsewhere.is_dir());
    fs::remove_file(elsewhere.join("dirty.txt")).expect("clean");
    let removed = repo.rabi(&["remove", "elsewhere"]);
    assert!(
        removed.status.success(),
        "{}\n{}",
        text(&removed.stdout),
        text(&removed.stderr)
    );
    assert!(!elsewhere.exists());
    assert!(
        elsewhere.parent().expect("project dir").is_dir(),
        "the other data directory's project dir stays"
    );

    let stray = repo.root.join("stray");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "stray",
        stray.to_str().expect("stray"),
        "HEAD",
    ]);
    fs::write(stray.join("stray.txt"), "stray\n").expect("stray note");
    for args in [vec!["add", "stray.txt"], vec!["commit", "-m", "stray"]] {
        let output = repo
            .command("git", &args)
            .current_dir(&stray)
            .output()
            .expect("commit stray");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    repo.git(&["merge", "--ff-only", "stray"]);
    let refused = repo.rabi(&["remove", "stray"]);
    assert!(!refused.status.success(), "{}", text(&refused.stdout));
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused.stderr));
    let refused_text = text(&refused.stderr);
    assert!(
        refused_text.contains("not created by rabi"),
        "{refused_text}"
    );
    assert!(
        refused_text.contains(&format!("git worktree remove {}", stray.display())),
        "{refused_text}"
    );
    let cleaned = repo.rabi(&["gc", "--apply"]);
    assert!(cleaned.status.success(), "{}", text(&cleaned.stderr));
    assert!(
        !text(&cleaned.stdout).contains("stray"),
        "{}",
        text(&cleaned.stdout)
    );
    assert!(stray.is_dir());
}

#[test]
fn gc_removes_a_merged_recorded_worktree_outside_the_resolved_directory() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["config", "set", "workspace-root", "custom/workspaces",])
        .status
        .success());
    let created = repo.rabi(&["create", "landed"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let landed = repo.repo.join("custom/workspaces/landed");
    fs::write(landed.join("landed.txt"), "landed\n").expect("note");
    for args in [vec!["add", "landed.txt"], vec!["commit", "-m", "landed"]] {
        let output = repo
            .command("git", &args)
            .current_dir(&landed)
            .output()
            .expect("commit");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    repo.git(&["merge", "--ff-only", "landed"]);
    assert!(repo
        .rabi(&["config", "unset", "workspace-root"])
        .status
        .success());
    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(
        applied.status.success(),
        "{}\n{}",
        text(&applied.stdout),
        text(&applied.stderr)
    );
    let stdout = text(&applied.stdout);
    assert!(
        stdout.contains("landed") && stdout.contains("Removed"),
        "{stdout}"
    );
    assert!(!landed.exists());
    assert!(repo.repo.join("custom/workspaces").is_dir());
}

#[test]
fn recorded_worktrees_outside_the_directory_keep_metadata_names() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["config", "set", "workspace-root", "custom/workspaces",])
        .status
        .success());
    let detached = repo.rabi(&["create", "next", "--detach"]);
    assert!(
        detached.status.success(),
        "{}\n{}",
        text(&detached.stdout),
        text(&detached.stderr)
    );
    let nested = repo.rabi(&["create", "feature/topic", "--detach"]);
    assert!(
        nested.status.success(),
        "{}\n{}",
        text(&nested.stdout),
        text(&nested.stderr)
    );
    let branched = repo.rabi(&["create", "kept"]);
    assert!(
        branched.status.success(),
        "{}\n{}",
        text(&branched.stdout),
        text(&branched.stderr)
    );
    repo.git(&["branch", "-m", "kept", "moved"]);
    assert!(repo
        .rabi(&["config", "unset", "workspace-root"])
        .status
        .success());

    let next = repo.repo.join("custom/workspaces/next");
    let topic = repo.repo.join("custom/workspaces/feature/topic");
    let kept = repo.repo.join("custom/workspaces/kept");
    // Give gc evidence of landed work rather than treating a fresh detached
    // checkout as merged. Its stored name must survive this HEAD change.
    fs::write(next.join("landed.txt"), "reviewed\n").unwrap();
    repo.git(&["-C", next.to_str().unwrap(), "add", "landed.txt"]);
    repo.git(&["-C", next.to_str().unwrap(), "commit", "-m", "reviewed"]);
    let head = repo
        .command("git", &["rev-parse", "--short=7", "HEAD"])
        .current_dir(&next)
        .output()
        .expect("short head");
    assert!(head.status.success(), "{}", text(&head.stderr));
    let sha = text(&head.stdout);
    repo.git(&["merge", "--ff-only", &sha]);

    let names = repo.rabi(&["list", "--names", "--kind", "workspace"]);
    assert!(names.status.success(), "{}", text(&names.stderr));
    let listed = text(&names.stdout);
    assert!(listed.lines().any(|line| line == "next"), "{listed}");
    assert!(
        listed.lines().any(|line| line == "feature/topic"),
        "{listed}"
    );
    assert!(listed.lines().any(|line| line == "kept"), "{listed}");
    assert!(!listed.lines().any(|line| line == sha), "{listed}");
    assert!(!listed.lines().any(|line| line == "moved"), "{listed}");

    let found = repo.rabi(&["path", "next"]);
    assert!(found.status.success(), "{}", text(&found.stderr));
    assert!(paths_match(text(&found.stdout).trim(), &next));
    let found_topic = repo.rabi(&["path", "feature/topic"]);
    assert!(
        found_topic.status.success(),
        "{}",
        text(&found_topic.stderr)
    );
    assert!(paths_match(text(&found_topic.stdout).trim(), &topic));
    let found_kept = repo.rabi(&["path", "kept"]);
    assert!(found_kept.status.success(), "{}", text(&found_kept.stderr));
    assert!(paths_match(text(&found_kept.stdout).trim(), &kept));

    let ran = repo.rabi(&["exec", "next", "--", "true"]);
    assert!(ran.status.success(), "{}", text(&ran.stderr));
    let marked = repo.rabi(&["set", "feature/topic", "--purpose", "review"]);
    assert!(marked.status.success(), "{}", text(&marked.stderr));
    let listed = text(
        &repo
            .rabi(&["list", "--names", "--kind", "workspace"])
            .stdout,
    );
    assert!(listed.lines().any(|name| name == "feature/topic"));
    assert!(text(&repo.rabi(&["list"]).stdout).contains("review"));

    let stray = repo.root.join("stray-detached");
    repo.git(&[
        "worktree",
        "add",
        "--detach",
        stray.to_str().expect("stray"),
        "HEAD",
    ]);
    let stray_head = repo
        .command("git", &["rev-parse", "--short=7", "HEAD"])
        .current_dir(&stray)
        .output()
        .expect("stray head");
    let stray_sha = text(&stray_head.stdout);
    let after_stray = text(
        &repo
            .rabi(&["list", "--names", "--kind", "workspace"])
            .stdout,
    );
    assert!(
        after_stray.lines().any(|line| line == stray_sha),
        "{after_stray}"
    );
    assert!(
        !after_stray.lines().any(|line| line == "stray-detached"),
        "{after_stray}"
    );

    let deleted = repo.rabi(&["remove", "kept", "--keep-branch"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(!kept.exists());
    let moved = repo
        .command("git", &["branch", "--list", "moved"])
        .output()
        .expect("branch");
    assert!(
        text(&moved.stdout).contains("moved"),
        "{}",
        text(&moved.stdout)
    );

    let file = repo.repo.join(".git/rabi/workspaces");
    let mut document: serde_json::Value =
        serde_json::from_str(&records::read_to_string(&file).expect("store")).expect("store json");
    for record in document["worktrees"].as_array_mut().expect("records") {
        if record["path"]
            .as_str()
            .unwrap_or("")
            .ends_with("/workspaces/next")
        {
            record["createdAt"] = serde_json::json!("2000-01-01T00:00:00.000Z");
        }
    }
    records::write(
        &file,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&document).expect("encode")
        ),
    )
    .expect("rewrite");
    let planned = repo.rabi(&["gc", "--older-than", "1d", "--json"]);
    assert!(planned.status.success(), "{}", text(&planned.stderr));
    let candidates = json_value(&planned)["data"]["items"]
        .as_array()
        .expect("candidates")
        .clone();
    let next_row = candidates
        .iter()
        .find(|entry| entry["name"] == "next")
        .expect("gc names the detached workspace next");
    assert_eq!(next_row["action"], "wouldRemove");
    let applied = repo.rabi(&["gc", "--older-than", "1d", "--apply", "--json"]);
    assert!(
        applied.status.success(),
        "{}\n{}",
        text(&applied.stdout),
        text(&applied.stderr)
    );
    let removed = json_value(&applied)["data"]["items"]
        .as_array()
        .expect("candidates")
        .iter()
        .find(|entry| entry["name"] == "next")
        .expect("removed next")
        .clone();
    assert_eq!(removed["action"], "removed");
    assert!(!next.exists());
}

#[test]
fn delete_requests_the_same_worktree_once() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "a"]).status.success());
    assert!(repo.rabi(&["create", "b"]).status.success());
    let deleted = repo.rabi(&["remove", "a", "b", "a"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    let stdout = text(&deleted.stdout);
    assert_eq!(
        stdout.matches("Deleted workspace 'a'").count(),
        1,
        "{stdout}"
    );
    assert_eq!(
        stdout.matches("Deleted workspace 'b'").count(),
        1,
        "{stdout}"
    );
    assert!(
        !text(&deleted.stderr).contains("not a working tree"),
        "{}",
        text(&deleted.stderr)
    );
    assert!(!managed(&repo, "a").exists());
    assert!(!managed(&repo, "b").exists());

    assert!(repo.rabi(&["create", "alias"]).status.success());
    repo.git(&["branch", "-m", "alias", "topic"]);
    let once = repo.rabi(&["remove", "alias", "topic"]);
    assert!(
        once.status.success(),
        "{}\n{}",
        text(&once.stdout),
        text(&once.stderr)
    );
    let once_stdout = text(&once.stdout);
    assert_eq!(
        once_stdout.matches("Deleted workspace").count(),
        1,
        "{once_stdout}"
    );
    assert!(
        once_stdout.contains("Deleted workspace 'alias'"),
        "{once_stdout}"
    );
    assert!(!managed(&repo, "alias").exists());
    assert!(
        !text(&once.stderr).contains("not a working tree"),
        "{}",
        text(&once.stderr)
    );
}

#[test]
fn list_paths_are_absolute_from_an_unrelated_directory() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "inside"]).status.success());
    let deep = repo.home.join("a/b/c");
    fs::create_dir_all(&deep).unwrap();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "list",
                "--repo",
                repo.repo.to_str().unwrap(),
                "--kind",
                "workspace",
                "--paths",
            ],
        )
        .current_dir(&deep)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        out.stdout,
        format!("{}\n", managed(&repo, "inside").display()).as_bytes()
    );
}

#[test]
fn symlink_spelling_keeps_metadata_and_preserves_the_project_directory() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "feature", "--purpose", "alice", "--json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let created_json = json_value(&created);
    let real = PathBuf::from(
        created_json["data"]["workspace"]["path"]
            .as_str()
            .expect("path"),
    );
    let created_from = created_json["data"]["workspace"]["createdFrom"]
        .as_str()
        .expect("createdFrom")
        .to_string();
    let created_at = created_json["data"]["workspace"]["createdAt"]
        .as_str()
        .expect("createdAt")
        .to_string();
    fs::write(real.join("landed.txt"), "landed\n").expect("note");
    for args in [vec!["add", "landed.txt"], vec!["commit", "-m", "landed"]] {
        let output = repo
            .command("git", &args)
            .current_dir(&real)
            .output()
            .expect("commit");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    repo.git(&["merge", "--ff-only", "feature"]);

    let via = repo.root.join("via-project");
    std::os::unix::fs::symlink(real.parent().expect("repo"), &via).expect("symlink");
    let spelled = via.join("feature");
    let gitdir = repo.repo.join(".git/worktrees/feature/gitdir");
    fs::write(&gitdir, format!("{}\n", spelled.join(".git").display())).expect("gitdir");

    let listed = repo.rabi(&["list", "--json", "--kind", "workspace"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let entry = &json_value(&listed)["data"]["workspaces"][0];
    assert_eq!(entry["purpose"], "alice");
    assert_eq!(entry["createdFrom"], created_from);
    assert_eq!(entry["createdAt"], created_at);
    let plan = repo.rabi(&["gc", "--json"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let candidate = &json_value(&plan)["data"]["items"][0];
    assert!(candidate.get("owner").is_none());
    assert_eq!(candidate["name"], "feature");
    let deleted = repo.rabi(&["remove", "feature", "--json"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    let removed = &json_value(&deleted)["data"]["items"][0]["workspace"];
    assert_eq!(removed["purpose"], "alice");
    assert_eq!(removed["createdFrom"], created_from);
    assert_eq!(removed["createdAt"], created_at);
    assert!(!real.exists());
    assert!(
        managed_root(&repo)
            .join(rabi::paths::PROJECT_MARKER)
            .is_file(),
        "last workspace deletion should keep the project directory and marker"
    );
    let store = records::read_to_string(repo.repo.join(".git/rabi/workspaces")).unwrap_or_default();
    assert!(!store.contains("alice"), "{store}");
}

#[test]
fn symlinked_xdg_data_home_create_attach_and_exec_succeed() {
    let repo = Repo::new();
    let real = repo.root.join("real-xdg");
    let link = repo.root.join("link-xdg");
    fs::create_dir_all(&real).expect("real xdg");
    std::os::unix::fs::symlink(&real, &link).expect("xdg link");
    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("XDG_DATA_HOME", &link)
            .output()
            .expect("rabi")
    };
    let created = run(&["create", "a", "--json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let created_json = json_value(&created);
    assert!(created_json["error"].is_null(), "{created_json}");
    assert_eq!(created_json["data"]["workspace"]["name"], "a");
    assert!(created_json["data"]["metadata"]["ok"]
        .as_bool()
        .unwrap_or(false));
    let json_path = created_json["data"]["workspace"]["path"]
        .as_str()
        .expect("path");
    let canonical = fs::canonicalize(Path::new(json_path)).expect("canonical");
    assert_eq!(Path::new(json_path), canonical);
    let message = text(&created.stderr);
    assert!(
        message.contains(json_path),
        "human message and JSON should use the same path\n{message}\n{json_path}"
    );
    assert!(!message.contains(link.to_str().expect("link")), "{message}");

    repo.git(&["branch", "attached"]);
    let attached = run(&["create", "--branch", "attached", "--json"]);
    assert!(
        attached.status.success(),
        "{}\n{}",
        text(&attached.stdout),
        text(&attached.stderr)
    );
    let attached_json = json_value(&attached);
    assert!(attached_json["error"].is_null(), "{attached_json}");
    assert_eq!(attached_json["data"]["workspace"]["name"], "attached");

    let executed = run(&["create", "b", "--json"]);
    assert!(
        executed.status.success(),
        "{}\n{}",
        text(&executed.stdout),
        text(&executed.stderr)
    );
    let executed_json = json_value(&executed);
    assert!(executed_json["error"].is_null(), "{executed_json}");
    assert!(run(&["exec", "b", "--", "true"]).status.success());
    assert_eq!(executed_json["data"]["workspace"]["name"], "b");
}

#[test]
fn delete_current_recognizes_a_worktree_created_through_a_symlink() {
    let repo = Repo::new();
    let real = repo.root.join("real-xdg");
    let link = repo.root.join("link-xdg");
    fs::create_dir_all(&real).expect("real xdg");
    std::os::unix::fs::symlink(&real, &link).expect("xdg link");
    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "feature"])
        .env("XDG_DATA_HOME", &link)
        .output()
        .expect("create");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    let project = managed_directory(&repo.repo, &real);
    let via_link = link
        .join("rabi")
        .join("workspaces")
        .join(project.file_name().expect("project name"))
        .join("feature");
    let via_real = project.join("feature");
    assert!(via_link.is_dir());
    let canonical = via_link.canonicalize().expect("real");
    assert_ne!(canonical, via_link, "enter through the symlink spelling");
    let git_bin = text(
        &repo
            .command("sh", &["-c", "command -v git"])
            .output()
            .expect("locate git")
            .stdout,
    );
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let wrapper = bin.join("git");
    fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"rev-parse\" ] && [ \"$2\" = \"--show-toplevel\" ]; then\n  real=$({git_bin} rev-parse --show-toplevel) || exit $?\n  if [ \"$real\" = \"$REAL_TOP\" ]; then\n    printf '%s\\n' \"$LOGICAL_TOP\"\n    exit 0\n  fi\n  printf '%s\\n' \"$real\"\n  exit 0\nfi\nexec {git_bin} \"$@\"\n"
        ),
    )
    .expect("wrapper");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&wrapper).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&wrapper, perms).expect("chmod");
    let path_env = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let with_logical_top = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("XDG_DATA_HOME", &link)
            .env("PATH", &path_env)
            .env("REAL_TOP", &canonical)
            .env("LOGICAL_TOP", &via_link)
            .current_dir(&via_link)
            .output()
            .expect("rabi")
    };
    let deleted = with_logical_top(&["remove", "."]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(
        text(&deleted.stdout).contains("Deleted workspace 'feature'"),
        "{}",
        text(&deleted.stdout)
    );
    assert!(!via_real.exists());
}

#[test]
fn recheckout_through_a_symlink_keeps_intent_and_creation_details() {
    let repo = Repo::new();
    // Bare links must belong to the repository returned by the gh fixture.
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/demo.git",
    ]);
    let created = repo.rabi(&["create", "issues/7", "--purpose", "keep me", "--json"]);
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(repo
        .rabi(&["set", "issues/7", "--issue", "7"])
        .status
        .success());
    let created_json = json_value(&created);
    let real = PathBuf::from(
        created_json["data"]["workspace"]["path"]
            .as_str()
            .expect("path"),
    );
    let created_at = created_json["data"]["workspace"]["createdAt"]
        .as_str()
        .expect("createdAt")
        .to_string();
    let created_from = created_json["data"]["workspace"]["createdFrom"]
        .as_str()
        .expect("createdFrom")
        .to_string();
    let via = repo.root.join("via-project");
    std::os::unix::fs::symlink(real.parent().expect("repo"), &via).expect("symlink");
    let git_dir = repo
        .command("git", &["rev-parse", "--git-dir"])
        .current_dir(&real)
        .output()
        .expect("git dir");
    assert!(git_dir.status.success(), "{}", text(&git_dir.stderr));
    let printed = PathBuf::from(text(&git_dir.stdout).trim());
    let git_admin = if printed.is_absolute() {
        printed
    } else {
        real.join(printed)
    };
    let gitdir = git_admin.join("gitdir");
    fs::write(
        &gitdir,
        format!("{}\n", via.join("7").join(".git").display()),
    )
    .expect("gitdir");
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nif [ \"$1\" = \"repo\" ]; then\n  printf '%s\\n' '{\"url\":\"https://github.com/acme/demo\",\"owner\":{\"login\":\"acme\"},\"name\":\"demo\"}'\n  exit 0\nfi\nprintf '%s\\n' '{\"number\":7}'\nexit 0\n",
    )
    .expect("gh");
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(&gh).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&gh, perms).expect("chmod");
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let again = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "create",
                "--issue",
                "7",
                "--reuse",
                "--purpose",
                "replace",
                "--json",
            ],
        )
        .env("PATH", &path)
        .output()
        .expect("checkout");
    assert!(
        again.status.success(),
        "{}\n{}",
        text(&again.stdout),
        text(&again.stderr)
    );
    let again_json = json_value(&again);
    assert!(again_json["error"].is_null(), "{again_json}");
    assert_eq!(again_json["data"]["source"]["reused"], true);
    let worktree = &again_json["data"]["workspace"];

    assert_eq!(worktree["purpose"], "keep me");
    assert_eq!(worktree["createdAt"], created_at);
    assert_eq!(worktree["createdFrom"], created_from);
    assert!(worktree.get("lastUsedAt").is_none());
    assert_eq!(worktree["links"]["issue"]["number"], 7);
}

fn empty_commit(repo: &Repo, dir: &Path, message: &str) {
    let output = repo
        .command("git", &["commit", "--allow-empty", "-m", message])
        .current_dir(dir)
        .output()
        .expect("commit");
    assert!(
        output.status.success(),
        "git commit: {}",
        text(&output.stderr)
    );
}

#[test]
fn delete_refuses_a_branch_with_unpushed_commits() {
    let repo = Repo::new();
    let help = repo.rabi(&["remove", "--help"]);
    assert!(help.status.success(), "{}", text(&help.stderr));
    let help_text = text(&help.stdout);
    assert!(help_text.contains("unpreserved"), "{help_text}");
    assert!(repo.rabi(&["create", "u"]).status.success());
    let worktree = managed(&repo, "u");
    empty_commit(&repo, &worktree, "x");
    let out = repo.rabi(&["remove", "u", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let document = json_value(&out);
    assert_eq!(
        document["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert_eq!(document["warnings"][0]["code"], "branch_kept_unpreserved");
    assert!(!worktree.exists());
    assert!(branch_listed(&repo, "u"));
    assert!(repo.rabi(&["create", "--branch", "u"]).status.success());

    assert!(repo.rabi(&["create", "kept"]).status.success());
    let kept_dir = managed(&repo, "kept");
    empty_commit(&repo, &kept_dir, "keep");
    let kept = repo.rabi(&["remove", "kept", "--keep-branch"]);
    assert!(
        kept.status.success(),
        "{}\n{}",
        text(&kept.stdout),
        text(&kept.stderr)
    );
    assert!(text(&kept.stdout).contains("kept its branch 'kept'"));
    assert!(!kept_dir.exists());
    assert!(branch_listed(&repo, "kept"));

    let forced = repo.rabi(&[
        "remove",
        "u",
        "--discard-changes",
        "--delete-unpreserved-branch",
    ]);
    assert!(
        forced.status.success(),
        "{}\n{}",
        text(&forced.stdout),
        text(&forced.stderr)
    );
    assert!(text(&forced.stdout).contains("and its branch 'u'"));
    assert!(!worktree.exists());
    assert!(!branch_listed(&repo, "u"));

    let bare = repo.root.join("origin.git");
    repo.git(&["init", "--bare", bare.to_str().expect("bare")]);
    repo.git(&["remote", "add", "origin", bare.to_str().expect("origin")]);
    repo.git(&["push", "-u", "origin", "main"]);

    assert!(repo.rabi(&["create", "shipped"]).status.success());
    let shipped = managed(&repo, "shipped");
    empty_commit(&repo, &shipped, "ship");
    repo.git(&["push", "-u", "origin", "shipped"]);
    let removed = repo.rabi(&["remove", "shipped"]);
    assert!(
        removed.status.success(),
        "{}\n{}",
        text(&removed.stdout),
        text(&removed.stderr)
    );
    assert!(!shipped.exists());
    assert!(!branch_listed(&repo, "shipped"));

    assert!(repo.rabi(&["create", "behind"]).status.success());
    let behind = managed(&repo, "behind");
    empty_commit(&repo, &behind, "first");
    repo.git(&["push", "-u", "origin", "behind"]);
    empty_commit(&repo, &behind, "second");
    let behind_kept = repo.rabi(&["remove", "behind", "--json"]);
    assert_eq!(behind_kept.status.code(), Some(0));
    assert_eq!(
        json_value(&behind_kept)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert!(!behind.exists());
    assert!(branch_listed(&repo, "behind"));

    assert!(repo.rabi(&["create", "landed"]).status.success());
    commit_in_worktree(&repo, "landed");
    repo.git(&["merge", "--ff-only", "landed"]);
    repo.git(&["push", "origin", "main"]);
    let landed = repo.rabi(&["remove", "landed"]);
    assert!(
        landed.status.success(),
        "{}\n{}",
        text(&landed.stdout),
        text(&landed.stderr)
    );
    assert!(!managed(&repo, "landed").exists());
    assert!(!branch_listed(&repo, "landed"));
}

#[test]
fn delete_refuses_when_unpushed_commits_cannot_be_checked() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "mystery"]).status.success());
    repo.git(&["branch", "-m", "main", "trunk"]);
    let out = repo.rabi(&["remove", "mystery", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        json_value(&out)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert!(branch_listed(&repo, "mystery"));
    assert!(!managed(&repo, "mystery").exists());
}

#[test]
fn gc_and_delete_accept_a_squash_merged_branch_whose_remote_is_gone() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    commit_in_worktree(&repo, "feature");
    commit_file(&repo, "feature", "more.txt", "more\n");
    publish_branch(&repo, "feature");
    squash_onto_main(&repo, "feature");

    let plan = repo.rabi(&["gc"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let planned = text(&plan.stdout);
    assert!(
        planned.contains("would remove feature [merged]"),
        "{planned}"
    );
    assert!(worktree_dir(&repo, "feature").is_dir());
    assert!(branch_listed(&repo, "feature"));

    let deleted = repo.rabi(&["remove", "feature"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(
        text(&deleted.stdout).contains("and its branch 'feature'"),
        "{}",
        text(&deleted.stdout)
    );
    assert!(!worktree_dir(&repo, "feature").exists());
    assert!(!branch_listed(&repo, "feature"));
}

#[test]
fn delete_refuses_a_squash_merged_branch_with_a_later_commit() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    commit_in_worktree(&repo, "feature");
    publish_branch(&repo, "feature");
    squash_onto_main(&repo, "feature");
    commit_file(&repo, "feature", "later.txt", "later\n");

    let plan = repo.rabi(&["gc"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let planned = text(&plan.stdout);
    assert!(!planned.contains("would remove feature"), "{planned}");
    assert!(worktree_dir(&repo, "feature").is_dir());
    assert!(branch_listed(&repo, "feature"));
    let removed = repo.rabi(&["remove", "feature", "--json"]);
    assert_eq!(removed.status.code(), Some(0));
    assert_eq!(
        json_value(&removed)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert!(!worktree_dir(&repo, "feature").exists());
    assert!(branch_listed(&repo, "feature"));
}

#[test]
fn gc_and_delete_accept_a_rebase_merged_branch() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature"]).status.success());
    commit_in_worktree(&repo, "feature");
    commit_file(&repo, "feature", "more.txt", "more\n");
    publish_branch(&repo, "feature");
    repo.git(&["cherry-pick", "feature~1"]);
    repo.git(&["cherry-pick", "feature"]);
    repo.git(&["push", "origin", "main"]);
    repo.git(&["push", "origin", "--delete", "feature"]);

    let plan = repo.rabi(&["gc"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let planned = text(&plan.stdout);
    assert!(
        planned.contains("would remove feature [merged]"),
        "{planned}"
    );

    let deleted = repo.rabi(&["remove", "feature"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(!worktree_dir(&repo, "feature").exists());
    assert!(!branch_listed(&repo, "feature"));
}

#[test]
fn gc_applies_a_squash_merged_branch_with_no_remote_and_keeps_a_local_one() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "local"]).status.success());
    commit_in_worktree(&repo, "local");
    assert!(repo.rabi(&["create", "squashed"]).status.success());
    commit_in_worktree(&repo, "squashed");
    squash_onto_main(&repo, "squashed");

    let applied = repo.rabi(&["gc", "--apply"]);
    assert!(applied.status.success(), "{}", text(&applied.stderr));
    let removed = text(&applied.stdout);
    assert!(removed.contains("removed squashed [merged]"), "{removed}");
    assert!(!removed.contains("local"), "{removed}");
    assert!(!worktree_dir(&repo, "squashed").exists());
    assert!(!branch_listed(&repo, "squashed"));
    assert!(worktree_dir(&repo, "local").is_dir());
    assert!(branch_listed(&repo, "local"));
}

#[test]
fn delete_refuses_the_worktree_that_contains_the_current_directory() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "inside"]).status.success());
    let path = managed(&repo, "inside");
    fs::create_dir(path.join("sub")).unwrap();
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["remove", ".", "--json"])
        .current_dir(path.join("sub"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(!path.exists());
    assert!(!branch_listed(&repo, "inside"));
}

#[test]
fn delete_and_gc_keep_a_branch_shadowed_by_a_tag() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "v1"]).status.success());
    empty_commit(&repo, &managed(&repo, "v1"), "only on the branch");
    let commit = text(
        &repo
            .command("git", &["rev-parse", "refs/heads/v1"])
            .output()
            .unwrap()
            .stdout,
    );
    repo.git(&["tag", "v1", "main"]);
    let gc = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert_eq!(json_value(&gc)["data"]["items"], serde_json::json!([]));
    assert!(managed(&repo, "v1").exists());
    let out = repo.rabi(&["remove", "v1", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        json_value(&out)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert_eq!(
        text(
            &repo
                .command("git", &["rev-parse", "refs/heads/v1"])
                .output()
                .unwrap()
                .stdout
        ),
        commit
    );
}

#[test]
fn delete_refuses_a_detached_worktree_until_force() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "dt", "--detach"]).status.success());
    let path = managed(&repo, "dt");
    empty_commit(&repo, &path, "unpreserved detached commit");
    for flags in [vec![], vec!["--keep-branch"], vec!["--discard-changes"]] {
        let mut args = vec!["remove", "dt", "--json"];
        args.extend(flags);
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(4));
        assert_eq!(json_value(&out)["error"]["code"], "unpreserved");
        assert!(path.exists());
    }
    let out = repo.rabi(&["remove", "dt", "--delete-unpreserved-branch", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_value(&out)["data"]["items"][0]["branchDeleted"], false);
    assert!(!path.exists());
}

#[test]
fn delete_checks_every_name_before_removing_any() {
    let repo = Repo::new();
    let marker = repo.root.join("hook-ran");
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\npre-remove = [\"touch {}\"]\n", marker.display()),
    )
    .unwrap();
    for name in ["a", "b"] {
        assert!(repo.rabi(&["create", name]).status.success());
    }
    fs::write(managed(&repo, "b/dirty"), "changes").unwrap();
    let out = repo.rabi(&["remove", "a", "b", "--json"]);
    assert_eq!(out.status.code(), Some(4));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(doc["data"]["items"][1]["error"]["code"], "dirty");
    assert!(managed(&repo, "a").is_dir());
    assert!(managed(&repo, "b").is_dir());
    assert!(!marker.exists());
    let out = repo.rabi(&["remove", "a", "bad..name", "--json"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(managed(&repo, "a").is_dir());
    assert!(!marker.exists());
}

#[test]
fn gc_keeps_the_current_worktree_when_its_parent_was_replaced_by_a_symlink() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "here"]).status.success());
    land_branch(&repo, "here");
    assert!(repo.rabi(&["create", "gone"]).status.success());
    land_branch(&repo, "gone");
    let parent = managed_root(&repo);
    let moved = repo.root.join("moved-parent");
    fs::rename(&parent, &moved).expect("move parent");
    std::os::unix::fs::symlink(&moved, &parent).expect("symlink parent");
    let here = parent.join("here");
    assert!(here.is_dir());
    let applied = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["gc", "--apply"])
        .current_dir(&here)
        .output()
        .expect("gc");
    assert!(
        applied.status.success(),
        "{}\n{}",
        text(&applied.stdout),
        text(&applied.stderr)
    );
    let stdout = text(&applied.stdout);
    assert!(
        stdout.contains("skip here [merged] current workspace"),
        "{stdout}"
    );
    assert!(stdout.contains("removed gone [merged]"), "{stdout}");
    assert!(
        moved.join("here").is_dir(),
        "gc removed the workspace that contains the current directory"
    );
    assert!(
        parent.join("here").is_dir(),
        "gc removed the workspace reached through the symlink"
    );
    assert!(moved.is_dir(), "gc removed the current workspace's parent");
    assert!(
        parent
            .symlink_metadata()
            .expect("parent")
            .file_type()
            .is_symlink(),
        "gc removed the symlink that replaced the parent directory"
    );
    assert!(!moved.join("gone").exists());
}

#[test]
fn meta_set_records_an_outside_worktree_by_branch_instead_of_a_colliding_basename() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "dup"]).status.success());
    let outside = repo.root.join("x").join("dup");
    fs::create_dir_all(outside.parent().expect("parent")).expect("x");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "zz",
        outside.to_str().expect("outside"),
        "HEAD",
    ]);
    let marked = repo.rabi(&["set", "zz", "--purpose", "a"]);
    assert!(
        marked.status.success(),
        "{}\n{}",
        text(&marked.stdout),
        text(&marked.stderr)
    );
    assert!(
        text(&marked.stdout).contains("zz"),
        "{}",
        text(&marked.stdout)
    );
    let found = repo.rabi(&["path", "dup"]);
    assert!(
        found.status.success(),
        "{}\n{}",
        text(&found.stdout),
        text(&found.stderr)
    );
    assert!(paths_match(
        text(&found.stdout).trim(),
        &managed(&repo, "dup")
    ));
    let outside_where = repo.rabi(&["path", "zz"]);
    assert!(
        outside_where.status.success(),
        "{}\n{}",
        text(&outside_where.stdout),
        text(&outside_where.stderr)
    );
    assert!(paths_match(text(&outside_where.stdout).trim(), &outside));
    let stored = records::read_to_string(repo.repo.join(".git/rabi/workspaces")).expect("meta");
    assert!(stored.contains("\"name\": \"zz\""), "{stored}");
    let names = text(
        &repo
            .rabi(&["list", "--names", "--kind", "workspace"])
            .stdout,
    );
    assert!(names.lines().any(|line| line == "dup"), "{names}");
    assert!(names.lines().any(|line| line == "zz"), "{names}");
    assert_eq!(
        names.lines().filter(|line| *line == "dup").count(),
        1,
        "{names}"
    );

    let loose = repo.root.join("x").join("loose");
    repo.git(&[
        "worktree",
        "add",
        "--detach",
        loose.to_str().expect("loose"),
        "HEAD",
    ]);
    assert!(repo.rabi(&["create", "loose"]).status.success());
    let head = repo
        .command("git", &["rev-parse", "--short=7", "HEAD"])
        .current_dir(&loose)
        .output()
        .expect("short head");
    assert!(head.status.success(), "{}", text(&head.stderr));
    let sha = text(&head.stdout);
    let detached = repo.rabi(&["set", &sha, "--purpose", "b"]);
    assert!(
        detached.status.success(),
        "{}\n{}",
        text(&detached.stdout),
        text(&detached.stderr)
    );
    let found_loose = repo.rabi(&["path", "loose"]);
    assert!(
        found_loose.status.success(),
        "{}",
        text(&found_loose.stderr)
    );
    assert!(paths_match(
        text(&found_loose.stdout).trim(),
        &managed(&repo, "loose")
    ));
    let found_sha = repo.rabi(&["path", &sha]);
    assert!(found_sha.status.success(), "{}", text(&found_sha.stderr));
    assert!(paths_match(text(&found_sha.stdout).trim(), &loose));
    let stored = records::read_to_string(repo.repo.join(".git/rabi/workspaces")).expect("meta");
    assert!(stored.contains(&format!("\"name\": \"{sha}\"")), "{stored}");
    fs::write(loose.join("later.txt"), "later\n").expect("later");
    for args in [vec!["add", "later.txt"], vec!["commit", "-m", "later"]] {
        let output = repo
            .command("git", &args)
            .current_dir(&loose)
            .output()
            .expect("commit");
        assert!(output.status.success(), "{}", text(&output.stderr));
    }
    let later = repo
        .command("git", &["rev-parse", "--short=7", "HEAD"])
        .current_dir(&loose)
        .output()
        .expect("later head");
    assert!(later.status.success(), "{}", text(&later.stderr));
    let later_sha = text(&later.stdout);
    assert_ne!(later_sha, sha);
    let names_after = text(
        &repo
            .rabi(&["list", "--names", "--kind", "workspace"])
            .stdout,
    );
    assert!(names_after.lines().any(|line| line == sha), "{names_after}");
    assert!(
        !names_after.lines().any(|line| line == later_sha),
        "{names_after}"
    );
    let still = repo.rabi(&["path", &sha]);
    assert!(still.status.success(), "{}", text(&still.stderr));
    assert!(paths_match(text(&still.stdout).trim(), &loose));
}

#[test]
fn meta_set_records_outside_worktrees_despite_orphaned_names() {
    for layout in [false, true] {
        let repo = Repo::new();
        let records_path = repo.repo.join(".git/rabi/workspaces");
        let missing = repo.root.join("missing");
        fs::create_dir_all(records_path.parent().expect("parent")).expect("directory");
        let records = serde_json::json!({
                        "worktrees": [{"schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()), "path": missing, "name": "shared"}]
        });
        records::write(&records_path, serde_json::to_vec(&records).expect("json"))
            .expect("records");
        let outside = if layout {
            repo.repo.join("custom/workspaces/shared")
        } else {
            repo.root.join("outside")
        };
        fs::create_dir_all(outside.parent().expect("parent")).expect("directory");
        repo.git(&[
            "worktree",
            "add",
            "-b",
            "shared",
            outside.to_str().expect("path"),
            "HEAD",
        ]);
        let marked = repo.rabi(&["set", "shared", "--purpose", "a"]);
        assert!(marked.status.success(), "{}", text(&marked.stderr));
        let found = repo.rabi(&["path", "shared"]);
        assert!(found.status.success(), "{}", text(&found.stderr));
        assert!(paths_match(text(&found.stdout).trim(), &outside));
        let listed = repo.rabi(&["list", "--json", "--kind", "workspace"]);
        assert!(listed.status.success(), "{}", text(&listed.stderr));
        let document: serde_json::Value = serde_json::from_slice(&listed.stdout).expect("json");
        let worktrees = document["data"]["workspaces"]
            .as_array()
            .expect("worktrees");
        assert_eq!(worktrees.len(), 2);
        assert!(worktrees
            .iter()
            .all(|worktree| worktree["name"] == "shared"));
        assert_eq!(
            worktrees
                .iter()
                .filter(|worktree| worktree["orphaned"] == true)
                .count(),
            1
        );
        let cleaned = repo.rabi(&["gc", "--apply"]);
        assert!(cleaned.status.success(), "{}", text(&cleaned.stderr));
        let records: serde_json::Value =
            serde_json::from_slice(&records::read(&records_path).expect("records")).expect("json");
        assert!(!records["worktrees"]
            .as_array()
            .expect("records")
            .iter()
            .any(|record| record["path"] == missing.to_str().expect("path")));
    }
}

#[test]
fn meta_set_refuses_to_record_an_outside_worktree_under_a_taken_name() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "dup"]).status.success());
    let outside = repo.root.join("rabi/workspaces/dup");
    fs::create_dir_all(outside.parent().expect("parent")).expect("parent");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "zz",
        outside.to_str().expect("outside"),
        "HEAD",
    ]);
    let meta_path = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&meta_path).expect("before");
    let marked = repo.rabi(&["set", "zz", "--purpose", "a"]);
    assert_eq!(
        marked.status.code(),
        Some(1),
        "{}\n{}",
        text(&marked.stdout),
        text(&marked.stderr)
    );
    let message = format!("{}{}", text(&marked.stdout), text(&marked.stderr));
    assert!(
        message.contains("dup") && message.contains("already used"),
        "{message}"
    );
    let stored = records::read(&meta_path).expect("after");
    assert_eq!(stored, before, "a refused set rewrote records");
    let found = repo.rabi(&["path", "dup"]);
    assert!(
        found.status.success(),
        "{}\n{}",
        text(&found.stdout),
        text(&found.stderr)
    );
    assert!(paths_match(
        text(&found.stdout).trim(),
        &managed(&repo, "dup")
    ));
}

#[test]
fn a_record_without_a_stored_name_keeps_the_folder_name() {
    let repo = Repo::new();
    let next = repo.repo.join("outside/workspaces/next");
    fs::create_dir_all(next.parent().expect("parent")).expect("parent");
    repo.git(&[
        "worktree",
        "add",
        "--detach",
        next.to_str().expect("next"),
        "HEAD",
    ]);
    let head = repo
        .command("git", &["rev-parse", "--short=7", "HEAD"])
        .current_dir(&next)
        .output()
        .expect("head");
    assert!(head.status.success(), "{}", text(&head.stderr));
    let sha = text(&head.stdout);
    let meta = repo.repo.join(".git/rabi/workspaces");
    fs::create_dir_all(meta.parent().expect("meta dir")).expect("meta dir");
    let fixture = serde_json::json!({"worktrees": [{
        "schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()),
        "path": next, "createdAt": "2000-01-01T00:00:00.000Z"
    }]});
    records::write(&meta, serde_json::to_vec(&fixture).unwrap()).expect("record");
    let names = text(
        &repo
            .rabi(&["list", "--names", "--kind", "workspace"])
            .stdout,
    );
    assert!(names.lines().any(|line| line == "next"), "{names}");
    assert!(!names.lines().any(|line| line == sha), "{names}");
    // Records without creation evidence cannot prove integration. Exercise gc's name display via
    // orphan detection, retaining the checkout for the remaining selector tests.
    let parked = repo.root.join("parked-next");
    fs::rename(&next, &parked).unwrap();
    let plan = repo.rabi(&["gc", "--older-than", "14d"]);
    assert!(plan.status.success(), "{}", text(&plan.stderr));
    let planned = text(&plan.stdout);
    assert!(planned.contains("would remove next"), "{planned}");
    assert!(!planned.contains(&sha), "{planned}");
    fs::rename(&parked, &next).unwrap();
    let found = repo.rabi(&["path", "next"]);
    assert!(found.status.success(), "{}", text(&found.stderr));
    assert!(paths_match(text(&found.stdout).trim(), &next));
    let ran = repo.rabi(&["exec", "next", "--", "true"]);
    assert!(ran.status.success(), "{}", text(&ran.stderr));
    let marked = repo.rabi(&["set", "next", "--purpose", "ada"]);
    assert!(marked.status.success(), "{}", text(&marked.stderr));
    assert!(
        text(&marked.stdout).contains("next"),
        "{}",
        text(&marked.stdout)
    );
    let stored = records::read_to_string(&meta).expect("after set");
    assert!(!stored.contains("\"name\""), "{stored}");
    let still = repo.rabi(&["path", "next"]);
    assert!(still.status.success(), "{}", text(&still.stderr));
    assert!(paths_match(text(&still.stdout).trim(), &next));
    let deleted = repo.rabi(&["remove", "next"]);
    assert!(
        deleted.status.success(),
        "{}\n{}",
        text(&deleted.stdout),
        text(&deleted.stderr)
    );
    assert!(
        text(&deleted.stdout).contains("next"),
        "{}",
        text(&deleted.stdout)
    );
    assert!(!next.exists());
}

fn publish_tracking_branch(repo: &Repo, remote: &str, branch: &str) {
    let bare = repo.root.join(format!("{remote}.git"));
    if !bare.exists() {
        let path = bare.to_str().expect("bare");
        let init = repo
            .command("git", &["init", "--bare", "-b", "main", path])
            .output()
            .expect("bare");
        assert!(init.status.success(), "{}", text(&init.stderr));
        repo.git(&["remote", "add", remote, path]);
        repo.git(&["push", remote, "main"]);
    }
    repo.git(&["checkout", "main"]);
    if local_branch(repo, branch).is_empty() {
        repo.git(&["branch", branch, "main"]);
    }
    repo.git(&["push", remote, branch]);
    repo.git(&["checkout", "main"]);
    if !local_branch(repo, branch).is_empty() {
        repo.git(&["branch", "-D", branch]);
    }
}

fn upstream_name(repo: &Repo, branch: &str) -> String {
    let output = repo
        .command(
            "git",
            &[
                "rev-parse",
                "--abbrev-ref",
                &format!("{branch}@{{upstream}}"),
            ],
        )
        .output()
        .expect("upstream");
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

#[test]
fn create_refuses_a_remote_tracking_name_and_attach_tracks_it() {
    let repo = Repo::new();
    repo.git(&["config", "branch.autoSetupMerge", "false"]);
    publish_tracking_branch(&repo, "origin", "feature/teammate");
    repo.git(&[
        "remote",
        "set-url",
        "origin",
        repo.root.join("missing.git").to_str().expect("missing"),
    ]);

    let created = repo.rabi(&["create", "feature/teammate"]);
    assert_eq!(created.status.code(), Some(4), "{}", text(&created.stderr));
    assert_eq!(
        text(&created.stderr),
        "Branch 'feature/teammate' already exists as a remote-tracking branch (origin). Run `rabi create --branch feature/teammate`."
    );
    let remote_json = repo.rabi(&["create", "feature/teammate", "--json"]);
    assert_eq!(remote_json.status.code(), Some(4));
    assert_eq!(json_value(&remote_json)["error"]["code"], "exists_remote");
    assert!(local_branch(&repo, "feature/teammate").is_empty());

    let attached = repo.rabi(&["create", "--branch", "feature/teammate"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    assert!(text(&attached.stdout).contains("Created workspace 'feature/teammate'"));
    assert_eq!(
        upstream_name(&repo, "feature/teammate"),
        "origin/feature/teammate"
    );
    assert!(managed(&repo, "feature/teammate")
        .join("README.md")
        .is_file());

    for command in ["create", "create --branch"] {
        let exists_json = repo.rabi(
            &command
                .split_whitespace()
                .chain(["feature/teammate", "--json"])
                .collect::<Vec<_>>(),
        );
        assert_eq!(exists_json.status.code(), Some(4));
        assert_eq!(json_value(&exists_json)["error"]["code"], "exists");
    }
    let again = repo.rabi(&["create", "--branch", "feature/teammate"]);
    assert_eq!(again.status.code(), Some(4));
    assert_eq!(
        text(&again.stderr),
        "Workspace 'feature/teammate' already exists"
    );
    let shadowed = repo.rabi(&["create", "feature/teammate"]);
    assert_eq!(shadowed.status.code(), Some(4));
    assert_eq!(
        text(&shadowed.stderr),
        "Workspace 'feature/teammate' already exists"
    );
}

#[test]
fn create_with_multiple_remotes_suggests_working_qualified_attach_commands() {
    for remote in ["origin", "upstream"] {
        let repo = Repo::new();
        repo.git(&["config", "branch.autoSetupMerge", "false"]);
        publish_tracking_branch(&repo, "origin", "feature/teammate");
        publish_tracking_branch(&repo, "upstream", "feature/teammate");

        let created = repo.rabi(&["create", "feature/teammate"]);
        assert_eq!(created.status.code(), Some(4), "{}", text(&created.stderr));
        let message = text(&created.stderr);
        assert_eq!(
            message,
            "Branch 'feature/teammate' already exists as a remote-tracking branch (origin, upstream). Run `rabi create --branch origin/feature/teammate` or `rabi create --branch upstream/feature/teammate`."
        );
        assert!(local_branch(&repo, "feature/teammate").is_empty());
        assert!(!managed(&repo, "feature/teammate").exists());

        let qualified = format!("{remote}/feature/teammate");
        let command = message
            .split('`')
            .find(|command| *command == format!("rabi create --branch {qualified}"))
            .expect("suggested attach command");
        let args: Vec<_> = command.split_whitespace().skip(1).collect();
        let attached = repo.rabi(&args);
        assert!(attached.status.success(), "{}", text(&attached.stderr));
        assert!(text(&attached.stdout).contains("Created workspace 'feature/teammate'"));
        assert_eq!(upstream_name(&repo, "feature/teammate"), qualified);
        assert!(managed(&repo, "feature/teammate")
            .join("README.md")
            .is_file());
    }
}

#[test]
fn attach_qualified_remote_drops_the_remote_prefix() {
    let repo = Repo::new();
    repo.git(&["config", "branch.autoSetupMerge", "false"]);
    publish_tracking_branch(&repo, "origin", "feature/teammate");
    let attached = repo.rabi(&["create", "--branch", "origin/feature/teammate"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    assert!(text(&attached.stdout).contains("Created workspace 'feature/teammate'"));
    assert_eq!(
        upstream_name(&repo, "feature/teammate"),
        "origin/feature/teammate"
    );
    assert!(managed(&repo, "feature/teammate").is_dir());
    assert!(local_branch(&repo, "origin/feature/teammate").is_empty());
}

#[test]
fn attach_qualified_remote_allows_special_characters_in_the_remote_name() {
    let repo = Repo::new();
    repo.git(&["config", "branch.autoSetupMerge", "false"]);
    publish_tracking_branch(&repo, "origin", "feature");
    publish_tracking_branch(&repo, "user@host", "feature");

    let ambiguous = repo.rabi(&["create", "--branch", "feature"]);
    assert_eq!(ambiguous.status.code(), Some(2));
    assert!(text(&ambiguous.stderr).contains("origin, user@host"));

    let attached = repo.rabi(&["create", "--branch", "user@host/feature"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    assert!(text(&attached.stdout).contains("Created workspace 'feature'"));
    assert_eq!(upstream_name(&repo, "feature"), "user@host/feature");
    assert!(managed(&repo, "feature").join("README.md").is_file());
    assert!(local_branch(&repo, "user@host/feature").is_empty());

    let invalid = repo.rabi(&["create", "--branch", "invalid@name"]);
    assert_eq!(invalid.status.code(), Some(2), "{}", text(&invalid.stderr));
    assert!(text(&invalid.stderr).contains("Workspace name can only contain"));
}

#[test]
fn attach_lists_every_remote_that_has_the_name() {
    let repo = Repo::new();
    repo.git(&["config", "branch.autoSetupMerge", "false"]);
    publish_tracking_branch(&repo, "origin", "solo");
    publish_tracking_branch(&repo, "upstream", "solo");
    let attached = repo.rabi(&["create", "--branch", "solo"]);
    assert_eq!(
        attached.status.code(),
        Some(2),
        "{}",
        text(&attached.stderr)
    );
    assert_eq!(
        text(&attached.stderr),
        "Branch 'solo' exists on more than one remote (origin, upstream). Run `rabi create --branch <remote>/solo`."
    );
    assert!(local_branch(&repo, "solo").is_empty());
    assert!(!managed(&repo, "solo").exists());

    let chosen = repo.rabi(&["create", "--branch", "upstream/solo"]);
    assert!(chosen.status.success(), "{}", text(&chosen.stderr));
    assert_eq!(upstream_name(&repo, "solo"), "upstream/solo");
    assert!(managed(&repo, "solo").is_dir());
}

#[test]
fn attach_uses_the_local_branch_when_it_already_exists() {
    let repo = Repo::new();
    publish_tracking_branch(&repo, "origin", "feature/teammate");
    repo.git(&[
        "branch",
        "--no-track",
        "feature/teammate",
        "origin/feature/teammate",
    ]);
    let attached = repo.rabi(&["create", "--branch", "feature/teammate"]);
    assert!(attached.status.success(), "{}", text(&attached.stderr));
    let upstream = repo
        .command(
            "git",
            &["rev-parse", "--abbrev-ref", "feature/teammate@{upstream}"],
        )
        .output()
        .expect("upstream");
    assert!(
        !upstream.status.success(),
        "local branch should keep no upstream"
    );
}

#[test]
fn create_still_fails_for_a_local_branch_that_is_not_on_a_remote() {
    let repo = Repo::new();
    repo.git(&["branch", "lonely"]);
    let created = repo.rabi(&["create", "lonely"]);
    assert_eq!(created.status.code(), Some(4), "{}", text(&created.stderr));
    assert!(text(&created.stderr).contains("Branch 'lonely' already exists"));
    assert!(text(&created.stderr).contains("create --branch"));
}

#[test]
fn help_describes_remote_aware_attach_and_create() {
    let repo = Repo::new();
    let attach_help = repo.rabi(&["create", "--help"]);
    assert!(
        attach_help.status.success(),
        "{}",
        text(&attach_help.stderr)
    );
    let attach = text(&attach_help.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(attach.contains("rabi does not fetch"));
    assert!(attach.contains("create --branch exits 2 and lists those remotes"));
    assert!(!attach.contains("must already exist locally"));
    assert!(!attach.contains("first checkout the branch with git"));
    let create_help = repo.rabi(&["create", "--help"]);
    assert!(
        create_help.status.success(),
        "{}",
        text(&create_help.stderr)
    );
    let create = text(&create_help.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(create.contains("rabi create --branch <name>"));
    assert!(create.contains("refused with exit 4"));
    let meta = repo.rabi(&["set", "--help"]);
    assert!(meta.status.success());
    assert!(text(&meta.stdout).contains("A missing name exits 3"));
}

// Deterministic name candidates are only supported in debug builds.
#[cfg(debug_assertions)]
#[test]
fn nameless_create_skips_a_remote_tracking_candidate() {
    let repo = Repo::new();
    publish_tracking_branch(&repo, "origin", "bold-badger");
    let created = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create"])
        .env("RABI_NAME_CANDIDATES", "bold-badger,calm-comet")
        .output()
        .expect("rabi");
    assert!(
        created.status.success(),
        "{}\n{}",
        text(&created.stdout),
        text(&created.stderr)
    );
    assert!(text(&created.stdout).contains("Created workspace 'calm-comet'"));
    assert!(local_branch(&repo, "bold-badger").is_empty());
    assert!(local_branch(&repo, "calm-comet").contains("calm-comet"));
    assert!(managed(&repo, "calm-comet").is_dir());
}

#[cfg(debug_assertions)]
#[test]
fn review4_nameless_create_skips_recorded_workspace_names() {
    assert_generated_name_skips_adopted_alias(false);
}

#[cfg(debug_assertions)]
#[test]
fn review4_nameless_detach_skips_recorded_workspace_names() {
    assert_generated_name_skips_adopted_alias(true);
}

#[cfg(debug_assertions)]
fn assert_generated_name_skips_adopted_alias(detach: bool) {
    for (inside_root, state) in [
        (true, "live"),
        (false, "live"),
        (false, "prunable"),
        (false, "orphaned"),
    ] {
        let repo = Repo::new();
        let path = if inside_root {
            let seed = repo.rabi(&["create", "seed", "--json"]);
            assert!(seed.status.success(), "{}", text(&seed.stdout));
            managed(&repo, "directory-name")
        } else {
            repo.root.join("outside")
        };
        repo.git(&[
            "worktree",
            "add",
            "-b",
            "topic",
            path.to_str().unwrap(),
            "main",
        ]);
        let adopted = repo.rabi(&[
            "adopt",
            path.to_str().unwrap(),
            "--name",
            "calm-comet",
            "--json",
        ]);
        assert!(adopted.status.success(), "{}", text(&adopted.stdout));
        match state {
            "prunable" => fs::remove_dir_all(&path).expect("remove checkout"),
            "orphaned" => repo.git(&["worktree", "remove", path.to_str().unwrap()]),
            _ => {}
        }
        let records_before = rabi::metadata::load_strict(&repo.repo).expect("records");
        let args = if detach {
            vec!["create", "--detach", "--json"]
        } else {
            vec!["create", "--json"]
        };
        let created = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_NAME_CANDIDATES", "calm-comet,bold-badger")
            .output()
            .expect("rabi");
        assert!(
            created.status.success(),
            "inside_root={inside_root}, state={state}: {}\n{}",
            text(&created.stdout),
            text(&created.stderr)
        );
        let doc = assert_envelope(&created, "create");
        assert_eq!(doc["data"]["workspace"]["name"], "bold-badger");
        assert_eq!(doc["data"]["workspace"]["detached"], detach);
        assert_eq!(
            doc["data"]["source"]["kind"],
            if detach { "detach" } else { "new" }
        );
        assert!(managed(&repo, "bold-badger").is_dir());
        assert!(!managed(&repo, "calm-comet").exists());
        assert!(local_branch(&repo, "calm-comet").is_empty());
        let records_after = rabi::metadata::load_strict(&repo.repo).expect("records");
        for (path, record) in records_before {
            assert_eq!(records_after.get(&path), Some(&record));
        }
    }
}

fn assert_no_upstream(repo: &Repo, branch: &str) {
    let spec = format!("{branch}@{{upstream}}");
    let output = repo
        .command("git", &["rev-parse", "--abbrev-ref", &spec])
        .output()
        .expect("upstream");
    let stderr = text(&output.stderr);
    assert!(
        !output.status.success(),
        "{branch} tracks {}",
        text(&output.stdout)
    );
    assert!(
        stderr.contains("no upstream configured"),
        "{branch} upstream check: {stderr}"
    );
    assert!(managed(repo, branch).is_dir(), "{branch} workspace missing");
}

#[test]
fn create_sets_no_upstream_from_the_base() {
    let repo = Repo::new();
    let origin = repo.root.join("origin.git");
    repo.git(&[
        "init",
        "--bare",
        "-b",
        "next",
        origin.to_str().expect("origin path"),
    ]);
    repo.git(&["branch", "next"]);
    repo.git(&[
        "remote",
        "add",
        "origin",
        origin.to_str().expect("origin path"),
    ]);
    repo.git(&["push", "origin", "next"]);
    repo.git(&["branch", "local-base"]);
    // `always` makes a local base and HEAD track too. Default git already
    // tracks a remote-tracking base. Create must refuse both.
    repo.git(&["config", "branch.autoSetupMerge", "always"]);

    let remote = repo.rabi(&["create", "x", "--base", "origin/next"]);
    assert!(
        remote.status.success(),
        "{}\n{}",
        text(&remote.stdout),
        text(&remote.stderr)
    );
    assert_no_upstream(&repo, "x");

    let local = repo.rabi(&["create", "from-local", "--base", "local-base"]);
    assert!(
        local.status.success(),
        "{}\n{}",
        text(&local.stdout),
        text(&local.stderr)
    );
    assert_no_upstream(&repo, "from-local");

    let default_base = repo.rabi(&["create", "from-head"]);
    assert!(
        default_base.status.success(),
        "{}\n{}",
        text(&default_base.stdout),
        text(&default_base.stderr)
    );
    assert_no_upstream(&repo, "from-head");
}

#[test]
fn orphaned_records_are_listed_and_deleted_without_hooks_or_branch_changes() {
    let repo = Repo::new();
    for name in ["gone", "json-gone"] {
        let created = repo.rabi(&["create", name]);
        assert!(created.status.success(), "{}", text(&created.stderr));
        repo.git(&[
            "worktree",
            "remove",
            managed(&repo, name).to_str().expect("path"),
        ]);
    }
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"exit 99\"]\n",
    )
    .expect("hooks");
    let listed = repo.rabi(&["list"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    assert!(text(&listed.stdout)
        .lines()
        .any(|line| line.trim_start().starts_with("gone  ") && line.ends_with("[orphaned]")));
    let listed = repo.rabi(&["list", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stderr));
    let document = json_value(&listed);
    let entries = document["data"]["workspaces"]
        .as_array()
        .expect("worktrees");
    assert_eq!(
        entries
            .iter()
            .find(|item| item["name"] == "gone")
            .expect("gone")["orphaned"],
        true
    );
    assert_eq!(
        entries
            .iter()
            .find(|item| item["kind"] == "primary")
            .expect("default")["orphaned"],
        false
    );
    for command in ["path", "exec"] {
        let args = if command == "exec" {
            vec![command, "gone", "--", "pwd"]
        } else {
            vec![command, "gone"]
        };
        let output = repo.rabi(&args);
        assert!(!output.status.success());
        assert!(
            text(&output.stderr).contains("orphaned"),
            "{}",
            text(&output.stderr)
        );
    }
    let deleted = repo.rabi(&["remove", "gone"]);
    assert_eq!(deleted.status.code(), Some(0), "{}", text(&deleted.stderr));
    assert!(text(&deleted.stdout).contains("already gone; removed only the stale record"));
    assert!(branch_listed(&repo, "gone"));
    let deleted = repo.rabi(&["remove", "json-gone", "--json"]);
    assert_eq!(deleted.status.code(), Some(0), "{}", text(&deleted.stderr));
    let document = json_value(&deleted);
    assert_eq!(document["data"]["items"][0]["workspace"]["orphaned"], true);
    assert_eq!(document["data"]["items"][0]["branchDeleted"], false);
    assert!(branch_listed(&repo, "json-gone"));
    let records: serde_json::Value = serde_json::from_slice(
        &records::read(repo.repo.join(".git/rabi/workspaces")).expect("records"),
    )
    .expect("json");
    assert_eq!(records["worktrees"], serde_json::json!([]));
    assert_eq!(repo.rabi(&["remove", "gone"]).status.code(), Some(3));
}

#[test]
fn delete_prefers_a_live_worktree_over_an_orphaned_record_with_the_same_name() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "shared"]).status.success());
    let records_path = repo.repo.join(".git/rabi/workspaces");
    let mut records: serde_json::Value =
        serde_json::from_slice(&records::read(&records_path).expect("records")).expect("json");
    let missing = repo.root.join("missing");
    records["worktrees"]
        .as_array_mut()
        .expect("records")
        .push(serde_json::json!({"schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()), "path": missing, "name": "shared"}));
    records::write(&records_path, serde_json::to_vec(&records).expect("json")).expect("records");
    let deleted = repo.rabi(&["remove", "shared"]);
    assert_eq!(deleted.status.code(), Some(0), "{}", text(&deleted.stderr));
    assert!(!managed(&repo, "shared").exists());
    let records: serde_json::Value =
        serde_json::from_slice(&records::read(records_path).expect("records")).expect("json");
    assert_eq!(records["worktrees"].as_array().expect("records").len(), 1);
    assert_eq!(
        records["worktrees"][0]["path"],
        missing.to_str().expect("path")
    );
}

#[test]
fn ambiguous_orphaned_names_leave_all_records_untouched() {
    let repo = Repo::new();
    let records_path = repo.repo.join(".git/rabi/workspaces");
    fs::create_dir_all(records_path.parent().expect("parent")).expect("directory");
    let records = serde_json::json!({
                "worktrees": [
            {"schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()), "path": repo.root.join("missing-one"), "name": "shared"},
            {"schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()), "path": repo.root.join("missing-two"), "name": "shared"}
        ]
    });
    let original = serde_json::to_vec(&records).expect("json");
    records::write(&records_path, &original).expect("records");
    let before = records::read(&records_path).expect("snapshot");
    let deleted = repo.rabi(&["remove", "shared"]);
    assert!(!deleted.status.success());
    assert!(text(&deleted.stderr).contains("More than one workspace is named"));
    assert_eq!(records::read(records_path).expect("records"), before);
}

fn assert_workspace_vocabulary(value: &serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            assert!(
                !object.contains_key("worktree"),
                "legacy result key: {value}"
            );
            assert!(
                !object.contains_key("worktrees"),
                "legacy result key: {value}"
            );
            if object.contains_key("dirty") && object.contains_key("detached") {
                assert!(
                    matches!(value["kind"].as_str(), Some("primary" | "workspace")),
                    "{value}"
                );
                assert!(
                    !object.contains_key("default"),
                    "legacy workspace kind: {value}"
                );
            }
            for child in object.values() {
                assert_workspace_vocabulary(child);
            }
        }
        serde_json::Value::Array(items) => {
            for child in items {
                assert_workspace_vocabulary(child);
            }
        }
        _ => {}
    }
}

fn assert_envelope(output: &std::process::Output, command: &str) -> serde_json::Value {
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("one JSON document");
    let keys = value
        .as_object()
        .expect("envelope")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        ["command", "data", "error", "ok", "schema", "warnings"]
    );
    assert_workspace_vocabulary(&value["data"]);
    assert_eq!(value["schema"], 2);
    assert_eq!(value["command"], command);
    assert_eq!(value["ok"], output.status.success());
    assert!(value["data"].is_object() || value["data"].is_null());
    if command != "--version" {
        assert!(value["data"].get("version").is_none());
    }
    assert!(value["data"].get("schema").is_none());
    for warning in value["warnings"].as_array().expect("warnings") {
        assert!(warning["code"].is_string());
        assert!(warning["message"].is_string());
    }
    if output.status.success() {
        assert!(value["error"].is_null());
    } else {
        assert!(value["error"]["code"].is_string());
        assert!(value["error"]["message"].is_string());
    }
    value
}

#[test]
fn unresolved_usage_errors_honor_json_output_and_explicit_text() {
    let repo = Repo::new();
    for (args, mode, command) in [
        (vec!["frobnicate", "--json"], "text", "frobnicate"),
        (vec!["frobnicate", "--json"], "json", "frobnicate"),
        (vec!["frobnicate", "--output", "json"], "text", "frobnicate"),
        (vec!["frobnicate", "--output=json"], "text", "frobnicate"),
        (vec!["frobnicate"], "json", "frobnicate"),
        (vec!["frobnicate", "--help"], "json", "frobnicate"),
        (vec!["frobnicate", "--help", "--json"], "text", "frobnicate"),
        (vec!["--json", "frobnicate"], "text", "frobnicate"),
        (vec!["--json"], "text", ""),
        (vec!["--output", "json"], "text", ""),
        (vec!["--output=json"], "text", ""),
        (vec!["--no-input"], "json", ""),
        (vec!["--json", "--output"], "text", ""),
    ] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_OUTPUT", mode)
            .output()
            .expect("rabi");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stdout)
        );
        let value = assert_envelope(&out, command);
        assert_eq!(value["error"]["code"], "usage");
        assert!(value["data"].is_null());
    }
    for args in [
        vec!["frobnicate", "--output", "text"],
        vec!["--output", "text"],
    ] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_OUTPUT", "json")
            .output()
            .expect("rabi");
        assert_eq!(out.status.code(), Some(2));
        assert!(out.stdout.is_empty());
        assert!(text(&out.stderr).contains("USAGE"));
    }
    for args in [vec!["--help"], vec!["list", "--help"]] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_OUTPUT", "json")
            .output()
            .expect("rabi");
        assert!(out.status.success());
        assert!(serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err());
    }
}

#[test]
fn unsupported_listing_modes_preserve_usage_output() {
    let repo = Repo::new();
    for (command, flag) in [
        ("create", "--names"),
        ("create", "--paths"),
        ("repo add", "--names"),
        ("repo add", "--paths"),
        ("frobnicate", "--names"),
        ("frobnicate", "--paths"),
        ("list", "--names=value"),
        ("repo list", "--paths=value"),
    ] {
        for output_args in [vec![], vec!["--json"], vec!["--output", "json"]] {
            let mut args = command.split_whitespace().collect::<Vec<_>>();
            args.push(flag);
            args.extend(output_args);
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", "json")
                .output()
                .expect("rabi");
            assert_eq!(out.status.code(), Some(2), "{args:?}");
            let value = assert_envelope(&out, command);
            assert_eq!(value["error"]["code"], "usage");
            assert!(value["data"].is_null());
            assert!(value["error"]["message"]
                .as_str()
                .expect("message")
                .contains(if command == "frobnicate" {
                    "Unknown command"
                } else {
                    "Unknown option"
                }));
        }
        let args = command
            .split_whitespace()
            .chain([flag, "--output", "text"])
            .collect::<Vec<_>>();
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_OUTPUT", "json")
            .output()
            .expect("rabi");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err());
        assert!(!out.stderr.is_empty());
    }

    assert!(repo.rabi(&["repo", "add"]).status.success());
    for flag in ["--names", "--paths"] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "list", flag])
            .env("RABI_OUTPUT", "json")
            .output()
            .expect("rabi");
        assert!(out.status.success(), "{}", text(&out.stderr));
        if flag == "--names" {
            assert_eq!(text(&out.stdout), "repo");
        } else {
            assert!(same_dir(&text(&out.stdout), &repo.repo));
        }
    }
}

#[test]
fn every_json_command_uses_the_same_error_envelope() {
    let repo = Repo::new();
    for command in [
        "create",
        "create",
        "list",
        "show",
        "remove",
        "sync",
        "gc",
        "trust",
        "trust remove",
        "set",
        "clone",
        "repo add",
        "repo list",
        "repo forget",
        "create",
    ] {
        let mut args = command.split_whitespace().collect::<Vec<_>>();
        args.extend(["--json", "--unknown-option"]);
        let out = repo.rabi(&args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{command}: {}",
            text(&out.stdout)
        );
        let value = assert_envelope(&out, command);
        assert_eq!(value["error"]["code"], "usage");
        assert!(value["data"].is_null());
    }
}

#[test]
fn output_environment_and_explicit_modes_have_deterministic_precedence() {
    let repo = Repo::new();
    let run = |args: &[&str], mode: &str| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("RABI_OUTPUT", mode)
            .output()
            .expect("rabi")
    };
    assert_envelope(&run(&["list"], "json"), "list");
    let out = run(&["list", "--output", "text"], "json");
    assert!(out.status.success());
    assert!(text(&out.stdout).contains("main  main"));
    let out = run(&["list", "--names"], "json");
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "main");
    assert_envelope(&run(&["list", "--output=json"], "text"), "list");
    for args in [&["list"][..], &["list", "--output", "text"][..]] {
        assert_eq!(run(args, "invalid").status.code(), Some(2));
    }
    assert_eq!(run(&["--version"], "invalid").status.code(), Some(2));
    assert!(run(&["list", "--help"], "invalid").status.success());
    for flag in [vec!["--json"], vec!["--output", "json"]] {
        let mut args = vec!["exec"];
        args.extend(flag);
        args.extend(["main", "true"]);
        assert_eq!(run(&args, "json").status.code(), Some(2));
    }
    let out = run(&["exec", "main", "--", "printf", "%s", "--json"], "invalid");
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "--json");
}

#[test]
fn non_interactive_git_gets_prompt_control_and_hooks_preserve_user_stdin() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Stdio;
    let repo = Repo::new();
    let bin = repo.root.join("bin");
    fs::create_dir_all(&bin).expect("bin");
    let wrapper = bin.join("git");
    let marker = repo.root.join("git-network-env");
    fs::write(&wrapper, "#!/bin/sh\nfor arg do case \"$arg\" in fetch|clone|ls-remote|pull|push) printf '%s' \"$GIT_TERMINAL_PROMPT\" > \"$PROMPT_MARKER\"; test \"$GIT_TERMINAL_PROMPT\" = 0 || exit 93;; esac; done\nexec /usr/bin/git \"$@\"\n").expect("wrapper");
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).expect("chmod");
    let remote = repo.repo.to_str().expect("repo");
    repo.git(&["remote", "add", "origin", remote]);
    repo.git(&["fetch", "origin"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["sync", "--json"])
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("PROMPT_MARKER", &marker)
        .output()
        .expect("sync");
    assert!(
        out.status.success(),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&marker).expect("network git ran"), "0");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"if read ignored; then exit 94; fi; echo HOOK_EOF\"]\n",
    )
    .expect("hook");
    let mut child = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["create", "stdin", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rabi");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"sentinel\n")
        .expect("input");
    let out = child.wait_with_output().expect("result");
    assert!(
        out.status.success(),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(text(&out.stderr).contains("HOOK_EOF"));
    assert_envelope(&out, "create");
}

#[test]
fn successful_json_commands_share_the_envelope_and_preserve_payload_keys() {
    let repo = Repo::new();
    repo.git(&["branch", "existing"]);
    let cases: &[(&[&str], &str, &str)] = &[
        (&["create", "topic", "--json"], "create", "workspace"),
        (
            &["create", "--branch", "existing", "--json"],
            "create",
            "workspace",
        ),
        (&["list", "--json"], "list", "workspaces"),
        (&["show", "topic", "--json"], "show", "workspace"),
        (
            &["set", "topic", "--purpose", "contract test", "--json"],
            "set",
            "workspace",
        ),
        (&["sync", "--json"], "sync", "items"),
        (&["gc", "--json"], "gc", "items"),
        (&["trust", "add", "--json"], "trust add", "trustedAt"),
        (&["trust", "remove", "--json"], "trust remove", "removed"),
        (&["repo", "add", "--json"], "repo add", "repo"),
        (&["repo", "list", "--json"], "repo list", "repos"),
        (&["repo", "forget", "repo", "--json"], "repo forget", "repo"),
        (&["remove", "topic", "--json"], "remove", "items"),
    ];
    for (args, command, key) in cases {
        let out = repo.rabi(args);
        assert!(
            out.status.success(),
            "{command}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let value = assert_envelope(&out, command);
        assert!(value["data"].get(key).is_some(), "{command}: {value}");
        if *command == "gc" {
            assert_eq!(value["data"]["plan"], true);
        }
    }
    publish_pr_topic(&repo);
    let gh_path = fake_gh_for_topic_pr(&repo);
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &gh_path)
        .output()
        .expect("checkout");
    let value = assert_envelope(&out, "create");
    assert!(out.status.success(), "{value}");
    assert!(value["data"]["workspace"].is_object());
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &gh_path)
        .output()
        .expect("gh checkout");
    let value = assert_envelope(&out, "create");
    assert!(out.status.success(), "{value}");
    assert_eq!(value["data"]["workspace"]["kind"], "workspace");
    let bare = repo.root.join("origin.git");
    append_instead_of(&repo, &bare, "https://github.com/example/widget.git");
    let out = clone_widget(&repo, Some(&repo.root.join("clones")));
    let value = assert_envelope(&out, "clone");
    assert!(out.status.success(), "{value}");
    assert_eq!(value["data"]["cloned"], true);
}

#[test]
fn standalone_exec_preserves_stdin_for_gh_and_signal_statuses() {
    use std::io::Write;
    use std::process::Stdio;
    let repo = Repo::new();
    let path = gh_path(&repo, "#!/bin/sh\nread line; printf '%s' \"$line\"");
    let mut child = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["exec", "main", "--", "gh"])
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("exec");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"sentinel\n")
        .expect("input");
    let out = child.wait_with_output().expect("exec result");
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(text(&out.stdout), "sentinel");
    let out = repo.rabi(&["exec", "main", "--", "sh", "-c", "kill -TERM $$"]);
    assert_eq!(out.status.code(), Some(143));
}

#[test]
fn delete_partial_results_and_late_common_option_errors_keep_the_contract() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "first"]).status.success());
    assert!(repo.rabi(&["create", "second"]).status.success());
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"if [ \\\"$RABI_WORKSPACE\\\" = second ]; then exit 42; fi\"]\n",
    )
    .expect("hook");
    let out = repo.rabi(&["remove", "first", "second", "--json"]);
    assert_eq!(out.status.code(), Some(6));
    let value = assert_envelope(&out, "remove");
    assert_eq!(value["error"]["code"], "partial");
    assert_eq!(value["data"]["items"].as_array().expect("items").len(), 2);
    assert!(!managed(&repo, "first").exists());
    assert!(managed(&repo, "second").exists());
    let out = repo.rabi(&["remove", "second", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(assert_envelope(&out, "remove")["error"]["code"], "failed");
    {
        let out = repo.rabi(&["create", "late", "--output", "bad", "--json"]);
        assert_eq!(out.status.code(), Some(2));
        assert_eq!(assert_envelope(&out, "create")["error"]["code"], "usage");
    }
}

#[test]
fn common_options_can_precede_the_command_path() {
    let repo = Repo::new();
    assert_envelope(&repo.rabi(&["--output", "json", "list"]), "list");
    assert_envelope(&repo.rabi(&["--json", "repo", "list"]), "repo list");
    let out = repo.rabi(&["--no-input", "exec", "main", "--", "printf", "%s", "--json"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "--json");
}

#[test]
fn unsupported_exec_options_do_not_hide_json_usage_requests() {
    let repo = Repo::new();
    for command in ["list", "repo list", "frobnicate", "repo bogus"] {
        for exec in ["--exec", "-x"] {
            let args = command
                .split_whitespace()
                .chain([exec, "ignored", "--json"])
                .collect::<Vec<_>>();
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", "text")
                .output()
                .expect("rabi");
            assert_eq!(out.status.code(), Some(2), "{args:?}");
            let key = if command == "repo bogus" {
                "repo"
            } else {
                command
            };
            let value = assert_envelope(&out, key);
            assert_eq!(value["error"]["code"], "usage");
            assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
        }
    }
}

#[test]
fn invalid_usage_preserves_json_across_all_command_groups() {
    let repo = Repo::new();
    // Every JSON command, every parent group (including the gh alias), and
    // unresolved top-level commands must use exactly one usage envelope.
    for (command, invalid, key) in [
        ("create", "--bogus", "create"),
        ("create --branch", "--bogus", "create"),
        ("list", "--bogus", "list"),
        ("show", "--bogus", "show"),
        ("remove", "--bogus", "remove"),
        ("sync", "--bogus", "sync"),
        ("gc", "--bogus", "gc"),
        ("trust", "--bogus", "trust"),
        ("repo", "--bogus", "repo"),
        ("clone", "--bogus", "clone"),
        ("set", "--bogus", "set"),
        ("repo add", "--bogus", "repo add"),
        ("repo list", "--bogus", "repo list"),
        ("repo forget", "--bogus", "repo forget"),
        ("create", "--bogus", "create"),
        ("create", "--bogus", "create"),
        ("repo", "bogus", "repo"),
        ("meta", "bogus", "meta"),
        ("config", "bogus", "config"),
        ("adopt", "--bogus", "adopt"),
        ("forget", "--bogus", "forget"),
        ("repo", "--bogus", "repo"),
        ("meta", "--bogus", "meta"),
        ("config", "--bogus", "config"),
        ("adopt", "--bogus", "adopt"),
        ("frobnicate", "--bogus", "frobnicate"),
    ] {
        for (output_args, env_mode) in [
            (vec!["--json"], "text"),
            (vec!["--output", "json"], "text"),
            (vec!["--output=json"], "text"),
            (vec![], "json"),
        ] {
            let mut args = command.split_whitespace().collect::<Vec<_>>();
            args.push(invalid);
            args.extend(output_args.iter().copied());
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", env_mode)
                .output()
                .expect("rabi");
            assert_eq!(
                out.status.code(),
                Some(2),
                "{args:?}: {}",
                text(&out.stdout)
            );
            let value = assert_envelope(&out, key);
            assert_eq!(value["error"]["code"], "usage", "{args:?}");
            assert!(value["data"].is_null(), "{args:?}");
            assert!(out.stderr.is_empty(), "{args:?}: {}", text(&out.stderr));
        }
    }
}

#[test]
fn resolved_text_usage_errors_never_reenable_json() {
    let repo = Repo::new();
    for args in [
        vec!["list", "--repo", "--json", "--output", "text"],
        vec!["list", "--json", "--output", "text", "--repo"],
        vec!["list", "--output=text", "--repo", "--json"],
        vec!["list", "--repo", "--json", "--output", "text"],
        vec!["list", "--json", "--output=text", "--repo"],
        vec!["repo", "bogus", "--json", "--output", "text"],
    ] {
        for env_mode in ["text", "json"] {
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", env_mode)
                .output()
                .expect("rabi");
            assert_eq!(out.status.code(), Some(2), "{args:?}");
            assert!(out.stdout.is_empty(), "{args:?}: {}", text(&out.stdout));
            assert!(!out.stderr.is_empty(), "{args:?}");
        }
    }
}

#[test]
fn help_requests_skip_option_values_and_reject_missing_values() {
    let repo = Repo::new();
    for command in [
        "list",
        "create",
        "create",
        "repo list",
        "set",
        "gc",
        "clone",
    ] {
        for output in [
            vec!["--output", "--help"],
            vec!["--output=--help"],
            vec!["--help", "--output"],
        ] {
            let args = command
                .split_whitespace()
                .chain(["--json"])
                .chain(output)
                .collect::<Vec<_>>();
            let out = repo.rabi(&args);
            assert_eq!(out.status.code(), Some(2), "{args:?}");
            let value = assert_envelope(&out, command);
            assert_eq!(value["error"]["code"], "usage");
            assert!(value["error"]["message"]
                .as_str()
                .expect("message")
                .contains("output"));
        }
    }
    for (command, option) in [
        ("list", "--repo"),
        ("create", "--repo"),
        ("adopt", "--repo"),
        ("set", "--repo"),
    ] {
        let args = command
            .split_whitespace()
            .chain(["--json", option, "--help"])
            .collect::<Vec<_>>();
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(assert_envelope(&out, command)["error"]["code"], "usage");
    }
    // A real help request does not hide a missing command-option value either.
    for (command, option) in [
        ("list", "--repo"),
        ("create", "--base"),
        ("create", "--branch"),
        ("gc", "--older-than"),
        ("clone", "--depth"),
    ] {
        let args = command
            .split_whitespace()
            .chain(["--help", option])
            .collect::<Vec<_>>();
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("RABI_OUTPUT", "json")
            .output()
            .expect("rabi");
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(assert_envelope(&out, command)["error"]["code"], "usage");
    }
    assert!(!managed(&repo, "--help").exists());
}

#[test]
fn output_and_help_scan_respects_command_option_arity_and_child_boundaries() {
    let repo = Repo::new();
    for args in [
        vec!["list", "--base", "--json"], // --base is not a list option.
        vec!["repo", "list", "--bogus", "--json"],
        vec!["gc", "--closed-pr", "--json", "--bogus"], // --closed-pr is a flag here.
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(json_value(&out)["error"]["code"], "usage");
    }
    let out = repo.rabi(&["list", "--repo", "alice", "--", "--json", "--help"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("Unexpected argument"));
    let out = repo.rabi(&["create", "--purpose=--help", "--output=text"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("looks like an option"));
}

#[test]
fn json_dirty_refusal_keeps_its_specific_cause() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "dirty"]).status.success());
    fs::write(managed(&repo, "dirty/local.txt"), "uncommitted").unwrap();
    let refused = repo.rabi(&["remove", "dirty", "--json"]);
    assert_eq!(refused.status.code(), Some(4));
    assert_eq!(json_value(&refused)["error"]["code"], "dirty");
    assert!(managed(&repo, "dirty").is_dir());
}

#[cfg(debug_assertions)]
#[test]
fn create_and_attach_setup_failures_report_text_and_json_without_duplicate_causes() {
    for command in ["create", "create --branch"] {
        for json in [false, true] {
            for cause in ["metadata", "copyFiles", "postCreate"] {
                let repo = Repo::new();
                if command == "create --branch" {
                    repo.git(&["branch", "topic"]);
                }
                if cause == "copyFiles" {
                    std::os::unix::fs::symlink("elsewhere", repo.repo.join(".worktreeinclude"))
                        .unwrap();
                }
                if cause == "postCreate" {
                    fs::write(
                        repo.repo.join(".rabi.toml"),
                        "[hooks]\nsetup = [\"exit 23\"]\n",
                    )
                    .unwrap();
                }
                let mut args = command
                    .split_whitespace()
                    .chain(["topic"])
                    .collect::<Vec<_>>();
                if json {
                    args.push("--json");
                }
                let mut process = repo.command(env!("CARGO_BIN_EXE_rabi"), &args);
                if cause == "metadata" {
                    process.env("RABI_TEST_FAIL_METADATA_SAVE", "1");
                }
                let output = process.output().unwrap();
                let stderr = text(&output.stderr);
                assert_eq!(output.status.code(), Some(6), "{args:?}: {stderr}");
                assert!(managed(&repo, "topic/README.md").is_file());
                let diagnostic = match cause {
                    "metadata" => "Warning: Failed to save workspace metadata: injected failure",
                    "copyFiles" => "Warning: Failed to copy some files:",
                    "postCreate" => "Warning: Setup command",
                    _ => "failed with exit code 24",
                };
                if json {
                    let document = json_value(&output);
                    assert_eq!(document["error"]["code"], "partial");
                    if cause == "copyFiles" {
                        assert!(document["data"]["copy"]["error"].is_string());
                    } else {
                        assert_eq!(
                            document["data"][if cause == "postCreate" {
                                "setup"
                            } else {
                                cause
                            }]["ok"],
                            false
                        );
                    }
                    assert_eq!(stderr.matches(diagnostic).count(), 1, "{stderr}");
                } else {
                    assert_eq!(stderr.matches(diagnostic).count(), 1, "{stderr}");
                    let summary = "Workspace created but setup did not complete";
                    assert_eq!(stderr.matches(summary).count(), 1, "{stderr}");
                }
            }
        }
    }
}

#[test]
fn every_command_help_uses_workspace_vocabulary() {
    let repo = Repo::new();
    let data: serde_json::Value = serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let mut commands = vec![String::new()];
    commands.extend(data["helps"].as_object().unwrap().keys().cloned());
    for command in commands {
        let mut args: Vec<_> = command.split_whitespace().collect();
        args.push("--help");
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{command}: {}", text(&out.stderr));
        // These are retained interface and file names, not entity vocabulary.
        // No Git diagnostic wording is currently needed in help.
        let help = text(&out.stdout)
            .to_ascii_lowercase()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .replace("worktrees.json", "")
            .replace("worktrees-directory", "")
            .replace(".worktreeinclude", "")
            .replace("git worktree", "git checkout")
            .replace("git worktrees", "git checkouts")
            .replace("linked worktree", "linked checkout");
        assert!(!help.contains("worktree"), "{command}: {help}");
    }
}

fn workspace_env_script(output: &Path) -> String {
    format!(
        "printf '%s\\n' \"$RABI\" \"$RABI_WORKSPACE\" \"$RABI_WORKSPACE_ID\" \"$RABI_WORKSPACE_PATH\" \"$RABI_REPO_PATH\" > '{}'",
        output.display()
    )
}

fn assert_workspace_env(output: &Path, repo: &Repo, name: &str, path: &Path, id: &str) {
    let values = fs::read_to_string(output).expect("environment capture");
    let expected = format!(
        "1\n{name}\n{id}\n{}\n{}\n",
        path.display(),
        repo.repo.display()
    );
    assert_eq!(values, expected);
}

#[test]
fn hooks_and_exec_export_workspace_identity() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "source"]).status.success());
    let source = managed(&repo, "source");
    let post = repo.root.join("post-env");
    let pre = repo.root.join("pre-env");
    let exec = repo.root.join("exec-env");
    let hook_config = format!(
        "[hooks]\nsetup = [{}]\npre-remove = [{}]\n",
        serde_json::to_string(&workspace_env_script(&post)).unwrap(),
        serde_json::to_string(&workspace_env_script(&pre)).unwrap()
    );
    fs::write(source.join(".rabi.toml"), hook_config).unwrap();
    // A linked source must determine the hook config, while RABI_REPO_PATH
    // always identifies the primary checkout. All inherited context is stale.
    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .current_dir(&source)
            .env("RABI", "stale")
            .env("RABI_WORKSPACE", "stale")
            .env("RABI_WORKSPACE_ID", "stale")
            .env("RABI_WORKSPACE_PATH", "/stale")
            .env("RABI_REPO_PATH", "/stale")
            .output()
            .unwrap()
    };
    let script = workspace_env_script(&exec);
    let out = run(&["create", "feature/env-contract", "--json"]);
    let value = assert_envelope(&out, "create");
    assert!(out.status.success(), "{value}");
    let id = value["data"]["workspace"]["id"].as_str().unwrap();
    assert!(id.starts_with("ws_"));
    let path = managed(&repo, "feature/env-contract");
    assert_workspace_env(&post, &repo, "feature/env-contract", &path, id);

    let out = run(&["exec", "feature/env-contract", "--", "sh", "-c", &script]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_workspace_env(&exec, &repo, "feature/env-contract", &path, id);
    let out = run(&["remove", "feature/env-contract", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert_workspace_env(&pre, &repo, "feature/env-contract", &path, id);
    assert!(!path.exists());
    // Unrecorded workspaces export an empty ID, including the primary checkout.
    let out = run(&["exec", "main", "--", "sh", "-c", &script]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_workspace_env(&exec, &repo, "main", &repo.repo, "");
}

#[test]
fn recorded_custom_directories_remain_usable_after_preference_changes() {
    for layout in ["xdg", "git"] {
        let repo = Repo::new();
        let common = rabi::git::canonical_common_dir(&repo.repo).unwrap();
        let project = rabi::paths::project_directory_name("repo", &common);
        let custom_root = if layout == "xdg" {
            repo.root.join("custom/workspaces").join(project)
        } else {
            repo.repo.join("custom/trees")
        };
        let set = repo.rabi(&[
            "config",
            "set",
            "workspace-root",
            custom_root.to_str().unwrap(),
        ]);
        assert!(set.status.success(), "{}", text(&set.stderr));
        for name in ["feature/delete", "feature/gc"] {
            let out = repo.rabi(&["create", name, "--base", "main", "--json"]);
            let doc = assert_envelope(&out, "create");
            assert!(out.status.success(), "{doc}");
            assert_eq!(
                doc["data"]["workspace"]["path"],
                custom_root.join(name).to_str().unwrap()
            );
        }
        assert!(repo
            .rabi(&["config", "unset", "workspace-root"])
            .status
            .success());
        let listed = assert_envelope(&repo.rabi(&["list", "--json"]), "list");
        for name in ["feature/delete", "feature/gc"] {
            let row = listed["data"]["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["name"] == name)
                .expect("recorded workspace in list");
            assert_eq!(row["kind"], "workspace");
            assert_eq!(row["path"], custom_root.join(name).to_str().unwrap());
            let located = assert_envelope(&repo.rabi(&["show", name, "--json"]), "show");
            assert_eq!(located["data"]["workspace"]["path"], row["path"]);
        }
        fs::write(repo.repo.join("updated"), "new base\n").unwrap();
        repo.git(&["add", "updated"]);
        repo.git(&["commit", "-m", "advance base"]);
        let sync = repo.rabi(&["sync", "--all", "--json"]);
        let doc = assert_envelope(&sync, "sync");
        assert!(sync.status.success(), "{doc}");
        assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 2);
        for name in ["feature/delete", "feature/gc"] {
            assert!(custom_root.join(name).join("updated").is_file());
            assert!(
                !managed(&repo, name).exists(),
                "recorded workspace must not move"
            );
        }
        let out = repo.rabi(&["remove", "feature/delete", "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        assert_envelope(&out, "remove");
        assert!(!custom_root.join("feature/delete").exists());
        empty_commit(
            &repo,
            &custom_root.join("feature/gc"),
            "retire this workspace",
        );
        repo.git(&["merge", "--ff-only", "refs/heads/feature/gc"]);
        let dir = repo.repo.join(".git/rabi/workspaces");
        for entry in fs::read_dir(&dir).unwrap() {
            let file = entry.unwrap().path();
            let mut record: serde_json::Value =
                serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
            record["createdAt"] = serde_json::json!("2000-01-01T00:00:00.000Z");
            fs::write(file, serde_json::to_vec(&record).unwrap()).unwrap();
        }
        let out = repo.rabi(&["gc", "--apply", "--older-than", "1m", "--json"]);
        let doc = assert_envelope(&out, "gc");
        assert!(out.status.success(), "{doc}");
        assert!(
            doc["data"]["items"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["name"] == "feature/gc" && row["action"] == "removed"),
            "{doc}"
        );
        assert!(!custom_root.join("feature/gc").exists());
        let out = repo.rabi(&["create", "fresh", "--json"]);
        let doc = assert_envelope(&out, "create");
        assert!(out.status.success(), "{doc}");
        assert!(
            Path::new(doc["data"]["workspace"]["path"].as_str().unwrap())
                .starts_with(repo.root.join("xdg/rabi/workspaces"))
        );
    }
}

fn outer_run(
    repo: &Repo,
    args: &[&str],
    json: bool,
    command: &str,
    status: i32,
) -> serde_json::Value {
    let mut argv = args.to_vec();
    if json {
        argv.push("--json");
    }
    let out = repo.rabi(&argv);
    assert_eq!(
        out.status.code(),
        Some(status),
        "{argv:?}: {} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    if json {
        assert_envelope(&out, command)
    } else {
        assert!(
            serde_json::from_slice::<serde_json::Value>(&out.stdout).is_err()
                || out.stdout.is_empty(),
            "{argv:?}"
        );
        serde_json::Value::Null
    }
}

#[test]
fn outer_repo_commands_text_json_and_exit_codes() {
    for json in [false, true] {
        let repo = Repo::new();
        repo.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/acme/widget.git",
        ]);
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = [\"true\"]\n",
        )
        .unwrap();
        let added = outer_run(&repo, &["repo", "add"], json, "repo add", 0);
        if json {
            assert_eq!(added["data"]["repo"]["name"], "repo");
            assert!(added["data"].get("project").is_none());
        }
        let shown = outer_run(
            &repo,
            &["repo", "show", "acme/widget"],
            json,
            "repo show",
            0,
        );
        if json {
            let item = &shown["data"]["repo"];
            assert_eq!(item["root"], canonical(&repo.repo).to_str().unwrap());
            assert_eq!(item["name"], "github.com/acme/widget");
            assert_eq!(item["defaultBranch"], "main");
            assert_eq!(item["configPresent"], true);
            assert_eq!(item["trusted"], true);
            assert_eq!(item["workspaceRoot"], managed_root(&repo).to_str().unwrap());
        }
        outer_run(&repo, &["repo", "show"], json, "repo show", 0);
        let listed = outer_run(
            &repo,
            &["repo", "list", "--registered"],
            json,
            "repo list",
            0,
        );
        if json {
            assert_eq!(listed["data"]["repos"].as_array().unwrap().len(), 1);
            let id = listed["data"]["repos"][0]["id"].as_str().unwrap();
            outer_run(&repo, &["repo", "show", id], true, "repo show", 0);
        }
        outer_run(
            &repo,
            &["repo", "forget", "github.com/acme/widget"],
            json,
            "repo forget",
            0,
        );
        assert!(repo.repo.join("README.md").is_file());
        assert!(repo.repo.join(".git").is_dir());
        for (args, command, status) in [
            (
                vec!["repo", "add", "/definitely/missing/rabi"],
                "repo add",
                3,
            ),
            (vec!["repo", "add", "a", "b"], "repo add", 2),
            (vec!["repo", "show", "unknown-repo"], "repo show", 3),
            (vec!["repo", "forget", "unknown-repo"], "repo forget", 3),
            (vec!["repo", "forget"], "repo forget", 2),
            (
                vec!["repo", "list", "--registered", "--discovered"],
                "repo list",
                2,
            ),
        ] {
            outer_run(&repo, &args, json, command, status);
        }
    }
}

#[test]
fn outer_repo_catalog_filters_registration_and_discovery() {
    let repo = Repo::new();
    let discovered = repo.home.join("ghq/example.com/acme/discovered");
    fs::create_dir_all(&discovered).unwrap();
    git_at(&repo, &discovered, &["init", "-b", "main"]);
    let ghq_path = silent_ghq_path(&repo);
    assert!(repo.rabi(&["repo", "add"]).status.success());
    for (filter, name) in [("--registered", "repo"), ("--discovered", "discovered")] {
        for mode in ["--names", "--paths", "--json"] {
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &["repo", "list", filter, mode])
                .env("PATH", &ghq_path)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
            match mode {
                "--names" => assert_eq!(text(&out.stdout), name),
                "--paths" => assert!(same_dir(
                    &text(&out.stdout),
                    if name == "repo" {
                        &repo.repo
                    } else {
                        &discovered
                    }
                )),
                _ => {
                    let value = assert_envelope(&out, "repo list");
                    assert_eq!(value["data"]["repos"].as_array().unwrap().len(), 1);
                    assert_eq!(value["data"]["repos"][0]["name"], name);
                }
            }
        }
    }
    assert!(repo
        .rabi(&["config", "set", "discover-ghq", "false"])
        .status
        .success());
    let out = repo.rabi(&["repo", "list", "--discovered", "--json"]);
    assert_eq!(
        assert_envelope(&out, "repo list")["data"]["repos"],
        serde_json::json!([])
    );
}

#[test]
fn outer_config_commands_text_json_sources_and_exit_codes() {
    for json in [false, true] {
        let repo = Repo::new();
        let defaults = outer_run(
            &repo,
            &["config", "list", "--sources"],
            json,
            "config list",
            0,
        );
        if json {
            let config = &defaults["data"]["config"];
            assert_eq!(config["workspace-root"]["value"], serde_json::Value::Null);
            assert_eq!(config["discover-ghq"]["value"], true);
            assert!(config.get("keep-branch").is_none());
            for key in rabi::preferences::SUPPORTED_KEYS {
                assert_eq!(config[key]["source"], "default");
            }
        }
        outer_run(
            &repo,
            &["config", "set", "workspace-root", "../my spaces"],
            json,
            "config set",
            0,
        );
        let get = outer_run(
            &repo,
            &["config", "get", "workspace-root", "--sources"],
            json,
            "config get",
            0,
        );
        if json {
            assert_eq!(
                get["data"]["config"]["workspace-root"]["value"],
                "../my spaces"
            );
            assert_eq!(get["data"]["config"]["workspace-root"]["source"], "config");
        }
        outer_run(
            &repo,
            &["config", "get", "workspace-root"],
            json,
            "config get",
            0,
        );
        outer_run(&repo, &["config", "list"], json, "config list", 0);
        outer_run(
            &repo,
            &["config", "unset", "workspace-root"],
            json,
            "config unset",
            0,
        );
        for (args, command, status) in [
            (vec!["config", "get", "unknown"], "config get", 2),
            (vec!["config", "get"], "config get", 2),
            (
                vec!["config", "set", "discover-ghq", "yes"],
                "config set",
                2,
            ),
            (
                vec!["config", "set", "clone-root", "relative"],
                "config set",
                2,
            ),
            (vec!["config", "set", "workspace-root"], "config set", 2),
            (vec!["config", "unset", "workspace-root"], "config unset", 3),
            (vec!["config", "unset", "unknown"], "config unset", 2),
            (vec!["config", "list", "extra"], "config list", 2),
        ] {
            outer_run(&repo, &args, json, command, status);
        }
        let path = repo.home.join(".config/rabi/config.toml");
        fs::write(path, "bad = [\n").unwrap();
        outer_run(&repo, &["config", "list"], json, "config list", 1);
    }
}

#[test]
fn outer_clone_root_sources_match_the_existing_resolution_chain() {
    let repo = Repo::new();
    let get = |env_root: Option<&str>| {
        let mut command = repo.command(
            env!("CARGO_BIN_EXE_rabi"),
            &["config", "get", "clone-root", "--sources", "--json"],
        );
        if let Some(value) = env_root {
            command.env("GHQ_ROOT", value);
        }
        let out = command.output().unwrap();
        assert_eq!(out.status.code(), Some(0));
        assert_envelope(&out, "config get")["data"]["config"]["clone-root"].clone()
    };
    let value = get(None);
    assert_eq!(value["value"], repo.home.join("ghq").to_str().unwrap());
    assert_eq!(value["source"], "default");
    assert_eq!(value["detail"], "~/ghq");
    let value = get(Some("~/env"));
    assert_eq!(value["value"], repo.home.join("env").to_str().unwrap());
    assert_eq!(value["source"], "env");
    assert_eq!(value["detail"], "GHQ_ROOT");
    repo.git(&["config", "--global", "ghq.root", "~/git"]);
    let value = get(Some("~/env"));
    assert_eq!(value["value"], repo.home.join("git").to_str().unwrap());
    assert_eq!(value["source"], "default");
    assert_eq!(value["detail"], "ghq.root");
    assert!(repo
        .rabi(&["config", "set", "clone-root", "~/config"])
        .status
        .success());
    let value = get(Some("~/env"));
    assert_eq!(value["value"], repo.home.join("config").to_str().unwrap());
    assert_eq!(value["source"], "config");
}

#[test]
fn outer_trust_commands_text_json_and_exit_codes() {
    for json in [false, true] {
        let repo = Repo::new();
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = [\"echo hook-visible\"]\n",
        )
        .unwrap();
        assert!(repo.rabi(&["repo", "add"]).status.success());
        let run = |args: &[&str], command: &str, status: i32| {
            let mut argv = args.to_vec();
            if json {
                argv.push("--json");
            }
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &argv)
                .env_remove("RABI_TRUST_ALL")
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(status),
                "{argv:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            if json {
                assert_envelope(&out, command);
            }
            out
        };
        let untrusted = run(&["create", "refused"], "create", 4);
        assert!(!managed(&repo, "refused").exists());
        assert!(
            format!("{} {}", text(&untrusted.stdout), text(&untrusted.stderr))
                .contains("rabi trust add")
        );
        let add = run(&["trust", "add", "repo"], "trust add", 0);
        assert!(text(&add.stderr).contains("echo hook-visible"));
        assert!(!text(&add.stderr).contains("[y/N]"));
        let list = run(&["trust", "list"], "trust list", 0);
        if json {
            assert_eq!(
                json_value(&list)["data"]["repos"].as_array().unwrap().len(),
                1
            );
        }
        let show = run(&["repo", "show"], "repo show", 0);
        if json {
            assert_eq!(json_value(&show)["data"]["repo"]["trusted"], true);
        }
        run(&["trust", "remove", "repo"], "trust remove", 0);
        let show = run(&["repo", "show"], "repo show", 0);
        if json {
            assert_eq!(json_value(&show)["data"]["repo"]["trusted"], false);
        }
        run(&["trust", "remove"], "trust remove", 0);
        run(&["trust", "list"], "trust list", 0);
        for (args, command, status) in [
            (vec!["trust", "add", "unknown-repo"], "trust add", 3),
            (vec!["trust", "remove", "unknown-repo"], "trust remove", 3),
            (vec!["trust", "add", "a", "b"], "trust add", 2),
            (vec!["trust", "remove", "a", "b"], "trust remove", 2),
            (vec!["trust", "list", "extra"], "trust list", 2),
        ] {
            run(&args, command, status);
        }
    }
}

#[test]
#[cfg(unix)]
fn outer_trust_output_escapes_repository_control_characters() {
    let mut failures = Vec::new();
    for json in [false, true] {
        let mut repo = Repo::new();
        let name = "repo\u{1b}[31m\nforged";
        let renamed = repo.root.join(name);
        fs::rename(&repo.repo, &renamed).unwrap();
        repo.repo = renamed;
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = [\"echo hook-visible\"]\n",
        )
        .unwrap();
        let root = canonical(&repo.repo).to_string_lossy().into_owned();
        let key = format!("{root}/.git");
        let visible_root = root.replace('\u{1b}', "\\u{1b}").replace('\n', "\\u{a}");
        let visible_key = format!("{visible_root}/.git");
        for (args, command, expected) in [
            (vec!["trust", "add"], "trust add", "Trusted "),
            (vec!["trust", "list"], "trust list", ""),
            (
                vec!["trust", "remove"],
                "trust remove",
                "Removed trust for ",
            ),
            (vec!["trust", "remove"], "trust remove", "Not trusted: "),
        ] {
            let mut argv = args;
            if json {
                argv.push("--json");
            }
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &argv)
                .env_remove("RABI_TRUST_ALL")
                .output()
                .unwrap();
            assert!(out.status.success(), "{argv:?}: {out:?}");
            let stderr = text(&out.stderr);
            if command == "trust add" {
                if !stderr.starts_with(&format!("{visible_root}\n")) {
                    failures.push(format!("{argv:?} stderr: {stderr:?}"));
                }
                assert!(stderr.contains("echo hook-visible"));
            }
            if stderr.contains('\u{1b}') || stderr.contains(name) {
                failures.push(format!("{argv:?} unsafe stderr: {stderr:?}"));
            }
            if json {
                let envelope = assert_envelope(&out, command);
                let data = &envelope["data"];
                let path = if command == "trust list" {
                    &data["repos"][0]["path"]
                } else {
                    &data["path"]
                };
                assert_eq!(path, &key);
            } else {
                let stdout = text(&out.stdout);
                if !stdout.starts_with(&format!("{expected}{visible_key}")) {
                    failures.push(format!("{argv:?} stdout: {stdout:?}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[cfg(unix)]
fn outer_repo_output_escapes_repository_control_characters() {
    let mut repo = Repo::new();
    let name = "repo\u{1b}[31m\nforged";
    let visible_name = "repo\\u{1b}[31m\\u{a}forged";
    let renamed = repo.root.join(name);
    fs::rename(&repo.repo, &renamed).unwrap();
    repo.repo = renamed;
    let root = canonical(&repo.repo).to_string_lossy().into_owned();
    let visible_root = root.replace(name, visible_name);
    let mut failures = Vec::new();
    for (args, expected) in [
        (
            vec!["repo", "add"],
            format!("'{visible_name}' ({visible_root})"),
        ),
        (
            vec!["repo", "add"],
            format!("'{visible_name}' is already registered ({visible_root})"),
        ),
        (
            vec!["repo", "list", "--registered"],
            format!("{visible_name} ({visible_root})"),
        ),
        (
            vec!["repo", "list", "--registered", "--names"],
            visible_name.into(),
        ),
        (
            vec!["repo", "list", "--registered", "--paths"],
            visible_root.clone(),
        ),
        (
            vec!["repo", "show"],
            format!("{visible_name}\nPrimary checkout: {visible_root}\n"),
        ),
        (
            vec!["repo", "forget", &root],
            format!("'{visible_name}' ({visible_root})"),
        ),
    ] {
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        let stdout = text(&out.stdout);
        if !stdout.contains(&expected) || stdout.contains(name) || stdout.contains('\u{1b}') {
            failures.push(format!("{args:?}: {stdout:?}"));
        }
        if args == ["repo", "show"] {
            let shown = repo.rabi(&["repo", "show", "--json"]);
            let envelope = assert_envelope(&shown, "repo show");
            assert_eq!(envelope["data"]["repo"]["root"], root);
            assert_eq!(envelope["data"]["repo"]["name"], name);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[cfg(unix)]
fn outer_discovery_and_ambiguity_escape_repository_paths() {
    let mut repo = Repo::new();
    let clone_root = repo.root.join("clones\u{1b}[31m\nforged");
    let checkout = clone_root.join("github.com/example/widget");
    fs::create_dir_all(checkout.parent().unwrap()).unwrap();
    fs::rename(&repo.repo, &checkout).unwrap();
    repo.repo = checkout.clone();
    assert!(repo
        .rabi(&["config", "set", "clone-root", clone_root.to_str().unwrap()])
        .status
        .success());
    let raw = checkout.to_str().unwrap();
    let visible = raw.replace('\u{1b}', "\\u{1b}").replace('\n', "\\u{a}");
    for (args, expected) in [
        (
            vec!["repo", "list", "--discovered"],
            format!("widget ({visible}) [ghq]"),
        ),
        (
            vec!["repo", "list", "--discovered", "--paths"],
            visible.clone(),
        ),
        (
            vec!["repo", "list", "--discovered", "--names"],
            "widget".into(),
        ),
    ] {
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{args:?}: {out:?}");
        assert_eq!(text(&out.stdout), expected, "{args:?}");
        assert!(!text(&out.stderr).contains('\u{1b}'), "{args:?}: {out:?}");
    }
    let out = repo.rabi(&["repo", "add", "--json"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        assert_envelope(&out, "repo add")["data"]["repo"]["rootPath"],
        raw
    );

    let other = clone_root.join("other/widget");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    repo.git(&[
        "clone",
        repo.repo.to_str().unwrap(),
        other.to_str().unwrap(),
    ]);
    assert!(repo
        .rabi(&["repo", "add", other.to_str().unwrap()])
        .status
        .success());
    for (args, command, lines) in [
        (vec!["repo", "show", "widget"], "repo show", 3),
        (vec!["repo", "forget", "widget"], "repo forget", 1),
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        let stderr = text(&out.stderr);
        assert!(!stderr.contains('\u{1b}'), "{args:?}: {stderr:?}");
        assert_eq!(stderr.lines().count(), lines, "{args:?}: {stderr:?}");
        if command == "repo show" {
            assert!(stderr.contains(&visible), "{stderr:?}");
        }
        let mut args = args;
        args.push("--json");
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2));
        let value = assert_envelope(&out, command);
        assert_eq!(value["error"]["code"], "ambiguous");
        if command == "repo show" {
            assert!(value["error"]["message"].as_str().unwrap().contains(raw));
        }
    }
}

#[test]
#[cfg(unix)]
fn outer_config_output_escapes_paths_in_values_and_sources() {
    let repo = Repo::new();
    let directory = repo.root.join("config\u{1b}[31m\nforged");
    fs::create_dir_all(&directory).unwrap();
    let raw = directory.to_str().unwrap();
    let visible = raw.replace('\u{1b}', "\\u{1b}").replace('\n', "\\u{a}");
    for args in [
        vec!["config", "set", "workspace-root", raw],
        vec!["config", "get", "workspace-root"],
        vec!["config", "get", "workspace-root", "--sources"],
        vec!["config", "list"],
        vec!["config", "list", "--sources"],
        vec!["repo", "show"],
        vec!["config", "unset", "workspace-root"],
    ] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("XDG_CONFIG_HOME", &directory)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        let stdout = text(&out.stdout);
        assert!(stdout.contains(&visible), "{args:?}: {stdout:?}");
        assert!(!stdout.contains(raw), "{args:?}: {stdout:?}");
        assert!(!stdout.contains('\u{1b}'), "{args:?}: {stdout:?}");
    }
}

#[test]
#[cfg(unix)]
fn outer_diagnostics_escape_repository_names_and_paths() {
    let repo = Repo::new();
    let directory = repo.root.join("repo\u{1b}[31m\nforged");
    fs::create_dir_all(&directory).unwrap();
    let missing = directory.join("missing");
    let path = missing.to_str().unwrap();
    let name = "missing\u{1b}[31m\nforged";
    for (args, command, reference) in [
        (vec!["repo", "show", name], "repo show", name),
        (vec!["repo", "forget", name], "repo forget", name),
        (vec!["repo", "add", path], "repo add", path),
        (vec!["trust", "add", path], "trust add", path),
        (vec!["trust", "remove", path], "trust remove", path),
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {out:?}");
        let stderr = text(&out.stderr);
        let visible = reference
            .replace('\u{1b}', "\\u{1b}")
            .replace('\n', "\\u{a}");
        assert!(stderr.contains(&visible), "{args:?}: {stderr:?}");
        assert_eq!(stderr.lines().count(), 1, "{args:?}: {stderr:?}");
        assert!(!stderr.contains('\u{1b}'), "{args:?}: {stderr:?}");
        let mut json_args = args;
        json_args.push("--json");
        let out = repo.rabi(&json_args);
        assert_eq!(out.status.code(), Some(3));
        let value = assert_envelope(&out, command);
        assert!(value["error"]["message"]
            .as_str()
            .unwrap()
            .contains(reference));
        assert!(out.stderr.is_empty());
    }

    // Discovery diagnostics are printed to stderr even in JSON mode.
    let path = silent_ghq_path(&repo);
    fs::write(
        repo.root.join("bin/ghq"),
        "#!/bin/sh\nprintf '%s\\n' \"$RABI_TEST_DIAGNOSTIC\" >&2\nexit 17\n",
    )
    .unwrap();
    for json in [false, true] {
        let mut args = vec!["repo", "list"];
        if json {
            args.push("--json");
        }
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("PATH", &path)
            .env("RABI_TEST_DIAGNOSTIC", &directory)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
        let stderr = text(&out.stderr);
        assert!(!stderr.contains('\u{1b}'), "{args:?}: {stderr:?}");
        assert!(
            stderr.contains("repo\\u{1b}[31m\\u{a}forged"),
            "{args:?}: {stderr:?}"
        );
        if json {
            let value = assert_envelope(&out, "repo list");
            assert!(value["warnings"][0]["message"]
                .as_str()
                .unwrap()
                .contains(directory.to_str().unwrap()));
        }
    }
}

#[test]
fn outer_global_repo_selection_works_before_commands_and_preserves_child_arguments() {
    let repo = Repo::new();
    assert!(repo.rabi(&["repo", "add"]).status.success());
    for args in [
        vec!["-R", "repo", "list", "--json"],
        vec!["--repo=repo", "list", "--json"],
        vec!["-Rrepo", "list", "--json"],
        vec!["--json", "-R", "repo", "list"],
        vec!["list", "-R", "repo", "--json"],
    ] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&repo.home)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert_eq!(
            assert_envelope(&out, "list")["data"]["workspaces"][0]["kind"],
            "primary"
        );
    }
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["-R", "repo", "list"])
        .current_dir(&repo.home)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out.stdout).contains("main  main"));
    outer_run(&repo, &["-R", "repo", "repo", "show"], true, "repo show", 0);
    outer_run(&repo, &["-R", "repo", "trust", "add"], true, "trust add", 0);
    outer_run(
        &repo,
        &["-R", "repo", "trust", "remove"],
        true,
        "trust remove",
        0,
    );
    for json in [false, true] {
        outer_run(&repo, &["-R", "unknown-repo", "list"], json, "list", 3);
        outer_run(
            &repo,
            &["-R", "repo", "list", "--repo", "repo"],
            json,
            "list",
            2,
        );
        outer_run(&repo, &["list", "--repo="], json, "list", 2);
    }
    let out = repo.rabi(&[
        "-R",
        "repo",
        "exec",
        "main",
        "--",
        "printf",
        "%s",
        "--project",
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text(&out.stdout), "--project");
}

#[test]
fn outer_parent_commands_require_subcommands_in_json_and_show_text_help() {
    let repo = Repo::new();
    for (command, key) in [("trust", "trust"), ("repo", "repo"), ("config", "config")] {
        let help = repo.rabi(&[command, "--help"]);
        assert_eq!(help.status.code(), Some(0));
        assert!(help.stderr.is_empty());
        assert!(text(&help.stdout).contains("USAGE"));
        for args in [vec![command], vec![command, "--output", "text"]] {
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", if args.len() == 1 { "text" } else { "json" })
                .output()
                .expect("rabi");
            assert_eq!(out.status.code(), Some(0), "{args:?}");
            assert_eq!(out.stdout, help.stdout, "{args:?}");
            assert!(out.stderr.is_empty(), "{args:?}");
        }
        for (args, mode) in [
            (vec![command, "--json"], "text"),
            (vec!["--json", command], "text"),
            (vec![command, "--output", "json"], "text"),
            (vec![command], "json"),
            (vec![command, "--no-input"], "json"),
        ] {
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", mode)
                .output()
                .expect("rabi");
            assert_eq!(
                out.status.code(),
                Some(2),
                "{args:?}: {}",
                text(&out.stdout)
            );
            let value = assert_envelope(&out, key);
            assert_eq!(value["error"]["code"], "usage");
            assert!(value["error"]["message"]
                .as_str()
                .unwrap()
                .contains("subcommand"));
            assert!(value["data"].is_null());
            assert!(out.stderr.is_empty(), "{args:?}");
        }
    }
}

#[test]
fn outer_help_flags_ignore_output_environment() {
    let repo = Repo::new();
    let expected = repo.rabi(&["help", "help"]);
    assert_eq!(expected.status.code(), Some(0));
    assert!(text(&expected.stdout).contains("USAGE"));
    for mode in ["json", "invalid"] {
        for args in [
            vec!["help", "--help"],
            vec!["help", "-h"],
            vec!["help", "--help", "--json"],
            vec!["--json", "help", "--help"],
        ] {
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_OUTPUT", mode)
                .output()
                .expect("rabi");
            assert_eq!(
                out.status.code(),
                Some(0),
                "{args:?}, {mode}: {}",
                text(&out.stderr)
            );
            assert!(out.stdout == expected.stdout, "{args:?}, {mode}");
            assert!(out.stderr.is_empty(), "{args:?}, {mode}");
        }
    }
}

#[test]
fn outer_help_and_version_contracts_text_json_and_exit_codes() {
    let repo = Repo::new();
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../help-data.json")).unwrap();
    let out = repo.rabi(&["help", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let value = assert_envelope(&out, "help");
    assert_eq!(value["data"], expected);
    assert_eq!(expected["schemaVersion"], 2);
    assert_eq!(expected["exitCodes"].as_object().unwrap().len(), 7);
    for (code, exit) in expected["errorCodes"].as_object().unwrap() {
        assert_eq!(
            rabi::error::exit_for_code(code),
            exit.as_i64().unwrap() as i32
        );
    }
    for command in [
        "create",
        "create",
        "remove",
        "show",
        "list",
        "sync",
        "gc",
        "exec",
        "set",
        "adopt",
        "adopt",
        "forget",
        "create",
        "clone",
        "repo",
        "repo add",
        "repo list",
        "repo show",
        "repo forget",
        "config",
        "config get",
        "config set",
        "config unset",
        "config list",
        "trust",
        "trust add",
        "trust remove",
        "trust list",
        "help",
        "--version",
    ] {
        let entry = &expected["helps"][command];
        assert!(entry.is_object(), "{command}");
        let mut help_args = vec!["help"];
        help_args.extend(command.split_whitespace());
        // --version is a flag, so select its help entry after the separator.
        if command == "--version" {
            help_args = vec!["help", "--", "--version"];
        }
        let text_help = repo.rabi(&help_args);
        let mut flag_args: Vec<_> = command.split_whitespace().collect();
        flag_args.push("--help");
        let flag_help = repo.rabi(&flag_args);
        assert_eq!(
            text_help.status.code(),
            Some(0),
            "{command}: {}",
            text(&text_help.stderr)
        );
        assert_eq!(
            text(&text_help.stdout),
            text(&flag_help.stdout),
            "{command}"
        );
        assert!(
            entry["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|option| option["name"] == "repo" && option["short"] == "R"),
            "{command}"
        );
    }
    let out = repo.rabi(&["help", "repo", "show", "--json"]);
    assert_eq!(
        assert_envelope(&out, "help")["data"]["selected"],
        expected["helps"]["repo show"]
    );
    assert_eq!(
        text(&repo.rabi(&["help"]).stdout),
        text(&repo.rabi(&["--help"]).stdout)
    );
    for args in [
        vec!["--version", "--json"],
        vec!["-v", "--json"],
        vec!["--json", "--version"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(0));
        let value = assert_envelope(&out, "--version");
        assert_eq!(value["data"]["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(
            value["data"]["display"],
            text(&repo.rabi(&["--version"]).stdout)
        );
    }
    for json in [false, true] {
        outer_run(&repo, &["help", "unknown-command"], json, "help", 2);
        outer_run(&repo, &["--version", "extra"], json, "--version", 2);
    }
}

#[test]
fn outer_repo_forget_ambiguous_canonical_suffix_is_usage_and_keeps_registrations() {
    let repo = Repo::new();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/one/widget.git",
    ]);
    let second = repo.root.join("second");
    fs::create_dir_all(&second).unwrap();
    git_at(&repo, &second, &["init", "-b", "main"]);
    git_at(
        &repo,
        &second,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/two/widget.git",
        ],
    );
    assert!(repo.rabi(&["repo", "add"]).status.success());
    assert!(repo
        .rabi(&["repo", "add", second.to_str().unwrap()])
        .status
        .success());
    for json in [false, true] {
        let value = outer_run(&repo, &["repo", "forget", "widget"], json, "repo forget", 2);
        if json {
            assert_eq!(value["error"]["code"], "ambiguous");
        }
    }
    let value = outer_run(
        &repo,
        &["repo", "list", "--registered"],
        true,
        "repo list",
        0,
    );
    assert_eq!(value["data"]["repos"].as_array().unwrap().len(), 2);
}

#[test]
fn unknown_commands_and_options_are_usage_errors() {
    let repo = Repo::new();
    for (args, command, expected) in [
        (
            vec!["delete", "topic"],
            "delete",
            "Unknown command 'delete topic",
        ),
        (
            vec!["help", "preferences"],
            "help",
            "Unknown command 'preferences'",
        ),
        (
            vec!["--project=repo", "list"],
            "--project=repo",
            "Unknown option '--project=repo'",
        ),
        (
            vec!["create", "topic", "--exec", "true"],
            "create",
            "Unknown option '--exec'.",
        ),
        (
            vec!["remove", "--force"],
            "remove",
            "Unknown option '--force'.",
        ),
        (vec!["sync", "--mine"], "sync", "Unknown option '--mine'."),
        (vec!["trust", "--yes"], "trust", "Unknown option '--yes'."),
        (
            vec!["trust", "bogus"],
            "trust",
            "Unknown command 'trust bogus'",
        ),
        (
            vec!["trust", "-R", "."],
            "trust",
            "Missing trust subcommand. Use add, remove, or list.",
        ),
    ] {
        for json in [false, true] {
            let mut argv = args.clone();
            if json {
                argv.push("--json");
            }
            let out = repo.rabi(&argv);
            assert_eq!(out.status.code(), Some(2), "{argv:?}: {out:?}");
            let message = if json {
                let doc = assert_envelope(&out, command);
                assert_eq!(doc["error"]["code"], "usage");
                assert!(doc["data"].is_null());
                doc["error"]["message"].as_str().unwrap().to_owned()
            } else {
                assert!(out.stdout.is_empty(), "{argv:?}: {out:?}");
                text(&out.stderr)
            };
            assert!(message.starts_with(expected), "{argv:?}: {message}");
            assert!(!message.contains("was removed"), "{argv:?}: {message}");
        }
    }
    // A help request follows the same precedence for every unknown option.
    let help = repo.rabi(&["list", "--help"]);
    assert!(help.status.success());
    for flag in ["--bogus", "--owner"] {
        let out = repo.rabi(&["list", flag, "--help"]);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(out.stdout, help.stdout);
        assert!(out.stderr.is_empty());
    }
    assert!(repo.rabi(&["create", "child"]).status.success());
    let out = repo.rabi(&["exec", "child", "--", "printf", "%s", "--owner"]);
    assert!(out.status.success(), "{out:?}");
    assert_eq!(text(&out.stdout), "--owner");
}

#[test]
fn unknown_config_keys_are_rejected_without_rewriting() {
    let repo = Repo::new();
    let path = repo.home.join(".config/rabi/config.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    for key in [
        "bogus",
        "worktrees-directory",
        "root",
        "ghq-discovery",
        "keep-branch",
        "directory-name-separator",
    ] {
        let contents = format!("{key} = true\n");
        fs::write(&path, &contents).unwrap();
        for args in [
            vec!["config", "list"],
            vec!["config", "set", "discover-ghq", "false"],
            vec!["config", "unset", "discover-ghq"],
        ] {
            let out = repo.rabi(&args);
            assert_eq!(out.status.code(), Some(1), "{args:?}: {out:?}");
            assert!(
                text(&out.stderr).contains(&format!("Unknown key '{key}'")),
                "{out:?}"
            );
            assert_eq!(fs::read_to_string(&path).unwrap(), contents);
        }
    }
}

#[test]
fn inspection_list_and_exec_leave_record_bytes_and_mtime_unchanged() {
    let repo = Repo::new();
    let created = json_value(&repo.rabi(&["create", "feature", "--purpose", "task", "--json"]));
    let dir = repo.repo.join(".git/rabi/workspaces");
    let file = dir.join(format!(
        "{}.json",
        created["data"]["workspace"]["id"].as_str().unwrap()
    ));
    let sentinel = std::time::UNIX_EPOCH + std::time::Duration::from_secs(946684800);
    fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(sentinel)
        .unwrap();
    let before = fs::read(&file).unwrap();
    let modified = fs::metadata(&file).unwrap().modified().unwrap();
    for args in [
        vec!["path", "feature"],
        vec!["path", "feature", "--json"],
        vec!["show", "feature"],
        vec!["show", "feature", "--json"],
        vec!["list"],
        vec!["list", "--json"],
        vec!["list", "--names"],
        vec!["list", "--paths"],
        vec!["exec", "feature", "--", "true"],
        vec!["exec", "feature", "--", "sh", "-c", "exit 17"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(
            out.status.code(),
            Some(if args.last() == Some(&"exit 17") {
                17
            } else {
                0
            }),
            "{args:?}: {}",
            text(&out.stderr)
        );
        assert_eq!(fs::read(&file).unwrap(), before, "{args:?}");
        assert_eq!(
            fs::metadata(&file).unwrap().modified().unwrap(),
            modified,
            "{args:?}"
        );
    }
}

#[test]
fn gc_age_selection_uses_creation_time() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "old"]).status.success());
    land_branch(&repo, "old");
    let dir = repo.repo.join(".git/rabi/workspaces");
    let mut record =
        serde_json::from_slice::<serde_json::Value>(&records::read(&dir).unwrap()).unwrap();
    record["worktrees"][0]["createdAt"] = serde_json::json!("2000-01-01T00:00:00.000Z");
    records::write(&dir, serde_json::to_vec(&record).unwrap()).unwrap();
    let out = repo.rabi(&["gc", "--older-than", "14d", "--json"]);
    assert!(out.status.success(), "{out:?}");
    let doc = json_value(&out);
    let candidates = doc["data"]["items"].as_array().unwrap();
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0]["action"], "wouldRemove");
}

#[test]
fn selectors_resolve_linked_workspaces_across_inspection_and_mutations() {
    for selector in [
        "feature/select",
        ".",
        "pr:42",
        "issue:73",
        "https://github.com/acme/demo/pull/42",
        "https://github.com/acme/demo/issues/73",
    ] {
        let repo = Repo::new();
        let created = repo.create_recorded(&[
            "create",
            "feature/select",
            "--pr",
            "https://github.com/acme/demo/pull/42",
            "--issue",
            "https://github.com/acme/demo/issues/73",
            "--json",
        ]);
        assert!(created.status.success(), "{}", text(&created.stderr));
        let path = managed(&repo, "feature/select");
        let cwd = path.join("nested");
        fs::create_dir_all(&cwd).unwrap();
        let run = |args: &[&str]| {
            repo.command(env!("CARGO_BIN_EXE_rabi"), args)
                .current_dir(&cwd)
                .output()
                .unwrap()
        };
        for command in ["show", "path", "set", "exec", "sync"] {
            let mut args = vec![command, selector];
            match command {
                "set" => args.extend(["--purpose", "selected", "--json"]),
                "exec" => args.extend(["--", "pwd"]),
                "sync" => args.extend(["--json"]),
                _ => args.push("--json"),
            }
            let out = run(&args);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{args:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            if command == "exec" {
                assert!(same_dir(&text(&out.stdout), &path));
            } else {
                let value = assert_envelope(&out, command);
                if command == "path" {
                    assert_eq!(value["data"], serde_json::json!({"path": path}));
                }
                if matches!(command, "show" | "set") {
                    assert_eq!(value["data"]["workspace"]["name"], "feature/select");
                    assert_eq!(value["data"]["workspace"]["current"], true);
                }
            }
        }
        let out = run(&[
            "remove",
            selector,
            "--discard-changes",
            "--delete-unpreserved-branch",
            "--no-hooks",
            "--json",
        ]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{selector}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(!path.exists());
    }
}

#[test]
fn selectors_fail_before_side_effects_for_missing_ambiguous_and_foreign_dot() {
    let repo = Repo::new();
    for name in ["one", "two"] {
        assert!(repo
            .create_recorded(&["create", name, "--pr", "42"])
            .status
            .success());
    }
    let other = repo.root.join("other");
    fs::create_dir(&other).unwrap();
    assert!(repo
        .command("git", &["init", "-b", "main", other.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    let records = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&records).unwrap();
    for command in ["show", "path", "set", "exec", "sync", "remove"] {
        for (selector, code, error) in [
            ("missing", 3, "not_found"),
            ("pr:999", 3, "not_found"),
            ("pr:42", 2, "ambiguous"),
            (".", 2, "usage"),
        ] {
            let mut args = vec![command];
            if selector == "." {
                args.extend(["-R", other.to_str().unwrap()]);
            }
            args.push(selector);
            match command {
                "set" => args.extend(["--purpose", "should not change"]),
                "exec" => args.extend(["--", "touch", "should-not-exist"]),
                "remove" => {}
                _ => {}
            }
            if command != "exec" {
                args.push("--json");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(code),
                "{args:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            if command != "exec" {
                if code == 0 {
                    assert_eq!(json_value(&out)["data"]["items"][0]["alreadyGone"], true);
                } else {
                    assert_eq!(assert_envelope(&out, command)["error"]["code"], error);
                }
            } else {
                assert!(out.stdout.is_empty());
            }
            assert_eq!(records::read(&records).unwrap(), before);
        }
        let mut args = vec![command, "."];
        if command == "set" {
            args.extend(["--purpose", "missing"]);
        }
        if command == "exec" {
            args.extend(["--", "true"]);
        } else {
            args.push("--json");
        }
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&repo.home)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(3),
            "{args:?}: {}",
            text(&out.stderr)
        );
    }
    assert!(!repo.repo.join("should-not-exist").exists());
}

#[test]
fn primary_and_unmanaged_checkout_inspection_and_default_exec() {
    let repo = Repo::new();
    let unmanaged = repo.root.join("unmanaged");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "plain",
        unmanaged.to_str().unwrap(),
    ]);
    for (cwd, kind) in [(&repo.repo, "primary"), (&unmanaged, "workspace")] {
        let run = |args: &[&str]| {
            repo.command(env!("CARGO_BIN_EXE_rabi"), args)
                .current_dir(cwd)
                .output()
                .unwrap()
        };
        for selector in [vec![], vec!["."]] {
            let mut args = vec!["show"];
            args.extend(selector.clone());
            args.push("--json");
            let out = run(&args);
            assert!(out.status.success(), "{}", text(&out.stderr));
            let object = &assert_envelope(&out, "show")["data"]["workspace"];
            assert_eq!(object["managed"], false);
            assert_eq!(object["kind"], kind);
            assert_eq!(object["current"], true);
            assert_eq!(object["repo"]["root"], repo.repo.to_str().unwrap());
            for field in [
                "id",
                "purpose",
                "createdAt",
                "updatedAt",
                "createdFrom",
                "base",
                "upstream",
            ] {
                assert!(object[field].is_null(), "{field}: {object}");
            }
            let mut args = vec!["path"];
            args.extend(selector);
            let out = run(&args);
            assert_eq!(out.stdout, format!("{}\n", cwd.display()).as_bytes());
        }
        let out = run(&["exec", "--", "pwd"]);
        assert_eq!(out.status.code(), Some(0));
        assert!(same_dir(&text(&out.stdout), cwd));
        let out = run(&["exec", "--", "sh", "-c", "exit 37"]);
        assert_eq!(out.status.code(), Some(37));
    }
}

#[test]
fn set_unset_takes_one_field_and_preserves_the_following_selector() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "feature/auth"]).status.success());
    for args in [
        vec!["set", "--unset", "purpose", "feature/auth", "--json"],
        vec!["set", "--unset=purpose", "feature/auth", "--json"],
        vec![
            "set",
            "--unset",
            "purpose",
            "--unset",
            "base",
            "feature/auth",
            "--json",
        ],
        vec![
            "set",
            "--unset",
            "purpose",
            "feature/auth",
            "--unset",
            "base",
            "--json",
        ],
    ] {
        assert!(repo
            .rabi(&[
                "set",
                "feature/auth",
                "--purpose",
                "task",
                "--base",
                "main",
                "--pr",
                "42"
            ])
            .status
            .success());
        let out = repo.rabi(&args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stdout)
        );
        let value = assert_envelope(&out, "set");
        let workspace = &value["data"]["workspace"];
        assert_eq!(workspace["name"], "feature/auth");
        assert!(workspace["purpose"].is_null());
        if args.contains(&"base") {
            assert!(workspace["base"].is_null());
        } else {
            assert_eq!(workspace["base"], "refs/heads/main");
        }
        assert_eq!(workspace["links"]["pr"]["number"], 42);
        let primary = assert_envelope(&repo.rabi(&["show", "--json"]), "show");
        assert!(primary["data"]["workspace"]["id"].is_null());
    }
}

#[test]
fn set_updates_and_unsets_every_intent_field_atomically() {
    let repo = Repo::new();
    let created = json_value(&repo.rabi(&["create", "intent", "--json"]));
    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .current_dir(managed(&repo, "intent"))
            .output()
            .unwrap()
    };
    let out = run(&[
        "set",
        "--purpose",
        "new task",
        "--base",
        "main",
        "--pr",
        "42",
        "--issue",
        "https://github.com/acme/demo/issues/73",
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let value = assert_envelope(&out, "set");
    let workspace = &value["data"]["workspace"];
    assert_eq!(workspace["purpose"], "new task");
    assert_eq!(workspace["base"], "refs/heads/main");
    assert_eq!(workspace["links"]["pr"]["number"], 42);
    assert_eq!(workspace["links"]["issue"]["number"], 73);
    assert_eq!(workspace["id"], created["data"]["workspace"]["id"]);
    assert_eq!(
        workspace["createdAt"],
        created["data"]["workspace"]["createdAt"]
    );
    assert!(workspace["updatedAt"].is_string());
    let records = repo.repo.join(".git/rabi/workspaces");
    for args in [
        vec!["set", "--unset", "unknown", "--json"],
        vec!["set", "--base", "does-not-exist", "--json"],
        vec!["set", "--purpose", "x", "--unset", "purpose", "--json"],
    ] {
        let before = records::read(&records).unwrap();
        let out = run(&args);
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stdout)
        );
        assert_eq!(assert_envelope(&out, "set")["error"]["code"], "usage");
        assert_eq!(records::read(&records).unwrap(), before);
    }
    for field in ["purpose", "base", "pr", "issue"] {
        let out = run(&["set", "--unset", field, "--json"]);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let value = assert_envelope(&out, "set");
        let object = &value["data"]["workspace"];
        assert!(if matches!(field, "pr" | "issue") {
            &object["links"][field]
        } else {
            &object[field]
        }
        .is_null());
    }
    let out = run(&[
        "set", "--unset", "purpose", "--unset", "base", "--unset", "pr", "--unset", "issue",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
}

#[test]
fn inspection_usage_keeps_stdout_clean() {
    let repo = Repo::new();
    for args in [
        vec!["nonsense"],
        vec!["nonsense", "--help"],
        vec!["show", "--bogus"],
        vec!["show", "a", "b"],
        vec!["path", "--bogus"],
        vec!["set", "--unset", "bogus"],
        vec!["list", "--kind", "bogus"],
        vec!["exec", "main", "true"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}: {}", text(&out.stdout));
        if args[0] == "exec" {
            assert!(text(&out.stderr).contains("exec [SEL] --"));
        }
    }
    // Exec ignores the output default even for a missing command delimiter.
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["exec", "main", "true"])
        .env("RABI_OUTPUT", "json")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(text(&out.stderr).contains("exec [SEL] --"));
}

#[test]
fn inspection_leaves_orphaned_checkout_observations_null() {
    let repo = Repo::new();
    let created = repo.rabi(&["create", "gone", "--base", "main"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    repo.git(&["branch", "--set-upstream-to=main", "gone"]);
    let path = managed(&repo, "gone");
    repo.git(&["worktree", "remove", path.to_str().unwrap()]);
    let trace = repo.root.join("git-trace.json");
    for command in ["show", "list"] {
        let args = if command == "show" {
            vec!["show", "gone", "--json"]
        } else {
            vec!["list", "--json"]
        };
        fs::write(&trace, "").unwrap();
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("GIT_TRACE2_EVENT", &trace)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        let value = assert_envelope(&out, command);
        let workspace = if command == "show" {
            &value["data"]["workspace"]
        } else {
            value["data"]["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|entry| entry["name"] == "gone")
                .unwrap()
        };
        assert_eq!(workspace["orphaned"], true);
        for field in ["upstream", "branch", "head", "dirty", "detached"] {
            assert!(workspace[field].is_null(), "{command}: {field}");
        }
        let events = fs::read_to_string(&trace).unwrap();
        for line in events.lines() {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            if let Some(argv) = event["argv"].as_array() {
                assert!(
                    !argv
                        .iter()
                        .any(|arg| arg == "refs/heads/gone" || arg == path.to_str().unwrap()),
                    "{line}"
                );
            }
            assert_ne!(event["worktree"], path.to_string_lossy().as_ref(), "{line}");
        }
    }
}

#[test]
fn inspection_uses_worktree_config_for_upstream() {
    assert_inspection_uses_checkout_config(false);
}

#[test]
fn inspection_uses_gitdir_include_for_upstream() {
    assert_inspection_uses_checkout_config(true);
}

fn assert_inspection_uses_checkout_config(include: bool) {
    let repo = Repo::new();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://example.invalid/repo.git",
    ]);
    repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
    repo.git(&[
        "update-ref",
        "refs/remotes/origin/checkout-upstream",
        "HEAD",
    ]);
    let created = repo.rabi(&["create", "local", "--base", "main", "--json"]);
    assert!(created.status.success(), "{}", text(&created.stderr));
    let path = managed(&repo, "local");
    repo.git(&["config", "branch.local.remote", "origin"]);
    repo.git(&[
        "config",
        "branch.local.merge",
        "refs/heads/checkout-upstream",
    ]);
    repo.git(&["config", "branch.main.remote", "origin"]);
    repo.git(&["config", "branch.main.merge", "refs/heads/main"]);
    let git = |args: &[&str]| {
        let out = repo
            .command("git", args)
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        text(&out.stdout)
    };
    fs::write(path.join("local"), "local\n").unwrap();
    git(&["add", "local"]);
    git(&["commit", "-m", "local change"]);
    let head = git(&["rev-parse", "HEAD"]);
    fs::write(repo.repo.join("primary"), "primary\n").unwrap();
    repo.git(&["add", "primary"]);
    repo.git(&["commit", "-m", "primary change"]);
    repo.git(&["branch", "checkout-upstream", "main"]);
    if include {
        let config = repo.root.join("checkout.gitconfig");
        fs::write(
            &config,
            "[branch \"local\"]\nremote = .\nmerge = refs/heads/checkout-upstream\n[status]\nshowUntrackedFiles = no\n",
        ).unwrap();
        let git_dir = git(&["rev-parse", "--absolute-git-dir"]);
        repo.git(&[
            "config",
            &format!("includeIf.gitdir:{git_dir}.path"),
            config.to_str().unwrap(),
        ]);
    } else {
        repo.git(&["config", "extensions.worktreeConfig", "true"]);
        git(&["config", "--worktree", "branch.local.remote", "."]);
        git(&[
            "config",
            "--worktree",
            "branch.local.merge",
            "refs/heads/checkout-upstream",
        ]);
        git(&["config", "--worktree", "status.showUntrackedFiles", "no"]);
    }
    fs::write(
        path.join("untracked"),
        "ignored by checkout status config\n",
    )
    .unwrap();
    assert_eq!(
        git(&["rev-parse", "--symbolic-full-name", "local@{upstream}"]),
        "refs/heads/checkout-upstream"
    );
    assert_eq!(upstream_name(&repo, "local"), "origin/checkout-upstream");
    let store = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&store).unwrap();
    for gone in [false, true] {
        if gone {
            repo.git(&["branch", "-D", "checkout-upstream"]);
        }
        for cwd in [&repo.repo, &path] {
            for command in ["show", "list"] {
                let args = if command == "show" {
                    vec!["show", "local", "--json"]
                } else {
                    vec!["list", "--json"]
                };
                let out = repo
                    .command(env!("CARGO_BIN_EXE_rabi"), &args)
                    .current_dir(cwd)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{}", text(&out.stderr));
                let value = assert_envelope(&out, command);
                let workspace = if command == "show" {
                    &value["data"]["workspace"]
                } else {
                    let entries = value["data"]["workspaces"].as_array().unwrap();
                    let primary = entries
                        .iter()
                        .find(|entry| entry["kind"] == "primary")
                        .unwrap();
                    assert_eq!(
                        primary["upstream"],
                        serde_json::json!({"ref":"refs/remotes/origin/main", "ahead":1, "behind":0, "gone":false})
                    );
                    entries
                        .iter()
                        .find(|entry| entry["name"] == "local")
                        .unwrap()
                };
                let expected = if gone {
                    serde_json::json!({"ref":"refs/heads/checkout-upstream", "ahead":null, "behind":null, "gone":true})
                } else {
                    serde_json::json!({"ref":"refs/heads/checkout-upstream", "ahead":1, "behind":1, "gone":false})
                };
                assert_eq!(
                    workspace["upstream"], expected,
                    "{command}, include={include}, gone={gone}, cwd={cwd:?}"
                );
                assert_eq!(workspace["branch"], "local");
                assert_eq!(workspace["head"], head);
                // Inspection must detect real work even when checkout config hides it.
                assert_eq!(workspace["dirty"], true);
                assert_eq!(workspace["detached"], false);
            }
        }
    }
    assert_eq!(records::read(&store).unwrap(), before);
}

#[test]
fn list_filters_current_links_upstream_and_all_known_repositories() {
    let repo = Repo::new();
    let second = repo.root.join("second");
    fs::create_dir(&second).unwrap();
    assert!(repo
        .command("git", &["init", "-b", "main", second.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    for path in [&repo.repo, &second] {
        assert!(repo
            .rabi(&["repo", "add", path.to_str().unwrap()])
            .status
            .success());
    }
    assert!(repo
        .create_recorded(&[
            "create",
            "listed",
            "--purpose",
            "inspect me",
            "--pr",
            "42",
            "--issue",
            "73"
        ])
        .status
        .success());
    repo.git(&["branch", "--set-upstream-to=main", "listed"]);
    let path = managed(&repo, "listed");
    fs::write(path.join("ahead"), "ahead").unwrap();
    let git = |args: &[&str]| {
        let out = repo
            .command("git", args)
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    };
    git(&["add", "ahead"]);
    git(&["commit", "-m", "ahead"]);
    fs::write(repo.repo.join("behind"), "behind").unwrap();
    repo.git(&["add", "behind"]);
    repo.git(&["commit", "-m", "behind"]);
    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .current_dir(&path)
            .output()
            .unwrap()
    };
    let out = run(&["list", "--kind", "workspace", "--json"]);
    let value = assert_envelope(&out, "list");
    assert_eq!(value["data"]["workspaces"].as_array().unwrap().len(), 1);
    let workspace = &value["data"]["workspaces"][0];
    assert_eq!(workspace["current"], true);
    assert_eq!(
        workspace["upstream"],
        serde_json::json!({"ref":"refs/heads/main", "ahead":1, "behind":1, "gone":false})
    );
    assert_eq!(workspace["links"]["pr"]["number"], 42);
    for (flag, expected) in [
        ("--names", "listed".to_owned()),
        ("--paths", path.to_string_lossy().into_owned()),
    ] {
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["list", "--kind", "workspace", flag],
            )
            .env("RABI_OUTPUT", "json")
            .output()
            .unwrap();
        assert_eq!(text(&out.stdout), expected);
    }
    let out = run(&["list", "--kind", "primary", "--json"]);
    let value = assert_envelope(&out, "list");
    assert_eq!(value["data"]["workspaces"].as_array().unwrap().len(), 1);
    assert_eq!(value["data"]["workspaces"][0]["kind"], "primary");
    let out = run(&["list"]);
    let display = text(&out.stdout);
    for expected in [
        "primary:",
        "* listed",
        "↑1 ↓1",
        "#42",
        "issue #73",
        "inspect me",
    ] {
        assert!(display.contains(expected), "{display}");
    }
    repo.git(&["branch", "upstream-gone", "main"]);
    repo.git(&["branch", "--set-upstream-to=upstream-gone", "listed"]);
    repo.git(&["branch", "-D", "upstream-gone"]);
    let out = run(&["show", "listed", "--json"]);
    let value = assert_envelope(&out, "show");
    assert_eq!(
        value["data"]["workspace"]["upstream"],
        serde_json::json!({"ref":"refs/heads/upstream-gone", "ahead":null, "behind":null, "gone":true})
    );
    assert!(text(&run(&["list"]).stdout).contains("upstream:gone"));
    let out = run(&["list", "--all-repos", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let value = assert_envelope(&out, "list");
    assert_eq!(value["data"]["repos"].as_array().unwrap().len(), 2);
    assert_eq!(value["data"]["workspaces"].as_array().unwrap().len(), 3);
    let out = run(&["list", "--all-repos"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout).matches("(primary:").count(), 2);
    assert!(text(&out.stdout).contains("* listed"));
}

#[test]
fn linked_urls_respect_repository_identity_and_url_navigation() {
    let repo = Repo::new();
    repo.git(&["remote", "add", "origin", "git@github.com:acme/demo.git"]);
    assert!(repo
        .create_recorded(&["create", "linked", "--base", "main", "--pr", "42", "--issue", "73"])
        .status
        .success());
    for selector in [
        "https://github.com/acme/demo/pull/42",
        "https://github.com/acme/demo/issues/73?plain=1#details",
    ] {
        let out = repo.rabi(&["show", selector, "--json"]);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(
            json_value(&out)["data"]["workspace"]["repo"]["name"],
            "github.com/acme/demo"
        );
    }
    for selector in [
        "https://github.com/other/demo/pull/42",
        "https://github.com/acme/demo/issues/42",
    ] {
        let out = repo.rabi(&["show", selector, "--json"]);
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(json_value(&out)["error"]["code"], "not_found");
    }
    for selector in [
        "pr:0",
        "issue:nope",
        "pr:https://github.com/acme/demo/pull/42",
        "https://github.com/acme/demo/tree/42",
    ] {
        assert_eq!(
            repo.rabi(&["show", selector, "--json"]).status.code(),
            Some(2),
            "{selector}"
        );
    }
    assert!(repo
        .rabi(&[
            "set",
            "linked",
            "--pr",
            "https://github.com/acme/demo/pull/42"
        ])
        .status
        .success());
    assert_eq!(
        repo.rabi(&[
            "show",
            "https://github.com/acme/demo/pull/42?diff=split#discussion",
            "--json"
        ])
        .status
        .code(),
        Some(0)
    );
}

#[test]
fn dot_primary_can_be_set_and_synced_and_is_protected_from_deletion() {
    let repo = Repo::new();
    let out = repo.rabi(&["set", "--purpose", "primary context", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let value = assert_envelope(&out, "set");
    assert_eq!(value["data"]["workspace"]["kind"], "primary");
    assert_eq!(value["data"]["workspace"]["managed"], false);
    assert_eq!(value["data"]["workspace"]["purpose"], "primary context");
    let out = repo.rabi(&["sync", ".", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    let out = repo.rabi(&[
        "remove",
        ".",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(assert_envelope(&out, "remove")["error"]["code"], "primary");
    assert!(repo.repo.is_dir());
}

#[test]
fn all_repos_uses_enabled_discovery_without_importing_workspace_records() {
    let repo = Repo::new();
    let clones = repo.root.join("clones");
    let discovered = clones.join("github.com/acme/discovered");
    fs::create_dir_all(&discovered).unwrap();
    assert!(repo
        .command("git", &["init", "-b", "main", discovered.to_str().unwrap()])
        .output()
        .unwrap()
        .status
        .success());
    assert!(repo.rabi(&["repo", "add"]).status.success());
    assert!(repo
        .rabi(&["config", "set", "clone-root", clones.to_str().unwrap()])
        .status
        .success());
    let out = repo.rabi(&["list", "--all-repos", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let value = assert_envelope(&out, "list");
    assert_eq!(value["data"]["repos"].as_array().unwrap().len(), 2);
    assert!(value["data"]["repos"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["name"] == "github.com/acme/discovered"));
    assert!(repo
        .rabi(&["config", "set", "discover-ghq", "false"])
        .status
        .success());
    let out = repo.rabi(&["list", "--all-repos", "--json"]);
    assert_eq!(
        json_value(&out)["data"]["repos"].as_array().unwrap().len(),
        1
    );
    for args in [
        vec!["list", "--all-repos", "-R", repo.repo.to_str().unwrap()],
        vec!["list", "--names", "--paths"],
        vec!["list", "--paths", "--json"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2));
    }
}

#[test]
fn delete_link_selector_cannot_change_paths_after_a_hook_relinks_it() {
    let repo = Repo::new();
    for (name, pr) in [("one", "42"), ("two", "43")] {
        assert!(repo
            .create_recorded(&["create", name, "--pr", pr])
            .status
            .success());
    }
    let second = managed(&repo, "two");
    let out = repo
        .command("git", &["checkout", "--ignore-other-worktrees", "one"])
        .current_dir(&second)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let script = format!(
        "test \"$RABI_WORKSPACE\" = one && '{}' set one --unset pr && '{}' set two --pr 42",
        env!("CARGO_BIN_EXE_rabi"),
        env!("CARGO_BIN_EXE_rabi")
    );
    let config = format!(
        "[hooks]\npre-remove = [{}, \"printf '%s\\n' \\\"$RABI_WORKSPACE_PATH\\\" > hook-target\"]\n",
        serde_json::to_string(&script).unwrap()
    );
    fs::write(repo.repo.join(".rabi.toml"), config).unwrap();
    let out = repo.rabi(&[
        "remove",
        "pr:42",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "--keep-branch",
        "--json",
    ]);
    assert_ne!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert!(managed(&repo, "one").is_dir());
    assert!(second.is_dir());
    assert_eq!(
        fs::read_to_string(managed(&repo, "one").join("hook-target"))
            .unwrap()
            .trim(),
        managed(&repo, "one").to_str().unwrap()
    );
    assert!(!second.join("hook-target").exists());
    assert_eq!(
        json_value(&repo.rabi(&["show", "pr:42", "--json"]))["data"]["workspace"]["name"],
        "two"
    );
}

#[test]
fn set_invalid_base_reports_a_clear_error_without_git_stderr() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "topic"]).status.success());
    for json in [false, true] {
        let mut args = vec!["set", "topic", "--base", "does-not-exist"];
        if json {
            args.push("--json");
        }
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2));
        let message = if json {
            let doc = assert_envelope(&out, "set");
            assert_eq!(doc["error"]["code"], "usage");
            assert!(out.stderr.is_empty(), "{}", text(&out.stderr));
            doc["error"]["message"].as_str().unwrap().to_owned()
        } else {
            assert!(out.stdout.is_empty());
            text(&out.stderr)
        };
        assert!(
            message.contains("Base 'does-not-exist' is not a valid ref"),
            "{message}"
        );
        assert!(!message.contains("fatal:"), "{message}");
    }
}

#[test]
fn delete_stops_before_next_hook_when_checkout_directory_is_replaced() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "topic"]).status.success());
    let path = managed(&repo, "topic");
    let replace = "mv \"$RABI_WORKSPACE_PATH\" \"$RABI_WORKSPACE_PATH.old\" && mkdir \"$RABI_WORKSPACE_PATH\" && cp \"$RABI_WORKSPACE_PATH.old/.git\" \"$RABI_WORKSPACE_PATH/.git\"";
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!(
            "[hooks]\npre-remove = [{}, \"touch second-hook-ran\"]\n",
            serde_json::to_string(replace).unwrap()
        ),
    )
    .unwrap();
    let out = repo.rabi(&[
        "remove",
        "topic",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "--keep-branch",
        "--json",
    ]);
    assert_ne!(out.status.code(), Some(0));
    assert!(
        text(&out.stdout).contains("Workspace changed before running hook command"),
        "{}",
        text(&out.stdout)
    );
    assert!(path.is_dir());
    assert!(!path.join("second-hook-ran").exists());
}

#[test]
fn delete_recorded_workspaces_with_hooks() {
    assert_delete_with_hooks(false);
}

#[test]
fn delete_unrecorded_workspaces_can_be_recorded_inside_hook() {
    assert_delete_with_hooks(true);
}

fn assert_delete_with_hooks(record_in_hook: bool) {
    let repo = Repo::new();
    let paths = [managed(&repo, "one"), managed(&repo, "two")];
    for (name, path) in ["one", "two"].iter().zip(&paths) {
        repo.git(&["worktree", "add", "-b", name, path.to_str().unwrap()]);
    }
    if !record_in_hook {
        for path in &paths {
            rabi::metadata::write(&repo.repo, path, &Default::default()).unwrap();
        }
    }
    let first = if record_in_hook {
        format!("'{}' set . --purpose recorded", env!("CARGO_BIN_EXE_rabi"))
    } else {
        "true".to_owned()
    };
    let commands = [
        first,
        "printf '%s\\n' \"$RABI_WORKSPACE\" >> \"$RABI_REPO_PATH/hooks-ran\"".to_owned(),
    ];
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!(
            "[hooks]\npre-remove = {}\n",
            serde_json::to_string(&commands).unwrap()
        ),
    )
    .unwrap();
    // Exercise persisted trust rather than the test helper's trust-all override.
    assert!(repo.rabi(&["trust", "add"]).status.success());
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "remove",
                "one",
                "two",
                "--discard-changes",
                "--delete-unpreserved-branch",
                "--keep-branch",
                "--json",
            ],
        )
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "record_in_hook={record_in_hook}: {} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.repo.join("hooks-ran")).unwrap(),
        "one\ntwo\n"
    );
    assert!(paths.iter().all(|path| !path.exists()));
    let records = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    let records: serde_json::Value = serde_json::from_slice(&records).unwrap();
    assert!(records["worktrees"].as_array().unwrap().is_empty());
}

#[test]
fn delete_stops_when_a_new_record_is_replaced_between_hooks() {
    let repo = Repo::new();
    let path = managed(&repo, "topic");
    repo.git(&["worktree", "add", "-b", "topic", path.to_str().unwrap()]);
    let bin = env!("CARGO_BIN_EXE_rabi");
    let commands = [
        format!("'{bin}' set . --purpose recorded"),
        format!("rm \"$RABI_REPO_PATH/.git/rabi/workspaces/$RABI_WORKSPACE_ID.json\" && '{bin}' set . --purpose replacement"),
        "touch third-hook-ran".to_owned(),
    ];
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!(
            "[hooks]\npre-remove = {}\n",
            serde_json::to_string(&commands).unwrap()
        ),
    )
    .unwrap();
    let out = repo.rabi(&[
        "remove",
        "topic",
        "--discard-changes",
        "--delete-unpreserved-branch",
        "--keep-branch",
        "--json",
    ]);
    assert_ne!(out.status.code(), Some(0));
    assert!(
        text(&out.stdout).contains("Workspace changed before running hook command"),
        "{}",
        text(&out.stdout)
    );
    assert!(path.is_dir());
    assert!(!path.join("third-hook-ran").exists());
    let out = repo.rabi(&["show", "topic", "--json"]);
    assert_eq!(
        assert_envelope(&out, "show")["data"]["workspace"]["purpose"],
        "replacement"
    );
}

#[test]
fn selector_urls_strip_the_final_link_suffix_for_reserved_repository_names() {
    for (repo_name, kind, flag, number) in [
        ("issues", "issues", "--issue", "73"),
        ("pull", "pull", "--pr", "5"),
        ("issue", "issue", "--issue", "73"),
        ("pulls", "pulls", "--pr", "5"),
    ] {
        for url_link in [false, true] {
            let repo = Repo::new();
            repo.git(&[
                "remote",
                "add",
                "origin",
                &format!("git@github.com:acme/{repo_name}.git"),
            ]);
            let url = format!("https://github.com/acme/{repo_name}/{kind}/{number}");
            let link = if url_link { &url } else { number };
            assert!(repo
                .create_recorded(&["create", "linked", "--base", "main", flag, link])
                .status
                .success());
            for selector in [url.clone(), format!("{url}/?plain=1#details")] {
                let out = repo.rabi(&["show", &selector, "--json"]);
                assert_eq!(
                    out.status.code(),
                    Some(0),
                    "{selector}, url_link={url_link}: {} {}",
                    text(&out.stdout),
                    text(&out.stderr)
                );
                assert_eq!(
                    assert_envelope(&out, "show")["data"]["workspace"]["name"],
                    "linked"
                );
            }
            let foreign = format!("https://github.com/other/{repo_name}/{kind}/{number}");
            let out = repo.rabi(&["show", &foreign, "--json"]);
            assert_eq!(out.status.code(), Some(3));
            assert_eq!(assert_envelope(&out, "show")["error"]["code"], "not_found");
        }
    }
}

#[test]
fn selector_urls_use_the_terminal_pair_for_selection_and_deletion() {
    for (kind, flag) in [
        ("pull", "--pr"),
        ("pulls", "--pr"),
        ("issues", "--issue"),
        ("issue", "--issue"),
    ] {
        for url_link in [false, true] {
            let repo = Repo::new();
            repo.git(&[
                "remote",
                "add",
                "origin",
                "git@git.example.com:pull/123.git",
            ]);
            let url = format!("https://git.example.com/pull/123/{kind}/42");
            assert!(repo
                .create_recorded(&["create", "earlier", "--base", "main", flag, "123"])
                .status
                .success());
            assert!(repo
                .create_recorded(&[
                    "create",
                    "terminal",
                    "--base",
                    "main",
                    flag,
                    if url_link { &url } else { "42" }
                ])
                .status
                .success());
            for selector in [url.clone(), format!("{url}/?plain=1#details")] {
                let out = repo.rabi(&["show", &selector, "--json"]);
                assert_eq!(
                    out.status.code(),
                    Some(0),
                    "{} {}",
                    text(&out.stdout),
                    text(&out.stderr)
                );
                assert_eq!(
                    assert_envelope(&out, "show")["data"]["workspace"]["name"],
                    "terminal",
                    "{selector}, url_link={url_link}"
                );
            }
            let out = repo.rabi(&[
                "remove",
                &url,
                "--discard-changes",
                "--delete-unpreserved-branch",
                "--keep-branch",
                "--json",
            ]);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            assert_eq!(
                assert_envelope(&out, "remove")["data"]["items"][0]["workspace"]["name"],
                "terminal"
            );
            assert!(managed(&repo, "earlier").is_dir());
            assert!(!managed(&repo, "terminal").exists());
        }
    }
}

#[test]
fn create_and_set_save_link_numbers_from_the_terminal_pair() {
    for (kind, flag, field) in [
        ("pull", "--pr", "pr"),
        ("pulls", "--pr", "pr"),
        ("issues", "--issue", "issue"),
        ("issue", "--issue", "issue"),
    ] {
        let repo = Repo::new();
        for (command, number) in [("create", 42), ("set", 43)] {
            let url = format!("https://git.example.com/pull/123/{kind}/{number}/?plain=1#details");
            let out = if command == "create" {
                repo.create_recorded(&[command, "linked", flag, &url, "--json"])
            } else {
                repo.rabi(&[command, "linked", flag, &url, "--json"])
            };
            assert_eq!(
                out.status.code(),
                Some(0),
                "{} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            let workspace =
                &assert_envelope(&out, if command == "create" { "show" } else { command })["data"]
                    ["workspace"];
            assert_eq!(workspace["links"][field]["number"], number);
            assert_eq!(workspace["links"][field]["url"], url);
            let out = repo.rabi(&["show", "linked", "--json"]);
            assert_eq!(
                assert_envelope(&out, "show")["data"]["workspace"]["links"][field]["number"],
                number
            );
        }
    }
}

#[test]
fn delete_current_does_not_revalidate_the_resolved_branch_name() {
    let repo = Repo::new();
    let tree = managed(&repo, "topic+fix");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic+fix",
        tree.to_str().unwrap(),
        "main",
    ]);
    fs::write(tree.join("dirty"), "local changes\n").unwrap();
    let refused = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["remove", "."])
        .current_dir(&tree)
        .output()
        .unwrap();
    assert_eq!(refused.status.code(), Some(4));
    assert!(text(&refused.stderr).contains("topic+fix"));
    fs::remove_file(tree.join("dirty")).unwrap();
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["remove", ".", "--json"])
        .current_dir(&tree)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(
        assert_envelope(&out, "remove")["data"]["items"][0]["workspace"]["branch"],
        "topic+fix"
    );
    assert!(!tree.exists());
    assert!(!branch_listed(&repo, "topic+fix"));
}

#[test]
fn github_rechecks_out_a_pull_request_after_git_worktree_remove() {
    assert_github_recreates_orphaned_checkout(false);
}

#[test]
fn github_recreates_an_issue_checkout_after_its_worktree_and_branch_are_removed() {
    assert_github_recreates_orphaned_checkout(true);
}

fn assert_github_recreates_orphaned_checkout(issue: bool) {
    let repo = Repo::new();
    publish_pr_topic(&repo);
    let path = fake_gh_for_topic_pr(&repo);
    let branch = if issue { "issues/7" } else { "topic" };
    let link = if issue { "issue" } else { "pr" };
    if issue {
        let gh = repo.root.join("bin/gh");
        let script = fs::read_to_string(&gh).expect("gh script");
        fs::write(
            &gh,
            script.replace("\"pull_request\":{}", "\"title\":\"issue\""),
        )
        .expect("issue");
    }
    let checkout = || {
        repo.command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "create",
                if issue { "--issue" } else { "--pr" },
                "7",
                "--json",
            ],
        )
        .env("PATH", &path)
        .output()
        .expect("checkout")
    };
    let first = checkout();
    assert!(first.status.success(), "{}", text(&first.stdout));
    let first = assert_envelope(&first, "create");
    assert_eq!(first["data"]["source"]["reused"], false);
    let directory = managed(&repo, branch);
    repo.git(&["worktree", "remove", directory.to_str().expect("path")]);
    assert!(!directory.exists());
    assert!(branch_listed(&repo, branch));
    // Issue checkout creates a new branch, so keep its existing branch-conflict policy.
    if issue {
        repo.git(&["branch", "-D", branch]);
    }
    let stale = repo.rabi(&["show", branch, "--json"]);
    assert!(stale.status.success(), "{}", text(&stale.stdout));
    assert_eq!(json_value(&stale)["data"]["workspace"]["orphaned"], true);

    let again = checkout();
    assert!(
        again.status.success(),
        "{}\n{}",
        text(&again.stdout),
        text(&again.stderr)
    );
    let again = assert_envelope(&again, "create");
    assert_eq!(again["data"]["source"]["reused"], false);
    assert_eq!(again["data"]["workspace"]["orphaned"], false);
    assert_eq!(again["data"]["workspace"]["branch"], branch);
    assert_eq!(again["data"]["workspace"]["links"][link]["number"], 7);
    assert_eq!(again["data"]["metadata"]["ok"], true);
    assert!(directory.join(".git").is_file());
    if !issue {
        assert!(directory.join("topic.txt").is_file());
        assert_eq!(upstream_name(&repo, branch), "origin/topic");
    }
    let listed = repo.rabi(&["list", "--kind", "workspace", "--json"]);
    assert!(listed.status.success(), "{}", text(&listed.stdout));
    let listed = json_value(&listed);
    assert_eq!(
        listed["data"]["workspaces"]
            .as_array()
            .expect("workspaces")
            .len(),
        1
    );
    assert_eq!(listed["data"]["workspaces"][0]["orphaned"], false);
}

#[test]
fn github_reuse_propagates_orphan_ambiguity_and_git_read_errors() {
    for issue in [false, true] {
        for read_error in [false, true] {
            let repo = Repo::new();
            let path = fake_gh_for_topic_pr(&repo);
            let name = if issue { "issues/7" } else { "topic" };
            if issue {
                let gh = repo.root.join("bin/gh");
                let script = fs::read_to_string(&gh).expect("gh script");
                fs::write(
                    &gh,
                    script.replace("\"pull_request\":{}", "\"title\":\"issue\""),
                )
                .expect("issue");
            }
            let file = repo.repo.join(".git/rabi/workspaces");
            let body = serde_json::json!({"worktrees": [
                {"schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()), "path": repo.root.join("missing-one"), "name": name},
                {"schema": 2, "id": format!("ws_{}", uuid::Uuid::new_v4()), "path": repo.root.join("missing-two"), "name": name}
            ]})
            .to_string();
            records::write(&file, body).expect("records");
            let before = records::read(&file).expect("records");
            let real_git = repo
                .command("sh", &["-c", "command -v git"])
                .output()
                .expect("git path");
            assert!(real_git.status.success());
            if read_error {
                let shim = repo.root.join("bin/git");
                fs::write(
                    &shim,
                    r#"#!/bin/sh
if [ "$1" = "worktree" ] && [ "$2" = "list" ]; then
  echo 'injected worktree list failure' >&2
  exit 1
fi
exec "$REAL_GIT" "$@"
"#,
                )
                .expect("git shim");
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).expect("chmod");
            }
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\nsetup = [\"touch should-not-run\"]\n",
            )
            .expect("hooks");
            let result = repo
                .command(
                    env!("CARGO_BIN_EXE_rabi"),
                    &[
                        "create",
                        if issue { "--issue" } else { "--pr" },
                        "7",
                        "--reuse",
                        "--json",
                    ],
                )
                .env("PATH", &path)
                .env("REAL_GIT", text(&real_git.stdout))
                .env_remove("RABI_TRUST_ALL")
                .output()
                .expect("checkout");
            assert_eq!(
                result.status.code(),
                Some(if read_error { 1 } else { 2 }),
                "{}",
                text(&result.stdout)
            );
            let value = assert_envelope(&result, "create");
            assert_eq!(
                value["error"]["code"],
                if read_error { "failed" } else { "ambiguous" }
            );
            if read_error {
                assert!(value["error"]["message"]
                    .as_str()
                    .expect("message")
                    .contains("injected worktree list failure"));
            }
            assert_eq!(records::read(&file).expect("unchanged records"), before);
            assert!(!managed(&repo, name).exists());
            assert!(!branch_listed(&repo, name));
        }
    }
}

#[test]
fn github_reuses_plain_git_checkout_with_a_git_only_branch_name() {
    let repo = Repo::new();
    let path = fake_gh_for_topic_pr(&repo);
    let gh = repo.root.join("bin/gh");
    let script = fs::read_to_string(&gh).expect("gh script");
    fs::write(&gh, script.replace("\"topic\"", "\"topic+fix\"")).expect("PR head");
    let checkout = repo.root.join("plain-checkout");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic+fix",
        checkout.to_str().expect("path"),
        "HEAD",
    ]);
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch should-not-run\"]\n",
    )
    .expect("hooks");
    let before = repo
        .command("git", &["worktree", "list", "--porcelain"])
        .output()
        .expect("list");
    let result = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &path)
        .env_remove("RABI_TRUST_ALL")
        .output()
        .expect("checkout");
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout),
        text(&result.stderr)
    );
    let value = json_value(&result);
    assert_eq!(value["schema"], 2);
    assert_eq!(value["data"]["source"]["reused"], true);
    assert_eq!(value["data"]["workspace"]["branch"], "topic+fix");
    assert!(paths_match(
        value["data"]["workspace"]["path"].as_str().expect("path"),
        &checkout
    ));
    assert_eq!(value["data"]["workspace"]["links"]["pr"]["number"], 7);
    assert_eq!(value["data"]["setup"]["ran"], false);
    assert!(!checkout.join("should-not-run").exists());
    let after = repo
        .command("git", &["worktree", "list", "--porcelain"])
        .output()
        .expect("list");
    assert_eq!(before.stdout, after.stdout);
    assert!(!managed(&repo, "topic+fix").exists());
    let linked = repo.rabi(&["path", "pr:7"]);
    assert!(linked.status.success(), "{}", text(&linked.stderr));
    assert!(paths_match(&text(&linked.stdout), &checkout));
}

#[test]
fn github_propagates_ambiguous_internal_lookups() {
    for issue in [false, true] {
        let repo = Repo::new();
        let path = fake_gh_for_topic_pr(&repo);
        let branch = if issue { "issues/7" } else { "topic" };
        if issue {
            let gh = repo.root.join("bin/gh");
            let script = fs::read_to_string(&gh).expect("gh script");
            fs::write(
                &gh,
                script.replace("\"pull_request\":{}", "\"title\":\"issue\""),
            )
            .expect("issue");
        }
        repo.git(&["branch", branch]);
        for directory in ["first", "second"] {
            let checkout = repo.root.join(directory);
            repo.git(&[
                "worktree",
                "add",
                "--force",
                checkout.to_str().expect("path"),
                branch,
            ]);
        }
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = [\"touch should-not-run\"]\n",
        )
        .expect("hooks");
        let result = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &[
                    "create",
                    if issue { "--issue" } else { "--pr" },
                    "7",
                    "--reuse",
                    "--json",
                ],
            )
            .env("PATH", &path)
            .env_remove("RABI_TRUST_ALL")
            .output()
            .expect("checkout");
        assert_eq!(
            result.status.code(),
            Some(2),
            "{}\n{}",
            text(&result.stdout),
            text(&result.stderr)
        );
        assert_eq!(json_value(&result)["error"]["code"], "ambiguous");
        assert!(!repo.repo.join(".git/rabi").exists());
    }
}

#[test]
fn gc_removes_a_recorded_git_only_branch_name() {
    let repo = Repo::new();
    let checkout = managed(&repo, "topic+fix");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic+fix",
        checkout.to_str().expect("path"),
        "HEAD",
    ]);
    let recorded = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["set", "--purpose", "internal lookup"],
        )
        .current_dir(&checkout)
        .output()
        .expect("record");
    assert!(recorded.status.success(), "{}", text(&recorded.stderr));
    land_branch(&repo, "topic+fix");
    let result = repo.rabi(&["gc", "--apply"]);
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout),
        text(&result.stderr)
    );
    assert!(
        text(&result.stdout).contains("removed topic+fix [merged]"),
        "{}",
        text(&result.stdout)
    );
    assert!(!checkout.exists());
    assert!(!branch_listed(&repo, "topic+fix"));
}

#[test]
fn user_selectors_still_reject_git_only_branch_names() {
    let repo = Repo::new();
    for args in [
        vec!["create", "topic+fix", "--json"],
        vec!["create", "--branch", "topic+fix", "--json"],
        vec!["show", "topic+fix", "--json"],
        vec!["path", "topic+fix", "--json"],
        vec!["set", "topic+fix", "--purpose", "task", "--json"],
        vec!["sync", "topic+fix", "--json"],
        vec!["remove", "topic+fix", "--json"],
        vec!["exec", "topic+fix", "--", "true"],
    ] {
        let result = repo.rabi(&args);
        assert_eq!(
            result.status.code(),
            Some(2),
            "{args:?}: {}\n{}",
            text(&result.stdout),
            text(&result.stderr)
        );
        if args.contains(&"--json") {
            assert_eq!(json_value(&result)["error"]["code"], "usage", "{args:?}");
        } else {
            assert!(text(&result.stderr).contains("Workspace name can only contain"));
        }
    }
}

#[test]
fn github_attaches_a_git_only_head_branch_without_user_name_validation() {
    let repo = Repo::new();
    publish_pr_topic(&repo);
    repo.git(&[
        "push",
        "origin",
        "refs/remotes/origin/topic:refs/heads/topic+fix",
    ]);
    let path = fake_gh_for_topic_pr(&repo);
    let gh = repo.root.join("bin/gh");
    let script = fs::read_to_string(&gh).expect("gh script");
    fs::write(&gh, script.replace("\"topic\"", "\"topic+fix\"")).expect("PR head");
    let result = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--reuse", "--json"],
        )
        .env("PATH", &path)
        .output()
        .expect("checkout");
    assert!(
        result.status.success(),
        "{}\n{}",
        text(&result.stdout),
        text(&result.stderr)
    );
    let value = json_value(&result);
    assert_eq!(value["data"]["source"]["reused"], false);
    assert_eq!(value["data"]["workspace"]["branch"], "topic+fix");
    assert!(managed(&repo, "topic+fix").join("topic.txt").is_file());
    assert_eq!(upstream_name(&repo, "topic+fix"), "origin/topic+fix");
}

#[test]
fn unified_create_reuse_is_idempotent_and_never_repeats_setup() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".gitignore"), "secret\n").unwrap();
    fs::write(repo.repo.join("secret"), "first").unwrap();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"echo setup >> calls; exit 21\"]\n",
    )
    .unwrap();
    let first = repo.rabi(&[
        "create",
        "retry",
        "--base",
        "main",
        "--copy-file",
        "secret",
        "--json",
    ]);
    assert_eq!(first.status.code(), Some(6));
    let first = assert_envelope(&first, "create");
    assert_eq!(first["data"]["source"]["kind"], "new");
    assert_eq!(first["data"]["source"]["base"], "refs/heads/main");
    assert_eq!(first["data"]["setup"]["exitCode"], 21);
    assert_eq!(
        first["data"]["copy"]["copied"],
        serde_json::json!(["secret"])
    );
    let records = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    fs::write(repo.repo.join("secret"), "changed").unwrap();
    for _ in 0..2 {
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &[
                    "create",
                    "retry",
                    "--base",
                    "main",
                    "--copy-file",
                    "secret",
                    "--reuse",
                    "--json",
                ],
            )
            .env_remove("RABI_TRUST_ALL")
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, "create");
        assert_eq!(doc["data"]["source"]["reused"], true);
        assert_eq!(doc["data"]["setup"]["ran"], false);
        assert_eq!(doc["data"]["copy"]["copied"], serde_json::json!([]));
        assert_eq!(
            doc["data"]["workspace"]["id"],
            first["data"]["workspace"]["id"]
        );
    }
    assert_eq!(
        fs::read_to_string(managed(&repo, "retry/calls")).unwrap(),
        "setup\n"
    );
    assert_eq!(
        fs::read_to_string(managed(&repo, "retry/secret")).unwrap(),
        "first"
    );
    assert_eq!(
        records::read(repo.repo.join(".git/rabi/workspaces")).unwrap(),
        records
    );
    repo.git(&["branch", "another"]);
    for args in [
        vec!["create", "retry", "--base", "another", "--reuse", "--json"],
        vec![
            "create", "retry", "--branch", "another", "--reuse", "--json",
        ],
        vec!["create", "retry", "--detach", "--reuse", "--json"],
        vec!["create", "retry", "--json"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "exists");
    }
}

#[test]
fn unified_create_uses_the_default_base_and_supports_named_branch_and_detach() {
    let repo = Repo::new();
    repo.git(&["remote", "add", "origin", repo.repo.to_str().unwrap()]);
    repo.git(&["fetch", "origin"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    empty_commit(&repo, &repo.repo, "local only");
    let default_head = text(
        &repo
            .command("git", &["rev-parse", "refs/remotes/origin/main"])
            .output()
            .unwrap()
            .stdout,
    );
    let out = repo.rabi(&["create", "from-default", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["workspace"]["head"], default_head);
    assert_eq!(doc["data"]["workspace"]["base"], "refs/remotes/origin/main");
    for _ in 0..2 {
        let out = repo.rabi(&[
            "create", "snapshot", "--detach", "--base", "main", "--reuse", "--json",
        ]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = json_value(&out);
        assert_eq!(doc["data"]["source"]["kind"], "detach");
        assert_eq!(doc["data"]["workspace"]["branch"], serde_json::Value::Null);
        assert_eq!(doc["data"]["workspace"]["detached"], true);
        assert_eq!(doc["data"]["workspace"]["base"], "refs/heads/main");
    }
    repo.git(&["branch", "existing"]);
    for _ in 0..2 {
        let out = repo.rabi(&[
            "create", "review", "--branch", "existing", "--issue", "12", "--reuse", "--json",
        ]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = json_value(&out);
        assert_eq!(doc["data"]["source"]["kind"], "branch");
        assert_eq!(doc["data"]["source"]["branch"], "existing");
        assert_eq!(doc["data"]["workspace"]["name"], "review");
        assert_eq!(doc["data"]["workspace"]["links"]["issue"]["number"], 12);
    }
}

#[test]
fn review10_create_pr_accepts_repository_url_case_variants() {
    assert_review10_source_url_case_variants(true);
}

#[test]
fn review10_create_issue_accepts_repository_url_case_variants() {
    assert_review10_source_url_case_variants(false);
}

fn assert_review10_source_url_case_variants(pr: bool) {
    let repo = Repo::new();
    let path = fake_gh_for_topic_pr(&repo);
    let gh = repo.root.join("bin/gh");
    let script = fs::read_to_string(&gh)
        .unwrap()
        .replace("acme/demo", "Acme/Demo")
        .replace("\"login\":\"acme\"", "\"login\":\"Acme\"")
        .replace("\"name\":\"demo\"", "\"name\":\"Demo\"");
    if pr {
        publish_pr_topic(&repo);
        fs::write(&gh, script).unwrap();
    } else {
        fs::write(
            &gh,
            script.replace("\"pull_request\":{}", "\"title\":\"Fix issue\""),
        )
        .unwrap();
    }
    let kind = if pr { "pull" } else { "issues" };
    let link_key = if pr { "pr" } else { "issue" };
    let canonical_url = format!("https://github.com/Acme/Demo/{kind}/7");
    let mut workspace_id = None;
    for (index, (repository, suffix)) in [
        ("github.com/Acme/demo", ""),
        ("github.com/acme/Demo", ""),
        ("GitHub.com/Acme/Demo", ""),
        ("GitHub.com/aCmE/dEmO", "/?plain=1#details"),
        ("github.com/Acme/Demo", "#details"),
    ]
    .into_iter()
    .enumerate()
    {
        let url = format!("https://{repository}/{kind}/7{suffix}");
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &[
                    "create",
                    "review",
                    if pr { "--pr" } else { "--issue" },
                    &url,
                    "--reuse",
                    "--no-setup",
                    "--json",
                ],
            )
            .env("PATH", &path)
            .output()
            .unwrap();
        assert!(out.status.success(), "{url}: {}", text(&out.stdout));
        let doc = assert_envelope(&out, "create");
        assert_eq!(doc["data"]["source"]["kind"], link_key);
        assert_eq!(doc["data"]["source"]["number"], 7);
        assert_eq!(doc["data"]["source"]["reused"], index > 0);
        let workspace = &doc["data"]["workspace"];
        assert_eq!(workspace["links"][link_key]["url"], canonical_url);
        assert_eq!(workspace["links"][link_key]["number"], 7);
        if let Some(id) = &workspace_id {
            assert_eq!(&workspace["id"], id);
        } else {
            workspace_id = Some(workspace["id"].clone());
        }
    }
}

#[test]
fn review10_create_rejects_foreign_repository_host_and_source_kind_urls() {
    let repo = Repo::new();
    let path = fake_gh_for_topic_pr(&repo);
    let gh = repo.root.join("bin/gh");
    let script = fs::read_to_string(&gh).unwrap();
    fs::write(
        &gh,
        script.replace(
            "#!/bin/sh\n",
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$RABI_REVIEW10_GH_LOG\"\n",
        ),
    )
    .unwrap();
    let log = repo.root.join("gh.log");
    for pr in [true, false] {
        let kind = if pr { "pull" } else { "issues" };
        let wrong_kind = if pr { "issues" } else { "pull" };
        for url in [
            format!("https://github.com/other/demo/{kind}/7"),
            format!("https://github.com/acme/other/{kind}/7"),
            format!("https://other.example/acme/demo/{kind}/7"),
            format!("https://github.com/acme/demo/{wrong_kind}/7"),
            format!(
                "https://github.com/acme/demo/{}/7",
                if pr { "pulls" } else { "issue" }
            ),
            format!("http://github.com/acme/demo/{kind}/7"),
        ] {
            fs::write(&log, "").unwrap();
            let out = repo
                .command(
                    env!("CARGO_BIN_EXE_rabi"),
                    &[
                        "create",
                        "review",
                        if pr { "--pr" } else { "--issue" },
                        &url,
                        "--json",
                    ],
                )
                .env("PATH", &path)
                .env("RABI_REVIEW10_GH_LOG", &log)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(2), "{url}: {}", text(&out.stdout));
            assert_eq!(assert_envelope(&out, "create")["error"]["code"], "usage");
            assert_eq!(
                fs::read_to_string(&log).unwrap(),
                "repo view --json url,owner,name\n"
            );
            assert!(!managed(&repo, "review").exists());
            assert!(!repo.repo.join(".git/rabi/workspaces").exists());
        }
    }
}

#[test]
fn unified_create_issue_defaults_and_pr_issue_links() {
    let repo = Repo::new();
    let path = fake_gh_for_topic_pr(&repo);
    let gh = repo.root.join("bin/gh");
    let script = fs::read_to_string(&gh).unwrap();
    fs::write(
        &gh,
        script.replace("\"pull_request\":{}", "\"title\":\"Fix issue\""),
    )
    .unwrap();
    let run = |args: &[&str]| {
        repo.command(env!("CARGO_BIN_EXE_rabi"), args)
            .env("PATH", &path)
            .output()
            .unwrap()
    };
    for _ in 0..2 {
        let out = run(&[
            "create",
            "--issue",
            "https://github.com/acme/demo/issues/7",
            "--base",
            "main",
            "--reuse",
            "--json",
        ]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = json_value(&out);
        assert_eq!(doc["data"]["source"]["kind"], "issue");
        assert_eq!(doc["data"]["source"]["number"], 7);
        assert_eq!(doc["data"]["workspace"]["name"], "issues/7");
        assert_eq!(doc["data"]["workspace"]["purpose"], "Fix issue");
    }
    fs::write(&gh, script).unwrap();
    publish_pr_topic(&repo);
    for _ in 0..2 {
        let out = run(&[
            "create",
            "review-pr",
            "--pr",
            "https://github.com/acme/demo/pull/7",
            "--issue",
            "12",
            "--reuse",
            "--json",
        ]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = json_value(&out);
        assert_eq!(doc["data"]["source"]["kind"], "pr");
        assert_eq!(doc["data"]["source"]["number"], 7);
        assert_eq!(doc["data"]["source"]["headRef"], "topic");
        assert_eq!(doc["data"]["workspace"]["links"]["issue"]["number"], 12);
    }
    let conflict = run(&[
        "create",
        "review-pr",
        "--pr",
        "7",
        "--issue",
        "13",
        "--reuse",
        "--json",
    ]);
    assert_eq!(conflict.status.code(), Some(4));
    assert_eq!(json_value(&conflict)["error"]["code"], "exists");
}

#[test]
fn adopt_and_forget_preserve_git_and_skip_setup() {
    let repo = Repo::new();
    let outside = repo.root.join("plain");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic+fix",
        outside.to_str().unwrap(),
        "main",
    ]);
    fs::write(outside.join("local"), "keep").unwrap();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch setup-ran\"]\npre-remove = [\"exit 23\"]\n",
    )
    .unwrap();
    let adopt = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "adopt",
                outside.to_str().unwrap(),
                "--name",
                "adopted",
                "--base",
                "main",
                "--purpose",
                "existing work",
                "--pr",
                "12",
                "--issue",
                "34",
                "--json",
            ],
        )
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert!(adopt.status.success(), "{}", text(&adopt.stdout));
    let doc = assert_envelope(&adopt, "adopt");
    assert_eq!(doc["data"]["workspace"]["name"], "adopted");
    assert_eq!(doc["data"]["workspace"]["managed"], true);
    assert_eq!(
        doc["data"]["workspace"]["createdAt"],
        serde_json::Value::Null
    );
    assert_eq!(
        doc["data"]["workspace"]["createdFrom"],
        serde_json::Value::Null
    );
    assert_eq!(doc["data"]["workspace"]["base"], "refs/heads/main");
    assert!(!outside.join("setup-ran").exists());
    let again = repo.rabi(&["adopt", outside.to_str().unwrap(), "--json"]);
    assert_eq!(again.status.code(), Some(4));
    assert_eq!(json_value(&again)["error"]["code"], "exists");
    assert!(repo
        .rabi(&["set", "main", "--issue", "99"])
        .status
        .success());
    let before = repo
        .command("git", &["worktree", "list", "--porcelain"])
        .output()
        .unwrap()
        .stdout;
    let forgotten = repo.rabi(&["forget", "pr:12", "issue:34", ".", "--json"]);
    assert!(forgotten.status.success(), "{}", text(&forgotten.stdout));
    let doc = assert_envelope(&forgotten, "forget");
    let items = doc["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|item| item["ok"] == true));
    assert_eq!(
        repo.command("git", &["worktree", "list", "--porcelain"])
            .output()
            .unwrap()
            .stdout,
        before
    );
    assert_eq!(fs::read_to_string(outside.join("local")).unwrap(), "keep");
    assert!(branch_listed(&repo, "topic+fix"));
    assert!(
        records::read_to_string(repo.repo.join(".git/rabi/workspaces"))
            .unwrap()
            .contains("\"worktrees\": []")
    );
    let show = repo.rabi(&["show", ".", "--json"]);
    assert_eq!(
        json_value(&show)["data"]["workspace"]["purpose"],
        serde_json::Value::Null
    );
}

#[test]
fn adopt_rejects_foreign_primary_subdirectories_missing_paths_and_name_collisions() {
    let repo = Repo::new();
    let foreign = Repo::new();
    let plain = repo.root.join("plain");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "existing",
        plain.to_str().unwrap(),
        "main",
    ]);
    fs::create_dir_all(plain.join("subdir")).unwrap();
    for path in [
        &repo.repo,
        &foreign.repo,
        &plain.join("subdir"),
        &repo.root.join("missing"),
    ] {
        let out = repo.rabi(&["adopt", path.to_str().unwrap(), "--base", "main", "--json"]);
        assert_eq!(out.status.code(), Some(3), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "not_found");
    }
    assert!(repo.rabi(&["create", "taken"]).status.success());
    let out = repo.rabi(&[
        "adopt",
        plain.to_str().unwrap(),
        "--name",
        "taken",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(4));
    let out = repo.rabi(&[
        "adopt",
        plain.to_str().unwrap(),
        "--base",
        "missing",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert!(repo
        .rabi(&["adopt", plain.to_str().unwrap()])
        .status
        .success());
    assert!(repo.rabi(&["forget", "existing"]).status.success());
    assert!(plain.exists());
    assert!(branch_listed(&repo, "existing"));
}

#[test]
fn unified_create_rejects_conflicting_sources() {
    let repo = Repo::new();
    for args in [
        vec!["--base", "main", "--branch", "existing"],
        vec!["--base", "main", "--pr", "7"],
        vec!["--branch", "existing", "--pr", "7"],
        vec!["--detach", "--branch", "existing"],
        vec!["--detach", "--pr", "7"],
    ] {
        let args = [vec!["create", "feature", "--json"], args].concat();
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "usage");
    }
}

#[test]
fn unified_create_branch_accepts_git_only_names_with_an_explicit_workspace_name() {
    let repo = Repo::new();
    repo.git(&["branch", "topic+fix"]);
    let out = repo.rabi(&["create", "review", "--branch", "topic+fix", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["workspace"]["name"], "review");
    assert_eq!(doc["data"]["workspace"]["branch"], "topic+fix");
    let out = repo.rabi(&[
        "create",
        "review",
        "--branch",
        "topic+fix",
        "--reuse",
        "--json",
    ]);
    assert!(out.status.success());
    assert_eq!(json_value(&out)["data"]["source"]["reused"], true);
}

#[test]
fn create_reuse_and_adopt_do_not_treat_a_missing_directory_as_a_live_workspace() {
    let repo = Repo::new();
    repo.git(&["branch", "existing"]);
    let out = repo.rabi(&["create", "--branch", "existing", "--json"]);
    assert!(out.status.success());
    let directory = managed(&repo, "existing");
    fs::remove_dir_all(&directory).unwrap();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"true\"]\n",
    )
    .unwrap();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--branch", "existing", "--reuse", "--json"],
        )
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(json_value(&out)["error"]["code"], "untrusted");
    let out = repo.rabi(&["adopt", directory.to_str().unwrap(), "--json"]);
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json_value(&out)["error"]["code"], "not_found");
}

#[test]
fn adopt_forget_and_create_escape_control_characters_in_paths() {
    let repo = Repo::new();
    let outside = repo.root.join("existing\n\u{1b}[31m");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "plain",
        outside.to_str().unwrap(),
        "main",
    ]);
    let out = repo.rabi(&["adopt", outside.to_str().unwrap(), "--name", "review"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("existing\\u{a}\\u{1b}[31m"));
    assert!(!text(&out.stdout).contains('\u{1b}'));
    let out = repo.rabi(&["forget", "review"]);
    assert!(out.status.success());
    assert_eq!(text(&out.stdout), "Forgot workspace 'review'");
    let root = repo.root.join("new\n\u{1b}[31m");
    assert!(repo
        .rabi(&["config", "set", "workspace-root", root.to_str().unwrap()])
        .status
        .success());
    let out = repo.rabi(&["create", "escaped"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("new\\u{a}\\u{1b}[31m"));
    assert!(!text(&out.stdout).contains('\u{1b}'));
}

#[test]
fn create_reuse_propagates_corrupt_record_errors() {
    let repo = Repo::new();
    repo.git(&["branch", "existing"]);
    let created = repo.rabi(&["create", "--branch", "existing", "--json"]);
    assert!(created.status.success());
    let id = json_value(&created)["data"]["workspace"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let file = repo.repo.join(format!(".git/rabi/workspaces/{id}.json"));
    fs::write(&file, "{invalid").unwrap();
    let out = repo.rabi(&["create", "--branch", "existing", "--reuse", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json_value(&out)["error"]["code"], "failed");
    assert_eq!(fs::read_to_string(file).unwrap(), "{invalid");
    assert!(managed(&repo, "existing").exists());
}

#[test]
fn review2_branch_usage_compares_branches_when_workspace_names_cross() {
    for reuse in [false, true] {
        let repo = Repo::new();
        repo.git(&["branch", "feature"]);
        repo.git(&["branch", "review"]);
        let first = repo.rabi(&["create", "review", "--branch", "feature", "--json"]);
        assert!(first.status.success(), "{}", text(&first.stdout));
        let mut args = vec!["create", "second", "--branch", "review", "--json"];
        if reuse {
            args.push("--reuse");
        }
        let second = repo.rabi(&args);
        assert!(second.status.success(), "{}", text(&second.stdout));
        let doc = assert_envelope(&second, "create");
        assert_eq!(doc["data"]["workspace"]["name"], "second");
        assert_eq!(doc["data"]["workspace"]["branch"], "review");
        assert_eq!(doc["data"]["source"]["reused"], false);
        for branch in ["feature", "review", "main"] {
            let out = repo.rabi(&["create", "third", "--branch", branch, "--json"]);
            assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
            assert_eq!(json_value(&out)["error"]["code"], "exists");
        }
        assert!(!managed(&repo, "third").exists());
    }
}

#[test]
fn review2_pr_usage_compares_branches_when_workspace_names_cross() {
    for reuse in [false, true] {
        let repo = Repo::new();
        let path = fake_gh_for_topic_pr(&repo);
        publish_pr_topic(&repo);
        repo.git(&["branch", "feature"]);
        let first = repo.rabi(&["create", "topic", "--branch", "feature", "--json"]);
        assert!(first.status.success(), "{}", text(&first.stdout));
        let mut args = vec!["create", "second", "--pr", "7", "--json"];
        if reuse {
            args.push("--reuse");
        }
        let run = |args: &[&str]| {
            repo.command(env!("CARGO_BIN_EXE_rabi"), args)
                .env("PATH", &path)
                .output()
                .expect("rabi")
        };
        let second = run(&args);
        assert!(second.status.success(), "{}", text(&second.stdout));
        let doc = assert_envelope(&second, "create");
        assert_eq!(doc["data"]["workspace"]["name"], "second");
        assert_eq!(doc["data"]["workspace"]["branch"], "topic");
        assert_eq!(doc["data"]["source"]["reused"], false);
        assert!(managed(&repo, "second/topic.txt").is_file());
        let out = run(&["create", "third", "--pr", "7", "--json"]);
        assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "exists");
        assert!(!managed(&repo, "third").exists());
    }
}

#[test]
fn review2_reuse_with_an_explicit_name_does_not_reuse_a_branch_alias() {
    for pr in [false, true] {
        let repo = Repo::new();
        let path = fake_gh_for_topic_pr(&repo);
        publish_pr_topic(&repo);
        let first = repo.rabi(&["create", "review", "--branch", "origin/topic", "--json"]);
        assert!(first.status.success(), "{}", text(&first.stdout));
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &[
                    "create",
                    "topic",
                    if pr { "--pr" } else { "--branch" },
                    if pr { "7" } else { "topic" },
                    "--reuse",
                    "--json",
                ],
            )
            .env("PATH", &path)
            .output()
            .expect("rabi");
        assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "exists");
        assert!(!managed(&repo, "topic").exists());
        let reused = repo.rabi(&["create", "review", "--branch", "topic", "--reuse", "--json"]);
        assert!(reused.status.success(), "{}", text(&reused.stdout));
        assert_eq!(json_value(&reused)["data"]["source"]["reused"], true);
    }
}

#[test]
fn review2_explicit_detached_name_can_match_another_workspaces_branch() {
    let repo = Repo::new();
    repo.git(&["branch", "feature"]);
    let first = repo.rabi(&["create", "review", "--branch", "feature", "--json"]);
    assert!(first.status.success(), "{}", text(&first.stdout));
    let new = repo.rabi(&["create", "feature", "--json"]);
    assert_eq!(new.status.code(), Some(4));
    assert_eq!(json_value(&new)["error"]["code"], "exists_remote");
    for reused in [false, true] {
        let out = repo.rabi(&["create", "feature", "--detach", "--reuse", "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, "create");
        assert_eq!(doc["data"]["workspace"]["name"], "feature");
        assert_eq!(doc["data"]["workspace"]["detached"], true);
        assert_eq!(doc["data"]["source"]["reused"], reused);
    }
}

#[test]
fn review1_create_rejects_names_outside_the_workspace_root_for_every_source() {
    for source in [
        vec![],
        vec!["--detach"],
        vec!["--branch", "existing"],
        vec!["--pr", "7"],
        vec!["--issue", "7"],
    ] {
        for spelling in ["absolute", "parent", "nested-parent"] {
            let repo = Repo::new();
            publish_pr_topic(&repo);
            repo.git(&["branch", "existing"]);
            let path = fake_gh_for_topic_pr(&repo);
            if source == ["--issue", "7"] {
                let gh = repo.root.join("bin/gh");
                let script = fs::read_to_string(&gh).unwrap();
                fs::write(
                    gh,
                    script.replace("\"pull_request\":{}", "\"title\":\"Issue\""),
                )
                .unwrap();
            }
            let outside = repo.root.join("escaped");
            let name = match spelling {
                "absolute" => outside.to_str().unwrap(),
                "parent" => "../escaped",
                _ => "nested/../../escaped",
            };
            let before = repo
                .command("git", &["worktree", "list", "--porcelain"])
                .output()
                .unwrap()
                .stdout;
            let args = [
                vec!["create", name],
                source.clone(),
                vec!["--no-setup", "--json"],
            ]
            .concat();
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("PATH", &path)
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(2),
                "{args:?}: {}",
                text(&out.stdout)
            );
            assert_eq!(json_value(&out)["error"]["code"], "usage");
            assert!(!outside.exists());
            assert_eq!(
                repo.command("git", &["worktree", "list", "--porcelain"])
                    .output()
                    .unwrap()
                    .stdout,
                before
            );
        }
    }
}

#[test]
fn review1_create_rejects_destination_escaping_through_a_parent_symlink() {
    use std::os::unix::fs::symlink;
    for source in [
        vec![],
        vec!["--detach"],
        vec!["--branch", "existing"],
        vec!["--pr", "7"],
        vec!["--issue", "7"],
    ] {
        let repo = Repo::new();
        publish_pr_topic(&repo);
        repo.git(&["branch", "existing"]);
        let gh_path = fake_gh_for_topic_pr(&repo);
        if source == ["--issue", "7"] {
            let gh = repo.root.join("bin/gh");
            let script = fs::read_to_string(&gh).unwrap();
            fs::write(
                gh,
                script.replace("\"pull_request\":{}", "\"title\":\"Issue\""),
            )
            .unwrap();
        }
        let root = repo.root.join("workspaces");
        let outside = repo.root.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        assert!(repo
            .rabi(&["config", "set", "workspace-root", root.to_str().unwrap()])
            .status
            .success());
        let args = [
            vec!["create", "escape/missing/review"],
            source,
            vec!["--no-setup", "--json"],
        ]
        .concat();
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("PATH", gh_path)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(2),
            "{args:?}: {}",
            text(&out.stdout)
        );
        assert_eq!(json_value(&out)["error"]["code"], "usage");
        assert!(!outside.join("missing").exists());
    }
}

#[test]
fn review1_reuse_links_resolve_bare_numbers_in_the_selected_repository() {
    let repo = Repo::new();
    repo.git(&[
        "remote",
        "add",
        "origin",
        "https://github.com/acme/demo.git",
    ]);
    repo.git(&["branch", "existing"]);
    let run = |issue: &str| {
        repo.rabi(&[
            "create", "review", "--branch", "existing", "--issue", issue, "--reuse", "--json",
        ])
    };
    assert!(run("12").status.success());
    for url in [
        "https://github.com/other/demo/issues/12",
        "https://github.com/acme/other/issues/12",
        "https://other.example/acme/demo/issues/12",
    ] {
        let out = run(url);
        assert_eq!(out.status.code(), Some(4), "{url}: {}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "exists");
    }
    for reference in [
        "#12",
        "https://github.com/acme/demo/issues/12",
        "https://github.com/acme/demo/issues/12/?plain=1#details",
    ] {
        let out = run(reference);
        assert!(out.status.success(), "{reference}: {}", text(&out.stdout));
    }
    assert!(repo
        .rabi(&[
            "set",
            "review",
            "--issue",
            "https://github.com/other/demo/issues/12"
        ])
        .status
        .success());
    let out = run("12");
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
}

#[test]
fn review1_reuse_links_share_terminal_url_normalization_with_selectors() {
    let repo = Repo::new();
    repo.git(&["branch", "existing"]);
    // The earlier /pull/123 belongs to owner/repo, not the subject number.
    let url = "https://git.example.com/pull/123/issues/42";
    let out = repo.rabi(&[
        "create", "review", "--branch", "existing", "--issue", url, "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    for url in [
        "https://git.example.com/pull/123/issues/42/?plain=1#details",
        "https://git.example.com/pull/123/issue/42#details",
    ] {
        let show = repo.rabi(&["show", url, "--json"]);
        assert!(show.status.success(), "{}", text(&show.stdout));
        let out = repo.rabi(&[
            "create", "review", "--branch", "existing", "--issue", url, "--reuse", "--json",
        ]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["data"]["source"]["reused"], true);
    }
}

#[test]
fn review1_adopt_name_is_used_inside_the_workspace_root() {
    let repo = Repo::new();
    let tree = managed(&repo, "directory-name");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        tree.to_str().unwrap(),
        "main",
    ]);
    let out = repo.rabi(&[
        "adopt",
        tree.to_str().unwrap(),
        "--name",
        "review",
        "--json",
    ]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert_eq!(json_value(&out)["data"]["workspace"]["name"], "review");
    for command in ["show", "path", "forget"] {
        let out = repo.rabi(&[command, "review", "--json"]);
        assert!(out.status.success(), "{command}: {}", text(&out.stdout));
    }
    assert!(tree.is_dir());
}

#[test]
fn review1_existing_branch_creation_does_not_require_a_default_or_source_head() {
    let repo = Repo::new();
    repo.git(&["branch", "-m", "develop"]);
    repo.git(&["checkout", "--orphan", "unborn"]);
    let out = repo.rabi(&["create", "review", "--branch", "develop", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["workspace"]["branch"], "develop");
    assert_eq!(doc["data"]["source"]["base"], "refs/heads/develop");
    assert_eq!(doc["data"]["workspace"]["base"], serde_json::Value::Null);
}

#[test]
fn review9_pr_unknown_base_stays_unknown_after_default_returns() {
    let repo = Repo::new();
    publish_pr_topic(&repo);
    let path = fake_gh_for_topic_pr(&repo);
    repo.git(&["checkout", "--orphan", "unborn"]);
    repo.git(&["branch", "-D", "main"]);
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--json"],
        )
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stdout));
    let doc = json_value(&out);
    assert!(doc["data"]["source"]["base"].is_null());
    let tree = PathBuf::from(doc["data"]["workspace"]["path"].as_str().unwrap());
    let head = || {
        let out = repo
            .command("git", &["rev-parse", "HEAD"])
            .current_dir(&tree)
            .output()
            .unwrap();
        assert!(out.status.success());
        text(&out.stdout)
    };
    let head_before = head();
    repo.git(&["checkout", "-b", "main", "topic"]);
    fs::write(repo.repo.join("new-default"), "new default\n").unwrap();
    repo.git(&["add", "new-default"]);
    repo.git(&["commit", "-m", "advance default"]);
    // Keep sync local so the PR's former remote default cannot hide the fallback.
    repo.git(&["remote", "remove", "origin"]);
    let store = repo.repo.join(".git/rabi/workspaces");
    let record_before = records::read(&store).unwrap();
    for selector in ["topic", "--all"] {
        let out = repo.rabi(&["sync", selector, "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = json_value(&out);
        assert_eq!(doc["data"]["items"][0]["status"], "base-unknown", "{doc}");
        assert_eq!(head(), head_before);
        assert_eq!(records::read(&store).unwrap(), record_before);
    }
    let record: serde_json::Value = serde_json::from_slice(&record_before).unwrap();
    assert_eq!(record["worktrees"][0]["baseCleared"], true);
}

#[test]
fn review1_pr_creation_does_not_require_a_default_or_source_head() {
    let repo = Repo::new();
    publish_pr_topic(&repo);
    let path = fake_gh_for_topic_pr(&repo);
    repo.git(&["checkout", "--orphan", "unborn"]);
    repo.git(&["branch", "-D", "main"]);
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "--pr", "7", "--json"],
        )
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert_eq!(json_value(&out)["data"]["workspace"]["branch"], "topic");
    assert_eq!(
        json_value(&out)["data"]["source"]["base"],
        serde_json::Value::Null
    );
    assert_eq!(
        json_value(&out)["data"]["workspace"]["base"],
        serde_json::Value::Null
    );
}

#[test]
fn review3_reuse_rejects_a_prunable_checkout_with_a_missing_git_file() {
    for source in ["branch", "new", "detach"] {
        let repo = Repo::new();
        let mut create = vec!["create", "review", "--json"];
        if source == "branch" {
            repo.git(&["branch", "topic"]);
            create.extend(["--branch", "topic"]);
        } else if source == "detach" {
            create.push("--detach");
        }
        let out = repo.rabi(&create);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let directory = managed(&repo, "review");
        fs::remove_file(directory.join(".git")).unwrap();
        assert!(directory.is_dir());
        let listed = repo
            .command("git", &["worktree", "list", "--porcelain"])
            .output()
            .unwrap();
        assert!(text(&listed.stdout).contains("prunable"));
        let records_before =
            records::read_to_string(repo.repo.join(".git/rabi/workspaces")).unwrap();
        create.push("--reuse");
        let out = repo.rabi(&create);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, "create");
        assert_eq!(doc["error"]["code"], "failed");
        let message = doc["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("Cannot reuse workspace 'review'"),
            "{message}"
        );
        assert!(message.contains(".git"), "{message}");
        assert_eq!(
            records::read_to_string(repo.repo.join(".git/rabi/workspaces")).unwrap(),
            records_before
        );
        assert!(directory.is_dir());
        assert!(!directory.join(".git").exists());
    }
}

#[test]
fn review3_reuse_rejects_git_discovery_falling_back_to_the_parent_checkout() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["config", "set", "workspace-root", "workspaces"])
        .status
        .success());
    assert!(repo.rabi(&["create", "review"]).status.success());
    let directory = repo.repo.join("workspaces/review");
    fs::remove_file(directory.join(".git")).unwrap();
    let observed = repo
        .command("git", &["rev-parse", "--show-toplevel"])
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(observed.status.success());
    assert_eq!(Path::new(&text(&observed.stdout)), repo.repo);
    let out = repo.rabi(&["create", "review", "--reuse", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    assert_eq!(assert_envelope(&out, "create")["error"]["code"], "failed");
}

#[test]
fn review3_reuse_rejects_a_checkout_pointing_to_a_foreign_common_directory() {
    let repo = Repo::new();
    let foreign = Repo::new();
    assert!(repo.rabi(&["create", "review"]).status.success());
    assert!(foreign.rabi(&["create", "other"]).status.success());
    let directory = managed(&repo, "review");
    fs::copy(
        managed(&foreign, "other").join(".git"),
        directory.join(".git"),
    )
    .unwrap();
    let observed = repo
        .command("git", &["rev-parse", "--show-toplevel"])
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(observed.status.success());
    assert_eq!(Path::new(&text(&observed.stdout)), directory);
    let out = repo.rabi(&["create", "review", "--reuse", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
    assert_eq!(assert_envelope(&out, "create")["error"]["code"], "failed");
}

fn assert_reuse_rejects_swapped_git_dir(primary: bool) {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "review"]).status.success());
    let directory = managed(&repo, "review");
    if primary {
        fs::write(
            directory.join(".git"),
            format!("gitdir: {}\n", repo.repo.join(".git").display()),
        )
        .unwrap();
    } else {
        assert!(repo.rabi(&["create", "other"]).status.success());
        fs::copy(managed(&repo, "other").join(".git"), directory.join(".git")).unwrap();
    }
    let observed = repo
        .command("git", &["rev-parse", "--show-toplevel"])
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(observed.status.success());
    assert_eq!(Path::new(&text(&observed.stdout)), directory);
    let actual_branch = repo
        .command("git", &["branch", "--show-current"])
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(actual_branch.status.success());
    assert_eq!(
        text(&actual_branch.stdout),
        if primary { "main" } else { "other" }
    );
    let records_before = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    let gitfile_before = fs::read(directory.join(".git")).unwrap();
    for source in [vec![], vec!["--branch", "review"]] {
        let mut args = vec!["create", "review", "--reuse", "--json"];
        args.extend(source);
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, "create");
        assert_eq!(doc["error"]["code"], "failed");
        assert!(doc["error"]["message"].as_str().unwrap().contains(".git"));
        assert_eq!(
            records::read(repo.repo.join(".git/rabi/workspaces")).unwrap(),
            records_before
        );
        assert_eq!(fs::read(directory.join(".git")).unwrap(), gitfile_before);
    }
}

#[test]
fn review5_reuse_rejects_a_git_file_pointing_to_primary() {
    assert_reuse_rejects_swapped_git_dir(true);
}

#[test]
fn review5_reuse_rejects_a_git_file_pointing_to_another_workspace() {
    assert_reuse_rejects_swapped_git_dir(false);
}

#[test]
fn review5_user_names_reject_reserved_selectors_without_records() {
    for command in ["adopt", "create"] {
        for name in [
            ".",
            "pr:7",
            "pr:topic",
            "issue:12",
            "issue:topic",
            "https://github.com/owner/repo/pull/7",
            "http://github.com/owner/repo/issues/12",
        ] {
            let repo = Repo::new();
            let directory = repo.root.join("directory-name");
            repo.git(&[
                "worktree",
                "add",
                "-b",
                "topic",
                directory.to_str().unwrap(),
            ]);
            let args = if command == "adopt" {
                vec![
                    "adopt",
                    directory.to_str().unwrap(),
                    "--name",
                    name,
                    "--json",
                ]
            } else {
                vec!["create", name, "--detach", "--json"]
            };
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(2),
                "{command} {name}: {}",
                text(&out.stdout)
            );
            assert_eq!(assert_envelope(&out, command)["error"]["code"], "usage");
            assert!(!repo.repo.join(".git/rabi/workspaces").exists());
            assert!(directory.join(".git").is_file());
        }
    }
}

#[test]
fn review3_forget_matches_show_and_list_after_record_removal() {
    for location in [
        "inside",
        "outside",
        "primary",
        "inside-alias",
        "outside-alias",
    ] {
        let repo = Repo::new();
        let (selector, after_selector, expected_managed) = match location {
            "inside" => {
                assert!(repo.rabi(&["create", "review"]).status.success());
                ("review", "review", true)
            }
            "outside" => {
                let outside = repo.root.join("plain");
                repo.git(&["worktree", "add", "-b", "review", outside.to_str().unwrap()]);
                assert!(repo
                    .rabi(&["adopt", outside.to_str().unwrap()])
                    .status
                    .success());
                ("review", "review", false)
            }
            "inside-alias" | "outside-alias" => {
                let inside = location == "inside-alias";
                let directory = if inside {
                    managed(&repo, "directory-name")
                } else {
                    repo.root.join("directory-name")
                };
                repo.git(&[
                    "worktree",
                    "add",
                    "-b",
                    "topic",
                    directory.to_str().unwrap(),
                ]);
                assert!(repo
                    .rabi(&["adopt", directory.to_str().unwrap(), "--name", "review"])
                    .status
                    .success());
                (
                    "review",
                    if inside { "directory-name" } else { "topic" },
                    inside,
                )
            }
            _ => (".", ".", false),
        };
        assert!(repo
            .rabi(&[
                "set",
                selector,
                "--purpose",
                "work",
                "--base",
                "main",
                "--pr",
                "34",
                "--issue",
                "12"
            ])
            .status
            .success());
        let out = repo.rabi(&["forget", selector, "issue:12", "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let forgotten = assert_envelope(&out, "forget");
        let show = repo.rabi(&["show", after_selector, "--json"]);
        assert!(show.status.success(), "{}", text(&show.stdout));
        let shown = assert_envelope(&show, "show");
        let list = repo.rabi(&["list", "--json"]);
        assert!(list.status.success(), "{}", text(&list.stdout));
        let listed = assert_envelope(&list, "list");
        let tree = &shown["data"]["workspace"];
        assert_eq!(tree["managed"], expected_managed);
        for item in forgotten["data"]["items"].as_array().unwrap() {
            assert_eq!(item["ok"], true);
            assert_eq!(&item["workspace"], tree, "{location}");
        }
        assert!(listed["data"]["workspaces"]
            .as_array()
            .unwrap()
            .contains(tree));
        for field in [
            "id",
            "base",
            "purpose",
            "createdAt",
            "createdFrom",
            "updatedAt",
            "setup",
        ] {
            assert_eq!(tree[field], serde_json::Value::Null, "{field}: {tree}");
        }
        assert_eq!(tree["links"]["pr"], serde_json::Value::Null);
        assert_eq!(tree["links"]["issue"], serde_json::Value::Null);
    }
}

#[test]
fn review1_forget_returns_the_workspace_after_removing_its_record() {
    let repo = Repo::new();
    let out = repo.create_recorded(&[
        "create",
        "review",
        "--base",
        "main",
        "--purpose",
        "work",
        "--issue",
        "12",
        "--pr",
        "34",
        "--json",
    ]);
    assert!(out.status.success());
    let out = repo.rabi(&["forget", "review", "issue:12", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    let doc = json_value(&out);
    for item in doc["data"]["items"].as_array().unwrap() {
        assert_eq!(item["ok"], true);
        let tree = &item["workspace"];
        let shown = repo.rabi(&["show", "review", "--json"]);
        assert!(shown.status.success());
        assert_eq!(tree, &json_value(&shown)["data"]["workspace"]);
        for field in [
            "id",
            "base",
            "purpose",
            "createdAt",
            "createdFrom",
            "updatedAt",
        ] {
            assert_eq!(tree[field], serde_json::Value::Null, "{field}: {tree}");
        }
        assert_eq!(tree["links"]["pr"], serde_json::Value::Null);
        assert_eq!(tree["links"]["issue"], serde_json::Value::Null);
        assert_eq!(tree["branch"], "review");
        assert!(tree["head"].is_string());
        assert!(Path::new(tree["path"].as_str().unwrap()).is_dir());
    }
}

fn assert_adopt_rejects_swapped_git_dir(primary: bool) {
    let repo = Repo::new();
    let directory = repo.root.join("review");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "topic",
        directory.to_str().unwrap(),
    ]);
    if primary {
        fs::write(
            directory.join(".git"),
            format!("gitdir: {}\n", repo.repo.join(".git").display()),
        )
        .unwrap();
    } else {
        let other = repo.root.join("other");
        repo.git(&["worktree", "add", "-b", "other", other.to_str().unwrap()]);
        fs::copy(other.join(".git"), directory.join(".git")).unwrap();
    }
    // Both pre-existing checks pass, despite Git operating on the wrong branch.
    let observed = repo
        .command("git", &["rev-parse", "--show-toplevel"])
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(observed.status.success());
    assert_eq!(Path::new(&text(&observed.stdout)), directory);
    assert_eq!(
        rabi::git::canonical_common_dir(&directory).unwrap(),
        repo.repo.join(".git")
    );
    let branch = repo
        .command("git", &["branch", "--show-current"])
        .current_dir(&directory)
        .output()
        .unwrap();
    assert!(branch.status.success());
    assert_eq!(text(&branch.stdout), if primary { "main" } else { "other" });
    let gitfile = fs::read(directory.join(".git")).unwrap();
    let out = repo.rabi(&[
        "adopt",
        directory.to_str().unwrap(),
        "--name",
        "review",
        "--json",
    ]);
    assert!(!out.status.success(), "{}", text(&out.stdout));
    assert_eq!(assert_envelope(&out, "adopt")["error"]["code"], "not_found");
    assert!(!repo.repo.join(".git/rabi/workspaces").exists());
    assert_eq!(fs::read(directory.join(".git")).unwrap(), gitfile);
}

#[test]
fn review6_adopt_rejects_a_git_file_pointing_to_primary() {
    assert_adopt_rejects_swapped_git_dir(true);
}

#[test]
fn review6_adopt_rejects_a_git_file_pointing_to_another_workspace() {
    assert_adopt_rejects_swapped_git_dir(false);
}

#[test]
fn review11_branch_reuse_refuses_primary_checkout() {
    assert_review11_primary_refused(false, false);
}

#[test]
fn review11_named_branch_reuse_refuses_primary_checkout() {
    assert_review11_primary_refused(false, true);
}

#[test]
fn review11_pr_reuse_refuses_primary_checkout() {
    assert_review11_primary_refused(true, false);
}

#[test]
fn review11_named_pr_reuse_refuses_primary_checkout() {
    assert_review11_primary_refused(true, true);
}

fn assert_review11_primary_refused(pr: bool, named: bool) {
    for recorded in [false, true] {
        let repo = Repo::new();
        let path = fake_gh_for_topic_pr(&repo);
        if pr {
            publish_pr_topic(&repo);
            repo.git(&["checkout", "-b", "topic", "origin/topic"]);
        }
        if recorded {
            let out = repo.rabi(&["set", ".", "--purpose", "Primary intent", "--json"]);
            assert!(out.status.success(), "{}", text(&out.stdout));
        }
        let before = json_value(&repo.rabi(&["show", ".", "--json"]));
        let primary_name = before["data"]["workspace"]["name"].as_str().unwrap();
        let record_path = repo.repo.join(".git/rabi/workspaces");
        let records_before = records::read(&record_path).ok();
        let mut args = vec!["create"];
        if named {
            args.push(primary_name);
        }
        args.extend([
            if pr { "--pr" } else { "--branch" },
            if pr { "7" } else { "main" },
            "--json",
        ]);
        let run = |args: &[&str]| {
            repo.command(env!("CARGO_BIN_EXE_rabi"), args)
                .env("PATH", &path)
                .output()
                .unwrap()
        };
        let refused = run(&args);
        assert_eq!(refused.status.code(), Some(4), "{}", text(&refused.stdout));
        let refused_doc = assert_envelope(&refused, "create");
        assert_eq!(refused_doc["error"]["code"], "exists");
        args.push("--reuse");
        let reuse = run(&args);
        assert_eq!(
            records::read(&record_path).ok(),
            records_before,
            "Primary metadata changed: {}",
            text(&reuse.stdout)
        );
        assert_eq!(
            reuse.status.code(),
            refused.status.code(),
            "{}",
            text(&reuse.stdout)
        );
        assert_eq!(
            assert_envelope(&reuse, "create")["error"],
            refused_doc["error"]
        );
        assert_eq!(json_value(&repo.rabi(&["show", ".", "--json"])), before);
        assert!(!managed(&repo, primary_name).exists());
    }
}

#[test]
fn review11_adopt_skips_worktree_lock_files() {
    assert_review11_stale_registration_skipped(true, false);
}

#[test]
fn review11_reuse_skips_worktree_lock_files() {
    assert_review11_stale_registration_skipped(false, false);
}

#[test]
fn review11_adopt_skips_admin_directories_without_gitdir() {
    assert_review11_stale_registration_skipped(true, true);
}

#[test]
fn review11_reuse_skips_admin_directories_without_gitdir() {
    assert_review11_stale_registration_skipped(false, true);
}

fn assert_review11_stale_registration_skipped(adopt: bool, directory: bool) {
    let repo = Repo::new();
    let registrations = repo.repo.join(".git/worktrees");
    fs::create_dir_all(&registrations).unwrap();
    let stale = registrations.join(if directory { "broken" } else { "pending.lock" });
    if directory {
        fs::create_dir(&stale).unwrap();
    } else {
        fs::write(&stale, "").unwrap();
    }
    let checkout = repo.root.join("review");
    repo.git(&["worktree", "add", "-b", "topic", checkout.to_str().unwrap()]);
    let gitfile = fs::read(checkout.join(".git")).unwrap();
    let args = if adopt {
        vec![
            "adopt",
            checkout.to_str().unwrap(),
            "--name",
            "review",
            "--json",
        ]
    } else {
        vec!["create", "--branch", "topic", "--reuse", "--json"]
    };
    let out = repo.rabi(&args);
    assert!(out.status.success(), "{}", text(&out.stdout));
    let doc = assert_envelope(&out, if adopt { "adopt" } else { "create" });
    let tree = &doc["data"]["workspace"];
    assert_eq!(tree["path"], checkout.to_str().unwrap());
    assert_eq!(tree["branch"], "topic");
    if adopt {
        assert!(tree["id"].is_string());
        assert_eq!(tree["name"], "review");
        assert_eq!(tree["createdAt"], serde_json::Value::Null);
    } else {
        assert_eq!(doc["data"]["source"]["reused"], true);
        assert!(!repo.repo.join(".git/rabi/workspaces").exists());
    }
    assert_eq!(fs::read(checkout.join(".git")).unwrap(), gitfile);
    assert!(stale.exists());
}

#[test]
fn setup_no_hooks_rejects_symlink_to_another_registered_workspace() {
    assert_rejects_symlink_to_registered_workspace("setup-no-hooks");
}

#[test]
fn setup_rejects_symlink_to_another_registered_workspace() {
    assert_rejects_symlink_to_registered_workspace("setup");
}

#[test]
fn create_reuse_rejects_symlink_to_another_registered_workspace() {
    assert_rejects_symlink_to_registered_workspace("create");
}

#[test]
fn adopt_rejects_symlink_to_another_registered_workspace() {
    assert_rejects_symlink_to_registered_workspace("adopt");
}

#[test]
fn preparation_rejects_git_file_symlink_to_another_registered_workspace() {
    for command in ["setup-no-hooks", "setup", "create"] {
        assert_rejects_symlink_to_registered_workspace_entry(command, true);
    }
}

fn assert_rejects_symlink_to_registered_workspace(command: &str) {
    assert_rejects_symlink_to_registered_workspace_entry(command, false);
}

fn assert_rejects_symlink_to_registered_workspace_entry(command: &str, gitfile: bool) {
    for recorded in [false, true] {
        let repo = Repo::new();
        for name in ["b", "a"] {
            assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        }
        if !recorded || command == "adopt" {
            assert!(repo.rabi(&["forget", "a"]).status.success());
        }
        if command == "adopt" && !recorded {
            assert!(repo.rabi(&["forget", "b"]).status.success());
        }
        let a = managed(&repo, "a");
        let b = managed(&repo, "b");
        // Make the vulnerable registration scan find b first regardless of readdir order.
        let admins: Vec<_> = fs::read_dir(repo.repo.join(".git/worktrees"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(admins.len(), 2);
        for (admin, (name, checkout)) in admins.iter().zip([("b", &b), ("a", &a)]) {
            fs::write(
                admin.join("gitdir"),
                format!("{}\n", checkout.join(".git").display()),
            )
            .unwrap();
            fs::write(admin.join("HEAD"), format!("ref: refs/heads/{name}\n")).unwrap();
            fs::write(
                checkout.join(".git"),
                format!("gitdir: {}\n", admin.display()),
            )
            .unwrap();
        }
        fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
        fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
        fs::write(repo.repo.join("secret.env"), "primary secret").unwrap();
        fs::write(repo.repo.join("existing.env"), "primary value").unwrap();
        fs::write(b.join("existing.env"), "keep b value").unwrap();
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = ['touch hook-ran']\n",
        )
        .unwrap();
        let replaced = if gitfile {
            let gitfile = a.join(".git");
            fs::remove_file(&gitfile).unwrap();
            std::os::unix::fs::symlink(b.join(".git"), &gitfile).unwrap();
            gitfile
        } else {
            fs::remove_dir_all(&a).unwrap();
            std::os::unix::fs::symlink(&b, &a).unwrap();
            a.clone()
        };
        let snapshot = |directory: &Path| {
            let mut files: Vec<_> = fs::read_dir(directory)
                .unwrap()
                .map(|entry| {
                    let path = entry.unwrap().path();
                    (
                        path.file_name().unwrap().to_owned(),
                        fs::read(&path).unwrap(),
                    )
                })
                .collect();
            files.sort();
            files
        };
        let a_before = snapshot(&a);
        let b_before = snapshot(&b);
        let records_path = repo.repo.join(".git/rabi/workspaces");
        let records_before = snapshot(&records_path);
        let args = match command {
            "setup-no-hooks" => vec!["setup", "a", "--no-hooks", "--json"],
            "setup" => vec!["setup", "a", "--json"],
            "create" => vec!["create", "--branch", "a", "--reuse", "--json"],
            "adopt" => vec!["adopt", a.to_str().unwrap(), "--name", "adopted", "--json"],
            _ => panic!("Unknown command"),
        };
        let out = repo.rabi(&args);
        let unchanged = snapshot(&b) == b_before && snapshot(&records_path) == records_before;
        assert_eq!(
            out.status.code(),
            Some(1),
            "{command}, recorded={recorded}, unchanged={unchanged}: {}",
            text(&out.stdout)
        );
        let doc = assert_envelope(&out, args[0]);
        assert_eq!(doc["error"]["code"], "failed");
        let message = doc["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(".git") && message.contains("repair"),
            "{message}"
        );
        assert_eq!(snapshot(&a), a_before);
        assert_eq!(snapshot(&b), b_before);
        assert_eq!(snapshot(&records_path), records_before);
        assert!(replaced
            .symlink_metadata()
            .unwrap()
            .file_type()
            .is_symlink());
    }
}

#[test]
fn setup_reuse_and_adopt_allow_symlinked_parent_directories() {
    let repo = Repo::new();
    let real = repo.root.join("real-workspaces");
    let via = repo.root.join("via-workspaces");
    fs::create_dir(&real).unwrap();
    std::os::unix::fs::symlink(&real, &via).unwrap();
    assert!(repo
        .rabi(&["config", "set", "workspace-root", via.to_str().unwrap()])
        .status
        .success());
    assert!(repo.rabi(&["create", "a", "--no-setup"]).status.success());
    // Preserve a symlink spelling in Git's independent registration, too.
    fs::write(
        repo.repo.join(".git/worktrees/a/gitdir"),
        format!("{}\n", via.join("a/.git").display()),
    )
    .unwrap();
    let primary_alias = repo.root.join("via-primary");
    std::os::unix::fs::symlink(&repo.repo, &primary_alias).unwrap();
    fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
    fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
    fs::write(repo.repo.join("secret.env"), "primary secret").unwrap();
    let via_a = via.join("a");
    for args in [
        vec!["setup", "a", "--no-hooks", "--json"],
        vec!["setup", "a", "--json"],
        vec!["create", "a", "--reuse", "--json"],
        vec!["forget", "a", "--json"],
        vec!["adopt", via_a.to_str().unwrap(), "--json"],
    ] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&primary_alias)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
    }
    assert_eq!(
        fs::read_to_string(real.join("a/secret.env")).unwrap(),
        "primary secret"
    );
}

#[test]
fn setup_rejects_checkout_replaced_by_foreign_repository() {
    assert_setup_rejects_replaced_checkout("foreign");
}

#[test]
fn setup_rejects_checkout_replaced_by_symlink() {
    assert_setup_rejects_replaced_checkout("symlink");
}

#[test]
fn setup_rejects_git_file_pointing_to_primary() {
    assert_setup_rejects_replaced_checkout("primary-admin");
}

#[test]
fn setup_rejects_git_discovery_falling_back_to_parent() {
    assert_setup_rejects_replaced_checkout("parent");
}

fn assert_setup_rejects_replaced_checkout(replacement: &str) {
    for recorded in [true, false] {
        for no_hooks in [true, false] {
            let repo = Repo::new();
            let foreign = Repo::new();
            assert!(repo
                .rabi(&["config", "set", "workspace-root", "workspaces"])
                .status
                .success());
            assert!(repo
                .rabi(&["create", "target", "--no-setup"])
                .status
                .success());
            if !recorded {
                assert!(repo.rabi(&["forget", "target"]).status.success());
            }
            fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
            fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
            fs::write(repo.repo.join("secret.env"), "primary secret").unwrap();
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\nsetup = ['git add README.md; touch hook-ran']\n",
            )
            .unwrap();
            let target = repo.repo.join("workspaces/target");
            match replacement {
                "foreign" => {
                    fs::remove_dir_all(&target).unwrap();
                    fs::rename(&foreign.repo, &target).unwrap();
                }
                "symlink" => {
                    fs::remove_dir_all(&target).unwrap();
                    std::os::unix::fs::symlink(&foreign.repo, &target).unwrap();
                }
                "primary-admin" => fs::write(
                    target.join(".git"),
                    format!("gitdir: {}\n", repo.repo.join(".git").display()),
                )
                .unwrap(),
                "parent" => fs::remove_file(target.join(".git")).unwrap(),
                _ => panic!("Unknown replacement"),
            }
            fs::write(target.join("README.md"), "changed in target\n").unwrap();
            let records_path = repo.repo.join(".git/rabi/workspaces");
            let records_before = records::read(&records_path).unwrap();
            let head_before = fs::read(repo.repo.join(".git/HEAD")).unwrap();
            let index_before = fs::read(repo.repo.join(".git/index")).unwrap();
            let gitfile_before = fs::read(target.join(".git")).ok();
            let mut args = vec!["setup", "target", "--json"];
            if no_hooks {
                args.push("--no-hooks");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{replacement}, recorded={recorded}, no_hooks={no_hooks}: {}",
                text(&out.stdout)
            );
            let doc = assert_envelope(&out, "setup");
            assert_eq!(doc["error"]["code"], "failed");
            let message = doc["error"]["message"].as_str().unwrap();
            assert!(message.contains(".git"), "{message}");
            assert!(message.contains("repair"), "{message}");
            assert!(!target.join("secret.env").exists());
            assert!(!target.join("hook-ran").exists());
            assert_eq!(records::read(&records_path).unwrap(), records_before);
            assert_eq!(fs::read(repo.repo.join(".git/HEAD")).unwrap(), head_before);
            assert_eq!(
                fs::read(repo.repo.join(".git/index")).unwrap(),
                index_before
            );
            assert_eq!(fs::read(target.join(".git")).ok(), gitfile_before);
        }
    }
}

#[test]
fn setup_and_create_escape_copy_failure_diagnostics() {
    for command in ["setup", "create"] {
        for json in [false, true] {
            let repo = Repo::new();
            let directory = "private\u{1b}[31m\t";
            let filename = "secret\u{7}.env";
            let outside = repo.root.join("outside");
            fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, repo.repo.join(directory)).unwrap();
            fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
            fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
            repo.git(&["add", ".gitignore", ".worktreeinclude", directory]);
            repo.git(&["commit", "-m", "copy failure fixture"]);
            fs::remove_file(repo.repo.join(directory)).unwrap();
            fs::create_dir(repo.repo.join(directory)).unwrap();
            fs::write(repo.repo.join(directory).join(filename), "secret").unwrap();
            if command == "setup" {
                assert!(repo
                    .rabi(&["create", "target", "--no-setup"])
                    .status
                    .success());
            }
            let mut args = vec![command, "target"];
            if json {
                args.push("--json");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(if command == "setup" { 1 } else { 6 })
            );
            let stderr = text(&out.stderr);
            assert!(!stderr.contains(['\u{1b}', '\t', '\u{7}']), "{stderr:?}");
            assert!(
                stderr.contains("private\\u{1b}[31m\\u{9}/secret\\u{7}.env"),
                "{stderr:?}"
            );
            if command == "setup" && !json {
                assert!(stderr
                    .lines()
                    .any(|line| line.starts_with("Failed to copy ")));
            }
            if json {
                let doc = assert_envelope(&out, command);
                assert!(doc["data"]["copy"]["error"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("{directory}/{filename}")));
            }
            assert!(!outside.join(filename).exists());
        }
    }
}

#[test]
fn setup_source_checkout_rejects_another_repository_before_changes() {
    let repo = Repo::new();
    let other = Repo::new();
    assert!(repo
        .rabi(&["create", "target", "--no-setup"])
        .status
        .success());
    let before = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
    fs::write(repo.repo.join("secret.env"), "secret").unwrap();
    fs::write(
        other.repo.join(".rabi.toml"),
        "[hooks]\nsetup = ['touch wrong-hook']\n",
    )
    .unwrap();
    let out = repo.rabi(&[
        "setup",
        "target",
        "--source-checkout",
        other.repo.to_str().unwrap(),
        "--copy-file",
        "*.env",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(json_value(&out)["error"]["message"]
        .as_str()
        .unwrap()
        .contains("selected repository"));
    assert_eq!(
        records::read(repo.repo.join(".git/rabi/workspaces")).unwrap(),
        before
    );
    assert!(!managed(&repo, "target/secret.env").exists());
    assert!(!managed(&repo, "target/wrong-hook").exists());
}

#[test]
fn setup_retry_hint_preserves_linked_source_hooks() {
    for json in [false, true] {
        for mode in 0..4 {
            if mode == 2 && !cfg!(debug_assertions) {
                continue;
            }
            let fallback = mode != 0;
            let repo = Repo::new();
            assert!(repo
                .rabi(&["create", "source", "--no-setup"])
                .status
                .success());
            let source = managed(&repo, "source");
            let gh_path = fallback.then(|| {
                let path = fake_gh_for_topic_pr(&repo);
                publish_pr_topic(&repo);
                let gh = repo.root.join("bin/gh");
                let script = fs::read_to_string(&gh).unwrap();
                fs::write(&gh, script.replace("\"topic\"", "\"topic+fix\"")).unwrap();
                if mode == 1 {
                    assert!(repo
                        .create_recorded(&["create", "duplicate", "--pr", "7", "--no-setup"])
                        .status
                        .success());
                }
                path
            });
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\nsetup = ['touch primary-hook']\n",
            )
            .unwrap();
            fs::write(
                source.join(".rabi.toml"),
                "[hooks]\nsetup = ['test -f ready && touch source-hook']\n",
            )
            .unwrap();
            let mut args = if fallback {
                vec!["create", "--pr", "7"]
            } else {
                vec!["create", "target"]
            };
            if json {
                args.push("--json");
            }
            let mut command = repo.command(env!("CARGO_BIN_EXE_rabi"), &args);
            command.current_dir(&source);
            if mode == 2 {
                command.env("RABI_TEST_FAIL_METADATA_SAVE", "1");
            }
            if let Some(path) = gh_path {
                command.env("PATH", path);
            }
            let created = command.output().unwrap();
            assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
            let target = managed(&repo, if fallback { "topic+fix" } else { "target" });
            let hint = setup_retry_hint(&created, json);
            assert!(!hint.chars().any(char::is_control), "{hint:?}");
            fs::write(target.join("ready"), "").unwrap();
            let retried = run_setup_retry_hint(&repo, &repo.root, &hint);
            assert!(
                retried.status.success(),
                "{hint}: {}",
                text(&retried.stderr)
            );
            assert!(target.join("source-hook").is_file(), "{hint}");
            assert!(!target.join("primary-hook").exists(), "{hint}");
            let shown = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &["show", ".", "--json"])
                .current_dir(&target)
                .output()
                .unwrap();
            assert_eq!(
                json_value(&shown)["data"]["workspace"]["setup"]["state"],
                "ok"
            );
            assert!(!source.join("source-hook").exists());
        }
    }
}

#[test]
fn setup_retry_hint_preserves_copy_patterns_after_copy_failure() {
    for json in [false, true] {
        for fallback in [false, true] {
            let repo = Repo::new();
            let directory = "private's $data `literal`\t";
            let outside = repo.root.join("outside");
            fs::create_dir(&outside).unwrap();
            std::os::unix::fs::symlink(&outside, repo.repo.join(directory)).unwrap();
            fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
            repo.git(&["add", ".gitignore", directory]);
            repo.git(&["commit", "-m", "copy retry fixture"]);
            let gh_path = fallback.then(|| {
                let path = fake_gh_for_topic_pr(&repo);
                publish_pr_topic(&repo);
                let gh = repo.root.join("bin/gh");
                let script = fs::read_to_string(&gh).unwrap();
                fs::write(&gh, script.replace("\"topic\"", "\"topic+fix\"")).unwrap();
                assert!(repo
                    .create_recorded(&["create", "duplicate", "--pr", "7", "--no-setup"])
                    .status
                    .success());
                path
            });
            fs::remove_file(repo.repo.join(directory)).unwrap();
            fs::create_dir(repo.repo.join(directory)).unwrap();
            fs::write(repo.repo.join(directory).join("secret.env"), "secret").unwrap();
            fs::write(repo.repo.join(".env"), "environment").unwrap();
            fs::write(repo.repo.join("extra.env"), "extra").unwrap();
            assert!(!repo.repo.join(".worktreeinclude").exists());
            let name = if fallback { "topic+fix" } else { "copy-retry" };
            let mut args = if fallback {
                vec!["create", "--pr", "7"]
            } else {
                vec!["create", name]
            };
            let pattern = format!("{directory}/*.env");
            args.extend(["--copy-file", ".env", "--copy-file", &pattern]);
            args.extend(["--copy-file", "extra.env"]);
            if json {
                args.push("--json");
            }
            let mut command = repo.command(env!("CARGO_BIN_EXE_rabi"), &args);
            if let Some(path) = gh_path {
                command.env("PATH", path);
            }
            let created = command.output().unwrap();
            assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
            let target = managed(&repo, name);
            let shown = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &["show", "--json"])
                .current_dir(&target)
                .output()
                .unwrap();
            let shown = json_value(&shown);
            assert_eq!(shown["data"]["workspace"]["setup"]["state"], "failed");
            assert!(!target.join(directory).join("secret.env").exists());
            assert!(!outside.join("secret.env").exists());
            // Fix the destination obstruction, then execute the displayed hint verbatim.
            fs::remove_file(target.join(directory)).unwrap();
            let hint = setup_retry_hint(&created, json);
            let retried = run_setup_retry_hint(&repo, &repo.root, &hint);
            assert!(
                retried.status.success(),
                "{hint}: {} {}",
                text(&retried.stdout),
                text(&retried.stderr)
            );
            assert_eq!(
                fs::read_to_string(target.join(directory).join("secret.env")).unwrap(),
                "secret",
                "{hint}"
            );
            assert_eq!(
                fs::read_to_string(target.join(".env")).unwrap(),
                "environment"
            );
            assert_eq!(
                fs::read_to_string(target.join("extra.env")).unwrap(),
                "extra"
            );
            assert_eq!(hint.starts_with("(cd "), fallback, "{hint}");
            assert_eq!(hint.matches(" --copy-file ").count(), 3, "{hint}");
            let shown = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &["show", "--json"])
                .current_dir(&target)
                .output()
                .unwrap();
            let shown = json_value(&shown);
            assert_eq!(shown["data"]["workspace"]["setup"]["state"], "ok");
        }
    }
}

#[test]
fn setup_retry_hint_escapes_copy_patterns_in_text_and_json() {
    let mut hints = Vec::new();
    for json in [false, true] {
        let repo = Repo::new();
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = ['exit 1']\n",
        )
        .unwrap();
        let mut args = vec![
            "create",
            "copy-retry",
            "--copy-file",
            "private's $data `literal`/*.env",
            "--copy-file",
            "control\u{1b}[31m\t\n\u{202e}*.env",
        ];
        if json {
            args.push("--json");
        }
        let created = repo.rabi(&args);
        assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
        let hint = setup_retry_hint(&created, json);
        assert!(!hint.chars().any(char::is_control), "{hint:?}");
        assert!(
            hint.ends_with(
                " --copy-file 'private'\\''s $data `literal`/*.env' --copy-file $'control\\x1b[31m\\t\\n\\xe2\\x80\\xae*.env'"
            ),
            "{hint}"
        );
        hints.push(hint.replace(repo.repo.to_str().unwrap(), "PRIMARY"));
    }
    assert_eq!(hints[0], hints[1]);
}

#[test]
fn setup_retry_hint_resolves_pr_names_and_ambiguous_links() {
    for json in [false, true] {
        for ambiguous in [false, true] {
            let repo = Repo::new();
            let path = fake_gh_for_topic_pr(&repo);
            publish_pr_topic(&repo);
            let gh = repo.root.join("bin/gh");
            let script = fs::read_to_string(&gh).unwrap();
            fs::write(&gh, script.replace("\"topic\"", "\"topic+fix\"")).unwrap();
            if ambiguous {
                assert!(repo
                    .create_recorded(&["create", "duplicate", "--pr", "7", "--no-setup"])
                    .status
                    .success());
            }
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\nsetup = [\"test -f ready && touch hook-ran\"]\n",
            )
            .unwrap();
            let mut args = vec!["create", "--pr", "7"];
            if json {
                args.push("--json");
            }
            let created = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("PATH", &path)
                .output()
                .unwrap();
            assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
            let hint = setup_retry_hint(&created, json);
            let target = managed(&repo, "topic+fix");
            fs::write(target.join("ready"), "").unwrap();
            let retried = run_setup_retry_hint(&repo, &repo.repo, &hint);
            assert!(
                retried.status.success(),
                "{hint}: {} {}",
                text(&retried.stdout),
                text(&retried.stderr)
            );
            assert!(target.join("hook-ran").is_file());
            if ambiguous {
                assert!(hint.starts_with("(cd "), "{hint}");
            } else {
                assert!(hint.ends_with("setup 'pr:7'"), "{hint}");
            }
            let listed = json_value(&repo.rabi(&["list", "--json"]));
            let workspace = listed["data"]["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|tree| tree["name"] == "topic+fix")
                .unwrap();
            assert_eq!(workspace["setup"]["state"], "ok");
            if ambiguous {
                assert!(!managed(&repo, "duplicate/hook-ran").exists());
            }
        }
    }
}

fn setup_retry_hint(out: &std::process::Output, json: bool) -> String {
    let message = if json {
        json_value(out)["error"]["message"]
            .as_str()
            .unwrap()
            .to_owned()
    } else {
        text(&out.stderr)
    };
    message
        .split_once("Retry with `")
        .expect("retry hint")
        .1
        .strip_suffix("`.")
        .expect("end of retry hint")
        .to_owned()
}

fn run_setup_retry_hint(repo: &Repo, cwd: &Path, hint: &str) -> std::process::Output {
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_rabi")).parent().unwrap();
    repo.command("bash", &["-c", hint])
        .current_dir(cwd)
        .env("RABI_TRUST_ALL", "1")
        .env(
            "PATH",
            format!(
                "{}:{}",
                binary_dir.display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap()
}

#[test]
fn setup_retry_hint_preserves_selected_repository_from_any_directory() {
    for json in [false, true] {
        for from_other_repo in [false, true] {
            for directory in [
                "primary's $repo `literal` with spaces",
                "primary's $repo `literal`\u{1b}[31m\t\nend",
            ] {
                let mut repo = Repo::new();
                let renamed = repo.root.join(directory);
                fs::rename(&repo.repo, &renamed).unwrap();
                repo.repo = renamed;
                let other = Repo::new();
                let outside = repo.root.join("outside");
                fs::create_dir(&outside).unwrap();
                let decoy = repo.rabi(&[
                    "-R",
                    other.repo.to_str().unwrap(),
                    "create",
                    "lifecycle",
                    "--no-setup",
                ]);
                assert!(decoy.status.success(), "{}", text(&decoy.stderr));
                let other_records = other.repo.join(".git/rabi/workspaces");
                let before = records::read(&other_records).unwrap();
                fs::write(
                    other.repo.join(".rabi.toml"),
                    "[hooks]\nsetup = [\"touch wrong-target\"]\n",
                )
                .unwrap();
                fs::write(
                    repo.repo.join(".rabi.toml"),
                    "[hooks]\nsetup = [\"test -f ready && touch hook-ran\"]\n",
                )
                .unwrap();
                let mut args = vec!["-R", repo.repo.to_str().unwrap(), "create", "lifecycle"];
                if json {
                    args.push("--json");
                }
                let created = repo
                    .command(env!("CARGO_BIN_EXE_rabi"), &args)
                    .current_dir(&outside)
                    .output()
                    .unwrap();
                assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
                let hint = setup_retry_hint(&created, json);
                assert!(!hint.chars().any(char::is_control), "{hint:?}");
                assert!(hint.starts_with("cd "), "{hint}");
                assert!(hint.ends_with("setup 'lifecycle'"), "{hint}");
                fs::write(managed(&repo, "lifecycle/ready"), "").unwrap();
                let cwd = if from_other_repo {
                    &other.repo
                } else {
                    &outside
                };
                let retried = run_setup_retry_hint(&repo, cwd, &hint);
                assert!(
                    retried.status.success(),
                    "{hint}: {} {}",
                    text(&retried.stdout),
                    text(&retried.stderr)
                );
                assert!(managed(&repo, "lifecycle/hook-ran").is_file());
                let shown = repo.rabi(&["show", "lifecycle", "--json"]);
                assert_eq!(
                    json_value(&shown)["data"]["workspace"]["setup"]["state"],
                    "ok"
                );
                assert_eq!(records::read(&other_records).unwrap(), before);
                let decoy_path =
                    managed_directory(&other.repo, &repo.root.join("xdg")).join("lifecycle");
                assert!(!decoy_path.join("wrong-target").exists());
            }
        }
    }
}

#[cfg(debug_assertions)]
#[test]
fn setup_retry_hint_works_without_saved_links() {
    for json in [false, true] {
        let repo = Repo::new();
        let path = fake_gh_for_topic_pr(&repo);
        publish_pr_topic(&repo);
        let gh = repo.root.join("bin/gh");
        let script = fs::read_to_string(&gh).unwrap();
        fs::write(&gh, script.replace("\"topic\"", "\"topic+fix\"")).unwrap();
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\nsetup = [\"test -f ready && touch hook-ran\"]\n",
        )
        .unwrap();
        let mut args = vec!["create", "--pr", "7"];
        if json {
            args.push("--json");
        }
        let created = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("PATH", &path)
            .env("RABI_TEST_FAIL_METADATA_SAVE", "1")
            .output()
            .unwrap();
        assert_eq!(created.status.code(), Some(6), "{}", text(&created.stderr));
        if json {
            let doc = json_value(&created);
            assert_eq!(doc["data"]["metadata"]["ok"], false);
            assert!(doc["data"]["workspace"]["links"]["pr"].is_null());
        }
        let hint = setup_retry_hint(&created, json);
        fs::write(managed(&repo, "topic+fix/ready"), "").unwrap();
        let retried = run_setup_retry_hint(&repo, &repo.root, &hint);
        assert!(
            retried.status.success(),
            "{hint}: {} {}",
            text(&retried.stdout),
            text(&retried.stderr)
        );
        assert!(hint.starts_with("(cd "), "{hint}");
        assert!(managed(&repo, "topic+fix/hook-ran").is_file());
        let shown = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["show", "--json"])
            .current_dir(managed(&repo, "topic+fix"))
            .output()
            .unwrap();
        assert_eq!(
            json_value(&shown)["data"]["workspace"]["setup"]["state"],
            "ok"
        );
    }
}

#[test]
fn setup_lifecycle_failure_retry_and_shared_state() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"test -f ready\"]\n",
    )
    .unwrap();
    let out = repo.rabi(&["create", "lifecycle", "--json"]);
    assert_eq!(out.status.code(), Some(6), "{}", text(&out.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["workspace"]["setup"]["state"], "failed");
    assert_eq!(doc["data"]["setup"]["exitCode"], 1);
    assert_eq!(
        setup_retry_hint(&out, true),
        format!("cd '{}' && rabi setup 'lifecycle'", repo.repo.display())
    );
    let path = managed(&repo, "lifecycle");
    assert!(text(&repo.rabi(&["list"]).stdout).contains("setup:failed"));
    let records = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&records).unwrap();
    let reused = repo.rabi(&["create", "lifecycle", "--reuse", "--json"]);
    assert!(reused.status.success(), "{}", text(&reused.stderr));
    assert_eq!(
        json_value(&reused)["data"]["workspace"]["setup"]["state"],
        "failed"
    );
    assert_eq!(records::read(&records).unwrap(), before);
    fs::write(path.join("ready"), "").unwrap();
    let retry = repo.rabi(&["setup", "lifecycle", "--json"]);
    assert!(retry.status.success(), "{}", text(&retry.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&retry.stdout).unwrap();
    assert_eq!(doc["data"]["setup"]["state"], "ok");
    for args in [
        vec!["show", "lifecycle", "--json"],
        vec!["set", "lifecycle", "--purpose", "done", "--json"],
    ] {
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{}", text(&out.stderr));
        let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["data"]["workspace"]["setup"]["state"], "ok");
        assert!(doc["data"]["workspace"]["setup"]["error"].is_null());
    }
}

#[test]
fn setup_lifecycle_skip_and_conflicting_copy_flag() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
    fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
    fs::write(repo.repo.join("secret.env"), "secret").unwrap();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch hook-ran\"]\n",
    )
    .unwrap();
    let out = repo.rabi(&[
        "create",
        "bad",
        "--no-setup",
        "--copy-file",
        "*.env",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(text(&out.stdout).contains("--no-setup"));
    assert!(!managed(&repo, "bad").exists());
    let out = repo.rabi(&["create", "skip", "--no-setup", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["workspace"]["setup"]["state"], "skipped");
    assert_eq!(doc["data"]["setup"]["state"], "skipped");
    assert!(!managed(&repo, "skip").join("secret.env").exists());
    assert!(!managed(&repo, "skip").join("hook-ran").exists());
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["setup", "skip", "--no-hooks", "--json"],
        )
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert!(managed(&repo, "skip").join("secret.env").exists());
    assert!(!managed(&repo, "skip").join("hook-ran").exists());
}

#[test]
fn setup_lifecycle_trust_refusal_and_primary_are_read_only() {
    let repo = Repo::new();
    assert!(repo.rabi(&["create", "target", "--json"]).status.success());
    fs::write(repo.repo.join(".gitignore"), "*.env\n").unwrap();
    fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
    fs::write(repo.repo.join("secret.env"), "secret").unwrap();
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch hook-ran\"]\n",
    )
    .unwrap();
    let records = repo.repo.join(".git/rabi/workspaces");
    let before: Vec<_> = fs::read_dir(&records)
        .unwrap()
        .map(|e| fs::read(e.unwrap().path()).unwrap())
        .collect();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["setup", "target", "--no-input", "--json"],
        )
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4), "{}", text(&out.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["code"], "untrusted");
    let after: Vec<_> = fs::read_dir(&records)
        .unwrap()
        .map(|e| fs::read(e.unwrap().path()).unwrap())
        .collect();
    assert_eq!(before, after);
    assert!(!managed(&repo, "target").join("secret.env").exists());
    assert!(!managed(&repo, "target").join("hook-ran").exists());
    let out = repo.rabi(&["setup", "--json"]);
    assert_eq!(out.status.code(), Some(4));
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["code"], "primary");
    assert_eq!(
        repo.rabi(&["setup", "missing", "--json"]).status.code(),
        Some(3)
    );
}

#[test]
fn setup_lifecycle_rejects_unknown_hook_keys() {
    let repo = Repo::new();
    for key in ["bogus", "post-create", "pre-delete"] {
        fs::write(
            repo.repo.join(".rabi.toml"),
            format!("[hooks]\n{key} = []\n"),
        )
        .unwrap();
        for args in [
            vec!["create", "target", "--json"],
            vec!["setup", "target", "--json"],
            vec!["trust", "add", "--json"],
            vec!["remove", "target", "--json"],
            vec!["gc", "--json"],
        ] {
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(1),
                "{args:?}: {}",
                text(&out.stderr)
            );
            let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(doc["error"]["code"], "failed");
            assert!(doc["error"]["message"]
                .as_str()
                .unwrap()
                .contains(&format!("unknown field `{key}`")));
        }
    }
}

#[test]
fn setup_lifecycle_copies_from_primary_from_other_checkout_and_subdirectory() {
    let repo = Repo::new();
    fs::write(repo.repo.join(".gitignore"), "*.env\n*.extra\n").unwrap();
    fs::write(repo.repo.join(".worktreeinclude"), "*.env\n").unwrap();
    repo.git(&["add", ".gitignore", ".worktreeinclude"]);
    repo.git(&["commit", "-m", "copy rules"]);
    fs::write(repo.repo.join("secret.env"), "primary").unwrap();
    fs::write(repo.repo.join("secret.extra"), "extra").unwrap();
    assert!(repo
        .rabi(&["create", "caller", "--no-setup"])
        .status
        .success());
    let caller = managed(&repo, "caller");
    fs::write(caller.join("secret.env"), "caller").unwrap();
    fs::write(caller.join(".worktreeinclude"), "nothing\n").unwrap();
    fs::create_dir(caller.join("sub")).unwrap();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["create", "target", "--copy-file", "*.extra", "--json"],
        )
        .current_dir(caller.join("sub"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    let target = managed(&repo, "target");
    assert_eq!(
        fs::read_to_string(target.join("secret.env")).unwrap(),
        "primary"
    );
    assert_eq!(
        fs::read_to_string(target.join("secret.extra")).unwrap(),
        "extra"
    );
    fs::write(target.join("secret.env"), "keep").unwrap();
    fs::remove_file(target.join("secret.extra")).unwrap();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "setup",
                "target",
                "--copy-file",
                "*.extra",
                "--no-hooks",
                "--json",
            ],
        )
        .current_dir(caller.join("sub"))
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        fs::read_to_string(target.join("secret.env")).unwrap(),
        "keep"
    );
    assert_eq!(
        fs::read_to_string(target.join("secret.extra")).unwrap(),
        "extra"
    );
}

#[test]
fn setup_lifecycle_interrupted_is_display_only() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "interrupted", "--json"])
        .status
        .success());
    let record = fs::read_dir(repo.repo.join(".git/rabi/workspaces"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let host = test_git::command("uname").arg("-n").output().unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    doc["setup"] = serde_json::json!({"state":"running", "at":"2026-10-21T00:00:00Z", "error":null, "pid":unused_pid(), "host":text(&host.stdout)});
    fs::write(&record, serde_json::to_vec(&doc).unwrap()).unwrap();
    let before = fs::read(&record).unwrap();
    let out = repo.rabi(&["show", "interrupted", "--json"]);
    assert!(out.status.success());
    let doc: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["data"]["workspace"]["setup"]["state"], "interrupted");
    assert_eq!(before, fs::read(&record).unwrap());
    let out = repo.rabi(&["list"]);
    assert!(text(&out.stdout).contains("setup:interrupted"));
    assert_eq!(before, fs::read(&record).unwrap());
    assert!(repo
        .rabi(&["setup", "interrupted", "--json"])
        .status
        .success());
    let doc: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    assert_eq!(doc["setup"]["state"], "ok");
}

#[test]
fn setup_lifecycle_running_is_saved_before_hooks_and_process_fields_stay_internal() {
    let repo = Repo::new();
    fs::write(
        repo.repo.join(".rabi.toml"),
        r#"[hooks]
setup = ['cat "$RABI_REPO_PATH/.git/rabi/workspaces/$RABI_WORKSPACE_ID.json" > observed.json']
"#,
    )
    .unwrap();
    let out = repo.rabi(&["create", "observe", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    let observed: serde_json::Value =
        serde_json::from_slice(&fs::read(managed(&repo, "observe/observed.json")).unwrap())
            .unwrap();
    assert_eq!(observed["setup"]["state"], "running");
    assert!(observed["setup"]["pid"].as_u64().is_some_and(|pid| pid > 0));
    assert!(observed["setup"]["host"].is_string());
    let out = json_value(&out);
    assert!(out["data"]["workspace"]["setup"].get("pid").is_none());
    assert!(out["data"]["workspace"]["setup"].get("host").is_none());
}

#[test]
fn setup_lifecycle_copy_failure_stops_hooks_and_is_retryable() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "copy-retry", "--no-setup"])
        .status
        .success());
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch hook-ran\"]\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("missing", repo.repo.join(".worktreeinclude")).unwrap();
    let out = repo.rabi(&["setup", "copy-retry", "--json"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out.stderr));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["workspace"]["setup"]["state"], "failed");
    assert_eq!(doc["data"]["setup"]["ran"], false);
    assert!(doc["data"]["copy"]["error"].is_string());
    assert!(!managed(&repo, "copy-retry/hook-ran").exists());
    fs::remove_file(repo.repo.join(".worktreeinclude")).unwrap();
    let out = repo.rabi(&["setup", "copy-retry", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_value(&out)["data"]["setup"]["state"], "ok");
    assert!(managed(&repo, "copy-retry/hook-ran").is_file());
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"exit 42\"]\n",
    )
    .unwrap();
    let out = repo.rabi(&["setup", "copy-retry", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json_value(&out)["data"]["setup"]["exitCode"], 42);
}

#[test]
fn setup_lifecycle_unknown_adopt_and_null_after_forget() {
    let repo = Repo::new();
    let external = repo.root.join("external");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "external",
        external.to_str().unwrap(),
    ]);
    let out = repo.rabi(&["adopt", external.to_str().unwrap(), "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        json_value(&out)["data"]["workspace"]["setup"],
        serde_json::json!({"state":"unknown", "at":null, "error":null})
    );
    // A pre-P6 individual record has no setup field.
    let record = fs::read_dir(repo.repo.join(".git/rabi/workspaces"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "json"))
        .unwrap();
    let mut doc: serde_json::Value = serde_json::from_slice(&fs::read(&record).unwrap()).unwrap();
    doc.as_object_mut().unwrap().remove("setup");
    fs::write(&record, serde_json::to_vec(&doc).unwrap()).unwrap();
    let before = fs::read(&record).unwrap();
    let out = repo.rabi(&["show", "external", "--json"]);
    assert_eq!(
        json_value(&out)["data"]["workspace"]["setup"]["state"],
        "unknown"
    );
    assert_eq!(before, fs::read(&record).unwrap());
    let out = repo.rabi(&["forget", "external", "--json"]);
    assert!(out.status.success());
    assert!(json_value(&out)["data"]["items"][0]["workspace"]["setup"].is_null());
}

#[test]
fn setup_lifecycle_default_dot_and_selectors_match_inspection() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "target", "--no-setup"])
        .status
        .success());
    assert!(repo.rabi(&["set", "target", "--pr", "7"]).status.success());
    let path = managed(&repo, "target/sub");
    fs::create_dir(&path).unwrap();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["setup", "--no-hooks", "--json"],
        )
        .current_dir(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_value(&out)["data"]["workspace"]["name"], "target");
    let out = repo.rabi(&["setup", "pr:7", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        json_value(&out)["data"]["workspace"]["setup"]["state"],
        "ok"
    );
    let listed = repo.rabi(&["list", "--json"]);
    let doc = json_value(&listed);
    let target = doc["data"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == "target")
        .unwrap();
    assert_eq!(target["setup"]["state"], "ok");
}

#[test]
fn setup_lifecycle_pre_remove_runs_for_delete_and_gc() {
    for command in ["remove", "gc"] {
        let repo = Repo::new();
        assert!(repo.rabi(&["create", "target"]).status.success());
        land_branch(&repo, "target");
        let marker = repo.root.join("pre-remove-ran");
        fs::write(
            repo.repo.join(".rabi.toml"),
            format!("[hooks]\npre-remove = ['touch {}']\n", marker.display()),
        )
        .unwrap();
        let args = if command == "remove" {
            vec!["remove", "target", "--json"]
        } else {
            vec!["gc", "--apply", "--merged", "--json"]
        };
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert!(marker.is_file(), "{command}: {}", text(&out.stdout));
        assert!(!managed(&repo, "target").exists());
    }
}

#[test]
fn setup_lifecycle_adopted_record_keeps_setup_unknown_until_explicit_run() {
    let repo = Repo::new();
    let external = repo.root.join("external");
    repo.git(&[
        "worktree",
        "add",
        "-b",
        "external",
        external.to_str().unwrap(),
    ]);
    assert!(repo
        .rabi(&["adopt", external.to_str().unwrap()])
        .status
        .success());
    let out = repo.rabi(&["show", "external", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        json_value(&out)["data"]["workspace"]["setup"]["state"],
        "unknown"
    );
    let out = repo.rabi(&["set", "external", "--purpose", "updated", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        json_value(&out)["data"]["workspace"]["setup"]["state"],
        "unknown"
    );
    let out = repo.rabi(&["setup", "external", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        json_value(&out)["data"]["workspace"]["setup"]["state"],
        "ok"
    );
}

#[test]
fn retirement_remove_keeps_unpreserved_branch_but_branch_alone_is_not_a_workspace() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "local", "--no-setup"])
        .status
        .success());
    let path = managed_root(&repo).join("local");
    fs::write(path.join("local.txt"), "local\n").expect("file");
    repo.command("git", &["add", "."])
        .current_dir(&path)
        .output()
        .expect("add");
    assert!(repo
        .command("git", &["commit", "-m", "local"])
        .current_dir(&path)
        .output()
        .expect("commit")
        .status
        .success());
    let out = repo.rabi(&["remove", "local", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    let value = assert_envelope(&out, "remove");
    assert_eq!(value["data"]["items"][0]["removed"], true);
    assert_eq!(value["data"]["items"][0]["branchKeptReason"], "unpreserved");
    assert_eq!(value["warnings"][0]["code"], "branch_kept_unpreserved");
    assert!(branch_listed(&repo, "local"));
    let retry = repo.rabi(&["remove", "local", "--json"]);
    assert_eq!(retry.status.code(), Some(3));
    assert_eq!(json_value(&retry)["error"]["code"], "not_found");
}

#[test]
fn retirement_ignored_content_is_clean_and_removed_without_discard_changes() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "safe", "--no-setup"])
        .status
        .success());
    let path = managed_root(&repo).join("safe");
    fs::create_dir_all(repo.repo.join(".git/info")).expect("info");
    fs::write(repo.repo.join(".git/info/exclude"), "secret\n").expect("ignore");
    fs::write(path.join("secret"), "keep me").expect("secret");
    assert_retirement_dirty(&repo, "safe", false);
    let out = repo.rabi(&["remove", "safe", "--dry-run", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
    assert_eq!(json_value(&out)["data"]["plan"], true);
    assert_eq!(json_value(&out)["data"]["items"][0]["removed"], false);
    assert_eq!(json_value(&out)["data"]["items"][0]["wouldRemove"], true);
    assert_eq!(json_value(&out)["warnings"], serde_json::json!([]));
    assert!(path.join("secret").exists());
    assert!(branch_listed(&repo, "safe"));
    let out = repo.rabi(&["remove", "safe", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    let doc = assert_envelope(&out, "remove");
    assert_eq!(doc["data"]["items"][0]["removed"], true);
    assert_eq!(doc["warnings"], serde_json::json!([]));
    assert!(!path.exists());
    assert!(!path.join("secret").exists());
    assert!(!worktree_registered(&repo, "safe"));
}

fn assert_retirement_dirty(repo: &Repo, name: &str, dirty: bool) {
    for command in ["show", "list"] {
        let args = if command == "show" {
            vec!["show", name, "--json"]
        } else {
            vec!["list", "--kind", "workspace", "--json"]
        };
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, command);
        let workspace = if command == "show" {
            &doc["data"]["workspace"]
        } else {
            doc["data"]["workspaces"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["name"] == name)
                .unwrap()
        };
        assert_eq!(workspace["dirty"], dirty, "{command}: {doc}");
    }
}

#[test]
fn retirement_ignored_content_does_not_hide_uncommitted_work() {
    for change in ["untracked", "modified", "staged", "renamed"] {
        let repo = Repo::new();
        repo.git(&["config", "status.showUntrackedFiles", "no"]);
        fs::create_dir_all(repo.repo.join(".git/info")).unwrap();
        fs::write(repo.repo.join(".git/info/exclude"), "target/\n").unwrap();
        assert!(repo
            .rabi(&["create", "dirty", "--no-setup"])
            .status
            .success());
        land_branch(&repo, "dirty");
        let path = managed(&repo, "dirty");
        fs::create_dir(path.join("target")).unwrap();
        fs::write(path.join("target/cache"), "disposable").unwrap();
        match change {
            "untracked" => fs::write(path.join("precious"), "keep").unwrap(),
            "modified" | "staged" => {
                fs::write(path.join("README.md"), "keep\n").unwrap();
                if change == "staged" {
                    let out = repo
                        .command("git", &["add", "README.md"])
                        .current_dir(&path)
                        .output()
                        .unwrap();
                    assert!(out.status.success());
                }
            }
            "renamed" => {
                let out = repo
                    .command("git", &["mv", "README.md", "renamed\nfile"])
                    .current_dir(&path)
                    .output()
                    .unwrap();
                assert!(out.status.success());
            }
            _ => unreachable!(),
        }
        assert_retirement_dirty(&repo, "dirty", true);
        for dry_run in [false, true] {
            let mut args = vec!["remove", "dirty", "--json"];
            if dry_run {
                args.push("--dry-run");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(4),
                "{change}: {}",
                text(&out.stdout)
            );
            let doc = assert_envelope(&out, "remove");
            assert_eq!(doc["error"]["code"], "dirty");
            assert!(
                doc["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("(1 files)"),
                "{doc}"
            );
            assert!(path.join("target/cache").is_file());
        }
        for apply in [false, true] {
            let mut args = vec!["gc", "--merged", "--json"];
            if apply {
                args.push("--apply");
            }
            let out = repo.rabi(&args);
            assert!(out.status.success(), "{}", text(&out.stdout));
            let doc = assert_envelope(&out, "gc");
            assert_eq!(doc["data"]["items"][0]["action"], "skipped");
            assert!(doc["data"]["items"][0]["detail"]
                .as_str()
                .unwrap()
                .contains("--discard-changes"));
            assert!(path.exists());
        }
        let out = repo.rabi(&["remove", "dirty", "--discard-changes", "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        assert!(!path.exists());
    }
}

#[test]
fn retirement_ignored_content_does_not_skip_gc_plan_or_apply() {
    let repo = Repo::new();
    fs::create_dir_all(repo.repo.join(".git/info")).unwrap();
    fs::write(
        repo.repo.join(".git/info/exclude"),
        "target/\nnode_modules/\n",
    )
    .unwrap();
    assert!(repo
        .rabi(&["create", "landed", "--no-setup"])
        .status
        .success());
    land_branch(&repo, "landed");
    let path = managed(&repo, "landed");
    for directory in ["target", "node_modules"] {
        fs::create_dir(path.join(directory)).unwrap();
        fs::write(path.join(directory).join("cache"), "disposable").unwrap();
    }
    assert_retirement_dirty(&repo, "landed", false);
    for apply in [false, true] {
        let mut args = vec!["gc", "--merged", "--json"];
        if apply {
            args.push("--apply");
        }
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, "gc");
        assert_eq!(doc["data"]["plan"], !apply);
        assert_eq!(
            doc["data"]["items"][0]["action"],
            if apply { "removed" } else { "wouldRemove" }
        );
        assert_eq!(doc["warnings"], serde_json::json!([]));
        assert_eq!(path.join("target/cache").exists(), !apply);
        assert_eq!(path.join("node_modules/cache").exists(), !apply);
        assert_eq!(path.exists(), !apply);
    }
}

#[test]
fn retirement_ignored_nested_repository_is_removed_with_workspace() {
    let repo = Repo::new();
    fs::create_dir_all(repo.repo.join(".git/info")).unwrap();
    fs::write(repo.repo.join(".git/info/exclude"), "vendor/\n").unwrap();
    assert!(repo
        .rabi(&["create", "nested", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "nested");
    let nested = path.join("vendor/x");
    fs::create_dir_all(&nested).unwrap();
    let out = repo
        .command("git", &["init", "-b", "main"])
        .current_dir(&nested)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    fs::write(nested.join("local"), "nested repository contents\n").unwrap();
    assert_retirement_dirty(&repo, "nested", false);
    let out = repo.rabi(&["remove", "nested", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    assert_eq!(
        assert_envelope(&out, "remove")["warnings"],
        serde_json::json!([])
    );
    assert!(!path.exists());
    assert!(!nested.exists());
}

fn retirement_populated_submodule_repo() -> Repo {
    let repo = Repo::new();
    let source = Repo::new();
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        source.repo.to_str().unwrap(),
        "sub",
    ]);
    repo.git(&["commit", "-m", "add submodule"]);
    repo.git(&["config", "diff.ignoreSubmodules", "all"]);
    repo.git(&["config", "submodule.sub.ignore", "all"]);
    fs::create_dir_all(repo.repo.join(".git/info")).unwrap();
    fs::write(repo.repo.join(".git/info/exclude"), "cache\n").unwrap();
    assert!(repo
        .rabi(&["create", "submodule", "--no-setup"])
        .status
        .success());
    land_branch(&repo, "submodule");
    let path = managed(&repo, "submodule");
    let out = repo
        .command(
            "git",
            &[
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "update",
                "--init",
            ],
        )
        .current_dir(&path)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    fs::write(path.join("cache"), "disposable\n").unwrap();
    assert_retirement_dirty(&repo, "submodule", false);
    repo
}

#[test]
fn retirement_populated_submodule_requires_discard_before_any_removal() {
    let repo = retirement_populated_submodule_repo();
    assert!(repo
        .rabi(&["create", "clean", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "submodule");
    let marker = repo.root.join("hook-ran");
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\npre-remove = ['touch {}']\n", marker.display()),
    )
    .unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&record_dir).unwrap();
    for dry_run in [false, true] {
        for json in [false, true] {
            let mut args = vec!["remove", "clean", "submodule"];
            if dry_run {
                args.push("--dry-run");
            }
            if json {
                args.push("--json");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(4),
                "{args:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            let detail = if json {
                let doc = assert_envelope(&out, "remove");
                assert_eq!(doc["error"]["code"], "dirty", "{doc}");
                assert!(
                    doc["data"]["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|item| item["removed"] == false),
                    "{doc}"
                );
                doc.to_string()
            } else {
                text(&out.stderr)
            };
            for expected in [
                "populated submodules",
                "unpushed commits",
                "--discard-changes",
            ] {
                assert!(detail.contains(expected), "{detail}");
            }
            assert!(!marker.exists());
            assert_eq!(records::read(&record_dir).unwrap(), before);
            for name in ["clean", "submodule"] {
                assert!(managed(&repo, name).is_dir());
                assert!(worktree_registered(&repo, name));
                assert!(branch_listed(&repo, name));
            }
            assert!(path.join("sub/README.md").exists());
            assert!(path.join("cache").exists());
        }
    }
    let plan = repo.rabi(&[
        "remove",
        "submodule",
        "--discard-changes",
        "--dry-run",
        "--json",
    ]);
    assert!(plan.status.success(), "{}", text(&plan.stdout));
    assert!(path.is_dir());
    assert!(!marker.exists());
    let out = repo.rabi(&["remove", "submodule", "--discard-changes", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert_eq!(
        assert_envelope(&out, "remove")["data"]["items"][0]["removed"],
        true
    );
    assert!(!path.exists());
    assert!(!worktree_registered(&repo, "submodule"));
    assert!(marker.exists());
    assert!(managed(&repo, "clean").is_dir());
}

#[test]
fn retirement_populated_submodule_is_skipped_by_gc_plan_and_apply() {
    let repo = retirement_populated_submodule_repo();
    let path = managed(&repo, "submodule");
    let marker = repo.root.join("hook-ran");
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\npre-remove = ['touch {}']\n", marker.display()),
    )
    .unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&record_dir).unwrap();
    for apply in [false, true] {
        let mut args = vec!["gc", "--merged", "--json"];
        if apply {
            args.push("--apply");
        }
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, "gc");
        let item = &doc["data"]["items"][0];
        assert_eq!(item["workspace"]["name"], "submodule", "{doc}");
        assert_eq!(item["action"], "skipped", "{doc}");
        let detail = item["detail"].as_str().unwrap();
        for expected in [
            "populated submodules",
            "unpushed commits",
            " remove --discard-changes -- 'submodule'",
        ] {
            assert!(detail.contains(expected), "{detail}");
        }
        assert!(!marker.exists());
        assert_eq!(records::read(&record_dir).unwrap(), before);
        assert!(path.join("sub/README.md").exists());
        assert!(path.join("cache").exists());
        assert!(worktree_registered(&repo, "submodule"));
    }
}

#[test]
fn retirement_submodule_layouts_follow_git_removal_guard() {
    for layout in ["uninitialized", "retained", "embedded"] {
        let repo = retirement_populated_submodule_repo();
        let path = managed(&repo, "submodule");
        let sub = path.join("sub");
        let out = repo
            .command("git", &["rev-parse", "--absolute-git-dir"])
            .current_dir(&sub)
            .output()
            .unwrap();
        assert!(out.status.success());
        let git_dir = PathBuf::from(text(&out.stdout).trim());
        let modules = git_dir.parent().unwrap();
        if layout == "embedded" {
            let out = repo
                .command("git", &["config", "--unset", "core.worktree"])
                .current_dir(&sub)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", text(&out.stderr));
            fs::remove_file(sub.join(".git")).unwrap();
            fs::rename(&git_dir, sub.join(".git")).unwrap();
            fs::remove_dir(modules).unwrap();
        } else {
            let out = repo
                .command("git", &["submodule", "deinit", "--force", "--all"])
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(out.status.success(), "{}", text(&out.stderr));
            if layout == "uninitialized" {
                fs::remove_dir_all(modules).unwrap();
            }
        }
        assert_retirement_dirty(&repo, "submodule", false);
        for dry_run in [true, false] {
            let mut args = vec!["remove", "submodule", "--json"];
            if dry_run {
                args.push("--dry-run");
            }
            let out = repo.rabi(&args);
            let expected = if layout == "uninitialized" { 0 } else { 4 };
            assert_eq!(
                out.status.code(),
                Some(expected),
                "{layout}: {}",
                text(&out.stdout)
            );
            if expected == 4 {
                assert_eq!(assert_envelope(&out, "remove")["error"]["code"], "dirty");
                assert!(path.join("cache").exists());
            }
        }
        if layout != "uninitialized" {
            let out = repo.rabi(&["remove", "submodule", "--discard-changes", "--json"]);
            assert!(out.status.success(), "{layout}: {}", text(&out.stdout));
        }
        assert!(!path.exists(), "{layout}");
        assert!(!worktree_registered(&repo, "submodule"), "{layout}");
    }
}

#[test]
fn retirement_ignored_content_does_not_hide_submodule_changes() {
    let repo = retirement_populated_submodule_repo();
    let path = managed(&repo, "submodule");
    for change in ["modified", "untracked", "head"] {
        let sub = path.join("sub");
        if change == "untracked" {
            fs::write(sub.join("precious"), "keep\n").unwrap();
        } else {
            fs::write(sub.join("README.md"), "keep\n").unwrap();
            if change == "head" {
                for args in [
                    vec!["add", "README.md"],
                    vec![
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.com",
                        "commit",
                        "-m",
                        "local submodule commit",
                    ],
                ] {
                    let out = repo
                        .command("git", &args)
                        .current_dir(&sub)
                        .output()
                        .unwrap();
                    assert!(out.status.success(), "{}", text(&out.stderr));
                }
            }
        }
        assert_retirement_dirty(&repo, "submodule", true);
        for dry_run in [false, true] {
            let mut args = vec!["remove", "submodule", "--json"];
            if dry_run {
                args.push("--dry-run");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(4),
                "{change}: {}",
                text(&out.stdout)
            );
            assert_eq!(assert_envelope(&out, "remove")["error"]["code"], "dirty");
        }
        for apply in [false, true] {
            let mut args = vec!["gc", "--merged", "--json"];
            if apply {
                args.push("--apply");
            }
            let out = repo.rabi(&args);
            assert!(out.status.success(), "{}", text(&out.stdout));
            assert_eq!(
                assert_envelope(&out, "gc")["data"]["items"][0]["action"],
                "skipped"
            );
        }
        if change == "modified" {
            let out = repo
                .command("git", &["restore", "README.md"])
                .current_dir(&sub)
                .output()
                .unwrap();
            assert!(out.status.success());
        } else if change == "untracked" {
            fs::remove_file(sub.join("precious")).unwrap();
        }
        assert!(path.join("cache").exists());
    }
    let out = repo.rabi(&["remove", "submodule", "--discard-changes", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stdout));
    assert_eq!(
        assert_envelope(&out, "remove")["data"]["items"][0]["removed"],
        true
    );
    assert!(!path.exists());
    assert!(!worktree_registered(&repo, "submodule"));
}

#[cfg(unix)]
#[test]
fn retirement_ignored_content_does_not_force_remove_late_untracked_files() {
    use std::os::unix::fs::PermissionsExt;
    for gc in [false, true] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "race", "--no-setup"])
            .status
            .success());
        land_branch(&repo, "race");
        repo.git(&["config", "status.showUntrackedFiles", "no"]);
        fs::create_dir_all(repo.repo.join(".git/info")).unwrap();
        fs::write(repo.repo.join(".git/info/exclude"), "cache\n").unwrap();
        let path = managed(&repo, "race");
        fs::write(path.join("cache"), "ignored\n").unwrap();
        let real_git = repo
            .command("sh", &["-c", "command -v git"])
            .output()
            .unwrap();
        assert!(real_git.status.success());
        let bin = repo.root.join("bin");
        fs::create_dir(&bin).unwrap();
        let shim = bin.join("git");
        fs::write(
            &shim,
            r#"#!/bin/sh
case " $* " in
  *" worktree remove "*)
    printf 'precious\n' > "$RACE_PATH/precious"
    ;;
esac
exec "$REAL_GIT" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        let search_path = std::env::join_paths(
            std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let args = if gc {
            vec!["gc", "--merged", "--apply", "--json"]
        } else {
            vec!["remove", "race", "--json"]
        };
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("PATH", search_path)
            .env("REAL_GIT", text(&real_git.stdout))
            .env("RACE_PATH", &path)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{}", text(&out.stdout));
        let doc = assert_envelope(&out, if gc { "gc" } else { "remove" });
        if gc {
            assert_eq!(doc["data"]["items"][0]["action"], "failed");
        } else {
            assert_eq!(doc["data"]["items"][0]["removed"], false);
        }
        assert_eq!(
            fs::read_to_string(path.join("precious")).unwrap(),
            "precious\n"
        );
        assert!(path.join("cache").exists());
        assert!(worktree_registered(&repo, "race"));
        assert!(branch_listed(&repo, "race"));
    }
}

#[test]
fn retirement_review6_remove_keeps_unpreserved_branch_when_hook_removes_directory() {
    assert_retirement_hook_loses_directory(false, false);
}

#[test]
fn retirement_review6_remove_honors_keep_branch_when_hook_removes_directory() {
    assert_retirement_hook_loses_directory(false, true);
}

#[test]
fn retirement_review6_gc_stops_when_hook_removes_directory() {
    assert_retirement_hook_loses_directory(true, false);
    assert_retirement_hook_loses_directory(true, true);
}

fn assert_retirement_hook_loses_directory(gc: bool, keep_branch: bool) {
    for move_directory in [false, true] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "topic", "--no-setup"])
            .status
            .success());
        commit_in_worktree(&repo, "topic");
        // GC needs integrated work; remove must also protect an unpreserved tip.
        if gc || keep_branch {
            repo.git(&["merge", "--ff-only", "refs/heads/topic"]);
        }
        let path = managed(&repo, "topic");
        let head = retirement_head_at(&repo, &path);
        let records = rabi::metadata::load_strict(&repo.repo).unwrap();
        let command = if move_directory {
            r#"cd "$RABI_REPO_PATH" && mv -- "$RABI_WORKSPACE_PATH" "$RABI_WORKSPACE_PATH.moved""#
        } else {
            r#"cd "$RABI_REPO_PATH" && rm -rf -- "$RABI_WORKSPACE_PATH""#
        };
        fs::write(
            repo.repo.join(".rabi.toml"),
            format!(
                "[hooks]\npre-remove = [{}]\n",
                serde_json::to_string(command).unwrap()
            ),
        )
        .unwrap();
        let mut args = if gc {
            vec!["gc", "--merged", "--apply", "--json"]
        } else {
            vec!["remove", "topic", "--json"]
        };
        if keep_branch {
            args.push("--keep-branch");
        }
        let out = repo.rabi(&args);
        assert!(!path.exists(), "hook must remove or move the directory");
        assert!(branch_listed(&repo, "topic"), "{}", text(&out.stdout));
        let tip = repo
            .command("git", &["rev-parse", "refs/heads/topic"])
            .output()
            .unwrap();
        assert_eq!(text(&tip.stdout), head);
        assert!(!out.status.success(), "{}", text(&out.stdout));
        assert!(worktree_registered(&repo, "topic"));
        assert_eq!(
            serde_json::to_value(rabi::metadata::load_strict(&repo.repo).unwrap()).unwrap(),
            serde_json::to_value(&records).unwrap(),
        );
        if move_directory {
            assert!(PathBuf::from(format!("{}.moved", path.display()))
                .join("topic.txt")
                .is_file());
        }
    }
}

#[test]
fn retirement_remove_rejects_missing_path_after_resolved_name() {
    assert_retirement_missing_remove_path(true);
}

#[test]
fn retirement_remove_rejects_missing_path_before_resolved_name() {
    assert_retirement_missing_remove_path(false);
}

fn assert_retirement_missing_remove_path(name_first: bool) {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "topic");
    fs::write(path.join("precious"), "keep this\n").unwrap();
    let marker = repo.root.join("hook-ran");
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\npre-remove = ['touch {}']\n", marker.display()),
    )
    .unwrap();
    let records = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    let missing_path = repo.root.join("missing");
    for dry_run in [true, false] {
        for json in [false, true] {
            let selectors = if name_first {
                ["topic", missing_path.to_str().unwrap()]
            } else {
                [missing_path.to_str().unwrap(), "topic"]
            };
            let mut args = vec![
                "remove",
                selectors[0],
                selectors[1],
                selectors[1],
                "--discard-changes",
            ];
            if dry_run {
                args.push("--dry-run");
            }
            if json {
                args.push("--json");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(3),
                "{args:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            assert_eq!(
                fs::read_to_string(path.join("precious")).unwrap(),
                "keep this\n"
            );
            assert!(worktree_registered(&repo, "topic"));
            assert!(branch_listed(&repo, "topic"));
            assert!(!marker.exists());
            assert_eq!(
                records::read(repo.repo.join(".git/rabi/workspaces")).unwrap(),
                records
            );
            if json {
                let doc = json_value(&out);
                assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 2);
                assert!(doc["data"]["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|item| !item["error"].is_null()));
            }
        }
    }
}

#[test]
fn retirement_review6_gc_keeps_fresh_branch_with_same_named_tag() {
    assert_retirement_creation_with_tag(false, false);
}

#[test]
fn retirement_review6_gc_keeps_fresh_branch_with_same_named_tag_using_reflog() {
    assert_retirement_creation_with_tag(true, false);
}

#[test]
fn retirement_review6_gc_keeps_synced_branch_with_same_named_tag_using_reflog() {
    assert_retirement_creation_with_tag(true, true);
}

fn assert_retirement_creation_with_tag(clear_creation: bool, synced: bool) {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    repo.git(&["checkout", "-b", "tag-source"]);
    repo.git(&["commit", "--allow-empty", "-m", "outside base"]);
    repo.git(&["tag", "topic", "refs/heads/tag-source"]);
    repo.git(&["checkout", "main"]);
    if synced {
        repo.git(&["commit", "--allow-empty", "-m", "base advance"]);
        let out = repo
            .command("git", &["merge", "--ff-only", "refs/heads/main"])
            .current_dir(managed(&repo, "topic"))
            .env(
                "GIT_REFLOG_ACTION",
                "rabi sync: fast-forward to refs/heads/main",
            )
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
    }
    if clear_creation {
        clear_created_from(&repo, "topic");
    }
    let records = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    for apply in [false, true] {
        let mut args = vec!["gc", "--merged", "--json"];
        if apply {
            args.push("--apply");
        }
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        assert_eq!(
            json_value(&out)["data"]["items"],
            serde_json::json!([]),
            "{}",
            text(&out.stdout)
        );
        assert!(managed(&repo, "topic").is_dir());
        assert!(worktree_registered(&repo, "topic"));
        assert!(branch_listed(&repo, "topic"));
        assert_eq!(
            records::read(repo.repo.join(".git/rabi/workspaces")).unwrap(),
            records
        );
    }
}

#[test]
fn retirement_review6_existing_branch_creation_uses_full_reflog_ref() {
    let repo = Repo::new();
    let creation = retirement_head_at(&repo, &repo.repo);
    repo.git(&["branch", "topic", "refs/heads/main"]);
    repo.git(&["tag", "topic", "refs/heads/main"]);
    let out = repo.rabi(&["create", "--branch", "topic", "--no-setup", "--json"]);
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(
        json_value(&out)["data"]["workspace"]["createdFrom"],
        creation
    );
}

#[test]
fn retirement_review6_gc_default_uses_full_remote_ref_with_same_named_tag() {
    for ambiguous_refs in ["false", "true"] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "topic", "--no-setup"])
            .status
            .success());
        land_branch(&repo, "topic");
        publish_branch(&repo, "topic");
        repo.git(&[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ]);
        repo.git(&["checkout", "-b", "tag-source"]);
        repo.git(&["commit", "--allow-empty", "-m", "outside default"]);
        repo.git(&["tag", "origin/main", "refs/heads/tag-source"]);
        repo.git(&["checkout", "refs/heads/main"]);
        repo.git(&["config", "core.warnAmbiguousRefs", ambiguous_refs]);
        let shown = repo.rabi(&["repo", "show", "--json"]);
        assert!(shown.status.success(), "{}", text(&shown.stderr));
        assert_eq!(
            json_value(&shown)["data"]["repo"]["defaultBranch"],
            "origin/main"
        );
        let out = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_eq!(
            json_value(&out)["data"]["items"][0]["action"],
            "removed",
            "{}",
            text(&out.stdout)
        );
        assert!(!managed(&repo, "topic").exists());
        assert!(!branch_listed(&repo, "topic"));
        assert!(branch_listed(&repo, "main"));
    }
}

#[test]
fn retirement_review7_keeps_reapplied_branch_without_distinct_evidence() {
    assert_retirement_reapplication(false, false);
}

#[test]
fn retirement_review7_keeps_reapplied_detached_without_distinct_evidence() {
    assert_retirement_reapplication(true, false);
}

#[test]
fn retirement_review7_accepts_reapplication_with_distinct_ordered_evidence() {
    assert_retirement_reapplication(false, true);
    assert_retirement_reapplication(true, true);
}

fn assert_retirement_reapplication(detached: bool, preserved: bool) {
    let repo = Repo::new();
    let path = retirement_replay_workspace(&repo, detached);
    for (directory, reapply, message) in [(&path, true, "topic"), (&repo.repo, preserved, "base")] {
        retirement_replay_file(&repo, directory, "a", Some("a\n"), message);
        retirement_replay_file(&repo, directory, "a", None, message);
        if reapply {
            retirement_replay_file(&repo, directory, "a", Some("a\n"), message);
        }
        retirement_replay_file(&repo, directory, "b", Some("b\n"), message);
    }
    // All base commits have different SHAs; exact ancestry cannot prove this.
    let head = retirement_head_at(&repo, &path);
    let ancestor = repo
        .command(
            "git",
            &["merge-base", "--is-ancestor", &head, "refs/heads/main"],
        )
        .output()
        .unwrap();
    assert_eq!(ancestor.status.code(), Some(1));
    if preserved {
        // Keep the existing contract: ordered rebase evidence survives later
        // base edits, even when the complete squash and base-tip checks fail.
        retirement_replay_file(&repo, &repo.repo, "a", Some("later\n"), "later edit");
        let plan = repo.rabi(&["gc", "--merged", "--json"]);
        assert!(plan.status.success(), "{}", text(&plan.stdout));
        assert_eq!(
            json_value(&plan)["data"]["items"][0]["action"],
            "wouldRemove"
        );
        let removed = repo.rabi(&["remove", "replay", "--json"]);
        assert!(removed.status.success(), "{}", text(&removed.stdout));
        assert!(!path.exists());
        assert!(!worktree_registered(&repo, "replay"));
        assert!(!branch_listed(&repo, "replay"));
    } else {
        assert_retirement_replay_is_unpreserved(&repo, detached);
    }
}

#[test]
fn retirement_review7_keeps_commits_landed_in_different_order() {
    for detached in [false, true] {
        let repo = Repo::new();
        let path = retirement_replay_workspace(&repo, detached);
        retirement_replay_file(&repo, &path, "a", Some("a\n"), "topic a");
        retirement_replay_file(&repo, &path, "b", Some("b\n"), "topic b");
        retirement_replay_file(&repo, &repo.repo, "b", Some("b\n"), "base b");
        retirement_replay_file(&repo, &repo.repo, "a", Some("a\n"), "base a");
        retirement_replay_file(&repo, &repo.repo, "a", None, "base removes a");
        assert_retirement_replay_is_unpreserved(&repo, detached);
    }
}

#[test]
fn retirement_review7_keeps_evidence_from_discarded_side_history() {
    for detached in [false, true] {
        let repo = Repo::new();
        let path = retirement_replay_workspace(&repo, detached);
        let ancestor = retirement_head_at(&repo, &repo.repo);
        retirement_replay_file(&repo, &path, "a", Some("a\n"), "topic a");
        retirement_replay_file(&repo, &path, "b", Some("b\n"), "topic b");
        retirement_replay_file(&repo, &repo.repo, "a", Some("a\n"), "base a");
        repo.git(&["checkout", "-b", "side", &ancestor]);
        retirement_replay_file(&repo, &repo.repo, "b", Some("b\n"), "side b");
        repo.git(&["checkout", "main"]);
        repo.git(&[
            "merge",
            "--no-ff",
            "-s",
            "ours",
            "side",
            "-m",
            "discard side tree",
        ]);
        assert!(!repo.repo.join("b").exists());
        assert_retirement_replay_is_unpreserved(&repo, detached);
    }
}

fn retirement_replay_workspace(repo: &Repo, detached: bool) -> PathBuf {
    let mut args = vec!["create", "replay", "--no-setup"];
    if detached {
        args.push("--detach");
    }
    let created = repo.rabi(&args);
    assert!(created.status.success(), "{}", text(&created.stderr));
    managed(repo, "replay")
}

fn retirement_replay_file(
    repo: &Repo,
    directory: &Path,
    file: &str,
    body: Option<&str>,
    message: &str,
) {
    if let Some(body) = body {
        fs::write(directory.join(file), body).unwrap();
        git_at(repo, directory, &["add", "--", file]);
    } else {
        git_at(repo, directory, &["rm", "--", file]);
    }
    git_at(repo, directory, &["commit", "-m", message]);
}

fn assert_retirement_replay_is_unpreserved(repo: &Repo, detached: bool) {
    let head = retirement_head_at(repo, &managed(repo, "replay"));
    let records = records::read(repo.repo.join(".git/rabi/workspaces")).unwrap();
    for apply in [false, true] {
        let mut args = vec!["gc", "--merged", "--json"];
        if apply {
            args.push("--apply");
        }
        let out = repo.rabi(&args);
        assert!(out.status.success(), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["data"]["items"], serde_json::json!([]));
        assert!(worktree_registered(repo, "replay"));
        assert_eq!(retirement_head_at(repo, &managed(repo, "replay")), head);
        assert_eq!(
            records::read(repo.repo.join(".git/rabi/workspaces")).unwrap(),
            records
        );
    }
    assert_retirement_review3_keeps_workspace(repo, "replay", detached);
    if !detached {
        let tip = repo
            .command("git", &["rev-parse", "refs/heads/replay"])
            .output()
            .unwrap();
        assert!(tip.status.success());
        assert_eq!(text(&tip.stdout), head);
    }
}

#[test]
fn retirement_review5_remove_batches_long_base_history() {
    assert_retirement_batches_long_history(false);
}

#[test]
fn retirement_review5_gc_reuses_integration_observation() {
    assert_retirement_batches_long_history(true);
}

fn assert_retirement_batches_long_history(gc: bool) {
    let repo = Repo::new();
    for name in ["local", "other", "rebased"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        commit_file(&repo, name, &format!("{name}.txt"), "first\n");
    }
    let first = retirement_head_at(&repo, &managed(&repo, "rebased"));
    commit_file(&repo, "rebased", "second.txt", "second\n");
    fs::write(repo.repo.join("base.txt"), "divergence\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "divergence"]);
    repo.git(&["cherry-pick", &first]);
    repo.git(&["cherry-pick", "refs/heads/rebased"]);
    // Require the per-commit fallback, including evidence before later edits.
    fs::write(repo.repo.join("rebased.txt"), "later base edit\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "later edit"]);
    let trace = repo.root.join("history-trace.json");
    let mut counts = Vec::new();
    let mut tree_counts = Vec::new();
    let mut advanced = 0;
    for extra_commits in [1, 50] {
        for index in advanced..extra_commits {
            // Include both empty and nonempty commits in the batched output.
            if index % 2 == 0 {
                fs::write(repo.repo.join("base.txt"), format!("base {index}\n")).unwrap();
                repo.git(&["add", "."]);
            }
            repo.git(&["commit", "--allow-empty", "-m", "advance base"]);
        }
        advanced = extra_commits;
        fs::write(&trace, "").unwrap();
        let args = if gc {
            vec!["gc", "--merged", "--json"]
        } else {
            vec!["remove", "local", "other", "rebased", "--dry-run", "--json"]
        };
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .env("GIT_TRACE2_EVENT", &trace)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", text(&out.stdout));
        let doc = json_value(&out);
        let items = doc["data"]["items"].as_array().unwrap();
        if gc {
            assert_eq!(items.len(), 1, "{doc}");
            assert_eq!(items[0]["workspace"]["name"], "rebased");
            assert_eq!(items[0]["action"], "wouldRemove");
        } else {
            assert_eq!(items.len(), 3, "{doc}");
            for item in items {
                assert_eq!(item["removed"], false);
                if item["workspace"]["name"] == "rebased" {
                    assert!(item["branchKeptReason"].is_null(), "{item}");
                } else {
                    assert_eq!(item["branchKeptReason"], "unpreserved");
                }
            }
        }
        let starts: Vec<Vec<String>> = fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|event| event["event"] == "start")
            .map(|event| serde_json::from_value(event["argv"].clone()).unwrap())
            .collect();
        let tree_processes = starts
            .iter()
            .filter(|args| {
                args.iter().any(|arg| arg == "diff-tree")
                    || (args.iter().any(|arg| arg == "log")
                        && args.iter().any(|arg| arg == "--raw"))
            })
            .count();
        // Three workspaces, each with one aggregate diff and two history reads.
        // GC must reuse its integrated result when checking preservation.
        tree_counts.push(tree_processes);
        counts.push(starts.len());
        for name in ["local", "other", "rebased"] {
            assert!(managed(&repo, name).is_dir());
            assert!(branch_listed(&repo, name));
        }
    }
    assert_eq!(tree_counts, [9, 9], "1 and 50 extra base commits");
    assert_eq!(counts[0], counts[1], "all Git starts must stay constant");
}

#[test]
fn retirement_tree_entries_preserve_rebased_work_after_later_base_edits() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "rebased", "--no-setup"])
        .status
        .success());
    commit_file(&repo, "rebased", "patch.txt", "first\n");
    let patch = text(
        &repo
            .command("git", &["rev-parse", "refs/heads/rebased"])
            .output()
            .unwrap()
            .stdout,
    );
    commit_file(&repo, "rebased", "second.txt", "second commit\n");
    fs::write(repo.repo.join("other.txt"), "base divergence\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "base divergence"]);
    repo.git(&["cherry-pick", &patch]);
    repo.git(&["cherry-pick", "refs/heads/rebased"]);
    fs::write(repo.repo.join("patch.txt"), "later base edit\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "later edit"]);
    let out = repo.rabi(&["gc", "--merged", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        json_value(&out)["data"]["items"][0]["action"],
        "wouldRemove"
    );
    let out = repo.rabi(&["remove", "rebased", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_value(&out)["data"]["items"][0]["branchDeleted"], true);
}

#[test]
fn retirement_dry_run_preserves_git_and_record_bytes() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "squash", "--no-setup"])
        .status
        .success());
    commit_in_worktree(&repo, "squash");
    squash_onto_main(&repo, "squash");
    let marker = repo.root.join("dry-run-hook");
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\npre-remove = [\"touch {}\"]\n", marker.display()),
    )
    .unwrap();
    fn snapshot(
        path: &Path,
    ) -> std::collections::BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
        fn visit(
            root: &Path,
            path: &Path,
            result: &mut std::collections::BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)>,
        ) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let file = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    visit(root, &file, result);
                } else {
                    result.insert(
                        file.strip_prefix(root).unwrap().to_path_buf(),
                        (
                            fs::read(&file).unwrap(),
                            fs::metadata(&file).unwrap().modified().unwrap(),
                        ),
                    );
                }
            }
        }
        let mut result = std::collections::BTreeMap::new();
        visit(path, path, &mut result);
        result
    }
    let before = snapshot(&repo.repo.join(".git"));
    for args in [
        vec!["remove", "squash", "--dry-run", "--json"],
        vec!["gc", "--merged", "--json"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["data"]["plan"], true);
        let after = snapshot(&repo.repo.join(".git"));
        let differences: Vec<_> = before
            .keys()
            .chain(after.keys())
            .filter(|key| before.get(*key) != after.get(*key))
            .collect();
        assert!(
            differences.is_empty(),
            "{args:?} changed Git state: {differences:?}"
        );
        assert!(!marker.exists());
        assert!(worktree_dir(&repo, "squash").exists());
    }
}

#[test]
fn retirement_gc_age_filters_evidence_and_excludes_unknown_creation_time() {
    let repo = Repo::new();
    for name in ["old", "recent", "unknown", "age-only"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        if name != "age-only" {
            land_branch(&repo, name);
        }
    }
    let dir = repo.repo.join(".git/rabi/workspaces");
    for entry in fs::read_dir(&dir).unwrap() {
        let file = entry.unwrap().path();
        let mut record: serde_json::Value =
            serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        match record["name"].as_str().unwrap() {
            "unknown" => record["createdAt"] = serde_json::Value::Null,
            "old" | "age-only" => record["createdAt"] = serde_json::json!("2000-01-01T00:00:00Z"),
            _ => {}
        }
        fs::write(file, serde_json::to_vec(&record).unwrap()).unwrap();
    }
    for flags in [
        vec!["--older-than", "14d"],
        vec!["--merged", "--older-than", "14d"],
    ] {
        let mut args = vec!["gc", "--json"];
        args.extend(flags);
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(0));
        let doc = json_value(&out);
        let items = doc["data"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 1, "{doc}");
        assert_eq!(items[0]["workspace"]["name"], "old");
        assert_eq!(items[0]["reasons"], serde_json::json!(["merged"]));
    }
}

#[test]
fn retirement_remove_partial_result_retries_only_unfinished_targets() {
    let repo = Repo::new();
    for name in ["one", "two", "three"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
    }
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"test \\\"$RABI_WORKSPACE\\\" != two\"]\n",
    )
    .unwrap();
    let args = ["remove", "one", "two", "three", "--json"];
    let out = repo.rabi(&args);
    assert_eq!(out.status.code(), Some(6), "{}", text(&out.stdout));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 3);
    assert_eq!(doc["data"]["items"][0]["removed"], true);
    assert_eq!(doc["data"]["items"][1]["error"]["code"], "failed");
    assert_eq!(doc["data"]["items"][2]["error"]["code"], "not_attempted");
    assert!(managed(&repo, "three").exists());
    let hint = doc["error"]["details"]["retryCommand"]
        .as_str()
        .expect("retry command");
    assert!(!hint.contains("'one'"));
    fs::remove_file(repo.repo.join(".rabi.toml")).unwrap();
    let out = repo.rabi(&args);
    assert_eq!(out.status.code(), Some(3), "{}", text(&out.stdout));
    assert!(managed(&repo, "two").exists());
    let out = run_setup_retry_hint(&repo, &repo.home, hint);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let doc = json_value(&out);
    assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 2);
    assert_eq!(doc["data"]["items"][0]["removed"], true);
    assert_eq!(doc["data"]["items"][1]["removed"], true);
}

#[test]
fn retirement_gc_closed_pr_and_missing_gh_report_separate_skip_hints() {
    let repo = Repo::new();
    for name in ["dirty-pr", "unpreserved-pr"] {
        assert!(repo
            .create_recorded(&[
                "create",
                name,
                "--pr",
                "https://github.com/acme/demo/pull/7"
            ])
            .status
            .success());
        if name == "dirty-pr" {
            land_branch(&repo, name);
            fs::write(managed(&repo, "dirty-pr/dirty"), "changes").unwrap();
        } else {
            commit_in_worktree(&repo, name);
        }
    }
    let fake = gh_path(
        &repo,
        "#!/bin/sh\nprintf '%s\\n' '{\"state\":\"CLOSED\"}'\n",
    );
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["gc", "--closed-pr", "--apply", "--json"],
        )
        .env("PATH", &fake)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    let doc = json_value(&out);
    let items = doc["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    for item in items {
        assert_eq!(item["action"], "skipped");
        assert_eq!(item["reasons"], serde_json::json!(["pullRequest"]));
        let hint = item["detail"].as_str().unwrap();
        if item["workspace"]["name"] == "dirty-pr" {
            assert!(hint.contains(" remove --discard-changes -- 'dirty-pr'"));
            assert!(!hint.contains("--delete-unpreserved-branch"));
        } else {
            assert!(hint.contains(" remove --delete-unpreserved-branch -- 'unpreserved-pr'"));
            assert!(!hint.contains("--discard-changes"));
        }
    }
    let bin = repo.root.join("without-gh");
    fs::create_dir(&bin).unwrap();
    let git = repo
        .command("sh", &["-c", "command -v git"])
        .output()
        .unwrap();
    std::os::unix::fs::symlink(text(&git.stdout), bin.join("git")).unwrap();
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["gc", "--closed-pr", "--json"])
        .env("PATH", bin)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let doc = json_value(&out);
    assert!(doc["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["detail"].as_str().unwrap().contains("unavailable")));
}

#[test]
fn retirement_bugbot_gc_keeps_live_unregistered_records() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "unregistered", "--no-setup"])
        .status
        .success());
    let pr = "https://github.com/acme/demo/pull/7";
    assert!(repo
        .rabi(&["set", "unregistered", "--pr", pr])
        .status
        .success());
    let fake = gh_path(
        &repo,
        "#!/bin/sh\nprintf '%s\\n' '{\"state\":\"CLOSED\"}'\n",
    );
    let path = managed(&repo, "unregistered");
    fs::write(path.join("precious"), "keep this\n").unwrap();
    let admin = text(
        &repo
            .command("git", &["rev-parse", "--absolute-git-dir"])
            .current_dir(&path)
            .output()
            .unwrap()
            .stdout,
    );
    fs::remove_dir_all(admin).unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&record_dir).unwrap();
    let listed = repo.rabi(&["list", "--json"]);
    assert!(listed.status.success());
    assert!(!json_value(&listed)["data"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["name"] == "unregistered"));
    for apply in [false, true] {
        for criterion in [None, Some("--orphaned"), Some("--closed-pr")] {
            let mut args = vec!["gc", "--json"];
            if let Some(criterion) = criterion {
                args.push(criterion);
            }
            if apply {
                args.push("--apply");
            }
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("PATH", &fake)
                .output()
                .unwrap();
            assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
            assert_eq!(json_value(&out)["data"]["items"], serde_json::json!([]));
            assert_eq!(records::read(&record_dir).unwrap(), before);
            assert_eq!(
                fs::read_to_string(path.join("precious")).unwrap(),
                "keep this\n"
            );
            assert!(branch_listed(&repo, "unregistered"));
        }
        for selector in ["unregistered", "pr:7", pr] {
            let mut args = vec!["remove", selector, "--json"];
            if !apply {
                args.push("--dry-run");
            }
            let out = repo.rabi(&args);
            assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
            assert_eq!(json_value(&out)["data"]["items"][0]["alreadyGone"], true);
            assert_eq!(json_value(&out)["data"]["items"][0]["removed"], false);
            assert_eq!(records::read(&record_dir).unwrap(), before);
            assert_eq!(
                fs::read_to_string(path.join("precious")).unwrap(),
                "keep this\n"
            );
            assert!(branch_listed(&repo, "unregistered"));
        }
    }
    // The same record becomes orphaned only after its directory is truly gone.
    fs::remove_dir_all(&path).unwrap();
    let plan = repo.rabi(&["gc", "--orphaned", "--json"]);
    assert_eq!(plan.status.code(), Some(0));
    assert_eq!(
        json_value(&plan)["data"]["items"][0]["action"],
        "wouldRemove"
    );
    assert_eq!(records::read(&record_dir).unwrap(), before);
    let out = repo.rabi(&["gc", "--orphaned", "--apply", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_value(&out)["data"]["items"][0]["action"], "removed");
    assert!(branch_listed(&repo, "unregistered"));
    assert!(rabi::metadata::load_strict(&repo.repo).unwrap().is_empty());
}

#[test]
fn retirement_bugbot_remove_reports_all_preflight_refusals() {
    let repo = Repo::new();
    for name in ["clean", "dirty-one", "dirty-two"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
    }
    for name in ["dirty-one", "dirty-two"] {
        fs::write(managed(&repo, name).join("precious"), "keep this\n").unwrap();
    }
    assert!(repo
        .rabi(&["create", "detached", "--detach", "--no-setup"])
        .status
        .success());
    commit_file(&repo, "detached", "committed", "unpreserved\n");
    let head = retirement_head_at(&repo, &managed(&repo, "detached"));
    let marker = repo.root.join("hook-ran");
    fs::write(
        repo.repo.join(".rabi.toml"),
        format!("[hooks]\npre-remove = ['touch {}']\n", marker.display()),
    )
    .unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&record_dir).unwrap();
    for invalid_first in [false, true] {
        for dry_run in [false, true] {
            for json in [false, true] {
                let mut args = vec![
                    "remove",
                    "clean",
                    "dirty-one",
                    "dirty-two",
                    "detached",
                    ".",
                    "dirty-one",
                ];
                if invalid_first {
                    args.insert(1, "bad+selector");
                } else {
                    args.push("bad+selector");
                }
                if dry_run {
                    args.push("--dry-run");
                }
                if json {
                    args.push("--json");
                }
                let out = repo.rabi(&args);
                assert_eq!(
                    out.status.code(),
                    Some(if invalid_first { 2 } else { 4 }),
                    "{args:?}: {} {}",
                    text(&out.stdout),
                    text(&out.stderr)
                );
                if json {
                    let doc = json_value(&out);
                    assert_eq!(
                        doc["error"]["code"],
                        if invalid_first { "usage" } else { "dirty" }
                    );
                    let items = doc["data"]["items"].as_array().unwrap();
                    assert_eq!(items.len(), 6, "{doc}");
                    for (name, code) in [
                        ("dirty-one", "dirty"),
                        ("dirty-two", "dirty"),
                        ("detached", "unpreserved"),
                        ("main", "primary"),
                        ("bad+selector", "usage"),
                    ] {
                        assert!(
                            items.iter().any(|item| item["workspace"]["name"] == name
                                && item["error"]["code"] == code),
                            "{doc}"
                        );
                    }
                    assert!(items.iter().all(|item| item["removed"] == false));
                } else {
                    let stderr = text(&out.stderr);
                    for reason in [
                        "dirty-one",
                        "dirty-two",
                        "unpreserved detached commits",
                        "primary checkout",
                        "Workspace name can only contain",
                    ] {
                        assert_eq!(stderr.matches(reason).count(), 1, "{args:?}: {stderr}");
                    }
                }
                assert!(!marker.exists());
                assert_eq!(records::read(&record_dir).unwrap(), before);
                assert_eq!(retirement_head_at(&repo, &managed(&repo, "detached")), head);
                for name in ["clean", "dirty-one", "dirty-two", "detached"] {
                    assert!(managed(&repo, name).is_dir());
                    assert!(worktree_registered(&repo, name));
                }
                for name in ["clean", "dirty-one", "dirty-two"] {
                    assert!(branch_listed(&repo, name));
                }
                for name in ["dirty-one", "dirty-two"] {
                    assert_eq!(
                        fs::read_to_string(managed(&repo, name).join("precious")).unwrap(),
                        "keep this\n"
                    );
                }
            }
        }
    }
}

#[test]
fn retirement_tree_comparison_keeps_distinct_non_utf8_changes() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "bytes", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "bytes");
    fs::write(path.join("bytes.txt"), [0x80, b'\n']).unwrap();
    git_at(&repo, &path, &["add", "."]);
    git_at(&repo, &path, &["commit", "-m", "first bytes"]);
    fs::write(repo.repo.join("bytes.txt"), [0x81, b'\n']).unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "different bytes"]);
    let out = repo.rabi(&["remove", "bytes", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(json_value(&out)["data"]["items"][0]["branchDeleted"], false);
    assert_eq!(
        json_value(&out)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert!(branch_listed(&repo, "bytes"));
}

#[test]
fn retirement_gc_skips_dirty_content_created_by_pre_remove_hook() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "hook-dirty", "--no-setup"])
        .status
        .success());
    land_branch(&repo, "hook-dirty");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"printf precious > dirty\"]\n",
    )
    .unwrap();
    let out = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out.stdout));
    let doc = json_value(&out);
    assert_eq!(doc["data"]["items"][0]["action"], "skipped");
    assert!(doc["data"]["items"][0]["detail"]
        .as_str()
        .unwrap()
        .contains(" remove --discard-changes -- 'hook-dirty'"));
    assert_eq!(
        fs::read_to_string(managed(&repo, "hook-dirty/dirty")).unwrap(),
        "precious"
    );
}

#[test]
fn retirement_primary_is_refused_with_every_destructive_permission() {
    let repo = Repo::new();
    for selector in [".", "main"] {
        let out = repo.rabi(&[
            "remove",
            selector,
            "--discard-changes",
            "--delete-unpreserved-branch",
            "--json",
        ]);
        assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["error"]["code"], "primary");
        assert!(repo.repo.join(".git").exists());
    }
}

#[test]
fn retirement_review_keeps_unique_merge_conflict_resolution() {
    let repo = Repo::new();
    fs::write(repo.repo.join("conflict.txt"), "initial\n").unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "common conflict file"]);
    repo.git(&["branch", "side"]);
    assert!(repo
        .rabi(&["create", "merge", "--no-setup"])
        .status
        .success());
    commit_file(&repo, "merge", "conflict.txt", "left\n");
    repo.git(&["checkout", "side"]);
    fs::write(repo.repo.join("conflict.txt"), "right\n").unwrap();
    repo.git(&["commit", "-am", "right"]);
    repo.git(&["checkout", "main"]);
    let path = managed(&repo, "merge");
    let conflict = repo
        .command("git", &["merge", "side"])
        .current_dir(&path)
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    fs::write(path.join("conflict.txt"), "unique resolution\n").unwrap();
    git_at(&repo, &path, &["add", "."]);
    git_at(&repo, &path, &["commit", "-m", "unique resolution"]);
    empty_commit(&repo, &repo.repo, "base divergence");
    repo.git(&["cherry-pick", "merge^1"]);
    repo.git(&["revert", "--no-edit", "HEAD"]);
    repo.git(&["cherry-pick", "side"]);
    let cherry = repo
        .command("git", &["cherry", "main", "merge"])
        .output()
        .unwrap();
    assert!(cherry.status.success());
    assert_eq!(text(&cherry.stdout).lines().count(), 2);
    assert!(text(&cherry.stdout)
        .lines()
        .all(|line| line.starts_with("- ")));
    let gc = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert!(gc.status.success(), "{}", text(&gc.stdout));
    assert!(path.exists(), "{}", text(&gc.stdout));
    let removed = repo.rabi(&["remove", "merge", "--json"]);
    assert!(removed.status.success());
    assert_eq!(
        json_value(&removed)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert!(branch_listed(&repo, "merge"));
}

#[test]
fn retirement_review3_keeps_other_block_in_squash() {
    assert_retirement_keeps_other_block(false, false);
}

#[test]
fn retirement_review3_keeps_other_block_in_rebased_history() {
    assert_retirement_keeps_other_block(true, false);
}

#[test]
fn retirement_review3_keeps_detached_other_block_in_squash() {
    assert_retirement_keeps_other_block(false, true);
}

#[test]
fn retirement_review3_keeps_detached_other_block_in_rebased_history() {
    assert_retirement_keeps_other_block(true, true);
}

fn assert_retirement_keeps_other_block(rebased: bool, detached: bool) {
    let repo = Repo::new();
    let block: String = (0..16).map(|line| format!("line {line}\n")).collect();
    let original = block.repeat(3);
    fs::write(repo.repo.join("blocks.txt"), &original).unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "repeated contexts"]);
    let mut args = vec!["create", "blocks", "--no-setup"];
    if detached {
        args.push("--detach");
    }
    assert!(repo.rabi(&args).status.success());
    let changed_block = block.replace("line 8\n", "changed\n");
    commit_file(
        &repo,
        "blocks",
        "blocks.txt",
        &format!("{block}{changed_block}{block}"),
    );
    fs::write(
        repo.repo.join("blocks.txt"),
        format!("{block}{block}{changed_block}"),
    )
    .unwrap();
    repo.git(&["commit", "-am", "different occurrence"]);
    if rebased {
        // Prevent complete-squash matching, so the per-commit fallback is tested too.
        commit_file(&repo, "blocks", "extra.txt", "shared extra change\n");
        let tip = retirement_head_at(&repo, &managed(&repo, "blocks"));
        repo.git(&["cherry-pick", tip.trim()]);
    }
    assert_retirement_review3_keeps_workspace(&repo, "blocks", detached);
}

#[test]
fn retirement_review3_keeps_gitlink_with_ignore_submodules_all() {
    assert_retirement_keeps_gitlink("diff.ignoreSubmodules", "all");
}

#[test]
fn retirement_review3_keeps_gitlink_with_submodule_log() {
    assert_retirement_keeps_gitlink("diff.submodule", "log");
}

#[test]
fn retirement_review3_accepts_exact_trees_despite_diff_settings() {
    let repo = Repo::new();
    let marker = repo.root.join("unexpected-diff-command");
    let command = format!("touch '{}'", marker.display());
    for (key, value) in [
        ("diff.external", command.as_str()),
        ("diff.check.textconv", command.as_str()),
        ("diff.relative", "true"),
        ("diff.renames", "copies"),
        ("diff.ignoreSubmodules", "all"),
        ("diff.submodule", "log"),
        ("color.ui", "always"),
        ("core.abbrev", "4"),
    ] {
        repo.git(&["config", key, value]);
    }
    fs::create_dir_all(repo.repo.join(".git/info")).unwrap();
    fs::write(repo.repo.join(".git/info/attributes"), "* diff=check\n").unwrap();
    let order = repo.root.join("diff-order");
    fs::write(&order, "z*\na*\n").unwrap();
    repo.git(&["config", "diff.orderFile", order.to_str().unwrap()]);
    assert!(repo
        .rabi(&["create", "exact", "--no-setup"])
        .status
        .success());
    let old = full_head(&repo);
    // An uninitialized gitlink still records a real commit ID in the tree.
    fs::create_dir(managed(&repo, "exact/module")).unwrap();
    git_at(
        &repo,
        &managed(&repo, "exact"),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "160000",
            &old,
            "module",
        ],
    );
    commit_file(&repo, "exact", "z-file.txt", "first\n");
    commit_file(&repo, "exact", "a-file.txt", "second\n");
    squash_onto_main(&repo, "exact");
    // A subsequent base merge must not hide the earlier squash evidence.
    repo.git(&["branch", "side", &old]);
    repo.git(&["checkout", "side"]);
    empty_commit(&repo, &repo.repo, "side divergence");
    repo.git(&["checkout", "main"]);
    repo.git(&["merge", "--no-ff", "side", "-m", "base merge"]);
    let plan = repo.rabi(&["gc", "--merged", "--json"]);
    assert!(plan.status.success(), "{}", text(&plan.stdout));
    assert_eq!(
        json_value(&plan)["data"]["items"][0]["action"],
        "wouldRemove"
    );
    let removed = repo.rabi(&["remove", "exact", "--json"]);
    assert!(removed.status.success(), "{}", text(&removed.stdout));
    assert_eq!(
        json_value(&removed)["data"]["items"][0]["branchDeleted"],
        true
    );
    assert!(!marker.exists(), "external diff or textconv executed");
}

fn assert_retirement_keeps_gitlink(key: &str, value: &str) {
    for rebased in [false, true] {
        for detached in [false, true] {
            let repo = Repo::new();
            let old = retirement_head_at(&repo, &repo.repo);
            empty_commit(&repo, &repo.repo, "new gitlink target");
            let new = retirement_head_at(&repo, &repo.repo);
            fs::create_dir(repo.repo.join("module")).unwrap();
            repo.git(&[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                old.trim(),
                "module",
            ]);
            repo.git(&["commit", "-m", "initial gitlink"]);
            let mut args = vec!["create", "gitlink", "--no-setup"];
            if detached {
                args.push("--detach");
            }
            assert!(repo.rabi(&args).status.success());
            let path = managed(&repo, "gitlink");
            git_at(
                &repo,
                &path,
                &[
                    "update-index",
                    "--cacheinfo",
                    "160000",
                    new.trim(),
                    "module",
                ],
            );
            commit_file(&repo, "gitlink", "ordinary.txt", "saved regular file\n");
            fs::write(repo.repo.join("ordinary.txt"), "saved regular file\n").unwrap();
            repo.git(&["add", "ordinary.txt"]);
            repo.git(&["commit", "-m", "regular file only"]);
            if rebased {
                commit_file(&repo, "gitlink", "extra.txt", "shared extra change\n");
                let tip = retirement_head_at(&repo, &path);
                repo.git(&["cherry-pick", tip.trim()]);
            }
            repo.git(&["config", key, value]);
            assert_retirement_review3_keeps_workspace(&repo, "gitlink", detached);
        }
    }
}

fn retirement_head_at(repo: &Repo, path: &Path) -> String {
    let output = repo
        .command("git", &["rev-parse", "HEAD"])
        .current_dir(path)
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    text(&output.stdout)
}

fn assert_retirement_review3_keeps_workspace(repo: &Repo, name: &str, detached: bool) {
    let path = managed(repo, name);
    let head = retirement_head_at(repo, &path);
    let dry_run = repo.rabi(&["remove", name, "--dry-run", "--json"]);
    if detached {
        assert_eq!(dry_run.status.code(), Some(4), "{}", text(&dry_run.stdout));
        assert_eq!(json_value(&dry_run)["error"]["code"], "unpreserved");
    } else {
        assert!(dry_run.status.success(), "{}", text(&dry_run.stdout));
        assert_eq!(
            json_value(&dry_run)["data"]["items"][0]["branchKeptReason"],
            "unpreserved"
        );
    }
    let gc = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert!(gc.status.success(), "{}", text(&gc.stdout));
    assert!(path.is_dir(), "{}", text(&gc.stdout));
    assert_eq!(retirement_head_at(repo, &path), head);
    let removed = repo.rabi(&["remove", name, "--json"]);
    if detached {
        assert_eq!(removed.status.code(), Some(4), "{}", text(&removed.stdout));
        assert_eq!(json_value(&removed)["error"]["code"], "unpreserved");
        assert_eq!(retirement_head_at(repo, &path), head);
    } else {
        assert!(removed.status.success(), "{}", text(&removed.stdout));
        assert_eq!(
            json_value(&removed)["data"]["items"][0]["branchDeleted"],
            false
        );
        assert_eq!(
            json_value(&removed)["data"]["items"][0]["branchKeptReason"],
            "unpreserved"
        );
        assert!(branch_listed(repo, name));
    }
}

#[test]
fn retirement_review_keeps_meaningful_whitespace_in_squash() {
    assert_retirement_preserves_whitespace(false);
}

#[test]
fn retirement_review_keeps_meaningful_whitespace_in_cherry() {
    assert_retirement_preserves_whitespace(true);
}

fn assert_retirement_preserves_whitespace(cherry_only: bool) {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "whitespace", "--no-setup"])
        .status
        .success());
    commit_file(
        &repo,
        "whitespace",
        "task.py",
        "if ready:\n    task_a()\n    task_b()\n",
    );
    if cherry_only {
        commit_file(&repo, "whitespace", "extra.txt", "same patch\n");
    }
    fs::write(
        repo.repo.join("task.py"),
        "if ready:\n    task_a()\ntask_b()\n",
    )
    .unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "different semantics"]);
    if cherry_only {
        repo.git(&["cherry-pick", "whitespace"]);
    }
    let gc = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert!(gc.status.success(), "{}", text(&gc.stdout));
    assert!(
        managed(&repo, "whitespace").exists(),
        "cherry_only={cherry_only}: {}",
        text(&gc.stdout)
    );
    let removed = repo.rabi(&["remove", "whitespace", "--json"]);
    assert!(removed.status.success());
    assert_eq!(
        json_value(&removed)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    assert!(branch_listed(&repo, "whitespace"));
}

#[test]
fn retirement_review_remove_keeps_missing_detached_registration_and_head() {
    assert_retirement_keeps_missing_detached("remove");
}

#[test]
fn retirement_review_gc_keeps_missing_detached_registration_and_head() {
    assert_retirement_keeps_missing_detached("gc");
}

fn assert_retirement_keeps_missing_detached(command: &str) {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "detached", "--detach", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "detached");
    fs::write(path.join("precious.txt"), "unpreserved\n").unwrap();
    git_at(&repo, &path, &["add", "."]);
    git_at(&repo, &path, &["commit", "-m", "unpreserved detached work"]);
    let admin = repo
        .command("git", &["rev-parse", "--absolute-git-dir"])
        .current_dir(&path)
        .output()
        .unwrap();
    let admin = PathBuf::from(text(&admin.stdout));
    let head = fs::read(admin.join("HEAD")).unwrap();
    let reflog = fs::read(admin.join("logs/HEAD")).unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let records_before = records::read(&record_dir).unwrap();
    fs::remove_dir_all(&path).unwrap();
    if command == "remove" {
        for dry_run in [true, false] {
            let mut args = vec!["remove", "detached", "--json"];
            if dry_run {
                args.push("--dry-run");
            }
            let out = repo.rabi(&args);
            assert_eq!(out.status.code(), Some(4), "{}", text(&out.stdout));
            assert_eq!(json_value(&out)["error"]["code"], "unpreserved");
        }
    } else {
        for apply in [false, true] {
            let mut args = vec!["gc", "--orphaned", "--json"];
            if apply {
                args.push("--apply");
            }
            let out = repo.rabi(&args);
            assert!(out.status.success(), "{}", text(&out.stdout));
            assert_eq!(json_value(&out)["data"]["items"][0]["action"], "skipped");
            assert!(json_value(&out)["data"]["items"][0]["detail"]
                .as_str()
                .unwrap()
                .contains("--delete-unpreserved-branch"));
        }
    }
    assert_eq!(fs::read(admin.join("HEAD")).unwrap(), head);
    assert_eq!(fs::read(admin.join("logs/HEAD")).unwrap(), reflog);
    assert_eq!(records::read(&record_dir).unwrap(), records_before);
    let permitted = repo.rabi(&[
        "remove",
        "detached",
        "--delete-unpreserved-branch",
        "--json",
    ]);
    assert!(permitted.status.success(), "{}", text(&permitted.stdout));
    assert!(!admin.exists());
}

fn retirement_review_gc_hint(out: &std::process::Output, json: bool) -> String {
    assert!(
        out.status.success(),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let detail = if json {
        json_value(out)["data"]["items"][0]["detail"]
            .as_str()
            .unwrap()
            .to_owned()
    } else {
        text(&out.stdout)
    };
    detail
        .split_once("; use ")
        .expect("targeted hint")
        .1
        .lines()
        .next()
        .unwrap()
        .trim()
        .to_owned()
}

#[test]
fn retirement_review_gc_hint_pins_selected_repository() {
    for json in [false, true] {
        let mut repo = Repo::new();
        let renamed = repo.root.join("primary's $repo `literal` with spaces");
        fs::rename(&repo.repo, &renamed).unwrap();
        repo.repo = renamed;
        let other = Repo::new();
        for root in [&repo.repo, &other.repo] {
            let created =
                repo.rabi(&["-R", root.to_str().unwrap(), "create", "same", "--no-setup"]);
            assert!(created.status.success(), "{}", text(&created.stderr));
        }
        land_branch(&repo, "same");
        fs::write(managed(&repo, "same/dirty"), "target\n").unwrap();
        let decoy = managed_directory(&other.repo, &repo.root.join("xdg")).join("same");
        fs::write(decoy.join("dirty"), "keep decoy\n").unwrap();
        let mut args = vec![
            "-R",
            repo.repo.to_str().unwrap(),
            "gc",
            "--merged",
            "--apply",
        ];
        if json {
            args.push("--json");
        }
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&other.repo)
            .output()
            .unwrap();
        let hint = retirement_review_gc_hint(&out, json);
        assert!(hint.contains("--discard-changes"), "{hint}");
        assert!(!hint.contains("--delete-unpreserved-branch"), "{hint}");
        let removed = run_setup_retry_hint(&repo, &other.repo, &hint);
        assert!(
            removed.status.success(),
            "{hint}: {} {}",
            text(&removed.stdout),
            text(&removed.stderr)
        );
        assert!(!managed(&repo, "same").exists(), "{hint}");
        assert_eq!(
            fs::read_to_string(decoy.join("dirty")).unwrap(),
            "keep decoy\n"
        );
    }
}

#[test]
fn retirement_review_gc_hint_resolves_git_only_name() {
    for json in [false, true] {
        let repo = Repo::new();
        let path = managed(&repo, "topic+fix");
        repo.git(&[
            "worktree",
            "add",
            "-b",
            "topic+fix",
            path.to_str().unwrap(),
            "HEAD",
        ]);
        let adopted = repo.rabi(&["adopt", path.to_str().unwrap()]);
        assert!(adopted.status.success(), "{}", text(&adopted.stderr));
        land_branch(&repo, "topic+fix");
        fs::write(path.join("dirty"), "target\n").unwrap();
        let mut args = vec!["gc", "--merged", "--apply"];
        if json {
            args.push("--json");
        }
        let hint = retirement_review_gc_hint(&repo.rabi(&args), json);
        let removed = run_setup_retry_hint(&repo, &repo.repo, &hint);
        assert!(
            removed.status.success(),
            "{hint}: {} {}",
            text(&removed.stdout),
            text(&removed.stderr)
        );
        assert!(!path.exists());
    }
}

#[test]
fn retirement_review_already_gone_is_visible_in_text_only() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "missing", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "missing");
    let admin = repo
        .command("git", &["rev-parse", "--absolute-git-dir"])
        .current_dir(&path)
        .output()
        .unwrap();
    fs::remove_dir_all(text(&admin.stdout)).unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&record_dir).unwrap();
    for args in [
        vec!["remove", "missing"],
        vec!["remove", "missing", "--dry-run"],
    ] {
        let out = repo.rabi(&args);
        assert_eq!(out.status.code(), Some(0));
        assert_eq!(text(&out.stdout), "Workspace 'missing' is already gone");
        assert!(out.stderr.is_empty());
    }
    let json = repo.rabi(&["remove", "missing", "--json"]);
    assert_eq!(json.status.code(), Some(0));
    let item = &json_value(&json)["data"]["items"][0];
    assert_eq!(item["alreadyGone"], true);
    assert_eq!(item["removed"], false);
    assert!(item["error"].is_null());
    for field in ["branch", "dirty", "detached", "upstream"] {
        assert!(item["workspace"][field].is_null(), "{field}: {item}");
    }
    assert!(path.is_dir());
    assert_eq!(records::read(&record_dir).unwrap(), before);
}

#[test]
fn retirement_review2_remove_dry_run_suppresses_git_hooks() {
    assert_retirement_plan_suppresses_fsmonitor(false);
}

#[test]
fn retirement_review2_gc_plan_suppresses_git_hooks() {
    assert_retirement_plan_suppresses_fsmonitor(true);
}

fn assert_retirement_plan_suppresses_fsmonitor(gc: bool) {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "landed", "--no-setup"])
        .status
        .success());
    land_branch(&repo, "landed");
    assert!(repo
        .rabi(&["create", "dirty", "--no-setup"])
        .status
        .success());
    land_branch(&repo, "dirty");
    fs::write(managed(&repo, "dirty/precious"), "keep\n").unwrap();
    let marker = repo.root.join("fsmonitor-writes");
    let hook = repo.root.join("fsmonitor-hook");
    fs::write(
        &hook,
        "#!/bin/sh\nprintf invoked >> \"$RABI_TEST_FSMONITOR_MARKER\"\nprintf 'token\\0'\n",
    )
    .unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["config", "core.fsmonitor", hook.to_str().unwrap()]);
    // Prove that this fixture actually runs and writes through the hook.
    let status = repo
        .command("git", &["--no-optional-locks", "status", "--porcelain"])
        .env("RABI_TEST_FSMONITOR_MARKER", &marker)
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(!fs::read(&marker).unwrap().is_empty());
    fs::write(&marker, "unchanged").unwrap();
    for json in [false, true] {
        let cases = if gc {
            vec![
                (vec!["gc"], 0),
                (vec!["gc", "--merged"], 0),
                (vec!["gc", "--orphaned"], 0),
            ]
        } else {
            vec![
                (vec!["remove", "missing", "--dry-run"], 3),
                (vec!["remove", "landed", "--dry-run"], 0),
                (vec!["remove", "dirty", "--dry-run"], 4),
                (vec!["remove", "dirty", "--discard-changes", "--dry-run"], 0),
                (vec!["remove", ".", "--dry-run"], 4),
            ]
        };
        for (mut args, exit) in cases {
            if json {
                args.push("--json");
            }
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .env("RABI_TEST_FSMONITOR_MARKER", &marker)
                .output()
                .unwrap();
            assert_eq!(
                out.status.code(),
                Some(exit),
                "{args:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            assert_eq!(
                fs::read_to_string(&marker).unwrap(),
                "unchanged",
                "{args:?} ran fsmonitor"
            );
            assert!(managed(&repo, "landed").exists());
            assert_eq!(
                fs::read_to_string(managed(&repo, "dirty/precious")).unwrap(),
                "keep\n"
            );
        }
    }
}

#[test]
fn retirement_review2_gc_keeps_fresh_detached_workspace() {
    for unknown_creation in [false, true] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "review", "--detach", "--no-setup"])
            .status
            .success());
        if unknown_creation {
            clear_created_from(&repo, "review");
        }
        let record_dir = repo.repo.join(".git/rabi/workspaces");
        let before = records::read(&record_dir).unwrap();
        for args in [
            vec!["gc", "--merged", "--json"],
            vec!["gc", "--json"],
            vec!["gc", "--apply", "--json"],
        ] {
            let out = repo.rabi(&args);
            assert!(out.status.success(), "{}", text(&out.stdout));
            assert_eq!(
                json_value(&out)["data"]["items"],
                serde_json::json!([]),
                "unknown_creation={unknown_creation}: {}",
                text(&out.stdout)
            );
            assert!(managed(&repo, "review").exists());
            assert_eq!(records::read(&record_dir).unwrap(), before);
        }
    }
}

#[test]
fn retirement_review2_gc_keeps_landed_detached_with_unknown_creation() {
    for creation in ["missing", "invalid", "unrecorded"] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "review", "--detach", "--no-setup"])
            .status
            .success());
        commit_in_worktree(&repo, "review");
        let head = repo
            .command("git", &["rev-parse", "HEAD"])
            .current_dir(managed(&repo, "review"))
            .output()
            .unwrap();
        assert!(head.status.success());
        repo.git(&["merge", "--ff-only", &text(&head.stdout)]);
        if creation == "unrecorded" {
            assert!(repo.rabi(&["forget", "review"]).status.success());
        } else if creation == "missing" {
            clear_created_from(&repo, "review");
        } else {
            let store = repo.repo.join(".git/rabi/workspaces");
            let mut document: serde_json::Value =
                serde_json::from_slice(&records::read(&store).unwrap()).unwrap();
            document["worktrees"][0]["createdFrom"] =
                serde_json::json!("unresolvable-creation-commit");
            records::write(&store, serde_json::to_vec(&document).unwrap()).unwrap();
        }
        for args in [
            vec!["gc", "--merged", "--json"],
            vec!["gc", "--apply", "--json"],
        ] {
            let out = repo.rabi(&args);
            assert!(out.status.success(), "{}", text(&out.stdout));
            assert_eq!(
                json_value(&out)["data"]["items"],
                serde_json::json!([]),
                "{creation}: {}",
                text(&out.stdout)
            );
            assert!(managed(&repo, "review").exists());
            assert!(worktree_registered(&repo, "review"));
        }
    }
}

#[test]
fn retirement_review2_gc_removes_detached_only_after_known_work_lands() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "review", "--detach", "--no-setup"])
        .status
        .success());
    commit_in_worktree(&repo, "review");
    let plan = repo.rabi(&["gc", "--merged", "--json"]);
    assert!(plan.status.success());
    assert_eq!(json_value(&plan)["data"]["items"], serde_json::json!([]));
    let head = repo
        .command("git", &["rev-parse", "HEAD"])
        .current_dir(managed(&repo, "review"))
        .output()
        .unwrap();
    assert!(head.status.success());
    repo.git(&["merge", "--ff-only", &text(&head.stdout)]);
    let plan = repo.rabi(&["gc", "--merged", "--json"]);
    assert!(plan.status.success());
    let doc = json_value(&plan);
    assert_eq!(doc["data"]["items"][0]["action"], "wouldRemove");
    assert_eq!(
        doc["data"]["items"][0]["reasons"],
        serde_json::json!(["merged"])
    );
    let applied = repo.rabi(&["gc", "--merged", "--apply", "--json"]);
    assert!(applied.status.success(), "{}", text(&applied.stdout));
    assert_eq!(
        json_value(&applied)["data"]["items"][0]["action"],
        "removed"
    );
    assert!(!managed(&repo, "review").exists());
    assert!(!worktree_registered(&repo, "review"));
}

#[test]
fn retirement_review2_gc_hint_accepts_leading_hyphen_alias() {
    for unpreserved in [false, true] {
        for apply in [false, true] {
            for json in [false, true] {
                let repo = Repo::new();
                let path = managed(&repo, "topic");
                if unpreserved {
                    repo.git(&[
                        "worktree",
                        "add",
                        "--detach",
                        path.to_str().unwrap(),
                        "HEAD",
                    ]);
                } else {
                    repo.git(&[
                        "worktree",
                        "add",
                        "-b",
                        "topic",
                        path.to_str().unwrap(),
                        "HEAD",
                    ]);
                }
                let adopted = repo.rabi(&["adopt", path.to_str().unwrap()]);
                assert!(adopted.status.success(), "{}", text(&adopted.stderr));
                persist_retirement_hyphen_alias(&repo);
                assert!(repo.rabi(&["show", "--", "-alias"]).status.success());
                if unpreserved {
                    commit_in_worktree(&repo, "topic");
                    fs::remove_dir_all(&path).unwrap();
                } else {
                    land_branch(&repo, "topic");
                    fs::write(path.join("dirty"), "target\n").unwrap();
                }
                let mut args = vec![
                    "gc",
                    if unpreserved {
                        "--orphaned"
                    } else {
                        "--merged"
                    },
                ];
                if apply {
                    args.push("--apply");
                }
                if json {
                    args.push("--json");
                }
                let hint = retirement_review_gc_hint(&repo.rabi(&args), json);
                assert_eq!(
                    hint.contains("--delete-unpreserved-branch"),
                    unpreserved,
                    "{hint}"
                );
                assert_eq!(hint.contains("--discard-changes"), !unpreserved, "{hint}");
                let removed = run_setup_retry_hint(&repo, &repo.repo, &hint);
                assert!(
                    removed.status.success(),
                    "{hint}: {} {}",
                    text(&removed.stdout),
                    text(&removed.stderr)
                );
                assert!(!path.exists());
                assert!(!worktree_registered(&repo, "topic"));
            }
        }
    }
}

fn persist_retirement_hyphen_alias(repo: &Repo) {
    // The current CLI rejects option-like --name values, but recorded aliases
    // (including imported metadata) are valid selectors and must remain usable.
    let store = repo.repo.join(".git/rabi/workspaces");
    let mut document: serde_json::Value =
        serde_json::from_slice(&records::read(&store).unwrap()).unwrap();
    document["worktrees"][0]["name"] = serde_json::json!("-alias");
    records::write(&store, serde_json::to_vec(&document).unwrap()).unwrap();
}

#[test]
fn retirement_review2_sync_base_hints_accept_leading_hyphen_alias() {
    for missing_base in [false, true] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "topic", "--no-setup"])
            .status
            .success());
        if missing_base {
            repo.git(&["branch", "obsolete"]);
            assert!(repo
                .rabi(&["set", "topic", "--base", "obsolete"])
                .status
                .success());
            repo.git(&["branch", "-D", "obsolete"]);
        } else {
            assert!(repo
                .rabi(&["set", "topic", "--unset", "base"])
                .status
                .success());
        }
        persist_retirement_hyphen_alias(&repo);
        let other = Repo::new();
        assert!(other
            .rabi(&["create", "topic", "--no-setup"])
            .status
            .success());
        assert!(other
            .rabi(&["set", "topic", "--unset", "base"])
            .status
            .success());
        persist_retirement_hyphen_alias(&other);
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &[
                    "-R",
                    repo.repo.to_str().unwrap(),
                    "sync",
                    "--json",
                    "--",
                    "-alias",
                ],
            )
            .current_dir(&other.repo)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let doc = json_value(&out);
        let item = &doc["data"]["items"][0];
        assert_eq!(
            item["status"],
            if missing_base {
                "base-gone"
            } else {
                "base-unknown"
            }
        );
        let hint = item["detail"].as_str().unwrap().split_once(": ").unwrap().1;
        let hint = hint.replace("<ref>", "main").replace("REF", "main");
        let set = run_setup_retry_hint(&repo, &other.repo, &hint);
        assert!(
            set.status.success(),
            "{hint}: {} {}",
            text(&set.stdout),
            text(&set.stderr)
        );
        let show = repo.rabi(&["show", "--json", "--", "-alias"]);
        assert_eq!(
            json_value(&show)["data"]["workspace"]["base"],
            "refs/heads/main"
        );
        let decoy = other.rabi(&["show", "--json", "--", "-alias"]);
        assert!(json_value(&decoy)["data"]["workspace"]["base"].is_null());
    }
}

#[test]
fn retirement_review4_rejects_own_branch_as_base() {
    assert_retirement_rejects_self_evidence("base");
}

#[test]
fn retirement_review4_rejects_symbolic_base_to_own_branch() {
    assert_retirement_rejects_self_evidence("symbolic-base");
}

#[test]
fn retirement_review4_rejects_own_branch_as_upstream() {
    assert_retirement_rejects_self_evidence("upstream");
}

#[test]
fn retirement_review4_rejects_symbolic_upstream_to_own_branch() {
    assert_retirement_rejects_self_evidence("symbolic-upstream");
}

fn assert_retirement_rejects_self_evidence(evidence: &str) {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    commit_file(&repo, "topic", "precious", "only on topic\n");
    if evidence.contains("base") {
        let base = if evidence == "base" { "topic" } else { "alias" };
        if base == "alias" {
            repo.git(&["branch", "alias"]);
        }
        assert!(repo
            .rabi(&["set", "topic", "--base", base])
            .status
            .success());
    } else {
        repo.git(&["config", "branch.topic.remote", "."]);
        repo.git(&[
            "config",
            "branch.topic.merge",
            if evidence == "upstream" {
                "refs/heads/topic"
            } else {
                "refs/heads/alias"
            },
        ]);
    }
    if evidence.starts_with("symbolic") {
        // Retarget after recording too: stored names must be resolved at deletion time.
        repo.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/topic"]);
    }
    assert_retirement_review3_keeps_workspace(&repo, "topic", false);
}

#[test]
fn retirement_review4_rejects_detached_head_and_unrooted_commit_bases() {
    for base in ["HEAD", "oid", "worktree-head"] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "topic", "--detach", "--no-setup"])
            .status
            .success());
        commit_file(&repo, "topic", "precious", "only in detached HEAD\n");
        let path = managed(&repo, "topic");
        let head = retirement_head_at(&repo, &path);
        let admin = repo
            .command("git", &["rev-parse", "--absolute-git-dir"])
            .current_dir(&path)
            .output()
            .unwrap();
        assert!(admin.status.success());
        let id = Path::new(&text(&admin.stdout))
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        let reference = match base {
            "oid" => head.trim().to_owned(),
            "worktree-head" => format!("worktrees/{id}/HEAD"),
            _ => "HEAD".to_owned(),
        };
        // Exercise imported/raw metadata as well as the OID produced by normalizing
        // --base HEAD in a detached checkout. HEAD must not prove its own retention.
        let store = repo.repo.join(".git/rabi/workspaces");
        let mut document: serde_json::Value =
            serde_json::from_slice(&records::read(&store).unwrap()).unwrap();
        document["worktrees"][0]["base"] = serde_json::json!(reference);
        records::write(&store, serde_json::to_vec(&document).unwrap()).unwrap();
        if base == "HEAD" {
            // The observing checkout's HEAD can itself be detached at this tip.
            repo.git(&["checkout", "--detach", head.trim()]);
        }
        assert_retirement_review3_keeps_workspace(&repo, "topic", true);
    }
}

#[test]
fn retirement_review4_accepts_independent_refs_at_same_commit() {
    for evidence in [
        "branch",
        "tag",
        "custom-fetch",
        "symbolic-base",
        "upstream",
        "symbolic-upstream",
    ] {
        let repo = Repo::new();
        assert!(repo
            .rabi(&["create", "topic", "--no-setup"])
            .status
            .success());
        commit_file(&repo, "topic", "precious", "saved independently\n");
        repo.git(&["branch", "saved", "topic"]);
        repo.git(&["symbolic-ref", "refs/heads/alias", "refs/heads/saved"]);
        if evidence.contains("upstream") {
            repo.git(&["config", "branch.topic.remote", "."]);
            repo.git(&[
                "config",
                "branch.topic.merge",
                if evidence == "upstream" {
                    "refs/heads/saved"
                } else {
                    "refs/heads/alias"
                },
            ]);
        } else {
            let base = match evidence {
                "custom-fetch" => {
                    repo.git(&["update-ref", "refs/cache/saved", "topic"]);
                    "refs/cache/saved"
                }
                "tag" => {
                    repo.git(&["tag", "archive", "topic"]);
                    "refs/tags/archive"
                }
                "symbolic-base" => "alias",
                _ => "saved",
            };
            assert!(repo
                .rabi(&["set", "topic", "--base", base])
                .status
                .success());
        }
        let out = repo.rabi(&["remove", "topic", "--json"]);
        assert!(out.status.success(), "{evidence}: {}", text(&out.stdout));
        assert_eq!(
            json_value(&out)["data"]["items"][0]["branchDeleted"],
            true,
            "{evidence}"
        );
        assert!(branch_listed(&repo, "saved"));
    }
}

#[test]
fn retirement_review4_missing_git_only_workspace_hint_uses_link() {
    assert_retirement_missing_git_only_hint("linked");
}

#[test]
fn retirement_review4_missing_git_only_workspace_hint_uses_git() {
    assert_retirement_missing_git_only_hint("unrecorded");
    assert_retirement_missing_git_only_hint("ambiguous");
}

fn assert_retirement_missing_git_only_hint(kind: &str) {
    for json in [false, true] {
        for apply in [false, true] {
            let repo = Repo::new();
            let other = Repo::new();
            let path = managed(&repo, "topic+fix");
            repo.git(&[
                "worktree",
                "add",
                "--detach",
                path.to_str().unwrap(),
                "HEAD",
            ]);
            if kind != "unrecorded" {
                let out = repo.rabi(&["adopt", path.to_str().unwrap(), "--issue", "17"]);
                assert!(out.status.success(), "{}", text(&out.stderr));
                if kind == "ambiguous" {
                    let decoy = managed(&repo, "decoy");
                    repo.git(&[
                        "worktree",
                        "add",
                        "-b",
                        "decoy",
                        decoy.to_str().unwrap(),
                        "HEAD",
                    ]);
                    let adopted = repo.rabi(&["adopt", decoy.to_str().unwrap(), "--issue", "17"]);
                    assert!(adopted.status.success(), "{}", text(&adopted.stderr));
                }
            }
            commit_file(
                &repo,
                "topic+fix",
                "precious",
                "unpreserved detached work\n",
            );
            let admin = repo
                .command("git", &["rev-parse", "--absolute-git-dir"])
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(admin.status.success());
            let admin = PathBuf::from(text(&admin.stdout));
            let head = fs::read(admin.join("HEAD")).unwrap();
            let reflog = fs::read(admin.join("logs/HEAD")).unwrap();
            fs::remove_dir_all(&path).unwrap();
            let mut args = vec!["-R", repo.repo.to_str().unwrap(), "gc", "--orphaned"];
            if json {
                args.push("--json");
            }
            if apply {
                args.push("--apply");
            }
            let out = repo
                .command(env!("CARGO_BIN_EXE_rabi"), &args)
                .current_dir(&other.repo)
                .output()
                .unwrap();
            let hint = retirement_review_gc_hint(&out, json);
            assert_eq!(fs::read(admin.join("HEAD")).unwrap(), head);
            assert_eq!(fs::read(admin.join("logs/HEAD")).unwrap(), reflog);
            assert!(!hint.contains("cd "), "{hint}");
            if kind == "linked" {
                assert!(hint.contains("'issue:17'"), "{hint}");
                assert!(hint.contains("--delete-unpreserved-branch"), "{hint}");
            } else {
                assert!(hint.contains("worktree remove --force"), "{hint}");
            }
            let removed = run_setup_retry_hint(&repo, &other.repo, &hint);
            assert!(
                removed.status.success(),
                "{hint}: {} {}",
                text(&removed.stdout),
                text(&removed.stderr)
            );
            assert!(!admin.exists(), "{hint}");
            assert!(!worktree_registered(&repo, "topic+fix"));
            assert!(other.repo.join(".git").is_dir());
            if kind == "ambiguous" {
                assert!(managed(&repo, "decoy").is_dir());
            }
        }
    }
}

#[test]
fn retirement_review4_sync_missing_directory_hint_can_set_base() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    assert!(repo
        .rabi(&["set", "topic", "--unset", "base"])
        .status
        .success());
    persist_retirement_hyphen_alias(&repo);
    fs::remove_dir_all(managed(&repo, "topic")).unwrap();
    let other = Repo::new();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "-R",
                repo.repo.to_str().unwrap(),
                "sync",
                "--json",
                "--",
                "-alias",
            ],
        )
        .current_dir(&other.repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    let doc = json_value(&out);
    let hint = doc["data"]["items"][0]["detail"]
        .as_str()
        .unwrap()
        .split_once(": ")
        .unwrap()
        .1
        .replace("REF", "main");
    assert!(!hint.contains("cd "), "{hint}");
    let set = run_setup_retry_hint(&repo, &other.repo, &hint);
    assert!(
        set.status.success(),
        "{hint}: {} {}",
        text(&set.stdout),
        text(&set.stderr)
    );
    let show = repo.rabi(&["show", "--json", "--", "-alias"]);
    assert_eq!(
        json_value(&show)["data"]["workspace"]["base"],
        "refs/heads/main"
    );
}

#[test]
fn retirement_remove_unknown_selectors_preflight_all_targets_without_hooks() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "known", "--no-setup"])
        .status
        .success());
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = [\"touch hook-ran\"]\n",
    )
    .unwrap();
    let records_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&records_dir).unwrap();
    let missing_path = repo.root.join("missing path's $(touch injected)");
    let unregistered_path = managed(&repo, "unregistered-directory");
    fs::create_dir_all(&unregistered_path).unwrap();
    fs::write(unregistered_path.join("precious"), "keep").unwrap();
    let selectors = [
        "typo",
        "pr:999",
        "issue:999",
        "https://github.com/acme/demo/pull/999",
        "https://github.com/acme/demo/issues/999",
        missing_path.to_str().unwrap(),
        "./missing",
        "../missing",
        "unregistered-directory",
        unregistered_path.to_str().unwrap(),
    ];
    for dry_run in [false, true] {
        for json in [false, true] {
            for mixed in [false, true] {
                let mut args = vec!["remove"];
                if mixed {
                    args.push("known");
                }
                args.extend(selectors);
                if dry_run {
                    args.push("--dry-run");
                }
                if json {
                    args.push("--json");
                }
                let out = repo.rabi(&args);
                assert_eq!(
                    out.status.code(),
                    Some(3),
                    "{args:?}: {} {}",
                    text(&out.stdout),
                    text(&out.stderr)
                );
                if json {
                    let doc = assert_envelope(&out, "remove");
                    assert_eq!(doc["error"]["code"], "not_found");
                    for item in doc["data"]["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .skip(usize::from(mixed))
                    {
                        assert_eq!(item["alreadyGone"], false);
                        assert_eq!(item["error"]["code"], "not_found");
                    }
                } else {
                    for selector in selectors {
                        assert!(
                            text(&out.stderr).contains(selector),
                            "{}",
                            text(&out.stderr)
                        );
                    }
                }
                assert!(managed(&repo, "known").is_dir());
                assert!(!managed(&repo, "known/hook-ran").exists());
                assert!(branch_listed(&repo, "known"));
                assert_eq!(
                    fs::read_to_string(unregistered_path.join("precious")).unwrap(),
                    "keep"
                );
                assert_eq!(records::read(&records_dir).unwrap(), before);
            }
        }
    }
    // Each spelling also fails by itself, without relying on another missing name.
    for selector in selectors {
        let out = repo.rabi(&["remove", selector, "--json"]);
        assert_eq!(
            out.status.code(),
            Some(3),
            "{selector}: {}",
            text(&out.stdout)
        );
        assert_eq!(json_value(&out)["error"]["code"], "not_found");
    }
}

#[test]
fn retirement_remove_already_gone_requires_metadata_or_git_registration() {
    for recorded in [false, true] {
        for registered in [false, true] {
            let repo = Repo::new();
            assert!(repo
                .create_recorded(&[
                    "create",
                    "gone",
                    "--pr",
                    "https://github.com/acme/demo/pull/7",
                    "--issue",
                    "https://github.com/acme/demo/issues/8"
                ])
                .status
                .success());
            let path = managed(&repo, "gone");
            if !recorded {
                assert!(repo.rabi(&["forget", "gone"]).status.success());
            }
            fs::remove_dir_all(&path).unwrap();
            if !registered {
                repo.git(&["worktree", "prune", "--expire", "now"]);
            }
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\npre-remove = [\"exit 99\"]\n",
            )
            .unwrap();
            let mut selectors = vec!["gone", path.to_str().unwrap()];
            if recorded {
                selectors.extend([
                    "pr:7",
                    "issue:8",
                    "https://github.com/acme/demo/pull/7",
                    "https://github.com/acme/demo/issues/8",
                ]);
            }
            for dry_run in [true, false] {
                // Aliases in one command deduplicate to the one evidenced target.
                let mut args = vec!["remove", "--json"];
                if dry_run {
                    args.push("--dry-run");
                }
                args.extend(selectors.iter().copied());
                let out = repo.rabi(&args);
                assert_eq!(
                    out.status.code(),
                    Some(if recorded || registered { 0 } else { 3 }),
                    "recorded={recorded}, registered={registered}: {}",
                    text(&out.stdout)
                );
                let doc = json_value(&out);
                if recorded || registered {
                    let items = doc["data"]["items"].as_array().unwrap();
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0]["alreadyGone"], true);
                    assert!(items[0]["error"].is_null());
                } else {
                    assert_eq!(doc["error"]["code"], "not_found");
                }
                assert!(branch_listed(&repo, "gone"));
            }
        }
    }
}

#[test]
fn retirement_remove_retry_quotes_repository_and_preserves_permission_flags() {
    for json in [false, true] {
        let repo = Repo::new();
        // Exercise quoting in -R, including shell syntax and control characters.
        let selected = repo.root.join("repo's $(touch injected)\ncheckout");
        fs::rename(&repo.repo, &selected).unwrap();
        let repo = Repo {
            root: repo.root.clone(),
            repo: selected,
            home: repo.home.clone(),
        };
        for name in ["done", "-remaining", "last"] {
            if name.starts_with('-') {
                let path = managed(&repo, name);
                repo.git(&[
                    "worktree",
                    "add",
                    "-b",
                    "remaining-branch",
                    path.to_str().unwrap(),
                ]);
                assert!(repo
                    .rabi(&["adopt", path.to_str().unwrap()])
                    .status
                    .success());
            } else {
                let out = repo.rabi(&["create", "--no-setup", "--", name]);
                assert!(
                    out.status.success(),
                    "{} {}",
                    text(&out.stdout),
                    text(&out.stderr)
                );
            }
            fs::write(managed(&repo, &format!("{name}/dirty")), "dirty").unwrap();
            commit_file(&repo, name, "committed.txt", "unpreserved history");
        }
        // The second hook fails only after the first workspace is removed.
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\npre-remove = [\"test \\\"$RABI_WORKSPACE\\\" != -remaining || exit 1\"]\n",
        )
        .unwrap();
        let mut args = vec![
            "-R",
            repo.repo.to_str().unwrap(),
            "remove",
            "--discard-changes",
            "--delete-unpreserved-branch",
            "--keep-branch",
            "--no-input",
        ];
        if json {
            args.push("--json");
        }
        args.extend(["--", "done", "-remaining", "last"]);
        let out = repo.rabi(&args);
        assert_eq!(
            out.status.code(),
            Some(6),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let hint = setup_retry_hint(&out, json);
        if json {
            assert_eq!(json_value(&out)["error"]["details"]["retryCommand"], hint);
        }
        assert!(hint.contains("-R "));
        for flag in [
            "--discard-changes",
            "--delete-unpreserved-branch",
            "--keep-branch",
            "--no-input",
            " -- ",
        ] {
            assert!(hint.contains(flag), "{hint}");
        }
        assert!(!hint.contains("'done'"));
        fs::remove_file(repo.repo.join(".rabi.toml")).unwrap();
        let out = run_setup_retry_hint(&repo, &repo.home, &hint);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{hint}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(!managed(&repo, "-remaining").exists());
        assert!(!managed(&repo, "last").exists());
        assert!(!repo.home.join("injected").exists());
    }
}

#[test]
fn retirement_remove_retry_pins_output_over_environment_defaults() {
    for json in [false, true] {
        let repo = Repo::new();
        for name in ["done", "remaining"] {
            assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        }
        let remaining = managed(&repo, "remaining");
        repo.git(&["worktree", "lock", remaining.to_str().unwrap()]);
        let env_output = if json { "text" } else { "json" };
        let output = if json { "json" } else { "text" };
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["remove", "done", "remaining", "--output", output],
            )
            .env("RABI_OUTPUT", env_output)
            .env("RABI_NO_INPUT", "1")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(6),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(!managed(&repo, "done").exists());
        assert!(remaining.exists());
        let hint = setup_retry_hint(&out, json);
        repo.git(&["worktree", "unlock", remaining.to_str().unwrap()]);
        // Execute the displayed command unchanged under the original defaults.
        let script = format!("export RABI_OUTPUT={env_output} RABI_NO_INPUT=1\n{hint}");
        let retried = run_setup_retry_hint(&repo, &repo.home, &script);
        assert_eq!(
            retried.status.code(),
            Some(0),
            "{hint}: {} {}",
            text(&retried.stdout),
            text(&retried.stderr)
        );
        if json {
            let doc = assert_envelope(&retried, "remove");
            assert_eq!(doc["ok"], true);
            assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 1);
        } else {
            assert!(
                serde_json::from_slice::<serde_json::Value>(&retried.stdout).is_err(),
                "retry switched to JSON: {}",
                text(&retried.stdout)
            );
            assert!(text(&retried.stdout).contains("remaining"));
        }
        assert!(hint.contains(if json { " --json " } else { " --output text " }));
        assert!(hint.contains(" --no-input "));
        assert!(!remaining.exists());
        assert!(rabi::metadata::load_strict(&repo.repo).unwrap().is_empty());
    }
}

#[test]
fn retirement_remove_missing_wins_over_other_preflight_errors() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "dirty", "--no-setup"])
        .status
        .success());
    fs::write(managed(&repo, "dirty/changed"), "keep").unwrap();
    for selectors in [["dirty", "unknown"], ["unknown", "dirty"]] {
        for dry_run in [false, true] {
            let mut args = vec!["remove", "--json"];
            if dry_run {
                args.push("--dry-run");
            }
            args.extend(selectors);
            let out = repo.rabi(&args);
            assert_eq!(out.status.code(), Some(3), "{}", text(&out.stdout));
            assert_eq!(json_value(&out)["error"]["code"], "not_found");
            assert!(managed(&repo, "dirty/changed").is_file());
        }
    }
}

#[test]
fn retirement_remove_branch_delete_failure_keeps_branch_and_continues() {
    for json in [false, true] {
        for force in [false, true] {
            let repo = Repo::new();
            for name in ["branch-failure", "last"] {
                assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
            }
            let lock = repo.repo.join(".git/refs/heads/branch-failure.lock");
            fs::write(&lock, "locked").unwrap();
            let mut args = vec!["remove", "branch-failure", "last", "--no-hooks"];
            if json {
                args.push("--json");
            }
            if force {
                args.push("--delete-unpreserved-branch");
            }
            let out = repo.rabi(&args);
            assert_eq!(
                out.status.code(),
                Some(0),
                "{} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            assert!(branch_listed(&repo, "branch-failure"));
            assert!(!managed(&repo, "branch-failure").exists());
            assert!(!managed(&repo, "last").exists());
            assert!(rabi::metadata::load_strict(&repo.repo).unwrap().is_empty());
            if json {
                let doc = json_value(&out); // Parsing the entire stdout rejects multiple documents.
                assert_eq!(doc["ok"], true);
                assert!(doc["error"].is_null());
                assert_eq!(doc["warnings"][0]["code"], "branch_kept_delete_failed");
                let item = &doc["data"]["items"][0];
                assert_eq!(item["removed"], true);
                assert_eq!(item["branchDeleted"], false);
                assert_eq!(item["branchKeptReason"], "delete_failed");
                assert!(item["error"].is_null());
                assert!(item["detail"]
                    .as_str()
                    .unwrap()
                    .contains("branch-failure.lock"));
            } else {
                assert!(text(&out.stderr).contains("kept branch 'branch-failure'"));
                assert!(text(&out.stderr).contains("branch-failure.lock"));
                assert!(!text(&out.stderr).contains("Retry with"));
            }
        }
    }
}

#[test]
fn retirement_remove_branch_failure_and_unfinished_targets_use_one_retry() {
    for json in [false, true] {
        let repo = Repo::new();
        for name in ["branch-failure", "remaining", "last"] {
            assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        }
        fs::write(repo.repo.join(".rabi.toml"), "[hooks]\npre-remove = ['test \"$RABI_WORKSPACE\" != last || test -f \"$RABI_REPO_PATH/allow-remove\"']\n").unwrap();
        let lock = repo.repo.join(".git/refs/heads/branch-failure.lock");
        fs::write(&lock, "locked").unwrap();
        let remaining = managed(&repo, "remaining");
        repo.git(&["worktree", "lock", remaining.to_str().unwrap()]);
        let mut args = vec!["remove", "branch-failure", "remaining", "last"];
        if json {
            args.push("--json");
        }
        let out = repo.rabi(&args);
        assert_eq!(
            out.status.code(),
            Some(6),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(!managed(&repo, "branch-failure").exists());
        assert!(remaining.exists());
        if json {
            let doc = json_value(&out);
            assert_eq!(doc["error"]["code"], "partial");
            assert_eq!(doc["data"]["items"][0]["branchKeptReason"], "delete_failed");
            assert!(doc["data"]["items"][0]["error"].is_null());
            assert!(!doc["data"]["items"][1]["error"].is_null());
            assert_eq!(doc["data"]["items"][2]["error"]["code"], "not_attempted");
        }
        let hint = setup_retry_hint(&out, json);
        assert_eq!(hint.matches("rabi ").count(), 1, "{hint}");
        assert_eq!(hint.matches(" && ").count(), 1, "{hint}");
        assert!(hint.contains("-R ") && hint.contains(" remove "), "{hint}");
        assert!(!hint.contains("branch-failure"), "{hint}");
        // The branch lock stays in place: workspace retries must never touch it.
        repo.git(&["worktree", "unlock", remaining.to_str().unwrap()]);
        let retried = run_setup_retry_hint(&repo, &repo.home, &hint);
        assert_eq!(
            retried.status.code(),
            Some(6),
            "{} {}",
            text(&retried.stdout),
            text(&retried.stderr)
        );
        assert!(!remaining.exists());
        assert!(managed(&repo, "last").exists());
        if json {
            let doc = json_value(&retried);
            assert_eq!(doc["error"]["code"], "partial");
            assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 2);
            assert_eq!(doc["data"]["items"][0]["removed"], true);
        }
        let next = setup_retry_hint(&retried, json);
        assert_eq!(next.matches("rabi ").count(), 1, "{next}");
        assert!(
            !next.contains("branch-failure") && !next.contains("remaining"),
            "{next}"
        );
        fs::write(repo.repo.join("allow-remove"), "allow").unwrap();
        let finished = run_setup_retry_hint(&repo, &repo.home, &next);
        assert_eq!(
            finished.status.code(),
            Some(0),
            "{} {}",
            text(&finished.stdout),
            text(&finished.stderr)
        );
        if json {
            assert_eq!(json_value(&finished)["ok"], true);
        }
        assert!(!managed(&repo, "last").exists());
        assert!(branch_listed(&repo, "branch-failure"));
        assert!(rabi::metadata::load_strict(&repo.repo).unwrap().is_empty());
    }
}

#[test]
fn retirement_remove_review3_retry_runs_in_shell_with_deleted_cwd() {
    let repo = Repo::new();
    for name in ["source", "remaining"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
    }
    let source = managed(&repo, "source");
    let remaining = managed(&repo, "remaining");
    repo.git(&["worktree", "lock", remaining.to_str().unwrap()]);
    let script = r#"
rabi remove . remaining --no-hooks 2> "$3"
status=$?
test "$status" -eq 6 || exit 90
test ! -d "$4" || exit 91
git -C "$1" worktree unlock "$2" || exit 92
retry=
while IFS= read -r line; do
    case "$line" in
        *'Retry with `'* )
            retry=${line#*'Retry with `'}
            retry=${retry%'`.'}
            ;;
    esac
done < "$3"
test -n "$retry" || exit 93
eval "$retry"
"#;
    let binary_dir = Path::new(env!("CARGO_BIN_EXE_rabi")).parent().unwrap();
    let out = repo
        .command(
            "bash",
            &[
                "-c",
                script,
                "retry-test",
                repo.repo.to_str().unwrap(),
                remaining.to_str().unwrap(),
                repo.root.join("remove-error").to_str().unwrap(),
                source.to_str().unwrap(),
            ],
        )
        .current_dir(&source)
        .env("RABI_TRUST_ALL", "1")
        .env(
            "PATH",
            format!(
                "{}:{}",
                binary_dir.display(),
                std::env::var("PATH").unwrap()
            ),
        )
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(!remaining.exists());
}

#[test]
fn retirement_remove_retry_finishes_metadata_failure_after_checkout_is_gone() {
    for (json, recorded, orphaned) in [
        (false, true, false),
        (true, true, false),
        (false, false, false),
        (true, false, false),
        (false, true, true),
        (true, true, true),
        (false, false, true),
        (true, false, true),
    ] {
        assert_remove_metadata_failure_retry(json, recorded, orphaned, None, false);
    }
}

#[test]
fn retirement_remove_review_retry_retains_recorded_symlink_path_after_metadata_failure() {
    for json in [false, true] {
        for kind in ["workspace-root", "XDG_DATA_HOME"] {
            assert_remove_metadata_failure_retry(json, true, false, Some(kind), false);
        }
    }
}

#[test]
fn retirement_remove_retry_cleans_metadata_after_hook_source_is_removed() {
    for json in [false, true] {
        assert_remove_metadata_failure_retry(json, true, false, None, true);
    }
}

fn assert_remove_metadata_failure_retry(
    json: bool,
    recorded: bool,
    orphaned: bool,
    symlink_root: Option<&str>,
    hook_source: bool,
) {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    let workspace_root = if let Some(kind) = symlink_root {
        let real = repo.root.join("real-workspaces");
        let via = repo.root.join(if kind == "XDG_DATA_HOME" {
            "xdg"
        } else {
            "via-workspaces"
        });
        fs::create_dir(&real).unwrap();
        std::os::unix::fs::symlink(&real, &via).unwrap();
        if kind == "workspace-root" {
            assert!(repo
                .rabi(&["config", "set", "workspace-root", via.to_str().unwrap()])
                .status
                .success());
            via
        } else {
            managed_root(&repo)
        }
    } else {
        managed_root(&repo)
    };
    for name in ["metadata-failure", "last"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
    }
    if symlink_root.is_some() {
        let canonical = fs::canonicalize(workspace_root.join("metadata-failure")).unwrap();
        let stored = rabi::metadata::matching_path(&repo.repo, &canonical)
            .unwrap()
            .unwrap();
        assert_eq!(Path::new(&stored), workspace_root.join("metadata-failure"));
        assert_ne!(Path::new(&stored), canonical);
    }
    if !recorded {
        assert!(repo.rabi(&["forget", "metadata-failure"]).status.success());
    }
    if orphaned {
        fs::remove_dir_all(workspace_root.join("metadata-failure")).unwrap();
    }
    let source = workspace_root.join("metadata-failure");
    if hook_source {
        fs::write(
            source.join(".rabi.toml"),
            "[hooks]\npre-remove = ['printf x >> \"$RABI_REPO_PATH/source-hooks\"']\n",
        )
        .unwrap();
        fs::write(
            repo.repo.join(".rabi.toml"),
            "[hooks]\npre-remove = ['touch unexpected-primary-hook; exit 1']\n",
        )
        .unwrap();
    }
    let real_git = repo
        .command("sh", &["-c", "command -v git"])
        .output()
        .unwrap();
    let bin = repo.root.join("bin");
    fs::create_dir(&bin).unwrap();
    let wrapper = bin.join("git");
    // Real Git removal succeeds, then a filesystem obstruction makes the
    // metadata write fail. Restore it before executing the supplied hint.
    fs::write(
        &wrapper,
        r#"#!/bin/sh
case " $* " in
  *" worktree remove "*)
    "$REAL_GIT" "$@" || exit $?
    case " $* " in
      *" $METADATA_WORKSPACE "*) ;;
      *) exit 0 ;;
    esac
    mv "$METADATA_DIR" "$METADATA_DIR.saved" || exit $?
    printf 'not a directory' > "$METADATA_DIR"
    ;;
  *) exec "$REAL_GIT" "$@" ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let metadata_dir = repo.repo.join(".git/rabi/workspaces");
    let mut args = vec!["remove", "metadata-failure", "last", "--no-hooks"];
    if hook_source {
        args = vec!["remove", "last", "metadata-failure", "--discard-changes"];
    }
    if json {
        args.push("--json");
    }
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &args)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("REAL_GIT", text(&real_git.stdout))
        .env("METADATA_DIR", &metadata_dir)
        .env(
            "METADATA_WORKSPACE",
            fs::canonicalize(&source).unwrap_or_else(|_| source.clone()),
        )
        .current_dir(if hook_source { &source } else { &repo.repo })
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(!workspace_root.join("metadata-failure").exists());
    let hint = setup_retry_hint(&out, json);
    assert_eq!(
        hint.contains(&format!(
            "'{}'",
            workspace_root.join("metadata-failure").display()
        )),
        recorded,
        "{hint}"
    );
    if hook_source {
        assert!(hint.contains(" --no-hooks"), "{hint}");
        assert!(!hint.contains("--source-checkout"), "{hint}");
        assert_eq!(
            fs::read_to_string(repo.repo.join("source-hooks")).unwrap(),
            "xx"
        );
    }
    fs::remove_file(&metadata_dir).unwrap();
    fs::rename(metadata_dir.with_extension("saved"), &metadata_dir).unwrap();
    let out = run_setup_retry_hint(&repo, &repo.home, &hint);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{hint}: {} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(!repo.repo.join("unexpected-primary-hook").exists());
    assert!(!workspace_root.join("last").exists());
    let records: serde_json::Value =
        serde_json::from_slice(&records::read(metadata_dir).unwrap()).unwrap();
    assert_eq!(records["worktrees"], serde_json::json!([]));
}

#[test]
fn retirement_remove_path_and_name_aliases_deduplicate_before_hooks() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "target", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "target");
    let marker = repo.repo.join("hook-count");
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = ['printf x >> \"$RABI_REPO_PATH/hook-count\"']\n",
    )
    .unwrap();
    let relative = format!("./{}", rabi::paths::relative(&repo.repo, &path));
    for dry_run in [true, false] {
        let mut args = vec![
            "remove",
            "target",
            path.to_str().unwrap(),
            &relative,
            "--json",
        ];
        if dry_run {
            args.push("--dry-run");
        }
        let out = repo.rabi(&args);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert_eq!(
            json_value(&out)["data"]["items"].as_array().unwrap().len(),
            1
        );
        assert_eq!(marker.exists(), !dry_run);
    }
    assert_eq!(fs::read_to_string(marker).unwrap(), "x");
    assert!(!path.exists());
}

#[test]
fn retirement_remove_source_checkout_rejects_another_repository_before_changes() {
    let repo = Repo::new();
    let other = Repo::new();
    assert!(repo
        .rabi(&["create", "target", "--no-setup"])
        .status
        .success());
    let store = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&store).unwrap();
    fs::write(
        other.repo.join(".rabi.toml"),
        "[hooks]\npre-remove = ['touch wrong-hook']\n",
    )
    .unwrap();
    let out = repo.rabi(&[
        "remove",
        "target",
        "--source-checkout",
        other.repo.to_str().unwrap(),
        "--json",
    ]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(json_value(&out)["error"]["code"], "usage");
    assert!(managed(&repo, "target").is_dir());
    assert!(!managed(&repo, "target/wrong-hook").exists());
    assert!(branch_listed(&repo, "target"));
    assert_eq!(records::read(&store).unwrap(), before);
}

#[test]
fn retirement_remove_retry_preserves_linked_source_hooks() {
    let repo = Repo::new();
    for name in ["source", "done", "remaining"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
    }
    let source = managed(&repo, "source");
    fs::write(source.join(".rabi.toml"), "[hooks]\npre-remove = ['test \"$RABI_WORKSPACE\" != remaining || test -f \"$RABI_REPO_PATH/allow-remove\"', 'printf x >> \"$RABI_REPO_PATH/source-hooks\"']\n").unwrap();
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &["remove", "done", "remaining", "--json"],
        )
        .current_dir(&source)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(6), "{}", text(&out.stdout));
    let hint = setup_retry_hint(&out, true);
    assert!(
        hint.starts_with(&format!(
            "cd '{}' && rabi -R '{}' remove",
            repo.repo.display(),
            repo.repo.display()
        )),
        "{hint}"
    );
    assert_eq!(hint.matches("rabi ").count(), 1, "{hint}");
    fs::write(repo.repo.join("allow-remove"), "allow").unwrap();
    let out = run_setup_retry_hint(&repo, &repo.home, &hint);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{hint}: {} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.repo.join("source-hooks")).unwrap(),
        "xx"
    );
    assert!(!managed(&repo, "remaining").exists());
    assert!(source.is_dir());
}

#[test]
fn retirement_remove_review_retry_survives_removed_source_without_enabling_primary_hooks() {
    for (json, no_hooks) in [(false, true), (true, true), (false, false), (true, false)] {
        let repo = Repo::new();
        for name in ["source", "remaining", "last"] {
            assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        }
        fs::write(
            repo.repo.join(".rabi.toml"),
            r#"[hooks]
pre-remove = ['touch "$RABI_REPO_PATH/unexpected-primary-hook"; exit 1']
"#,
        )
        .unwrap();
        let marker = repo.repo.join("unexpected-primary-hook");
        let source = managed(&repo, "source");
        let remaining = managed(&repo, "remaining");
        repo.git(&["worktree", "lock", remaining.to_str().unwrap()]);
        let mut args = vec!["remove", ".", "remaining", "last"];
        if no_hooks {
            args.push("--no-hooks");
        }
        if json {
            args.push("--json");
        }
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&source)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(6),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(!source.exists());
        assert!(!marker.exists());
        let hint = setup_retry_hint(&out, json);
        assert!(hint.contains("-R "), "{hint}");
        assert!(
            !hint.contains(&format!("cd '{}'", source.display())),
            "{hint}"
        );
        repo.git(&["worktree", "unlock", remaining.to_str().unwrap()]);
        let out = run_setup_retry_hint(&repo, &repo.home, &hint);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{hint}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(hint.contains(" --no-hooks"), "{hint}");
        assert!(!marker.exists());
        assert!(!remaining.exists());
        assert!(!managed(&repo, "last").exists());
    }
}

#[test]
fn retirement_remove_review_retains_hook_source_until_other_targets_finish() {
    let repo = Repo::new();
    for name in ["done", "source", "remaining", "last"] {
        assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
    }
    let source = managed(&repo, "source");
    let remaining = managed(&repo, "remaining");
    fs::write(
        source.join(".rabi.toml"),
        r#"[hooks]
pre-remove = ['printf "%s\n" "$RABI_WORKSPACE" >> "$RABI_REPO_PATH/hooks-ran"']
"#,
    )
    .unwrap();
    repo.git(&["worktree", "lock", remaining.to_str().unwrap()]);
    let out = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "remove",
                "done",
                ".",
                "remaining",
                "last",
                "--discard-changes",
                "--json",
            ],
        )
        .current_dir(&source)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(6),
        "{} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(source.is_dir(), "keep the hook source available for retry");
    let hint = setup_retry_hint(&out, true);
    repo.git(&["worktree", "unlock", remaining.to_str().unwrap()]);
    let out = run_setup_retry_hint(&repo, &repo.home, &hint);
    assert_eq!(
        out.status.code(),
        Some(0),
        "{hint}: {} {}",
        text(&out.stdout),
        text(&out.stderr)
    );
    assert!(!source.exists());
    assert!(!remaining.exists());
    assert!(!managed(&repo, "last").exists());
    assert_eq!(
        fs::read_to_string(repo.repo.join("hooks-ran")).unwrap(),
        "done\nremaining\nremaining\nlast\nsource\n"
    );
}

#[test]
fn retirement_remove_review_resolves_paths_before_deleting_current_checkout() {
    for relative in [false, true] {
        let repo = Repo::new();
        for name in ["source", "other"] {
            assert!(repo.rabi(&["create", name, "--no-setup"]).status.success());
        }
        let source = managed(&repo, "source");
        let other = managed(&repo, "other");
        let selector = if relative {
            "../other"
        } else {
            other.to_str().unwrap()
        };
        let out = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["remove", ".", selector, "--no-hooks", "--json"],
            )
            .current_dir(&source)
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(0),
            "{selector}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert!(!source.exists());
        assert!(!other.exists());
        assert!(json_value(&out)["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["removed"] == true));
    }
}

#[test]
fn retirement_remove_review_dot_resolves_record_after_registration_disappears() {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "unregistered", "--no-setup"])
        .status
        .success());
    let path = managed(&repo, "unregistered");
    let nested = path.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("precious"), "keep").unwrap();
    let admin = text(
        &repo
            .command("git", &["rev-parse", "--absolute-git-dir"])
            .current_dir(&path)
            .output()
            .unwrap()
            .stdout,
    );
    fs::remove_dir_all(admin).unwrap();
    let record_dir = repo.repo.join(".git/rabi/workspaces");
    let before = records::read(&record_dir).unwrap();
    for cwd in [&path, &nested] {
        for selector in [".", "unregistered", path.to_str().unwrap(), "./.."] {
            if selector == "./.." && cwd == &path {
                continue;
            }
            for dry_run in [false, true] {
                let mut args = vec![
                    "-R",
                    repo.repo.to_str().unwrap(),
                    "remove",
                    selector,
                    "--json",
                ];
                if dry_run {
                    args.push("--dry-run");
                }
                let out = repo
                    .command(env!("CARGO_BIN_EXE_rabi"), &args)
                    .current_dir(cwd)
                    .output()
                    .unwrap();
                assert_eq!(
                    out.status.code(),
                    Some(0),
                    "{selector}: {} {}",
                    text(&out.stdout),
                    text(&out.stderr)
                );
                let doc = json_value(&out);
                assert_eq!(doc["data"]["items"][0]["alreadyGone"], true);
                assert_eq!(doc["data"]["items"][0]["removed"], false);
                assert_eq!(
                    doc["data"]["items"][0]["workspace"]["path"],
                    path.to_str().unwrap()
                );
                assert_eq!(records::read(&record_dir).unwrap(), before);
                assert_eq!(fs::read_to_string(nested.join("precious")).unwrap(), "keep");
                assert!(branch_listed(&repo, "unregistered"));
            }
        }
    }
}

#[test]
fn shell_commands_output_and_usage_contract() {
    let repo = Repo::new();
    for command in ["completions", "shell-init"] {
        for shell in ["bash", "zsh", "fish"] {
            let normal = repo.rabi(&[command, shell]);
            assert!(
                normal.status.success(),
                "{command} {shell}: {}",
                text(&normal.stderr)
            );
            assert!(!normal.stdout.is_empty());
            for mode in ["json", "invalid"] {
                let output = repo
                    .command(env!("CARGO_BIN_EXE_rabi"), &[command, shell])
                    .env("RABI_OUTPUT", mode)
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{}", text(&output.stderr));
                assert_eq!(output.stdout, normal.stdout);
            }
            for flags in [
                vec!["--json"],
                vec!["--output", "json"],
                vec!["--output=json"],
                vec!["--json", "--help"],
            ] {
                let mut args = vec![command, shell];
                args.extend(flags);
                let output = repo.rabi(&args);
                assert_eq!(output.status.code(), Some(2), "{args:?}");
                assert!(output.stdout.is_empty());
                assert!(text(&output.stderr).contains("JSON"));
            }
            assert_closed_stdout(&repo, &[command, shell], 0);
        }
        for args in [
            vec![command],
            vec![command, "powershell"],
            vec![command, "bash", "extra"],
        ] {
            let output = repo.rabi(&args);
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            let error = text(&output.stderr);
            assert!(
                error.contains("bash") && error.contains("zsh") && error.contains("fish"),
                "{error}"
            );
        }
        let help = repo.rabi(&["help", command]);
        assert!(help.status.success());
        assert!(text(&help.stdout).contains("bash|zsh|fish"));
        let help = repo.rabi(&["help", command, "--json"]);
        let value: serde_json::Value = serde_json::from_slice(&help.stdout).unwrap();
        assert_eq!(value["schema"], 2);
        assert_eq!(value["data"]["selected"]["name"], command);
    }
}

#[test]
fn bash_shell_smoke() {
    shell_smoke("bash");
}
#[test]
fn zsh_shell_smoke() {
    shell_smoke("zsh");
}
#[test]
fn fish_shell_smoke() {
    shell_smoke("fish");
}

fn shell_available(shell: &str) -> bool {
    match Command::new(shell).arg("--version").output() {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            rabi::output::stderr(format_args!(
                "SKIP {shell} shell smoke: {shell} is not installed\n"
            ));
            return false;
        }
        other => assert!(other.unwrap().status.success()),
    }
    true
}

#[test]
fn bash_completion_quotes_shell_input() {
    if !shell_available("bash") {
        return;
    }
    let repo = Repo::new();
    let completions = repo.root.join("completions.bash");
    fs::write(&completions, repo.rabi(&["completions", "bash"]).stdout).unwrap();
    let bin = repo.root.join("bin");
    fs::create_dir(&bin).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_rabi"), bin.join("rabi")).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    // These Git refs reach completion through actual unmanaged checkouts.
    let names = [
        "review;>pwned",
        "$(>pwned)",
        "single'quote",
        "double\"quote",
    ];
    for (index, name) in names.iter().enumerate() {
        repo.git(&[
            "worktree",
            "add",
            "-b",
            name,
            repo.root
                .join(format!("checkout-{index}"))
                .to_str()
                .unwrap(),
        ]);
    }
    // Git rejects spaces, backslashes and newlines in refs; mock those records.
    // Newlines delimit candidates, so check both records for safe insertion.
    for name in names
        .into_iter()
        .chain(["space argument", "back\\slash", "line;>pwned\n$(>pwned)"])
    {
        for expected in name.split('\n') {
            let script = r#"
source "$RABI_TEST_COMPLETIONS"
if [ "$RABI_TEST_MOCK" = 1 ]; then
    _rabi_candidates() { printf '%s\n' "$RABI_TEST_NAME"; }
fi
COMP_WORDS=(rabi path "$RABI_TEST_EXPECTED"); COMP_CWORD=2; _rabi
[ "${#COMPREPLY[@]}" = 1 ] || exit 40
rabi() { printf '%s\0' "$@" > "$RABI_TEST_ARGS"; }
eval "rabi path ${COMPREPLY[0]}"
"#;
            let output = repo
                .command("bash", &["--noprofile", "--norc", "-c", script])
                .env("PATH", &path)
                .env("RABI_TEST_COMPLETIONS", &completions)
                .env("RABI_TEST_NAME", name)
                .env("RABI_TEST_EXPECTED", expected)
                .env(
                    "RABI_TEST_MOCK",
                    if names.contains(&name) { "0" } else { "1" },
                )
                .env("RABI_TEST_ARGS", repo.root.join("args"))
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{name:?}: {}",
                text(&output.stderr)
            );
            assert!(
                !repo.repo.join("pwned").exists(),
                "candidate executed shell syntax: {name:?}"
            );
            assert_eq!(
                fs::read(repo.root.join("args")).unwrap(),
                format!("path\0{expected}\0").as_bytes(),
                "{name:?}"
            );
        }
    }
}

fn shell_smoke(shell: &str) {
    if !shell_available(shell) {
        return;
    }
    let repo = Repo::new();
    // Reproduce compaudit's failure independently of host fpath permissions.
    let insecure_fpath = repo.root.join("insecure-functions");
    fs::create_dir(&insecure_fpath).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&insecure_fpath, fs::Permissions::from_mode(0o777)).unwrap();
    }
    for suffix in ["\n", "\n\n"] {
        let target = repo.root.join(format!("trailing{suffix}"));
        repo.git(&["worktree", "add", "--detach", target.to_str().unwrap()]);
    }
    // A trimmed path must never silently select this different directory.
    fs::create_dir(repo.root.join("trailing")).unwrap();
    let parent = repo.root.join("parent with ' spaces");
    fs::create_dir(&parent).unwrap();
    let target = parent.join("-workspace");
    let name = "-workspace";
    let config = repo.rabi(&["config", "set", "workspace-root", parent.to_str().unwrap()]);
    assert!(config.status.success(), "{}", text(&config.stderr));
    let output = repo.rabi(&["create", "--detach", "--no-setup", "--", name]);
    assert!(output.status.success(), "{}", text(&output.stderr));
    let ordinary = repo.rabi(&["create", "ordinary", "--detach", "--no-setup"]);
    assert!(ordinary.status.success(), "{}", text(&ordinary.stderr));
    let bin = repo.root.join("bin");
    fs::create_dir(&bin).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_rabi"), bin.join("rabi")).unwrap();
    for command in ["shell-init", "completions"] {
        let output = repo.rabi(&[command, shell]);
        assert!(output.status.success(), "{}", text(&output.stderr));
        let file = repo.root.join(format!("{command}.{shell}"));
        fs::write(&file, output.stdout).unwrap();
        let syntax = repo
            .command(
                shell,
                &[
                    if shell == "fish" {
                        "--no-execute"
                    } else {
                        "-n"
                    },
                    file.to_str().unwrap(),
                ],
            )
            .output()
            .unwrap();
        assert!(syntax.status.success(), "{shell}: {}", text(&syntax.stderr));
    }
    let script = if shell == "fish" {
        r#"
source "$RABI_TEST_INIT"
source "$RABI_TEST_COMPLETIONS"
set -l start "$PWD"
rabi cd missing 2> "$RABI_TEST_ERROR"
set -l result $status
test $result -eq 3; or exit 11
test "$PWD" = "$start"; or exit 12
rabi -R "$RABI_TEST_REPO" cd -- "$RABI_TEST_NAME"; or exit 13
test "$PWD" = "$RABI_TEST_TARGET"; or exit 14
builtin cd -- "$start"
rabi cd --repo="$RABI_TEST_REPO" -- "$RABI_TEST_NAME"; or exit 15
test "$PWD" = "$RABI_TEST_TARGET"; or exit 16
builtin cd -- "$start"
rabi cd; or exit 19
test "$PWD" = "$start"; or exit 20
rabi exec -- sh -c 'exit 23'
test $status -eq 23; or exit 21
rabi exec -- printf '%s\n' 'space argument' '' '-leading' > "$RABI_TEST_ARGS"; or exit 17
complete -C 'rabi ' > "$RABI_TEST_COMMANDS"
complete -C 'rabi path ' > "$RABI_TEST_NAMES"
complete -C 'rabi repo ' > "$RABI_TEST_CHILDREN"
complete -C 'rabi sync --' > "$RABI_TEST_FLAGS"
# The token under the cursor is a prefix to complete, not a consumed word.
complete -C 'rabi pat' | string match -q -r '^path\t'; or exit 41
complete -C 'rabi path ordin' | string match -q -r '^ordinary$'; or exit 42
"#
    } else {
        r#"
if [ -n "$ZSH_VERSION" ]; then
    fpath=("$RABI_TEST_INSECURE_FPATH" $fpath)
    autoload -Uz compinit compaudit
    compaudit > "$RABI_TEST_RESULTS/compaudit" 2>&1 && exit 35
    # Only this test fixture bypasses the host's completion-directory audit.
    compinit -u -D || exit 10
fi
source "$RABI_TEST_INIT"
source "$RABI_TEST_COMPLETIONS"
start=$PWD
rabi cd missing 2> "$RABI_TEST_ERROR"
result=$?
[ "$result" -eq 3 ] || exit 11
[ "$PWD" = "$start" ] || exit 12
rabi -R "$RABI_TEST_REPO" cd -- "$RABI_TEST_NAME" || exit 13
[ "$PWD" = "$RABI_TEST_TARGET" ] || exit 14
builtin cd -- "$start"
rabi cd --repo="$RABI_TEST_REPO" -- "$RABI_TEST_NAME" || exit 15
[ "$PWD" = "$RABI_TEST_TARGET" ] || exit 16
builtin cd -- "$start"
rabi cd || exit 19
[ "$PWD" = "$start" ] || exit 20
rabi exec -- sh -c 'exit 23'
[ "$?" -eq 23 ] || exit 21
rabi exec -- printf '%s\n' 'space argument' '' '-leading' > "$RABI_TEST_ARGS" || exit 17
if [ -n "$ZSH_VERSION" ]; then
    [ "${_comps[rabi]}" = _rabi ] || exit 18
    # Stub only the ZLE presentation; exercise the registered adapter.
    _describe() { print -rl -- "${rabi_entries[@]}"; }
    words=(rabi ''); CURRENT=2; _rabi > "$RABI_TEST_COMMANDS"
    words=(rabi path ''); CURRENT=3; _rabi > "$RABI_TEST_NAMES"
    words=(rabi repo ''); CURRENT=3; _rabi > "$RABI_TEST_CHILDREN"
    words=(rabi sync --); CURRENT=3; _rabi > "$RABI_TEST_FLAGS"
else
    complete -p rabi || exit 18
    COMP_WORDS=(rabi ''); COMP_CWORD=1; _rabi
    printf '%s\n' "${COMPREPLY[@]}" > "$RABI_TEST_COMMANDS"
    COMP_WORDS=(rabi path ''); COMP_CWORD=2; _rabi
    printf '%s\n' "${COMPREPLY[@]}" > "$RABI_TEST_NAMES"
    COMP_WORDS=(rabi repo ''); COMP_CWORD=2; _rabi
    printf '%s\n' "${COMPREPLY[@]}" > "$RABI_TEST_CHILDREN"
    COMP_WORDS=(rabi sync --); COMP_CWORD=2; _rabi
    printf '%s\n' "${COMPREPLY[@]}" > "$RABI_TEST_FLAGS"
fi
"#
    };
    let mut script = script.to_string();
    script.push_str(r#"
rabi cd ordinary || exit 29
test "$PWD" = "$RABI_TEST_ORDINARY" || exit 30
builtin cd -- "$RABI_TEST_REPO"
rabi "-R$RABI_TEST_REPO" cd -- "$RABI_TEST_NAME" || exit 22
test "$PWD" = "$RABI_TEST_TARGET" || exit 24
builtin cd -- "$RABI_TEST_REPO"
rabi --repo="$RABI_TEST_REPO" cd -- "$RABI_TEST_NAME" || exit 25
test "$PWD" = "$RABI_TEST_TARGET" || exit 26
builtin cd -- "$RABI_TEST_REPO"
rabi cd "$RABI_TEST_TARGET" || exit 27
test "$PWD" = "$RABI_TEST_TARGET" || exit 28
rabi cd "$RABI_TEST_NEWLINE_TARGET" || exit 31
test "$PWD" = "$RABI_TEST_NEWLINE_TARGET" || exit 32
builtin cd -- "$RABI_TEST_REPO"
rabi cd "$RABI_TEST_NEWLINES_TARGET" || exit 33
test "$PWD" = "$RABI_TEST_NEWLINES_TARGET" || exit 34
builtin cd -- "$HOME"
_rabi_candidates -R "$RABI_TEST_REPO" path -- > "$RABI_TEST_RESULTS/repo-short"
_rabi_candidates --repo="$RABI_TEST_REPO" path -- > "$RABI_TEST_RESULTS/repo-long"
_rabi_candidates "-R$RABI_TEST_REPO" path -- > "$RABI_TEST_RESULTS/repo-attached"
_rabi_candidates --repo "$HOME/missing-repo" path -- > "$RABI_TEST_RESULTS/failed-query"
_rabi_candidates exec -- > "$RABI_TEST_RESULTS/exec-boundary"
_rabi_candidates exec workspace > "$RABI_TEST_RESULTS/exec-selector"
_rabi_candidates path -- "$RABI_TEST_NAME" > "$RABI_TEST_RESULTS/single-selector"
_rabi_candidates -R "$RABI_TEST_REPO" remove -- "$RABI_TEST_NAME" > "$RABI_TEST_RESULTS/multiple-selectors"
_rabi_candidates -R "$RABI_TEST_REPO" setup --copy-file pattern -- > "$RABI_TEST_RESULTS/value-consumed"
_rabi_candidates setup --copy-file > "$RABI_TEST_RESULTS/value-pending"
"#);
    let contract: serde_json::Value =
        serde_json::from_str(include_str!("../help-data.json")).unwrap();
    for (index, key) in contract["helps"].as_object().unwrap().keys().enumerate() {
        script.push_str(&format!(
            "_rabi_candidates {key} > \"$RABI_TEST_RESULTS/contract-{index}\"\n"
        ));
    }
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut command = repo.command(
        shell,
        &[
            if shell == "fish" {
                "--no-config"
            } else {
                "--norc"
            },
            "-c",
            &script,
        ],
    );
    // zsh uses -f to skip startup files.
    if shell == "zsh" {
        command = repo.command(shell, &["-f", "-c", &script]);
    }
    let output = command
        .env("PATH", path)
        .env("ZDOTDIR", &repo.home)
        .env("RABI_TEST_INSECURE_FPATH", &insecure_fpath)
        .env("RABI_TEST_NEWLINE_TARGET", repo.root.join("trailing\n"))
        .env("RABI_TEST_NEWLINES_TARGET", repo.root.join("trailing\n\n"))
        .env("RABI_OUTPUT", "json")
        .env(
            "RABI_TEST_INIT",
            repo.root.join(format!("shell-init.{shell}")),
        )
        .env(
            "RABI_TEST_COMPLETIONS",
            repo.root.join(format!("completions.{shell}")),
        )
        .env("RABI_TEST_RESULTS", &repo.root)
        .env("RABI_TEST_REPO", &repo.repo)
        .env("RABI_TEST_TARGET", &target)
        .env("RABI_TEST_ORDINARY", parent.join("ordinary"))
        .env("RABI_TEST_NAME", name)
        .env("RABI_TEST_ERROR", repo.root.join("error"))
        .env("RABI_TEST_ARGS", repo.root.join("args"))
        .env("RABI_TEST_COMMANDS", repo.root.join("commands"))
        .env("RABI_TEST_NAMES", repo.root.join("names"))
        .env("RABI_TEST_CHILDREN", repo.root.join("children"))
        .env("RABI_TEST_FLAGS", repo.root.join("flags"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{shell} {:?}: {}\n{}",
        output.status.code(),
        text(&output.stderr),
        text(&output.stdout)
    );
    if shell == "zsh" {
        let audit = fs::read_to_string(repo.root.join("compaudit")).unwrap();
        assert!(audit.contains(insecure_fpath.to_str().unwrap()), "{audit}");
    }
    assert_eq!(
        fs::read_to_string(repo.root.join("args")).unwrap(),
        "space argument\n\n-leading\n"
    );
    let lookup_failure = repo.rabi(&["path", "--output", "text", "missing"]);
    assert_eq!(lookup_failure.status.code(), Some(3));
    assert_eq!(
        fs::read(repo.root.join("error")).unwrap(),
        lookup_failure.stderr
    );
    for (file, candidate) in [
        ("commands", "create"),
        ("names", name),
        ("children", "add"),
        ("flags", "--rewrite-published"),
    ] {
        let candidates = fs::read_to_string(repo.root.join(file)).unwrap();
        assert!(
            candidates.contains(candidate),
            "{shell} {file}: {candidates}"
        );
    }
    for file in [
        "repo-short",
        "repo-long",
        "repo-attached",
        "multiple-selectors",
        "value-consumed",
    ] {
        let candidates = fs::read_to_string(repo.root.join(file)).unwrap();
        assert!(
            candidates.lines().any(|line| line == name),
            "{shell} {file}: {candidates}"
        );
    }
    for file in [
        "failed-query",
        "exec-boundary",
        "exec-selector",
        "single-selector",
        "value-pending",
    ] {
        assert_eq!(
            fs::read_to_string(repo.root.join(file)).unwrap(),
            "",
            "{shell} {file}"
        );
    }
    for (index, (key, help)) in contract["helps"].as_object().unwrap().iter().enumerate() {
        let candidates = fs::read_to_string(repo.root.join(format!("contract-{index}"))).unwrap();
        for option in help["options"].as_array().unwrap() {
            let long = format!("--{}", option["name"].as_str().unwrap());
            assert!(
                candidates
                    .lines()
                    .any(|line| line.split('\t').next() == Some(long.as_str())),
                "{shell} {key} {long}: {candidates}"
            );
            if let Some(short) = option["short"].as_str() {
                let short = format!("-{short}");
                assert!(
                    candidates
                        .lines()
                        .any(|line| line.split('\t').next() == Some(short.as_str())),
                    "{shell} {key} {short}"
                );
            }
        }
    }
}

// GitHub retirement evidence must never turn an uncertain response into deletion.
fn merged_pr_fixture(link: Option<&str>) -> Repo {
    let repo = Repo::new();
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    commit_file(&repo, "topic", "work.txt", "topic work\n");
    if let Some(link) = link {
        assert!(repo.rabi(&["set", "topic", "--pr", link]).status.success());
    }
    repo
}

fn merged_pr_response(oid: &str, state: &str) -> serde_json::Value {
    serde_json::json!({"number":7,"url":"https://github.com/acme/demo/pull/7",
        "state":state,"headRefOid":oid})
}

fn merged_pr_command(repo: &Repo, args: &[&str], response: &serde_json::Value) -> Command {
    let path = gh_path(
        repo,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$GH_CALLS"
if [ -n "$GH_CHECK_INPUT" ]; then
  [ "$GH_PROMPT_DISABLED" = 1 ] || exit 9
  # A character device distinguishes /dev/null from a closed stdin pipe.
  [ -c /dev/stdin ] || exit 9
  read line && exit 9
fi
case "$1 $2" in
  'repo view') printf '%s\n' "$GH_REPO" ;;
  'pr view'|'pr list')
    case "$GH_PR_FAILURE" in
      exit) exit 1 ;;
      json) printf 'invalid json'; exit 0 ;;
      timeout) exec sleep 5 ;;
      descendant) sleep 5 & exit 0 ;;
    esac
    printf '%s\n' "$GH_RESPONSE" ;;

  *) exit 1 ;;
esac
"#,
    );
    let mut command = repo.command(env!("CARGO_BIN_EXE_rabi"), args);
    command
        .env("PATH", path)
        .env("GH_CALLS", repo.root.join("gh-calls"))
        .env("GH_RESPONSE", response.to_string())
        .env(
            "GH_REPO",
            r#"{"url":"https://github.com/acme/demo","owner":{"login":"acme"},"name":"demo"}"#,
        );
    command
}

#[test]
fn merged_pr_remove_linked_exact_and_ancestor() {
    for ancestor in [false, true] {
        for link in ["7", "https://github.com/acme/demo/pull/7"] {
            let repo = merged_pr_fixture(Some(link));
            let local = retirement_head_at(&repo, &managed(&repo, "topic"));
            if ancestor {
                commit_file(&repo, "topic", "later.txt", "later PR work\n");
            }
            let head = retirement_head_at(&repo, &managed(&repo, "topic"));
            if ancestor {
                git_at(
                    &repo,
                    &managed(&repo, "topic"),
                    &["reset", "--hard", &local],
                );
            }
            let response = merged_pr_response(&head, "MERGED");
            let plan = merged_pr_command(
                &repo,
                &["remove", "topic", "--dry-run", "--json"],
                &response,
            )
            .output()
            .unwrap();
            assert!(plan.status.success(), "{}", text(&plan.stderr));
            let item = &json_value(&plan)["data"]["items"][0];
            assert_eq!(item["wouldDeleteBranch"], true, "{item}");
            assert_eq!(item["preservationReason"], "merged_pr");
            assert!(managed(&repo, "topic").is_dir());
            fs::remove_file(repo.root.join("gh-calls")).unwrap();
            let removed = merged_pr_command(&repo, &["remove", "topic", "--json"], &response)
                .output()
                .unwrap();
            assert!(removed.status.success(), "{}", text(&removed.stderr));
            assert_eq!(
                json_value(&removed)["data"]["items"][0]["branchDeleted"],
                true
            );
            assert!(!branch_listed(&repo, "topic"));
            let calls = fs::read_to_string(repo.root.join("gh-calls")).unwrap();
            assert_eq!(
                calls
                    .lines()
                    .filter(|line| line.starts_with("pr view"))
                    .count(),
                1,
                "{calls}"
            );
            assert!(calls.contains(&format!("pr view {link} --json")), "{calls}");
        }
    }
}

#[test]
fn merged_pr_partial_clone_requires_exact_head_without_object_probes() {
    for config in ["extension", "promisor", "not-promisor"] {
        for relation in ["exact", "ancestor", "missing"] {
            for gc in [false, true] {
                let repo = merged_pr_fixture(Some("7"));
                let local = retirement_head_at(&repo, &managed(&repo, "topic"));
                let head = match relation {
                    "ancestor" => {
                        commit_file(&repo, "topic", "later.txt", "later PR work\n");
                        let head = retirement_head_at(&repo, &managed(&repo, "topic"));
                        git_at(
                            &repo,
                            &managed(&repo, "topic"),
                            &["reset", "--hard", &local],
                        );
                        head
                    }
                    "missing" => "1234567890123456789012345678901234567890".to_owned(),
                    _ => local.clone(),
                };
                match config {
                    "extension" => repo.git(&["config", "extensions.partialClone", "origin"]),
                    "promisor" => {
                        repo.git(&["config", "remote.other.promisor", "false"]);
                        repo.git(&["config", "remote.origin.promisor", "true"]);
                    }
                    _ => repo.git(&["config", "remote.origin.promisor", "false"]),
                }
                let partial = config != "not-promisor";
                let preserved = relation == "exact" || (!partial && relation == "ancestor");
                let response = merged_pr_response(&head, "MERGED");
                for plan in [true, false] {
                    let args = match (gc, plan) {
                        (true, true) => vec!["gc", "--merged", "--json"],
                        (true, false) => vec!["gc", "--merged", "--apply", "--json"],
                        (false, true) => vec!["remove", "topic", "--dry-run", "--json"],
                        (false, false) => vec!["remove", "topic", "--json"],
                    };
                    let trace = repo.root.join("git-trace.json");
                    fs::write(&trace, "").unwrap();
                    let mut child = merged_pr_command(&repo, &args, &response)
                        .env("GIT_TRACE2_EVENT", &trace)
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::piped())
                        .spawn()
                        .unwrap();
                    let started = std::time::Instant::now();
                    while child.try_wait().unwrap().is_none() {
                        if started.elapsed() > std::time::Duration::from_secs(10) {
                            child.kill().unwrap();
                            child.wait().unwrap();
                            panic!("PR preservation hung: {config} {relation} {args:?}");
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    let out = child.wait_with_output().unwrap();
                    assert!(out.status.success(), "{}", text(&out.stderr));
                    let doc = json_value(&out);
                    let items = doc["data"]["items"].as_array().unwrap();
                    if gc && !preserved {
                        assert!(items.is_empty(), "{config} {relation}: {doc}");
                    } else {
                        let item = &items[0];
                        if gc {
                            assert_eq!(
                                item["action"],
                                if plan { "wouldRemove" } else { "removed" }
                            );
                        } else {
                            assert_eq!(
                                item[if plan {
                                    "wouldDeleteBranch"
                                } else {
                                    "branchDeleted"
                                }],
                                preserved,
                                "{config} {relation}: {item}"
                            );
                            if !preserved {
                                assert_eq!(item["branchKeptReason"], "unpreserved");
                            }
                        }
                        if preserved {
                            assert_eq!(item["preservationReason"], "merged_pr");
                        }
                    }
                    for line in fs::read_to_string(&trace).unwrap().lines() {
                        let event: serde_json::Value = serde_json::from_str(line).unwrap();
                        let Some(argv) = event["argv"].as_array() else {
                            continue;
                        };
                        assert!(!argv.iter().any(|arg| arg == "fetch"), "{event}");
                        if partial {
                            assert!(
                                !argv
                                    .windows(3)
                                    .any(|args| args == ["cat-file", "-t", head.as_str()]),
                                "{event}"
                            );
                            assert!(
                                !argv.windows(4).any(|args| args
                                    == [
                                        "merge-base",
                                        "--is-ancestor",
                                        local.as_str(),
                                        head.as_str()
                                    ]),
                                "{event}"
                            );
                        }
                    }
                    assert_eq!(branch_listed(&repo, "topic"), plan || !preserved);
                }
            }
        }
    }
}

#[test]
fn merged_pr_remove_keeps_uncertain_and_unmerged_tips() {
    for case in [
        "ahead",
        "open",
        "closed",
        "missing-object",
        "foreign-url",
        "foreign-response",
        "wrong-number",
        "bad-oid",
        "repo-unavailable",
        "detached",
    ] {
        let link = if case == "foreign-url" {
            "https://github.com/other/demo/pull/7"
        } else {
            "https://github.com/acme/demo/pull/7"
        };
        let repo = merged_pr_fixture(Some(link));
        let head = retirement_head_at(&repo, &managed(&repo, "topic"));
        let mut response = merged_pr_response(&head, "MERGED");
        match case {
            "ahead" => commit_file(&repo, "topic", "unpublished", "keep this\n"),
            "open" => response["state"] = "OPEN".into(),
            "closed" => response["state"] = "CLOSED".into(),
            "missing-object" => {
                response["headRefOid"] = "1234567890123456789012345678901234567890".into()
            }
            "foreign-response" => response["url"] = "https://github.com/other/demo/pull/7".into(),
            "wrong-number" => response["number"] = 8.into(),
            "bad-oid" => response["headRefOid"] = "main".into(),
            "detached" => git_at(&repo, &managed(&repo, "topic"), &["checkout", "--detach"]),
            _ => (),
        }
        let mut cmd = merged_pr_command(
            &repo,
            &["remove", "topic", "--dry-run", "--json"],
            &response,
        );
        if case == "repo-unavailable" {
            cmd.env("GH_REPO", "{}");
        }
        let out = cmd.output().unwrap();
        if case == "detached" {
            assert_eq!(
                json_value(&out)["error"]["code"],
                "unpreserved",
                "{case}: {}",
                text(&out.stdout)
            );
        } else {
            assert!(out.status.success(), "{case}: {}", text(&out.stderr));
            assert_eq!(
                json_value(&out)["data"]["items"][0]["branchKeptReason"],
                "unpreserved",
                "{case}"
            );
            let mut cmd = merged_pr_command(&repo, &["remove", "topic", "--json"], &response);
            if case == "repo-unavailable" {
                cmd.env("GH_REPO", "{}");
            }
            let removed = cmd.output().unwrap();
            assert!(
                removed.status.success(),
                "{case}: {}",
                text(&removed.stderr)
            );
            assert!(
                !json_value(&removed)["data"]["items"][0]["branchDeleted"]
                    .as_bool()
                    .unwrap(),
                "{case}"
            );
        }
        assert!(branch_listed(&repo, "topic"), "{case}");
    }
}

#[test]
fn merged_pr_remove_gh_failures_keep_branch() {
    for body in [
        "exit 1",
        "printf 'invalid json'",
        "exec sleep 5",
        "exit 127",
        "printf '{}'",
        "printf '{\"state\":\"MERGED\"}'",
    ] {
        let repo = merged_pr_fixture(Some("7"));
        let path = gh_path(&repo, &format!("#!/bin/sh\n{body}\n"));
        let started = std::time::Instant::now();
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["remove", "topic", "--json"])
            .env("PATH", path)
            .env("RABI_GH_TIMEOUT_MS", "50")
            .output()
            .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{body}"
        );
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert_eq!(
            json_value(&out)["data"]["items"][0]["branchKeptReason"],
            "unpreserved",
            "{body}"
        );
        assert!(branch_listed(&repo, "topic"));
    }
    let repo = merged_pr_fixture(Some("7"));
    let bin = repo.root.join("without-gh");
    fs::create_dir(&bin).unwrap();
    let git = repo
        .command("sh", &["-c", "command -v git"])
        .output()
        .unwrap();
    std::os::unix::fs::symlink(text(&git.stdout), bin.join("git")).unwrap();
    let out = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["remove", "topic", "--json"])
        .env("PATH", bin)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(branch_listed(&repo, "topic"));
}

#[test]
fn merged_pr_unlinked_requires_one_exact_head() {
    for case in [
        "exact",
        "multiple",
        "none",
        "ancestor",
        "wrong-head",
        "foreign",
    ] {
        let repo = merged_pr_fixture(None);
        let local = retirement_head_at(&repo, &managed(&repo, "topic"));
        let mut response = merged_pr_response(&local, "MERGED");
        response["headRefName"] = "topic".into();
        if case == "ancestor" {
            commit_file(&repo, "topic", "later", "later\n");
            response["headRefOid"] = retirement_head_at(&repo, &managed(&repo, "topic")).into();
            git_at(
                &repo,
                &managed(&repo, "topic"),
                &["reset", "--hard", &local],
            );
        }
        if case == "wrong-head" {
            response["headRefName"] = "other".into();
        }
        if case == "foreign" {
            response["url"] = "https://github.com/other/demo/pull/7".into();
        }
        let response = match case {
            "multiple" => serde_json::json!([response, response]),
            "none" => serde_json::json!([]),
            _ => serde_json::json!([response]),
        };
        let out = merged_pr_command(&repo, &["remove", "topic", "--json"], &response)
            .output()
            .unwrap();
        assert!(out.status.success(), "{case}: {}", text(&out.stderr));
        assert_eq!(
            json_value(&out)["data"]["items"][0]["branchDeleted"],
            case == "exact",
            "{case}: {}",
            text(&out.stdout)
        );
        let calls = fs::read_to_string(repo.root.join("gh-calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("pr list"))
                .count(),
            1,
            "{calls}"
        );
    }
}

#[test]
fn merged_pr_gc_merged_candidates_share_queries_and_recheck_local_tips() {
    for apply in [false, true] {
        let repo = merged_pr_fixture(Some("https://github.com/acme/demo/pull/7"));
        let head = retirement_head_at(&repo, &managed(&repo, "topic"));
        repo.git(&["branch", "copy", "topic"]);
        assert!(repo
            .rabi(&["create", "copy", "--branch", "copy", "--no-setup"])
            .status
            .success());
        assert!(repo.rabi(&["set", "copy", "--pr", "7"]).status.success());
        let response = merged_pr_response(&head, "MERGED");
        let args = if apply {
            vec!["gc", "--merged", "--closed-pr", "--apply", "--json"]
        } else {
            vec!["gc", "--merged", "--json"]
        };
        let out = merged_pr_command(&repo, &args, &response).output().unwrap();
        assert!(
            out.status.success(),
            "{} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        let doc = json_value(&out);
        let items = doc["data"]["items"].as_array().unwrap();
        assert_eq!(items.len(), 2, "{doc}");
        for item in items {
            assert_eq!(
                item["action"],
                if apply { "removed" } else { "wouldRemove" },
                "{item}"
            );
            assert!(item["reasons"]
                .as_array()
                .unwrap()
                .contains(&"merged".into()));
            assert_eq!(item["preservationReason"], "merged_pr");
        }
        assert_eq!(managed(&repo, "topic").is_dir(), !apply);
        let calls = fs::read_to_string(repo.root.join("gh-calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("pr view"))
                .count(),
            1,
            "{calls}"
        );
    }
}

#[test]
fn merged_pr_local_proof_avoids_gh_and_noninteractive_stdin_is_null() {
    let repo = merged_pr_fixture(Some("7"));
    repo.git(&["merge", "--ff-only", "topic"]);
    let out = merged_pr_command(
        &repo,
        &["remove", "topic", "--dry-run", "--json"],
        &serde_json::Value::Null,
    )
    .output()
    .unwrap();
    assert!(out.status.success());
    assert_eq!(
        json_value(&out)["data"]["items"][0]["wouldDeleteBranch"],
        true
    );
    assert!(!repo.root.join("gh-calls").exists());
    for env in ["RABI_NO_INPUT", "CI", "NO_TTY"] {
        let repo = merged_pr_fixture(Some("7"));
        let response = merged_pr_response(
            &retirement_head_at(&repo, &managed(&repo, "topic")),
            "MERGED",
        );
        let out = merged_pr_command(
            &repo,
            &["remove", "topic", "--dry-run", "--output", "text"],
            &response,
        )
        .env(env, "1")
        .env("GH_CHECK_INPUT", "1")
        .stdin(std::process::Stdio::piped())
        .output()
        .unwrap();
        assert!(out.status.success(), "{}", text(&out.stderr));
        assert!(
            text(&out.stdout).contains("and its branch (preserved by merged PR)"),
            "{env}: {}",
            text(&out.stdout)
        );
    }
}

#[test]
fn merged_pr_query_failures_are_cached_and_gc_reports_unavailable() {
    for failure in ["exit", "json", "timeout", "descendant"] {
        let repo = merged_pr_fixture(Some("https://github.com/acme/demo/pull/7"));
        let response = merged_pr_response(
            &retirement_head_at(&repo, &managed(&repo, "topic")),
            "MERGED",
        );
        let started = std::time::Instant::now();
        let out = merged_pr_command(&repo, &["gc", "--merged", "--apply", "--json"], &response)
            .env("GH_PR_FAILURE", failure)
            .env("RABI_GH_TIMEOUT_MS", "200")
            .output()
            .unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(3),
            "{failure}"
        );
        assert!(out.status.success(), "{}", text(&out.stderr));
        let doc = json_value(&out);
        assert_eq!(
            doc["data"]["items"][0]["action"], "skipped",
            "{failure}: {doc}"
        );
        assert_eq!(doc["data"]["items"][0]["detail"], "pr status unavailable");
        assert!(doc["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning["code"] == "pull_request_unavailable"));
        assert!(branch_listed(&repo, "topic"));
        let calls = fs::read_to_string(repo.root.join("gh-calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("pr view"))
                .count(),
            1,
            "{calls}"
        );
    }
}

#[test]
fn merged_pr_after_base_edit_and_squash_proves_retirement() {
    let repo = Repo::new();
    fs::write(repo.repo.join("work.txt"), "start\n").unwrap();
    repo.git(&["add", "work.txt"]);
    repo.git(&["commit", "-m", "original file"]);
    assert!(repo
        .rabi(&["create", "topic", "--no-setup"])
        .status
        .success());
    commit_file(&repo, "topic", "work.txt", "start\ntopic\n");
    assert!(repo.rabi(&["set", "topic", "--pr", "7"]).status.success());
    let head = retirement_head_at(&repo, &managed(&repo, "topic"));
    for (body, message) in [
        ("base\nstart\n", "base edit after PR creation"),
        ("base\nstart\ntopic\n", "squash PR on updated base"),
    ] {
        fs::write(repo.repo.join("work.txt"), body).unwrap();
        repo.git(&["add", "work.txt"]);
        repo.git(&["commit", "-m", message]);
    }
    // Exact tree-entry transitions cannot establish this squash locally.
    let kept = merged_pr_command(
        &repo,
        &["remove", "topic", "--dry-run", "--json"],
        &serde_json::Value::Null,
    )
    .env("GH_REPO", "{}")
    .output()
    .unwrap();
    assert_eq!(
        json_value(&kept)["data"]["items"][0]["branchKeptReason"],
        "unpreserved"
    );
    let response = merged_pr_response(&head, "MERGED");
    let out = merged_pr_command(&repo, &["gc", "--merged", "--apply", "--json"], &response)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", text(&out.stderr));
    assert_eq!(json_value(&out)["data"]["items"][0]["action"], "removed");
    assert!(!branch_listed(&repo, "topic"));
}

#[test]
fn merged_pr_cached_proof_does_not_cover_new_local_commits_after_hooks() {
    for command in ["remove", "gc"] {
        let repo = merged_pr_fixture(Some("7"));
        let head = retirement_head_at(&repo, &managed(&repo, "topic"));
        let hook = "printf 'new unpublished work\\n' > later.txt && git add later.txt && git commit -m later";
        fs::write(
            repo.repo.join(".rabi.toml"),
            format!(
                "[hooks]\npre-remove = [{}]\n",
                serde_json::to_string(hook).unwrap()
            ),
        )
        .unwrap();
        let args = if command == "gc" {
            vec!["gc", "--merged", "--apply", "--json"]
        } else {
            vec!["remove", "topic", "--json"]
        };
        let out = merged_pr_command(&repo, &args, &merged_pr_response(&head, "MERGED"))
            .output()
            .unwrap();
        assert!(
            managed(&repo, "topic").is_dir(),
            "{command}: {}",
            text(&out.stdout)
        );
        assert!(branch_listed(&repo, "topic"));
        assert_ne!(
            retirement_head_at(&repo, &managed(&repo, "topic")),
            head,
            "hook must create new work"
        );
        let calls = fs::read_to_string(repo.root.join("gh-calls")).unwrap();
        assert_eq!(
            calls
                .lines()
                .filter(|line| line.starts_with("pr view"))
                .count(),
            1,
            "{calls}"
        );
    }
}

#[test]
fn current_storage_ignores_unsupported_stores_without_changing_them() {
    let repo = Repo::new();
    let ignored = [
        repo.home.join(".config/mabo/config.toml"),
        repo.root.join("state/mabo/trust.toml"),
        repo.root.join("state/mabo/repos/proj_ignored.json"),
        repo.root.join("state/mabo/projects.json"),
        repo.root.join("state/rabi/projects.json"),
        repo.repo.join(".git/mabo/workspaces/ws_ignored.json"),
        repo.repo.join(".git/mabo/worktrees.json"),
        repo.repo.join(".git/rabi/worktrees.json"),
    ];
    for path in &ignored {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Parsing any of these would fail, even on a read-only command.
        fs::write(path, "unsupported store: leave these bytes alone\n").unwrap();
    }
    let before: Vec<_> = ignored
        .iter()
        .map(|path| {
            (
                fs::read(path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect();
    for args in [
        vec!["config", "get", "workspace-root", "--json"],
        vec!["repo", "list", "--json"],
        vec!["trust", "list", "--json"],
        vec!["list", "--json"],
        vec!["show", ".", "--json"],
        vec!["gc", "--json"],
        vec!["config", "set", "discover-ghq", "false", "--json"],
        vec!["repo", "add", "--json"],
        vec!["trust", "add", "--json"],
        vec!["create", "current", "--no-setup", "--json"],
        vec!["set", "current", "--purpose", "current format", "--json"],
        vec!["remove", "current", "--json"],
    ] {
        let out = repo.rabi(&args);
        assert!(
            out.status.success(),
            "{args:?}: {} {}",
            text(&out.stdout),
            text(&out.stderr)
        );
        assert_eq!(json_value(&out)["warnings"], serde_json::json!([]));
        for (path, (bytes, modified)) in ignored.iter().zip(&before) {
            assert_eq!(
                fs::read(path).unwrap(),
                *bytes,
                "{args:?}: {}",
                path.display()
            );
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), *modified);
        }
    }
    assert!(repo.home.join(".config/rabi/config.toml").is_file());
    assert!(repo.root.join("state/rabi/trust.toml").is_file());
    assert_eq!(
        fs::read_dir(repo.root.join("state/rabi/repos"))
            .unwrap()
            .count(),
        1
    );
    assert!(managed_root(&repo).join(".rabi-common-dir").is_file());
    assert!(repo.repo.join(".git/rabi/workspaces").is_dir());
}

#[test]
fn config_and_state_xdg_defaults_apply_to_reads_and_writes() {
    for mode in ["absolute", "unset", "empty", "relative"] {
        let repo = Repo::new();
        let config_base = if mode == "absolute" {
            repo.root.join("config")
        } else {
            repo.home.join(".config")
        };
        let state_base = if mode == "absolute" {
            repo.root.join("state")
        } else {
            repo.home.join(".local/state")
        };
        let run = |args: &[&str]| {
            let mut command = repo.command(env!("CARGO_BIN_EXE_rabi"), args);
            for (key, base) in [
                ("XDG_CONFIG_HOME", &config_base),
                ("XDG_STATE_HOME", &state_base),
            ] {
                match mode {
                    "absolute" => {
                        command.env(key, base);
                    }
                    "unset" => {
                        command.env_remove(key);
                    }
                    "empty" => {
                        command.env(key, "");
                    }
                    _ => {
                        command.env(key, "relative");
                    }
                }
            }
            let out = command.output().unwrap();
            assert!(
                out.status.success(),
                "{mode} {args:?}: {} {}",
                text(&out.stdout),
                text(&out.stderr)
            );
            out
        };
        run(&["config", "get", "discover-ghq"]);
        run(&["repo", "list"]);
        run(&["trust", "list"]);
        assert!(!config_base.join("rabi").exists());
        assert!(!state_base.join("rabi").exists());
        run(&["config", "set", "discover-ghq", "false"]);
        assert_eq!(
            text(&run(&["config", "get", "discover-ghq"]).stdout),
            "false"
        );
        run(&["repo", "add"]);
        run(&["trust", "add"]);
        assert!(config_base.join("rabi/config.toml").is_file());
        assert!(state_base.join("rabi/trust.toml").is_file());
        assert_eq!(
            fs::read_dir(state_base.join("rabi/repos")).unwrap().count(),
            1
        );
        assert!(!repo.repo.join("relative").exists());
    }
}

#[test]
fn unsupported_layouts_require_a_current_record_to_be_managed() {
    let repo = Repo::new();
    let common = rabi::git::canonical_common_dir(&repo.repo).unwrap();
    let project = rabi::paths::project_directory_name("repo", &common);
    let roots = [
        repo.root.join("xdg/mabo/workspaces").join(&project),
        repo.root.join("xdg/mabo/worktrees").join(&project),
        repo.repo.join(".git/mabo/worktrees"),
        repo.repo.join(".git/ptm/worktrees"),
    ];
    for (index, root) in roots.iter().enumerate() {
        let path = root.join("feature/nested");
        let branch = format!("outside-{index}");
        repo.git(&["worktree", "add", "-b", &branch, path.to_str().unwrap()]);
        let out = repo.rabi(&["show", path.to_str().unwrap(), "--json"]);
        assert!(out.status.success());
        let workspace = &json_value(&out)["data"]["workspace"];
        assert_eq!(workspace["managed"], false);
        assert_eq!(workspace["name"], branch);
        for args in [
            vec!["remove", path.to_str().unwrap(), "--dry-run"],
            vec!["remove", path.to_str().unwrap()],
        ] {
            assert!(!repo.rabi(&args).status.success());
            assert!(path.is_dir());
        }
        let plan = json_value(&repo.rabi(&["gc", "--json"]));
        assert!(plan["data"]["items"].as_array().unwrap().is_empty());
        let out = repo.rabi(&["adopt", path.to_str().unwrap(), "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        assert_eq!(json_value(&out)["data"]["workspace"]["managed"], true);
        let out = repo.rabi(&["remove", path.to_str().unwrap(), "--json"]);
        assert!(out.status.success(), "{}", text(&out.stdout));
        assert!(!path.exists());
    }
}

#[test]
fn create_success_and_setup_failure_keep_the_complete_json_envelope() {
    for failed in [false, true] {
        let repo = Repo::new();
        if failed {
            fs::write(
                repo.repo.join(".rabi.toml"),
                "[hooks]\nsetup = [\"exit 42\"]\n",
            )
            .unwrap();
        }
        let out = repo.rabi(&[
            "create",
            "envelope-topic",
            "--base",
            "main",
            "--purpose",
            "contract test",
            "--json",
        ]);
        assert_eq!(out.status.code(), Some(if failed { 6 } else { 0 }));
        let mut doc = assert_envelope(&out, "create");
        // Normalize only generated IDs/timestamps and the initial commit.
        for field in ["id", "createdAt", "updatedAt", "head", "createdFrom"] {
            assert!(doc["data"]["workspace"][field].is_string(), "{field}");
            doc["data"]["workspace"][field] = serde_json::json!(format!("<{field}>"));
        }
        for at in ["/data/workspace/setup/at", "/data/setup/at"] {
            assert!(doc.pointer(at).unwrap().is_string());
            *doc.pointer_mut(at).unwrap() = serde_json::json!("<time>");
        }
        let path = managed(&repo, "envelope-topic");
        let setup_error = failed.then_some("Setup command 'exit 42' failed with exit code 42");
        let setup_state = if failed { "failed" } else { "ok" };
        let warning = format!(
            "Warning: Setup command failed with exit code 42: exit 42\nWorkspace path: {}",
            path.display()
        );
        let retry = format!(
            "cd '{}' && rabi setup 'envelope-topic'",
            repo.repo.display()
        );
        let expected = serde_json::json!({
            "schema":2, "command":"create", "ok":!failed,
            "data":{
                "workspace":{
                    "id":"<id>", "name":"envelope-topic", "path":path,
                    "kind":"workspace", "current":false, "repo":{"root":repo.repo,"name":"repo"},
                    "managed":true, "upstream":null, "branch":"envelope-topic", "head":"<head>",
                    "base":"refs/heads/main", "dirty":false, "detached":false, "orphaned":false,
                    "purpose":"contract test", "createdAt":"<createdAt>", "updatedAt":"<updatedAt>",
                    "createdFrom":"<createdFrom>", "links":{"pr":null,"issue":null},
                    "setup":{"state":setup_state, "at":"<time>", "error":setup_error}
                },
                "source":{"kind":"new","base":"refs/heads/main","reused":false},
                "setup":{"state":setup_state,"at":"<time>","error":setup_error,"ran":failed,"ok":!failed,
                         "command":failed.then_some("exit 42"),"exitCode":failed.then_some(42),"signal":null},
                "copy":{"copied":[],"error":null},"metadata":{"ok":true,"error":null}
            },
            "warnings":if failed {vec![serde_json::json!({"code":"setup","message":warning})]} else {vec![]},
            "error":if failed {serde_json::json!({"code":"partial","message":format!("Workspace created but setup did not complete. Retry with `{retry}`.")})} else {serde_json::Value::Null}
        });
        assert_eq!(doc, expected);
        assert!(
            !repo.repo.join(".git/rabi/runs").exists(),
            "create itself must not create run history"
        );
        if failed {
            assert_eq!(setup_retry_hint(&out, true), retry);
        }
    }
}

fn workflow_file(repo: &Repo, name: &str, yaml: &str) -> PathBuf {
    let path = repo
        .repo
        .join(".rabi/workflows")
        .join(format!("{name}.yaml"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, yaml).unwrap();
    path
}

const DEFAULT_WORKFLOW: &str = include_str!("../src/workflow/default.yaml");

#[test]
fn workflow_inspection_lists_sources_and_shows_the_complete_json_contract_without_trust() {
    let repo = Repo::new();
    let path = workflow_file(
        &repo,
        "custom",
        &DEFAULT_WORKFLOW.replace("name: default", "name: embedded-name"),
    );
    fs::write(
        repo.repo.join(".rabi.toml"),
        "[hooks]\nsetup = [\"touch hook-ran\"]\n",
    )
    .unwrap();
    let list = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["workflow", "list", "--json"])
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert_eq!(list.status.code(), Some(0));
    assert_eq!(
        assert_envelope(&list, "workflow list"),
        serde_json::json!({
            "schema":2,"command":"workflow list","ok":true,"data":{"workflows":[
                {"name":"default","source":"builtin","path":null},
                {"name":"custom","source":"repository","path":path}
            ]},"warnings":[],"error":null
        })
    );
    for (name, source, path_value) in [
        ("default", "builtin", serde_json::Value::Null),
        ("custom", "repository", serde_json::json!(path)),
    ] {
        let shown = repo
            .command(
                env!("CARGO_BIN_EXE_rabi"),
                &["workflow", "show", name, "--output", "json"],
            )
            .env_remove("RABI_TRUST_ALL")
            .output()
            .unwrap();
        assert!(shown.status.success(), "{}", text(&shown.stdout));
        let doc = assert_envelope(&shown, "workflow show");
        assert_eq!(
            doc["data"]
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["workflow"]
        );
        let workflow = &doc["data"]["workflow"];
        assert_eq!(
            workflow
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                "agents",
                "definition",
                "name",
                "path",
                "schemaVersion",
                "source",
                "steps",
                "version"
            ]
        );
        assert_eq!(workflow["name"], name);
        assert_eq!(workflow["source"], source);
        assert_eq!(workflow["path"], path_value);
        assert_eq!(workflow["schemaVersion"], 1);
        assert_eq!(workflow["version"], 6);
        assert_eq!(workflow["definition"]["start"], "prepare");
        for name in ["implement", "fix", "review", "draft-pull-request"] {
            assert_eq!(
                workflow["agents"][name],
                serde_json::json!({"provider":"codex","model":"gpt-6.1-sol","effort":"high"})
            );
        }
        assert_eq!(workflow["agents"].as_object().unwrap().len(), 4);
        let checked = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &["workflow", "check", name])
            .env("RABI_OUTPUT", "json")
            .env_remove("RABI_TRUST_ALL")
            .output()
            .unwrap();
        let checked = assert_envelope(&checked, "workflow check");
        assert_eq!(checked["ok"], true);
        assert_eq!(
            checked["data"]["items"][0]["diagnostics"],
            serde_json::json!([])
        );
    }
    let shown = repo.rabi(&["workflow", "show", "custom"]);
    let shown = text(&shown.stdout);
    assert!(shown.contains("Workflow: custom (repository), version 6"));
    assert!(shown.contains("name: embedded-name"));
    assert!(shown.contains("Agent implement:"));
    assert!(!repo.repo.join("hook-ran").exists());
    assert!(!repo.root.join("state/rabi/trust.toml").exists());
    assert!(!repo.repo.join(".git/rabi/runs").exists());
}

#[test]
fn workflow_check_reports_invalid_definitions_and_keeps_every_item() {
    let repo = Repo::new();
    let cases: Vec<(&str,String,&str)> = vec![
        ("broken-target",DEFAULT_WORKFLOW.replacen("on: {ok: implement}","on: {ok: missing}",1),"Transition target"),
        ("unbounded-cycle",DEFAULT_WORKFLOW.replacen("on: {ok: check}","on: {ok: implement}",1),"Cycle without a loop edge"),
        ("bounded-cycle",DEFAULT_WORKFLOW.replacen("on: {ok: check}","on: {ok: {to: implement, loop: repair}}",1),""),
        ("undefined-loop",DEFAULT_WORKFLOW.replace("loop: repair","loop: undefined"),"Undefined loop"),
        ("zero-step-limit",DEFAULT_WORKFLOW.replace("max_steps: 15","max_steps: 0"),"positive"),
        ("negative-step-limit",DEFAULT_WORKFLOW.replace("max_steps: 15","max_steps: -1"),"positive"),
        ("missing-step-limit",DEFAULT_WORKFLOW.replace("limits: {max_steps: 15}\n",""),"missing field"),
        ("zero-loop-limit",DEFAULT_WORKFLOW.replace("repair: 2","repair: 0"),"positive"),
        ("review-continue",DEFAULT_WORKFLOW.replace("  review:\n    type: agent\n    mode: read-only\n    output: review-v1\n    context: fresh", "  review:\n    type: agent\n    mode: read-only\n    output: review-v1\n    context: {continue: implement}"),"context fresh"),
        ("different-provider",DEFAULT_WORKFLOW.replace("  fix:\n    type: agent", "  fix:\n    type: agent\n    agent: {provider: cursor}"),"same provider"),
        ("missing-continue",DEFAULT_WORKFLOW.replace("continue: implement","continue: missing"),"must exist and be an agent"),
        ("non-agent-continue",DEFAULT_WORKFLOW.replace("continue: implement","continue: check"),"must exist and be an agent"),
        ("invalid-result",DEFAULT_WORKFLOW.replacen("on: {ok: implement}","on: {pass: implement}",1),"Invalid result"),
        ("missing-start",DEFAULT_WORKFLOW.replace("start: prepare","start: missing"),"Start step"),
        ("invalid-yaml","schema_version: [".into(),"Invalid workflow YAML"),
    ];
    for (name, yaml, message) in &cases {
        let path = workflow_file(&repo, name, yaml);
        let bytes = fs::read(&path).unwrap();
        let checked = repo.rabi(&["workflow", "check", name, "--json"]);
        let valid = message.is_empty();
        assert_eq!(
            checked.status.code(),
            Some(if valid { 0 } else { 2 }),
            "{name}: {}",
            text(&checked.stdout)
        );
        let doc = assert_envelope(&checked, "workflow check");
        assert_eq!(doc["data"]["items"].as_array().unwrap().len(), 1);
        let item = &doc["data"]["items"][0];
        assert_eq!(item["valid"], valid, "{name}: {doc}");
        if !valid {
            assert_eq!(doc["error"]["code"], "usage");
            assert!(
                item["diagnostics"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|d| d["message"].as_str().unwrap().contains(message)),
                "{name}: {doc}"
            );
            for diagnostic in item["diagnostics"].as_array().unwrap() {
                assert_eq!(
                    diagnostic
                        .as_object()
                        .unwrap()
                        .keys()
                        .map(String::as_str)
                        .collect::<Vec<_>>(),
                    ["column", "line", "message", "step"]
                );
            }
            if *name == "invalid-yaml" {
                assert!(item["diagnostics"][0]["line"].is_number());
            }
            let shown = repo.rabi(&["workflow", "show", name, "--json"]);
            assert_eq!(shown.status.code(), Some(2));
            assert!(json_value(&shown)["error"]["details"]["diagnostics"].is_array());
        }
        assert_eq!(fs::read(path).unwrap(), bytes);
    }
    workflow_file(&repo, "default", DEFAULT_WORKFLOW);
    let checked = repo.rabi(&["workflow", "check", "--json"]);
    assert_eq!(checked.status.code(), Some(2));
    let doc = assert_envelope(&checked, "workflow check");
    let items = doc["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), cases.len() + 2);
    assert_eq!(items[0]["source"], "builtin");
    assert_eq!(items[0]["valid"], true);
    let reserved = items
        .iter()
        .find(|item| item["name"] == "default" && item["source"] == "repository")
        .unwrap();
    assert_eq!(reserved["valid"], false);
    assert!(reserved["diagnostics"][0]["message"]
        .as_str()
        .unwrap()
        .contains("reserved"));
    assert!(
        repo.rabi(&["workflow", "show", "default"]).status.success(),
        "reserved file cannot shadow builtin"
    );
}

#[test]
fn workflow_selection_follows_invoking_checkout_and_selected_primary_and_trust_lists_the_source() {
    let repo = Repo::new();
    let primary = workflow_file(&repo, "primary", DEFAULT_WORKFLOW);
    assert!(repo
        .rabi(&["create", "workflow-source", "--no-setup"])
        .status
        .success());
    let linked = managed(&repo, "workflow-source");
    fs::create_dir_all(linked.join(".rabi/workflows")).unwrap();
    let linked_definition = linked.join(".rabi/workflows/linked.yaml");
    fs::write(&linked_definition, DEFAULT_WORKFLOW).unwrap();
    for (args, expected) in [
        (vec!["workflow", "list", "--json"], "linked"),
        (
            vec![
                "-R",
                repo.repo.to_str().unwrap(),
                "workflow",
                "list",
                "--json",
            ],
            "primary",
        ),
    ] {
        let out = repo
            .command(env!("CARGO_BIN_EXE_rabi"), &args)
            .current_dir(&linked)
            .output()
            .unwrap();
        assert!(out.status.success());
        let doc = assert_envelope(&out, "workflow list");
        assert_eq!(doc["data"]["workflows"].as_array().unwrap().len(), 2);
        assert_eq!(doc["data"]["workflows"][1]["name"], expected);
    }
    let trusted = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["trust", "add", "--json"])
        .current_dir(&linked)
        .output()
        .unwrap();
    assert!(trusted.status.success());
    let stderr = text(&trusted.stderr);
    assert!(stderr.contains("workflows:"));
    assert!(stderr.contains("linked"));
    assert!(stderr.contains(linked_definition.to_str().unwrap()));
    assert!(!stderr.contains(primary.to_str().unwrap()));
    assert!(stderr.contains("Workflow agent steps and check commands"));
    let data = assert_envelope(&trusted, "trust add")["data"].clone();
    assert_eq!(
        data.as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["path", "trustedAt"]
    );
}

#[test]
fn workflow_and_run_help_completion_and_usage_use_existing_contracts() {
    let repo = Repo::new();
    for command in ["show", "check"] {
        let out = repo.rabi(&["workflow", command, "missing", "--json"]);
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(json_value(&out)["error"]["code"], "not_found");
    }
    for args in [
        vec!["run"],
        vec!["run", "show", "some-id"],
        vec!["run", "start"],
        vec!["workflow", "show"],
        vec!["workflow", "list", "extra"],
        vec!["workflow", "check", "a", "b"],
    ] {
        let mut json_args = args.clone();
        json_args.push("--json");
        let out = repo.rabi(&json_args);
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert_eq!(json_value(&out)["error"]["code"], "usage");
    }
    let help = repo.rabi(&["help", "workflow", "show", "--json"]);
    assert_eq!(
        assert_envelope(&help, "help")["data"]["selected"]["name"],
        "workflow show"
    );
    let doc = json_value(&help);
    assert!(doc["data"]["helps"].get("run").is_some());
    assert!(doc["data"]["helps"].get("run show").is_some());
    assert!(doc["data"]["helps"].get("run start").is_some());
    for shell in ["bash", "zsh", "fish"] {
        let out = repo.rabi(&["completions", shell]);
        assert!(out.status.success());
        let script = text(&out.stdout);
        assert!(script.contains("workflow show"));
        assert!(script.contains("workflow check"));
        assert!(script.contains("run show"));
        assert!(script.contains("run start"));
    }
}

#[test]
fn workflow_allow_dirty_is_typed_defaulted_and_included_for_every_step_type() {
    let repo = Repo::new();
    let yaml = DEFAULT_WORKFLOW
        .replace(
            "    type: system",
            "    type: system\n    allow_dirty: true",
        )
        .replace("    type: agent", "    type: agent\n    allow_dirty: true")
        .replace("    type: check", "    type: check\n    allow_dirty: true");
    workflow_file(&repo, "dirty-policy", &yaml);
    let shown = repo.rabi(&["workflow", "show", "dirty-policy", "--json"]);
    assert!(shown.status.success(), "{}", text(&shown.stdout));
    let doc = assert_envelope(&shown, "workflow show");
    for name in ["prepare", "implement", "check"] {
        assert_eq!(
            doc["data"]["workflow"]["definition"]["steps"][name]["allow_dirty"],
            true
        );
        assert_eq!(doc["data"]["workflow"]["steps"][name]["allowDirty"], true);
    }
    let text_output = repo.rabi(&["workflow", "show", "dirty-policy"]);
    assert!(text(&text_output.stdout).contains("Step check: allow_dirty=true"));
    let default = repo.rabi(&["workflow", "show", "default", "--json"]);
    let default = json_value(&default);
    assert_eq!(
        default["data"]["workflow"]["definition"]["steps"]["prepare"]["allow_dirty"],
        false
    );
    assert_eq!(
        default["data"]["workflow"]["steps"]["prepare"]["allowDirty"],
        false
    );
    workflow_file(
        &repo,
        "invalid-dirty-policy",
        &yaml.replace("allow_dirty: true", "allow_dirty: definitely"),
    );
    for command in ["show", "check"] {
        let out = repo.rabi(&["workflow", command, "invalid-dirty-policy", "--json"]);
        assert_eq!(out.status.code(), Some(2), "{}", text(&out.stdout));
        let doc = json_value(&out);
        let diagnostics = if command == "show" {
            &doc["error"]["details"]["diagnostics"]
        } else {
            &doc["data"]["items"][0]["diagnostics"]
        };
        assert!(
            diagnostics
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["message"].as_str().unwrap().contains("boolean")),
            "{doc}"
        );
    }
}

#[test]
fn workflow_read_only_requires_a_capable_resolved_provider_and_honors_step_overrides() {
    let repo = Repo::new();
    for provider in ["codex", "cursor", "claude"] {
        let yaml = DEFAULT_WORKFLOW.replace("provider: codex", &format!("provider: {provider}"));
        workflow_file(&repo, provider, &yaml);
        for command in ["check", "show"] {
            let out = repo.rabi(&["workflow", command, provider, "--json"]);
            assert_eq!(
                out.status.code(),
                Some(if provider == "codex" { 0 } else { 2 }),
                "{}",
                text(&out.stdout)
            );
            if provider != "codex" {
                let doc = json_value(&out);
                let diagnostics = if command == "show" {
                    &doc["error"]["details"]["diagnostics"]
                } else {
                    &doc["data"]["items"][0]["diagnostics"]
                };
                assert!(diagnostics.as_array().unwrap().iter().any(|d| d["step"]
                    == "steps.review.mode"
                    && d["message"]
                        .as_str()
                        .unwrap()
                        .contains("cannot enforce read-only")));
            }
        }
    }
    let overridden = DEFAULT_WORKFLOW
        .replace("provider: codex", "provider: cursor")
        .replace(
            "  review:\n    type: agent",
            "  review:\n    type: agent\n    agent: {provider: codex}",
        );
    workflow_file(&repo, "override", &overridden);
    let checked = repo.rabi(&["workflow", "check", "override", "--json"]);
    assert!(checked.status.success(), "{}", text(&checked.stdout));
    let shown = repo.rabi(&["workflow", "show", "override", "--json"]);
    assert!(shown.status.success());
    let doc = json_value(&shown);
    assert_eq!(
        doc["data"]["workflow"]["agents"]["review"]["provider"],
        "codex"
    );
    assert_eq!(
        doc["data"]["workflow"]["agents"]["implement"]["provider"],
        "cursor"
    );
    // The capability check applies to read-only report steps as well as reviews.
    let report = overridden.replace("    mode: edit", "    mode: read-only");
    workflow_file(&repo, "read-only-report", &report);
    assert_eq!(
        repo.rabi(&["workflow", "check", "read-only-report", "--json"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn workflow_check_diagnoses_non_regular_files_and_accepts_regular_file_symlinks() {
    let repo = Repo::new();
    let directory = repo.repo.join(".rabi/workflows/directory.yaml");
    fs::create_dir_all(&directory).unwrap();
    std::os::unix::fs::symlink(
        &directory,
        directory.with_file_name("linked-directory.yaml"),
    )
    .unwrap();
    // Regular files reached through symlinks remain supported, including files
    // outside the checkout. Only the file type determines whether it is read.
    let external = repo.root.join("external.yaml");
    fs::write(&external, DEFAULT_WORKFLOW).unwrap();
    std::os::unix::fs::symlink(&external, directory.with_file_name("regular-link.yaml")).unwrap();
    let checked = repo.rabi(&["workflow", "check", "--json"]);
    assert_eq!(checked.status.code(), Some(2));
    let document = assert_envelope(&checked, "workflow check");
    let items = document["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    for name in ["directory", "linked-directory"] {
        let item = items.iter().find(|item| item["name"] == name).unwrap();
        assert_eq!(item["valid"], false);
        assert!(item["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| {
                diagnostic["message"]
                    .as_str()
                    .unwrap()
                    .contains("must be a regular file")
            }));
        let shown = repo.rabi(&["workflow", "show", name, "--json"]);
        assert_eq!(shown.status.code(), Some(2));
        assert!(json_value(&shown)["error"]["details"]["diagnostics"].is_array());
    }
    let linked = items
        .iter()
        .find(|item| item["name"] == "regular-link")
        .unwrap();
    assert_eq!(linked["valid"], true);
    assert!(directory.is_dir());
    assert_eq!(fs::read_to_string(external).unwrap(), DEFAULT_WORKFLOW);
}

fn run_tools(repo: &Repo) -> String {
    // Any rabi-side publication or PR inspection is a regression. The fake agent
    // reports publication without running Git/gh; rabi advances on its result.
    let path = gh_path(repo, "#!/bin/sh\necho unexpected-gh-call >&2\nexit 99\n");
    let codex = repo.root.join("bin/codex");
    fs::write(&codex, r#"#!/bin/sh
set -eu
answer=
readonly=no
printf '%s\n' "$@" >> "$RUN_TEST_ROOT/codex-args"
while [ "$#" -gt 0 ]; do
    case "$1" in
        -o) shift; answer=$1 ;;
        read-only) readonly=yes ;;
    esac
    shift
done
cat > "$RUN_TEST_ROOT/prompt"
printf '{"type":"thread.started","thread_id":"test-thread"}\n'
if [ "$readonly" = yes ]; then
    printf '0 actionable findings (Important: 0 / Nit: 0 / Pre-existing: 0).\n判定: 問題なし\n' > "$answer"
elif head -n 1 "$RUN_TEST_ROOT/prompt" | command grep -q 'Push only the run branch to origin'; then
    printf 'Draft PR: https://github.com/example/repo/pull/23\n' > "$answer"
else
    printf 'implemented\n' >> change.txt
    git add change.txt
    git -c user.name=Test -c user.email=test@example.com commit -qm 'feat: task'
    printf 'Changed change.txt; checks recorded by rabi.\n' > "$answer"
fi
"#).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(codex, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn run_start_completes_agent_draft_pull_request_and_show_reads_without_rewriting() {
    let repo = Repo::new();
    fs::write(repo.repo.join("task.md"), "# Implement sample\nA task.\n").unwrap();
    let path = run_tools(&repo);
    let started = repo
        .command(
            env!("CARGO_BIN_EXE_rabi"),
            &[
                "run",
                "start",
                "task.md",
                "--name",
                "fix/example",
                "--base",
                "main",
                "--provider",
                "codex",
                "--model",
                "example-model",
                "--check",
                "test -f change.txt",
                "--check",
                "echo check-output",
                "--json",
            ],
        )
        .env("PATH", &path)
        .env("RUN_TEST_ROOT", &repo.root)
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}\n{}",
        text(&started.stdout),
        text(&started.stderr)
    );
    let document = assert_envelope(&started, "run start");
    let run = &document["data"]["run"];
    assert_eq!(run["state"], "succeeded");
    assert_eq!(run["stepsStarted"], 5);
    assert!(run["pr"].is_null());
    assert!(text(&started.stderr).contains("starting draft-pull-request (5/15)"));
    assert!(!text(&started.stdout).contains("check-output"));
    assert!(fs::read_to_string(repo.root.join("codex-args"))
        .unwrap()
        .contains("-m\nexample-model\n"));
    let prompt = fs::read_to_string(repo.root.join("prompt")).unwrap();
    for expected in [
        "Run branch: fix/example",
        "Base branch: main",
        "Remote: origin",
        "DRAFT PR",
        "check-output",
        "verdict 判定: 問題なし",
        "rounds 1",
    ] {
        assert!(prompt.contains(expected), "{prompt}");
    }
    let shown_workspace = repo.rabi(&["show", "fix/example", "--json"]);
    assert!(
        assert_envelope(&shown_workspace, "show")["data"]["workspace"]["links"]["pr"].is_null()
    );
    let id = run["id"].as_str().unwrap();
    let directory = Path::new(run["directory"].as_str().unwrap());
    assert!(!directory.join("pr-body.md").exists());
    assert!(
        fs::read_to_string(directory.join("steps/005-draft-pull-request/answer.md"))
            .unwrap()
            .contains("https://github.com/example/repo/pull/23")
    );
    let original = fs::read(directory.join("run.json")).unwrap();
    let shown = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &["run", "show", id, "--json"])
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert!(shown.status.success());
    assert_eq!(assert_envelope(&shown, "run show")["data"]["run"], *run);
    let shown = repo.rabi(&["run", "show", id]);
    assert!(shown.status.success());
    for expected in ["State: succeeded", "Loop repair: 0/2", "Records:"] {
        assert!(text(&shown.stdout).contains(expected));
    }
    assert_eq!(fs::read(directory.join("run.json")).unwrap(), original);
}

#[test]
fn workflow_check_rejects_removed_publication_actions() {
    let repo = Repo::new();
    for action in ["push", "draft-pr"] {
        workflow_file(
            &repo,
            action,
            &DEFAULT_WORKFLOW.replace("action: prepare", &format!("action: {action}")),
        );
        let checked = repo.rabi(&["workflow", "check", action, "--json"]);
        assert_eq!(checked.status.code(), Some(2));
        let doc = assert_envelope(&checked, "workflow check");
        assert_eq!(doc["data"]["items"][0]["valid"], false);
        assert!(text(&checked.stdout).contains("unknown variant"));
    }
}

#[test]
fn run_show_formats_multiline_stop_reasons_and_escapes_other_controls() {
    let repo = Repo::new();
    let id = format!("run_{}", uuid::Uuid::new_v4());
    let directory = repo.repo.join(".git/rabi/runs").join(&id);
    fs::create_dir_all(&directory).unwrap();
    for (message, expected) in [
        ("", "Stopped (operation_failed): \nRecords:"),
        (
            "Cannot commit",
            "Stopped (operation_failed): Cannot commit\nRecords:",
        ),
        (
            "Cannot commit\n\nGit failed:\r\n\t\u{1b}[31mdenied\u{7}\u{7f}\u{202e}\n",
            "Stopped (operation_failed): Cannot commit\n  \n  Git failed:\\u{d}\n  \\u{9}\\u{1b}[31mdenied\\u{7}\\u{7f}\\u{202e}\n  \nRecords:",
        ),
    ] {
        let record = serde_json::json!({
            "schema": 1,
            "id": id,
            "state": "stopped",
            "workflow": {"name": "default", "version": 4, "source": "builtin"},
            "currentStep": null,
            "stepsStarted": 2,
            "loops": {},
            "workspace": {"id": null, "name": "topic", "path": "/unused", "branch": "topic"},
            "pr": null,
            "stopReason": {"kind": "operation_failed", "message": message},
            "startedAt": "2026-10-25T00:00:00Z",
            "finishedAt": "2026-10-25T00:01:00Z"
        });
        let original = serde_json::to_vec_pretty(&record).unwrap();
        let path = directory.join("run.json");
        fs::write(&path, &original).unwrap();

        let shown = repo.rabi(&["run", "show", &id]);
        assert!(shown.status.success(), "{}", text(&shown.stderr));
        assert!(text(&shown.stdout).contains(expected), "{}", text(&shown.stdout));

        let shown = repo.rabi(&["run", "show", &id, "--json"]);
        assert!(shown.status.success(), "{}", text(&shown.stderr));
        let document = assert_envelope(&shown, "run show");
        assert_eq!(document["data"]["run"]["stopReason"], record["stopReason"]);
        assert_eq!(fs::read(path).unwrap(), original);
    }
}

#[test]
fn run_start_refuses_untrusted_and_invalid_inputs_before_workspace_creation() {
    let repo = Repo::new();
    fs::write(repo.repo.join("task.md"), "# Task").unwrap();
    let args = [
        "run", "start", "task.md", "--name", "topic", "--base", "main", "--check", "true", "--json",
    ];
    let output = repo
        .command(env!("CARGO_BIN_EXE_rabi"), &args)
        .env_remove("RABI_TRUST_ALL")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert_eq!(
        assert_envelope(&output, "run start")["error"]["code"],
        "untrusted"
    );
    for (option, value) in [
        ("--workspace", "topic"),
        ("--provider", "cursor"),
        ("--provider", "claude"),
    ] {
        let mut args = args.to_vec();
        args.extend([option, value]);
        let output = repo.rabi(&args);
        assert_eq!(output.status.code(), Some(2), "{}", text(&output.stdout));
        if option == "--provider" {
            assert!(text(&output.stdout).contains("read-only"));
        }
    }
    let no_checks = repo.rabi(&[
        "run", "start", "task.md", "--name", "topic", "--base", "main", "--json",
    ]);
    assert_eq!(no_checks.status.code(), Some(2));
    assert!(!repo.repo.join(".git/rabi/runs").exists());
    let worktrees = repo
        .command("git", &["worktree", "list", "--porcelain"])
        .output()
        .unwrap();
    assert_eq!(text(&worktrees.stdout).matches("worktree ").count(), 1);
    let unknown = format!("run_{}", uuid::Uuid::new_v4());
    assert_eq!(
        repo.rabi(&["run", "show", &unknown, "--json"])
            .status
            .code(),
        Some(3)
    );
    assert_eq!(
        repo.rabi(&["run", "show", "../outside", "--json"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn documented_security_review_workflow_passes_public_check() {
    let repo = Repo::new();
    workflow_file(
        &repo,
        "security-review",
        include_str!("../docs/workflows/security-review.yaml"),
    );
    let output = repo.rabi(&["workflow", "check", "security-review", "--json"]);
    assert!(output.status.success(), "{}", text(&output.stdout));
    let shown = repo.rabi(&["workflow", "show", "security-review", "--json"]);
    let workflow = assert_envelope(&shown, "workflow show");
    assert_eq!(
        workflow["data"]["workflow"]["definition"]["limits"]["max_steps"],
        18
    );
    assert_eq!(
        workflow["data"]["workflow"]["agents"]["security-review"]["provider"],
        "codex"
    );
}
